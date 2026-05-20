//! macOS overlay application orchestration (NSApplication startup + caption bridge).
//! Phase 2 implementation: caption bridge + OverlayCommand dispatch loop.
//! Phase 6: full OverlayCommand handlers, transcript window, drag observer, caption buffer.

use std::sync::{Arc, Mutex};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSPanel, NSTextField, NSApplicationActivationPolicy};
use objc2::rc::Retained;
use crate::config::{Config, OverlayMode};
use crate::overlay::{OverlayCommand, CaptionsEnabled, caption_buffer::CaptionBuffer, transcript_log::TranscriptLog};
use super::{panel, transcript_window, drag};

/// Handles to overlay UI elements (panel, label, transcript window, caption buffer, log).
/// Wrapped in Arc so references can be cloned into worker closure scopes.
/// SAFETY: OverlayHandles is Send + Sync because all *use* of the contained
/// Retained<T> values happens only within dispatch2::DispatchQueue::main().exec_async
/// closures, which run on the main thread where AppKit calls are safe. The MainThreadMarker
/// acquired inside each closure proves main-thread affinity at the point of dereference.
/// The Arc ensures the objects outlive all worker closures that capture clones.
#[derive(Clone)]
struct OverlayHandles {
    panel: Retained<NSPanel>,
    label: Retained<NSTextField>,
    transcript_state: transcript_window::TranscriptWindowState,
    caption_buffer: Arc<Mutex<CaptionBuffer>>,
    transcript_log: Arc<Mutex<TranscriptLog>>,
    config: Arc<Mutex<Config>>,
}

unsafe impl Send for OverlayHandles {}
unsafe impl Sync for OverlayHandles {}

