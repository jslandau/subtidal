//! GTK4 overlay: orchestration, command dispatch, and public API.

mod caption_buffer;
mod drag;
mod transcript_log;
mod window;

pub mod input_region;

use crate::config::{AppearanceConfig, Config, OverlayMode};
use crate::overlay::caption_buffer::CaptionBuffer;
use crate::overlay::drag::add_drag_handler;
use crate::overlay::window::{
    apply_appearance, build_overlay_window, configure_docked, estimate_max_chars,
    find_caption_label,
};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use gtk4::glib;
use gtk4_layer_shell::{Edge, KeyboardMode, LayerShell};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

/// Commands sent to the overlay from the tray / main integration.
#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Show or hide the overlay.
    SetVisible(bool),
    /// Switch overlay mode (docked ↔ floating).
    SetMode(OverlayMode),
    /// Lock or unlock the floating overlay.
    SetLocked(bool),
    /// Update appearance from config.
    UpdateAppearance(AppearanceConfig),
    /// Update caption text (also sent as plain String via glib channel in normal flow).
    #[allow(dead_code)]
    SetCaption(String),
    /// Enable or disable caption emission. On the disable edge the overlay
    /// will (in Phase 6) clear all caption surfaces; for now the placeholder
    /// arm just mirrors the AtomicBool stored in `CaptionsEnabled`.
    SetCaptionsEnabled(bool),
    /// Quit the application cleanly (sent by tray Quit and SIGTERM handler).
    Quit,
}

/// Shared visibility flag (AtomicBool for tray ↔ overlay signaling).
pub type CaptionsEnabled = Arc<AtomicBool>;

