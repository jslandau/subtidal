//! GTK4 overlay window: docked (wlr-layer-shell) and floating modes with caption display.

use crate::config::{AppearanceConfig, Config, OverlayMode, ScreenEdge};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};
use gtk4::glib;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::sync::{Arc, atomic::{AtomicBool, AtomicI32, Ordering}};
use std::cell::RefCell;

pub mod input_region;

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
    SetCaption(String),
    /// Quit the application cleanly (sent by tray Quit and SIGTERM handler).
    Quit,
}

/// Shared visibility flag (AtomicBool for tray ↔ overlay signaling).
pub type CaptionsEnabled = Arc<AtomicBool>;

/// Build and run the GTK4 application.
///
/// This function must be called on the main thread. It blocks until the GTK4
/// main loop exits.
///
/// Parameters:
/// - `config`: initial configuration
/// - `caption_rx`: mpsc channel receiver delivering caption strings from inference thread
/// - `cmd_rx`: mpsc channel receiver delivering OverlayCommand from tray
/// - `captions_enabled`: shared bool for left-click tray toggle
pub fn run_gtk_app(
    config: Config,
    caption_rx: std::sync::mpsc::Receiver<String>,
    cmd_rx: std::sync::mpsc::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    let app = Application::builder()
        .application_id("com.example.live-captions")
        .build();

    let config = Arc::new(std::sync::Mutex::new(config));
    let config_clone = Arc::clone(&config);
    let captions_enabled_clone = Arc::clone(&captions_enabled);

    // Wrap channels in Arc so they can be shared with closures
    let caption_rx = Arc::new(std::sync::Mutex::new(caption_rx));
    let cmd_rx = Arc::new(std::sync::Mutex::new(cmd_rx));

    app.connect_activate(move |app| {
        let cfg = config_clone.lock().unwrap().clone();
        let window = build_overlay_window(app, &cfg);

        // Apply initial appearance.
        apply_appearance(&cfg.appearance);

        // Wire up caption receiver using glib timeout_add to poll.
        let label = find_caption_label(&window);
        let window_clone = window.clone();
        let enabled = Arc::clone(&captions_enabled_clone);
        let caption_rx_clone = Arc::clone(&caption_rx);

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if let Ok(rx) = caption_rx_clone.try_lock() {
                while let Ok(text) = rx.try_recv() {
                    if enabled.load(Ordering::Relaxed) {
                        label.set_text(&text);
                        window_clone.set_visible(true);
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        // Wire up command receiver using glib timeout_add to poll.
        let window_clone2 = window.clone();
        let config_for_cmd = Arc::clone(&config_clone);
        let cmd_rx_clone = Arc::clone(&cmd_rx);

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if let Ok(rx) = cmd_rx_clone.try_lock() {
                while let Ok(cmd) = rx.try_recv() {
                    handle_overlay_command(&window_clone2, cmd, &config_for_cmd);
                }
            }
            glib::ControlFlow::Continue
        });

        window.present();
    });

    app.run_with_args::<&str>(&[]);
}

/// Build the overlay window for the given config.
/// Uses gtk4-layer-shell for both docked and floating modes (Layer::Top).
fn build_overlay_window(app: &Application, cfg: &Config) -> ApplicationWindow {
    let window = ApplicationWindow::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .title("live-captions")
        .build();

    // Initialize layer shell.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_exclusive_zone(0); // don't push other windows aside

    match cfg.overlay_mode {
        OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge),
        OverlayMode::Floating => configure_floating(&window, cfg),
    }

    // Build caption label.
    let label = Label::builder()
        .label("")
        .wrap(true)
        .max_width_chars(80)
        .build();
    label.set_widget_name("caption-label");
    window.set_child(Some(&label));

    // Set click-through after window maps.
    let is_locked = cfg.locked || cfg.overlay_mode == OverlayMode::Docked;
    window.connect_map(move |win| {
        if is_locked {
            input_region::set_empty_input_region(win);
        }
    });

    // Drag handle for floating + unlocked.
    if cfg.overlay_mode == OverlayMode::Floating && !cfg.locked {
        add_drag_handler(&window);
    }

    window
}

fn configure_docked(window: &ApplicationWindow, edge: &ScreenEdge) {
    // Anchor to the selected screen edge and stretch horizontally/vertically as appropriate.
    let (anchor_edge, stretch_edges) = match edge {
        ScreenEdge::Bottom => (Edge::Bottom, vec![Edge::Left, Edge::Right]),
        ScreenEdge::Top    => (Edge::Top,    vec![Edge::Left, Edge::Right]),
        ScreenEdge::Left   => (Edge::Left,   vec![Edge::Top, Edge::Bottom]),
        ScreenEdge::Right  => (Edge::Right,  vec![Edge::Top, Edge::Bottom]),
    };
    window.set_anchor(anchor_edge, true);
    for e in stretch_edges {
        window.set_anchor(e, true);
    }
    // Keyboard and pointer click-through: handled by keyboard_mode + empty input region.
    window.set_keyboard_mode(KeyboardMode::None);
}

fn configure_floating(window: &ApplicationWindow, cfg: &Config) {
    // Floating: no anchors (window positioned freely).
    // gtk4-layer-shell with no anchors centres the surface; we then move it.
    window.set_keyboard_mode(if cfg.locked {
        KeyboardMode::None
    } else {
        KeyboardMode::OnDemand
    });

    // Position the window after it realizes.
    let pos = cfg.position.clone();
    window.connect_realize(move |win| {
        // Note: direct position setting on layer-shell surfaces is not standard.
        // For Wayland, the compositor controls position for layer-shell surfaces.
        // The "floating" appearance can be achieved by using margins:
        win.set_margin(Edge::Left, pos.x);
        win.set_margin(Edge::Top, pos.y);
    });
}