/// Run the macOS overlay application.
///
/// Entry point for the overlay subsystem. Acquires MainThreadMarker,
/// initializes NSApplication, builds the overlay panel, spawns caption bridge
/// and command dispatch threads, and blocks in NSApplication.run() until shutdown.
///
/// **Lifecycle notes:**
/// - Workers are spawned before run() and enqueue closures via dispatch2.
/// - Channels are closed by run_app's caller (main_macos::main) dropping the senders
///   *after* this function returns from NSApplication.run().
/// - When channels close, workers exit on their next recv_blocking() call.
/// - The OverlayHandles Arc is cloned into each worker closure; the last clone
///   (held by the worker that outlives all dispatched closures) keeps the panel
///   and label alive until both workers fully exit.
/// - This ensures AppKit Retained<T> objects remain valid for all main-queue dispatches.
///
/// **Shutdown chain (SIGINT):**
/// 1. ctrlc signal handler thread calls cmd_tx.send_blocking(Quit)
/// 2. overlay-cmd worker receives Quit via cmd_rx.recv_blocking()
/// 3. Quit closure is dispatched to main queue
/// 4. Main queue executes closure: NSApplication.terminate() is called
/// 5. NSApplication.run() returns (step 6 below)
/// 6. run_app returns; main_macos::main drops senders
/// 7. Workers see recv_blocking errors and exit
/// 8. main_macos::main calls std::process::exit(0)
///
/// This chain depends on the run loop draining the main dispatch queue.
/// Phase 3 may require an NSApplicationDelegate with applicationShouldTerminate
/// if the shutdown becomes fragile with real STT/audio workers.
pub fn run_app(
    config: Config,
    caption_rx: async_channel::Receiver<String>,
    cmd_rx: async_channel::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    // 1. Acquire MainThreadMarker. run_app is always called from main_macos::main,
    // which has already verified main-thread-ness, but be explicit at the boundary.
    let mtm = MainThreadMarker::new()
        .expect("run_app must run on the main thread");

    // 2. Get NSApplication and set to Accessory (no Dock icon, matches LSUIElement=true).
    let app = NSApplication::sharedApplication(mtm);
    // Accessory matches LSUIElement=true in Info.plist: no Dock icon, UI allowed.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // 3. Build the overlay panel and retain both panel and content label.
    let (panel, label) = panel::build_overlay_panel(mtm, &config);
    // Apply mode-specific geometry + mouse-event state on startup so the
    // initial Floating panel is draggable without the user having to
    // toggle Lock Position to force a property write.
    panel::apply_geometry(&panel, mtm, config.overlay_mode.clone(), &config);
    panel.orderFront(None);

    // 4. Build the transcript window.
    let transcript_log = Arc::new(Mutex::new(TranscriptLog::new(std::time::Duration::from_millis(1500))));
    let transcript_window_bundle = transcript_window::build_transcript_window(mtm, Arc::clone(&transcript_log));
    let transcript_state = transcript_window_bundle.state.clone();
    // Save-button target is held weakly by NSButton; keep the actions object alive.
    let _transcript_actions = transcript_window_bundle.actions;

    // 5. Create caption buffer with initial config.
    let max_chars = derive_max_chars(&config.appearance);
    let caption_buffer = Arc::new(Mutex::new(CaptionBuffer::new(
        config.appearance.max_lines as usize,
        max_chars,
        config.appearance.effective_expire_secs(),
    )));

    // 6. Create config Arc for command handlers.
    let config_arc = Arc::new(Mutex::new(config));

    // 7. Install drag observer for floating mode.
    let _drag_observer = drag::install_drag_observer(&panel, Arc::clone(&config_arc), mtm);
    // 7b. AC1.6 — re-apply geometry on display attach/detach.
    let _screen_observer = panel::install_screen_observer(&panel, Arc::clone(&config_arc), mtm);

    // Wrap all handles in Arc so workers can share ownership.
    let handles = Arc::new(OverlayHandles {
        panel,
        label,
        transcript_state,
        caption_buffer,
        transcript_log,
        config: config_arc,
    });

    // 8. Spawn the caption-bridge thread. It blocks on caption_rx.recv_blocking()
    // and posts each caption onto the main run loop via dispatch2 for UI update.
    let caption_captions_enabled = Arc::clone(&captions_enabled);
    let handles_copy = Arc::clone(&handles);
    std::thread::Builder::new()
        .name("caption-bridge".into())
        .spawn(move || {
            while let Ok(text) = caption_rx.recv_blocking() {
                if !caption_captions_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let handles_closure = Arc::clone(&handles_copy);
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("dispatch main queue runs on main thread");

                    // Route captions through CaptionBuffer and TranscriptLog.
                    let display = {
                        let mut buf = handles_closure.caption_buffer.lock().unwrap();
                        buf.push(text.clone());
                        buf.display_text()
                    };
                    let cfg_snapshot = handles_closure.config.lock().unwrap().clone();
                    panel::set_caption_text(
                        &handles_closure.panel,
                        &handles_closure.label,
                        &display,
                        mtm,
                        &cfg_snapshot,
                    );

                    // Append to transcript log (always, regardless of mode).
                    let mode = handles_closure.config.lock().unwrap().overlay_mode.clone();
                    {
                        let mut log = handles_closure.transcript_log.lock().unwrap();
                        log.push(text.clone());
                    }

                    // If in Transcript mode, update the window.
                    if matches!(mode, OverlayMode::Transcript) {
                        transcript_window::append_fragment(
                            &handles_closure.transcript_state,
                            mtm,
                            text,
                            chrono::Utc::now(),  // passed but not used in append_fragment
                        );
                    }
                });
            }
        })
        .expect("spawn caption-bridge thread");

    // 9. Spawn a caption-expiry timer (1 second) for CaptionBuffer.expire().
    {
        let handles_timer = Arc::clone(&handles);
        std::thread::Builder::new()
            .name("caption-expiry".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let handles_closure = Arc::clone(&handles_timer);
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        let _mtm = MainThreadMarker::new()
                            .expect("dispatch main queue runs on main thread");
                        let display_opt = {
                            let mut buf = handles_closure.caption_buffer.lock().unwrap();
                            if buf.expire() { Some(buf.display_text()) } else { None }
                        };
                        if let Some(display) = display_opt {
                            let cfg_snapshot = handles_closure.config.lock().unwrap().clone();
                            panel::set_caption_text(
                                &handles_closure.panel,
                                &handles_closure.label,
                                &display,
                                _mtm,
                                &cfg_snapshot,
                            );
                        }
                    });
                }
            })
            .expect("spawn caption-expiry thread");
    }

    // 10. Spawn the OverlayCommand dispatch loop. It blocks on cmd_rx.recv_blocking()
    // and posts each command onto the main run loop for execution.
    let cmd_captions_enabled = Arc::clone(&captions_enabled);
    let handles_copy = Arc::clone(&handles);
    std::thread::Builder::new()
        .name("overlay-cmd".into())
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv_blocking() {
                let captions_enabled = Arc::clone(&cmd_captions_enabled);
                let handles_closure = Arc::clone(&handles_copy);
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("main queue runs on main thread");
                    handle_overlay_command(
                        cmd,
                        &handles_closure,
                        mtm,
                        &captions_enabled,
                    );
                });
            }
        })
        .expect("spawn overlay-cmd thread");

    // 11. Call NSApplication.run() — blocks until terminate() is called from
    // any dispatched closure (e.g. the Quit handler above).
    app.run();

    // 12. After run() returns, workers exit on next iteration when the channels close.
    // The OverlayHandles Arc is dropped when the last worker closes.
}