/// Build and run the GTK4 application.
///
/// This function must be called on the main thread. It blocks until the GTK4
/// main loop exits.
pub fn run_gtk_app(
    config: Config,
    caption_rx: async_channel::Receiver<String>,
    cmd_rx: async_channel::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    let app = Application::builder()
        .application_id("com.subtidal.app")
        .build();

    let config = Arc::new(std::sync::Mutex::new(config));
    let config_clone = Arc::clone(&config);
    let captions_enabled_clone = Arc::clone(&captions_enabled);

    // async-channel Receivers are Send + Clone; we move them into the activate
    // closure and then into per-task futures. No Mutex needed.
    let caption_rx_outer = std::sync::Mutex::new(Some(caption_rx));
    let cmd_rx_outer = std::sync::Mutex::new(Some(cmd_rx));

    app.connect_activate(move |app| {
        let cfg = config_clone.lock().unwrap().clone();
        let window = build_overlay_window(app, &cfg);

        apply_appearance(&cfg.appearance);

        // Dragging flag: when true, suppress layout-changing mutations to avoid
        // relayout jitter of the layer-shell surface.
        let is_dragging = Rc::new(Cell::new(false));

        if cfg.overlay_mode == OverlayMode::Floating && !cfg.locked {
            add_drag_handler(&window, &is_dragging);
        }

        let label = find_caption_label(&window);
        let max_chars_per_line = estimate_max_chars(
            cfg.appearance.width,
            cfg.appearance.font_size,
            cfg.appearance.effective_char_width_fraction(),
        ) as usize;
        let caption_buffer = Rc::new(RefCell::new(CaptionBuffer::new(
            cfg.appearance.max_lines as usize,
            max_chars_per_line,
            cfg.appearance.effective_expire_secs(),
        )));

        // connect_activate may fire more than once (e.g. on second instance
        // activation). Pull the channels out the first time and no-op after.
        let Some(caption_rx) = caption_rx_outer.lock().unwrap().take() else { return };
        let Some(cmd_rx) = cmd_rx_outer.lock().unwrap().take() else { return };

        // Caption consumer future — driven by the glib main context, wakes
        // only when a caption arrives.
        {
            let buf = Rc::clone(&caption_buffer);
            let label = label.clone();
            let window = window.clone();
            let enabled = Arc::clone(&captions_enabled_clone);
            let dragging = Rc::clone(&is_dragging);
            glib::MainContext::default().spawn_local(async move {
                while let Ok(text) = caption_rx.recv().await {
                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }
                    buf.borrow_mut().push(text);
                    if !dragging.get() {
                        label.set_text(&buf.borrow().display_text());
                        window.set_visible(true);
                    }
                }
            });
        }

        // Expire timer — still a 1Hz tick since expiry is wall-clock-driven,
        // not event-driven.
        {
            let buf = Rc::clone(&caption_buffer);
            let label = label.clone();
            let dragging = Rc::clone(&is_dragging);
            glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
                if !dragging.get() {
                    let mut b = buf.borrow_mut();
                    if b.expire() {
                        label.set_text(&b.display_text());
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // Command consumer future. Quit and SetVisible bypass the drag-suppression
        // gate; only layout-changing commands are deferred during a drag.
        {
            let window = window.clone();
            let config = Arc::clone(&config_clone);
            let dragging = Rc::clone(&is_dragging);
            let buf = Rc::clone(&caption_buffer);
            let captions_enabled = Arc::clone(&captions_enabled_clone);
            glib::MainContext::default().spawn_local(async move {
                while let Ok(cmd) = cmd_rx.recv().await {
                    let bypass_drag = matches!(
                        cmd,
                        OverlayCommand::Quit
                            | OverlayCommand::SetVisible(_)
                            | OverlayCommand::SetCaptionsEnabled(_)
                            | OverlayCommand::SetMode(_)
                    );
                    if bypass_drag || !dragging.get() {
                        handle_overlay_command(&window, cmd, &config, &dragging, &buf, &captions_enabled);
                    }
                }
            });
        }

        if cfg.overlay_mode == OverlayMode::Transcript {
            window.set_visible(false);
        } else {
            window.present();
        }
    });

    app.run_with_args::<&str>(&[]);
}

fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
    caption_buffer: &Rc<RefCell<CaptionBuffer>>,
    captions_enabled: &CaptionsEnabled,
) {
    match cmd {
        OverlayCommand::SetVisible(v) => window.set_visible(v),
        OverlayCommand::SetMode(mode) => {
            // Reconfigure the existing layer-shell window for the new mode.
            // gtk4-layer-shell allows changing anchors/keyboard mode on a realized window.
            let mut cfg = config.lock().unwrap();
            cfg.overlay_mode = mode.clone();
            match mode {
                OverlayMode::Docked => {
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    configure_docked(window, &cfg.screen_edge, &cfg.dock_position);
                    input_region::set_empty_input_region(window);
                }
                OverlayMode::Floating => {
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    window.set_anchor(Edge::Top, true);
                    window.set_anchor(Edge::Left, true);
                    window.set_margin(Edge::Left, cfg.position.x);
                    window.set_margin(Edge::Top, cfg.position.y);
                    window.set_keyboard_mode(if cfg.locked {
                        KeyboardMode::None
                    } else {
                        KeyboardMode::OnDemand
                    });
                    if cfg.locked {
                        input_region::set_empty_input_region(window);
                    } else {
                        input_region::clear_input_region(window);
                        add_drag_handler(window, is_dragging);
                    }
                }
                OverlayMode::Transcript => {
                    // Phase 2 placeholder: hide the overlay window. The transcript
                    // window is built and shown in Phase 4.
                    window.set_visible(false);
                }
            }
        }
        OverlayCommand::SetLocked(locked) => {
            if locked {
                input_region::set_empty_input_region(window);
                window.set_keyboard_mode(KeyboardMode::None);
            } else {
                input_region::clear_input_region(window);
                window.set_keyboard_mode(KeyboardMode::OnDemand);
                add_drag_handler(window, is_dragging);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            apply_appearance(&appearance);
            let label = find_caption_label(window);
            let max_chars = estimate_max_chars(appearance.width, appearance.font_size, appearance.effective_char_width_fraction());
            label.set_max_width_chars(max_chars);
            label.set_lines(appearance.max_lines as i32);
            window.set_width_request(appearance.width);
            let mut buf = caption_buffer.borrow_mut();
            buf.update_config(appearance.max_lines as usize, max_chars as usize, appearance.effective_expire_secs());
        }
        OverlayCommand::SetCaption(text) => {
            let label = find_caption_label(window);
            label.set_text(&text);
        }
        OverlayCommand::SetCaptionsEnabled(enabled) => {
            // Phase 2 placeholder: store the AtomicBool. Phase 6 expands this
            // to also clear all caption surfaces on the disable edge.
            captions_enabled.store(enabled, Ordering::Relaxed);
        }
        OverlayCommand::Quit => {
            // Quit the GTK4 application cleanly so all cleanup (Drop impls) runs.
            if let Some(app) = window.application() {
                app.quit();
            }
        }
    }
}
