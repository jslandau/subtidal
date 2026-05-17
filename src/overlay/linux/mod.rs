//! Linux GTK4 + layer-shell overlay implementation.
//!
//! Contains the `run_gtk_app` entry point, the `OverlayCommand` dispatch loop body,
//! and the per-window GTK construction submodules. This entire subtree is cfg-gated
//! to `target_os = "linux"`; neutral overlay items (`OverlayCommand`,
//! `caption_buffer`, `transcript_log`) live one level up in `crate::overlay`.

pub mod drag;
pub mod input_region;
pub mod transcript_window;
pub mod window;

use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use gtk4::glib;
use gtk4_layer_shell::{Edge, KeyboardMode, LayerShell, Layer};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::config::{Config, OverlayMode};
use crate::overlay::{
    caption_buffer::CaptionBuffer,
    CaptionsEnabled, OverlayCommand,
};

use drag::add_drag_handler;
use input_region::{clear_input_region, set_empty_input_region};
use window::{apply_appearance, build_overlay_window, configure_docked, estimate_max_chars, find_caption_label};

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

        // Transcript log: accumulates every recognized fragment from session start.
        // Wrapped in Rc<RefCell<>> so the caption consumer (mutating push) and the
        // command consumer (clear-on-disable in Phase 6) can share it on the GTK
        // main thread without locking.
        let transcript_log = Rc::new(RefCell::new(
            crate::overlay::transcript_log::TranscriptLog::new(std::time::Duration::from_millis(1500))
        ));

        // Engine display name and session start: computed once at activation, both
        // passed into the transcript window for use by the Save dialog (Phase 5).
        //
        // The explicit match (instead of `Display`/`Debug`) is intentional — when a
        // future engine variant is added to `crate::config::Engine`, the Rust
        // compiler will fail this match exhaustively, forcing the developer to
        // pick a stable display string for the new engine. Using `format!("{:?}", cfg.engine)`
        // would silently produce e.g. "Whisper" without thinking about JSON-stability.
        let engine_name = match cfg.engine {
            crate::config::Engine::Nemotron => "nemotron".to_string(),
        };
        let session_start = chrono::Local::now();

        // Current overlay mode tracker — read by the caption and command consumers
        // to route updates to the correct surface. OverlayMode derives Clone but not Copy;
        // use Rc<RefCell<OverlayMode>> and borrow it for read.
        let current_mode: Rc<RefCell<OverlayMode>> = Rc::new(RefCell::new(cfg.overlay_mode.clone()));

        // Construct the transcript window (always built, initially hidden by the
        // builder). Phase 5 wires the Save button.
        let transcript_state = transcript_window::build_transcript_window(
            app,
            Rc::clone(&transcript_log),
            engine_name.clone(),
            session_start,
        );

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
            let log = Rc::clone(&transcript_log);
            let mode = Rc::clone(&current_mode);
            let tstate = transcript_state.clone();
            glib::MainContext::default().spawn_local(async move {
                while let Ok(text) = caption_rx.recv().await {
                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }

                    // Always: append to the durable transcript log AND to the transcript
                    // window's TextBuffer (safe even while the window is hidden — GTK
                    // queues layout updates and they materialize on .present()).
                    let kind = log.borrow_mut().push(text.clone());
                    let fragment = log
                        .borrow()
                        .fragments()
                        .last()
                        .cloned()
                        .expect("just pushed a fragment");
                    transcript_window::append_fragment_to_view(&tstate, &fragment, kind);

                    // Overlay surfaces (caption_buffer + label) only update when the
                    // overlay is the active mode.
                    let m = mode.borrow().clone();
                    if matches!(m, OverlayMode::Docked | OverlayMode::Floating) {
                        buf.borrow_mut().push(text);
                        if !dragging.get() {
                            label.set_text(&buf.borrow().display_text());
                            window.set_visible(true);
                        }
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
            let mode = Rc::clone(&current_mode);
            let tstate = transcript_state.clone();
            let log = Rc::clone(&transcript_log);
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
                        handle_overlay_command(
                            &window, cmd, &config, &dragging, &buf,
                            &captions_enabled, &mode, &tstate, &log,
                        );
                    }
                }
            });
        }

        match cfg.overlay_mode {
            OverlayMode::Docked | OverlayMode::Floating => {
                transcript_state.window.set_visible(false);
                window.present();
            }
            OverlayMode::Transcript => {
                window.set_visible(false);
                transcript_state.window.present();
            }
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
    current_mode: &Rc<RefCell<OverlayMode>>,
    transcript_state: &transcript_window::TranscriptWindowState,
    transcript_log: &Rc<RefCell<crate::overlay::transcript_log::TranscriptLog>>,
) {
    match cmd {
        OverlayCommand::SetVisible(v) => {
            // Route to whichever window is currently active.
            let m = current_mode.borrow().clone();
            match m {
                OverlayMode::Docked | OverlayMode::Floating => window.set_visible(v),
                OverlayMode::Transcript => transcript_state.window.set_visible(v),
            }
        }
        OverlayCommand::SetMode(mode) => {
            // Persist the new mode locally and in config.
            //
            // NOTE on dual-write (pre-existing pattern; do NOT "fix"): the tray's
            // RadioGroup `select` closure ALREADY writes `cfg.overlay_mode` to disk
            // via `cfg.save()` (see `src/tray/mod.rs:464-467`). This handler updates
            // the in-memory `Arc<Mutex<Config>>` shared with the GTK side. The two
            // stores eventually reconverge via the notify-debouncer-mini hot-reload
            // watcher at `src/config.rs:312-365`. This dual-write is intentional
            // and predates the transcript work; preserve it.
            *current_mode.borrow_mut() = mode.clone();
            let mut cfg = config.lock().unwrap();
            cfg.overlay_mode = mode.clone();
            match mode {
                OverlayMode::Docked => {
                    transcript_state.window.set_visible(false);
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    configure_docked(window, &cfg.screen_edge, &cfg.dock_position);
                    set_empty_input_region(window);
                    window.set_visible(true);
                }
                OverlayMode::Floating => {
                    transcript_state.window.set_visible(false);
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
                        set_empty_input_region(window);
                    } else {
                        clear_input_region(window);
                        add_drag_handler(window, is_dragging);
                    }
                    window.set_visible(true);
                }
                OverlayMode::Transcript => {
                    window.set_visible(false);
                    // present() raises the window above other toplevel windows
                    // and is a no-op if it's already presented and visible.
                    transcript_state.window.present();
                }
            }
        }
        OverlayCommand::SetAboveFullscreen(above) => {
            // Apply to the layer-shell overlay window only; transcript mode uses
            // a regular toplevel and is unaffected by layer-shell stacking.
            window.set_layer(if above { Layer::Overlay } else { Layer::Top });
            // Persist to the in-memory shared config so subsequent rebuilds match.
            if let Ok(mut cfg) = config.lock() {
                cfg.above_fullscreen = above;
            }
        }
        OverlayCommand::SetLocked(locked) => {
            // No-op when in Transcript mode: lock controls only affect the
            // floating layer-shell overlay.
            if matches!(*current_mode.borrow(), OverlayMode::Transcript) {
                return;
            }
            if locked {
                set_empty_input_region(window);
                window.set_keyboard_mode(KeyboardMode::None);
            } else {
                clear_input_region(window);
                window.set_keyboard_mode(KeyboardMode::OnDemand);
                add_drag_handler(window, is_dragging);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            // No-op when in Transcript mode: appearance config applies to the
            // overlay only (per design "explicitly out of scope").
            if matches!(*current_mode.borrow(), OverlayMode::Transcript) {
                return;
            }
            apply_appearance(&appearance);
            let label = find_caption_label(window);
            let max_chars = estimate_max_chars(
                appearance.width, appearance.font_size,
                appearance.effective_char_width_fraction(),
            );
            label.set_max_width_chars(max_chars);
            label.set_lines(appearance.max_lines as i32);
            window.set_width_request(appearance.width);
            let mut buf = caption_buffer.borrow_mut();
            buf.update_config(
                appearance.max_lines as usize, max_chars as usize,
                appearance.effective_expire_secs(),
            );
        }
        OverlayCommand::SetCaption(text) => {
            // SetCaption is a legacy command path (currently `#[allow(dead_code)]`)
            // for direct overlay-label updates. It is NOT part of transcript
            // routing — the transcript window is updated only by the caption
            // consumer future via `append_fragment_to_view`. Leave SetCaption's
            // behavior unchanged.
            let label = find_caption_label(window);
            label.set_text(&text);
        }
        OverlayCommand::SetCaptionsEnabled(enabled) => {
            // Update the AtomicBool first — the caption consumer future reads this
            // and short-circuits when false. Setting it before clearing prevents any
            // in-flight caption from being appended back into a buffer we just cleared.
            captions_enabled.store(enabled, Ordering::Relaxed);

            if !enabled {
                // Clear all four caption surfaces:
                // 1. Durable transcript log.
                transcript_log.borrow_mut().clear();
                // 2. Transcript window's TextBuffer (visible even while hidden;
                //    must be cleared so a future mode switch shows nothing).
                transcript_window::clear_view(transcript_state);
                // 3. Overlay caption buffer (line-fill state).
                caption_buffer.borrow_mut().clear();
                // 4. Overlay caption label (the visible text in the layer-shell window).
                let label = find_caption_label(window);
                label.set_text("");
            }
            // On (true): no clearing — the prior disable already cleared everything,
            // and there is no carryover state from a freshly re-enabled recognizer.
        }
        OverlayCommand::Quit => {
            if let Some(app) = window.application() {
                app.quit();
            }
        }
    }
}