/// Derive max_chars_per_line from appearance config.
///
/// Mimics the Linux formula: (width * char_width_fraction) / char_width,
/// where char_width scales with font size — a monospace glyph is roughly
/// font_size * 0.6 wide. The earlier hard-coded 8px assumed a 13pt font;
/// at larger sizes it overestimated capacity and NSTextField wrapped
/// mid-line, producing more visual lines than the buffer budgeted for
/// and clipping the bottom of the panel.
fn derive_max_chars(appearance: &crate::config::AppearanceConfig) -> usize {
    let char_width_pixels = (appearance.font_size as f64 * 0.6).max(6.0);
    let effective_width = appearance.width as f64
        * appearance.effective_char_width_fraction() as f64;
    (effective_width / char_width_pixels).max(20.0) as usize
}

/// Handle an OverlayCommand with full Phase 6 implementation.
fn handle_overlay_command(
    cmd: OverlayCommand,
    handles: &OverlayHandles,
    mtm: MainThreadMarker,
    captions_enabled: &CaptionsEnabled,
) {
    match cmd {
        OverlayCommand::Quit => {
            let app = NSApplication::sharedApplication(mtm);
            app.terminate(None);
        }
        OverlayCommand::SetAboveFullscreen(on) => {
            panel::set_above_fullscreen(&handles.panel, mtm, on);
            let mut cfg = handles.config.lock().unwrap();
            cfg.above_fullscreen = on;
            let _ = cfg.save();
        }
        OverlayCommand::SetVisible(visible) => {
            unsafe {
                if visible {
                    handles.panel.orderFront(None);
                } else {
                    handles.panel.orderOut(None);
                }
            }
        }
        OverlayCommand::SetMode(mode) => {
            let mut cfg = handles.config.lock().unwrap();
            cfg.overlay_mode = mode.clone();
            let _ = cfg.save();
            drop(cfg);

            let cfg_ref = handles.config.lock().unwrap();
            panel::apply_geometry(&handles.panel, mtm, mode.clone(), &cfg_ref);

            match mode {
                OverlayMode::Docked | OverlayMode::Floating => {
                    unsafe { handles.panel.orderFront(None); }
                    transcript_window::order_out(&handles.transcript_state, mtm);
                }
                OverlayMode::Transcript => {
                    transcript_window::order_front(&handles.transcript_state, mtm);
                    unsafe { handles.panel.orderOut(None); }
                }
            }
        }
        OverlayCommand::SetLocked(locked) => {
            {
                let mut cfg = handles.config.lock().unwrap();
                cfg.locked = locked;
                let _ = cfg.save();
            }

            let cfg = handles.config.lock().unwrap();
            if matches!(cfg.overlay_mode, OverlayMode::Floating) {
                handles.panel.setMovableByWindowBackground(!locked);
                handles.panel.setIgnoresMouseEvents(locked);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            // Update in-memory config (don't re-save — the watcher just read
            // this from disk; writing back can cause a debouncer ping-pong).
            {
                let mut cfg = handles.config.lock().unwrap();
                cfg.appearance = appearance.clone();
            }

            // Re-apply font to the label.
            let font_size = appearance.font_size as f64;
            let font: objc2::rc::Retained<objc2_app_kit::NSFont> = unsafe {
                use objc2::ClassType;
                objc2::msg_send![
                    objc2_app_kit::NSFont::class(),
                    userFixedPitchFontOfSize: font_size
                ]
            };
            handles.label.setFont(Some(&font));

            // Re-apply background color from CSS string.
            panel::apply_background_color(&handles.label, &appearance.background_color);

            // Re-apply geometry so width / max_lines changes take effect.
            let cfg_snapshot = handles.config.lock().unwrap().clone();
            panel::apply_geometry(
                &handles.panel,
                mtm,
                cfg_snapshot.overlay_mode.clone(),
                &cfg_snapshot,
            );

            // Update CaptionBuffer wrap/expiry to match new appearance.
            let max_chars = derive_max_chars(&appearance);
            handles.caption_buffer.lock().unwrap().update_config(
                appearance.max_lines as usize,
                max_chars,
                appearance.effective_expire_secs(),
            );
        }
        OverlayCommand::SetCaptionsEnabled(enabled) => {
            captions_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
            if !enabled {
                // 4-surface clear on captions disable.
                handles.transcript_log.lock().unwrap().clear();                  // surface 1
                transcript_window::clear_view(&handles.transcript_state, mtm);    // surface 2
                handles.caption_buffer.lock().unwrap().clear();                   // surface 3
                let cfg_snapshot = handles.config.lock().unwrap().clone();
                panel::set_caption_text(                                          // surface 4
                    &handles.panel,
                    &handles.label,
                    "",
                    mtm,
                    &cfg_snapshot,
                );
            }
        }
        OverlayCommand::SetCaption(_text) => {
            // SetCaption is handled via the caption-bridge; this arm is a no-op.
        }
    }
}