/// Set CSS on the caption label and window to reflect appearance config.
///
/// Uses a thread-local provider to avoid resource leaks: old provider is removed
/// before creating a new one on each call.
pub fn apply_appearance(appearance: &AppearanceConfig) {
    thread_local! {
        static CSS_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
    }

    let css = format!(
        r#"
        window {{
            background-color: {bg};
        }}
        #caption-label {{
            color: {fg};
            font-size: {fs}pt;
            padding: 8px 12px;
        }}
        "#,
        bg = appearance.background_color,
        fg = appearance.text_color,
        fs = appearance.font_size,
    );

    let display = gtk4::gdk::Display::default().expect("no GDK display");

    CSS_PROVIDER.with(|provider_cell| {
        let mut provider_opt = provider_cell.borrow_mut();

        // Remove old provider if it exists
        if let Some(ref old_provider) = *provider_opt {
            gtk4::style_context_remove_provider_for_display(&display, old_provider);
        }

        // Create and add new provider
        let new_provider = gtk4::CssProvider::new();
        new_provider.load_from_data(&css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &new_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Store the new provider for next call
        *provider_opt = Some(new_provider);
    });
}

fn find_caption_label(window: &ApplicationWindow) -> Label {
    window
        .child()
        .and_downcast::<Label>()
        .expect("caption label not found")
}

fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
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
                    // Clear any floating anchors, set docked anchors.
                    // First, clear all anchors.
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    configure_docked(window, &cfg.screen_edge);
                    // Docked mode is always click-through.
                    input_region::set_empty_input_region(window);
                }
                OverlayMode::Floating => {
                    // Clear all anchors (layer-shell will centre the surface).
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    // Restore position from config.
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
                        add_drag_handler(window);
                    }
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
                add_drag_handler(window);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            apply_appearance(&appearance);
        }
        OverlayCommand::SetCaption(text) => {
            if let Some(label) = window.child().and_downcast::<Label>() {
                label.set_text(&text);
            }
        }
        OverlayCommand::Quit => {
            // Quit the GTK4 application cleanly so all cleanup (Drop impls) runs.
            if let Some(app) = window.application() {
                app.quit();
            }
        }
    }
}

fn remove_drag_handlers(window: &ApplicationWindow) {
    // Remove existing GestureDrag controllers to prevent accumulation.
    // On repeated calls to add_drag_handler (e.g., SetLocked(false), SetMode(Floating)),
    // we must clean up previous gesture controllers to avoid erratic drag behavior.
    let controllers = window.observe_controllers();
    let n = controllers.n_items();
    for i in (0..n).rev() {
        if let Some(obj) = controllers.item(i) {
            if obj.downcast_ref::<gtk4::GestureDrag>().is_some() {
                if let Ok(ctrl) = obj.downcast::<gtk4::EventController>() {
                    window.remove_controller(&ctrl);
                }
            }
        }
    }
}

fn add_drag_handler(window: &ApplicationWindow) {
    // Remove any existing drag handlers first to prevent accumulation.
    remove_drag_handlers(window);

    // For gtk4-layer-shell floating windows, position is controlled by margins
    // (not compositor-managed coordinates). We use GestureDrag to track delta
    // and update set_margin() on each drag update.
    //
    // Note: begin_move_drag() is a GTK3 API that does not exist in GTK4.
    // On Wayland with layer-shell, the compositor positions the surface via margins.
    let gesture = gtk4::GestureDrag::new();

    // Capture starting margins when drag begins.
    let start_x = Arc::new(AtomicI32::new(0));
    let start_y = Arc::new(AtomicI32::new(0));
    let sx = Arc::clone(&start_x);
    let sy = Arc::clone(&start_y);
    let win_begin = window.clone();
    gesture.connect_drag_begin(move |_, _, _| {
        sx.store(win_begin.margin(Edge::Left), std::sync::atomic::Ordering::Relaxed);
        sy.store(win_begin.margin(Edge::Top), std::sync::atomic::Ordering::Relaxed);
    });

    // Update margins on each drag update.
    let sx2 = Arc::clone(&start_x);
    let sy2 = Arc::clone(&start_y);
    let win_update = window.clone();
    gesture.connect_drag_update(move |_, dx, dy| {
        let new_x = sx2.load(std::sync::atomic::Ordering::Relaxed) + dx as i32;
        let new_y = sy2.load(std::sync::atomic::Ordering::Relaxed) + dy as i32;
        win_update.set_margin(Edge::Left, new_x.max(0));
        win_update.set_margin(Edge::Top, new_y.max(0));
    });

    // After drag: detect window position change via margin query.
    // gtk4-layer-shell provides get_margin(Edge) to read current margins.
    let win_for_release = window.clone();
    // Store position on drag end (connect_drag_end is the correct GestureDrag signal).
    gesture.connect_drag_end(move |_, _offset_x, _offset_y| {
        let x = win_for_release.margin(Edge::Left);
        let y = win_for_release.margin(Edge::Top);
        eprintln!("info: overlay dragged to ({x}, {y})");
        let mut cfg = crate::config::Config::load();
        cfg.position.x = x;
        cfg.position.y = y;
        if let Err(e) = cfg.save() {
            eprintln!("warn: failed to save position: {e}");
        }
    });

    window.add_controller(gesture);
}
