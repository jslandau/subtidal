//! macOS overlay application orchestration (NSApplication startup + caption bridge).
//! Phase 2 implementation: caption bridge + OverlayCommand dispatch loop.
//! Phase 6: full OverlayCommand handlers, transcript window, drag observer, caption buffer.

use super::{drag, panel, rename_dialog, transcript_window};
use crate::config::{Config, OverlayMode};
use crate::overlay::{
    caption_buffer::CaptionBuffer, transcript_log::TranscriptLog, CaptionsEnabled, OverlayCommand,
};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSPanel, NSTextField};
use std::sync::{Arc, Mutex};

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
    cmd_tx: async_channel::Sender<OverlayCommand>,
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
    mut config: Config,
    caption_rx: async_channel::Receiver<crate::overlay::CaptionEvent>,
    cmd_rx: async_channel::Receiver<OverlayCommand>,
    cmd_tx: async_channel::Sender<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    // 1. Acquire MainThreadMarker. run_app is always called from main_macos::main,
    // which has already verified main-thread-ness, but be explicit at the boundary.
    let mtm = MainThreadMarker::new().expect("run_app must run on the main thread");

    // 2. Get NSApplication and use Regular activation policy so Transcript
    // mode participates in the Dock like a normal document-style window.
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    // 3. Repair a persisted Floating position before constructing the panel.
    // This handles a display being detached while Subtidal was not running;
    // the screen-change observer cannot see that transition retroactively.
    panel::repair_floating_position(mtm, &mut config);

    // 4. Build the overlay panel and retain both panel and content label.
    let (panel, label) = panel::build_overlay_panel(mtm, &config);
    // Apply mode-specific geometry + mouse-event state on startup so the
    // initial window state matches the configured mode.
    panel::apply_geometry(&panel, &label, mtm, config.overlay_mode.clone(), &config);

    // 4. Build the transcript window.
    let transcript_log = Arc::new(Mutex::new(TranscriptLog::new(
        std::time::Duration::from_millis(1500),
    )));
    let transcript_window_bundle =
        transcript_window::build_transcript_window(mtm, Arc::clone(&transcript_log));
    let transcript_state = transcript_window_bundle.state.clone();
    // Save-button target is held weakly by NSButton; keep the actions object alive.
    let _transcript_actions = transcript_window_bundle.actions;
    match config.overlay_mode {
        OverlayMode::Docked | OverlayMode::Floating => panel.orderFront(None),
        OverlayMode::Transcript => transcript_window::order_front(&transcript_state, mtm),
    }

    // 5. Create caption buffer with initial config.
    let max_chars = derive_max_chars(&config.appearance);
    let mut initial_caption_buffer = CaptionBuffer::new(
        config.appearance.max_lines as usize,
        max_chars,
        config.appearance.effective_expire_secs(),
    );
    initial_caption_buffer.speaker_names = config.speaker_names.clone();
    let caption_buffer = Arc::new(Mutex::new(initial_caption_buffer));

    // 6. Create config Arc for command handlers.
    let config_arc = Arc::new(Mutex::new(config));

    // 7. Install drag observer for floating mode.
    let _drag_observer = drag::install_drag_observer(&panel, Arc::clone(&config_arc), mtm);
    // 7b. AC1.6 — re-apply geometry on display attach/detach.
    let _screen_observer =
        panel::install_screen_observer(&panel, &label, Arc::clone(&config_arc), mtm);

    // Wrap all handles in Arc so workers can share ownership.
    let handles = Arc::new(OverlayHandles {
        panel,
        label,
        transcript_state,
        caption_buffer,
        transcript_log,
        config: config_arc,
        cmd_tx,
    });

    let initial_mode = handles.config.lock().unwrap().overlay_mode.clone();
    let initial_enabled = captions_enabled.load(std::sync::atomic::Ordering::Relaxed);
    reconcile_caption_surface_visibility(&handles, mtm, &initial_mode, initial_enabled);

    // 8. Spawn the caption-bridge thread. It blocks on caption_rx.recv_blocking()
    // and posts each caption onto the main run loop via dispatch2 for UI update.
    let caption_captions_enabled = Arc::clone(&captions_enabled);
    let handles_copy = Arc::clone(&handles);
    std::thread::Builder::new()
        .name("caption-bridge".into())
        .spawn(move || {
            while let Ok(event) = caption_rx.recv_blocking() {
                if !caption_captions_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let handles_closure = Arc::clone(&handles_copy);
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("dispatch main queue runs on main thread");

                    match event {
                        crate::overlay::CaptionEvent::Append { text, speaker_id, emit_sample } => {
                            // Route captions through CaptionBuffer and TranscriptLog.
                            let display = {
                                let mut buf = handles_closure.caption_buffer.lock().unwrap();
                                buf.push_with_speaker_and_sample(text.clone(), speaker_id, emit_sample);
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
                                log.push_with_speaker_and_sample(text.clone(), speaker_id, emit_sample);
                            }

                            // If in Transcript mode, rebuild from fragments so speaker labels
                            // and paragraph boundaries stay consistent with relabel/name changes.
                            if matches!(mode, OverlayMode::Transcript) {
                                let speaker_names = handles_closure.config.lock().unwrap().speaker_names.clone();
                                transcript_window::rebuild_view(
                                    &handles_closure.transcript_state,
                                    mtm,
                                    &speaker_names,
                                );
                            }
                        }
                        crate::overlay::CaptionEvent::Relabel { from_sample, new_speaker_id } => {
                            // Retroactively re-attribute. Transcript log update is
                            // unconditional; rebuild the visible transcript so late
                            // Sortformer corrections are reflected immediately.
                            let n_log = handles_closure.transcript_log
                                .lock().unwrap()
                                .relabel_since(from_sample, new_speaker_id);
                            let (n_buf, display) = {
                                let mut buf = handles_closure.caption_buffer.lock().unwrap();
                                let n = buf.relabel_since(from_sample, new_speaker_id);
                                (n, buf.display_text())
                            };
                            if n_log + n_buf > 0 {
                                eprintln!(
                                    "info: diarization: relabeled {n_log} log fragment(s), {n_buf} overlay line(s) to Speaker {}",
                                    new_speaker_id + 1,
                                );
                            }
                            if n_buf > 0 {
                                let cfg_snapshot = handles_closure.config.lock().unwrap().clone();
                                panel::set_caption_text(
                                    &handles_closure.panel,
                                    &handles_closure.label,
                                    &display,
                                    mtm,
                                    &cfg_snapshot,
                                );
                            }
                            if n_log > 0 {
                                let cfg = handles_closure.config.lock().unwrap().clone();
                                if matches!(cfg.overlay_mode, OverlayMode::Transcript) {
                                    transcript_window::rebuild_view(
                                        &handles_closure.transcript_state,
                                        mtm,
                                        &cfg.speaker_names,
                                    );
                                }
                            }
                        }
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
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let handles_closure = Arc::clone(&handles_timer);
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let _mtm =
                        MainThreadMarker::new().expect("dispatch main queue runs on main thread");
                    let display_opt = {
                        let mut buf = handles_closure.caption_buffer.lock().unwrap();
                        if buf.expire() {
                            Some(buf.display_text())
                        } else {
                            None
                        }
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
                    let mtm = MainThreadMarker::new().expect("main queue runs on main thread");
                    handle_overlay_command(cmd, &handles_closure, mtm, &captions_enabled);
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
    // Glyph width: monospace digits/letters in SF Mono / Menlo measure ~0.62×
    // font_size; round up to 0.65 to leave headroom for wider glyphs (m, w)
    // and avoid the just-barely-overflow case that NSTextField silently wraps.
    let char_width_pixels = (appearance.font_size as f64 * 0.65).max(6.0);
    // Subtract the wrapper inset on each side (panel::INSET) so the budget
    // matches the label's actual rendered width, not the panel width.
    let label_width =
        (appearance.width as f64 - 2.0 * crate::overlay::macos::panel::INSET).max(40.0);
    let effective_width = label_width * appearance.effective_char_width_fraction() as f64;
    (effective_width / char_width_pixels).max(20.0) as usize
}

fn reconcile_caption_surface_visibility(
    handles: &OverlayHandles,
    mtm: MainThreadMarker,
    mode: &OverlayMode,
    captions_enabled: bool,
) {
    if !captions_enabled {
        handles.panel.orderOut(None);
        transcript_window::order_out(&handles.transcript_state, mtm);
        return;
    }

    match mode {
        OverlayMode::Docked | OverlayMode::Floating => {
            handles.panel.orderFront(None);
            transcript_window::order_out(&handles.transcript_state, mtm);
        }
        OverlayMode::Transcript => {
            handles.panel.orderOut(None);
            transcript_window::order_front(&handles.transcript_state, mtm);
        }
    }
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
            // Compatibility shim: route visibility through the same surface
            // reconciliation used by mode and captions-enabled changes.
            let mode = handles.config.lock().unwrap().overlay_mode.clone();
            reconcile_caption_surface_visibility(handles, mtm, &mode, visible);
        }
        OverlayCommand::SetMode(mode) => {
            // Snapshot then drop the lock before apply_geometry. setFrame_display
            // synchronously fires NSWindowDidMoveNotification, which the drag
            // observer handles by locking the same config — holding the lock
            // here would re-enter and deadlock (or crash on macOS pthread).
            let cfg_snapshot = {
                let mut cfg = handles.config.lock().unwrap();
                cfg.overlay_mode = mode.clone();
                let _ = cfg.save();
                cfg.clone()
            };

            panel::apply_geometry(
                &handles.panel,
                &handles.label,
                mtm,
                mode.clone(),
                &cfg_snapshot,
            );
            let enabled = captions_enabled.load(std::sync::atomic::Ordering::Relaxed);
            reconcile_caption_surface_visibility(handles, mtm, &mode, enabled);
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
        OverlayCommand::ResetFloatingPosition => {
            let cfg_snapshot = {
                let mut cfg = handles.config.lock().unwrap();
                cfg.position = crate::config::OverlayPosition::default();
                if let Err(e) = cfg.save() {
                    eprintln!("warn: failed to save reset floating position: {e}");
                }
                cfg.clone()
            };
            if matches!(cfg_snapshot.overlay_mode, OverlayMode::Floating) {
                panel::apply_geometry(
                    &handles.panel,
                    &handles.label,
                    mtm,
                    OverlayMode::Floating,
                    &cfg_snapshot,
                );
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            // Update in-memory config (don't re-save — the watcher just read
            // this from disk; writing back can cause a debouncer ping-pong).
            {
                let mut cfg = handles.config.lock().unwrap();
                cfg.appearance = appearance.clone();
            }

            // Re-apply font + text color.
            let font =
                panel::resolve_font(&appearance.font_family, appearance.font_size as f64, mtm);
            handles.label.setFont(Some(&font));
            handles
                .label
                .setTextColor(Some(&panel::resolve_text_color(&appearance.text_color)));

            // Re-apply background color from CSS string. The wrapper view
            // (panel's contentView) carries the rounded translucent layer.
            if let Some(wrapper) = handles.panel.contentView() {
                panel::apply_background_color(&wrapper, &appearance.background_color);
            }

            // Re-apply geometry so width / max_lines changes take effect.
            let cfg_snapshot = handles.config.lock().unwrap().clone();
            panel::apply_geometry(
                &handles.panel,
                &handles.label,
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
            let mode = handles.config.lock().unwrap().overlay_mode.clone();
            if !enabled {
                // 4-surface clear on captions disable.
                handles.transcript_log.lock().unwrap().clear(); // surface 1
                transcript_window::clear_view(&handles.transcript_state, mtm); // surface 2
                handles.caption_buffer.lock().unwrap().clear(); // surface 3
                let cfg_snapshot = handles.config.lock().unwrap().clone();
                panel::set_caption_text(
                    // surface 4
                    &handles.panel,
                    &handles.label,
                    "",
                    mtm,
                    &cfg_snapshot,
                );
            }
            reconcile_caption_surface_visibility(handles, mtm, &mode, enabled);
        }
        OverlayCommand::SetCaption(_text) => {
            // SetCaption is handled via the caption-bridge; this arm is a no-op.
        }
        OverlayCommand::SetSpeakerNames(names) => {
            let old_names = {
                let mut cfg = handles.config.lock().unwrap();
                let old = cfg.speaker_names.clone();
                cfg.speaker_names = names.clone();
                old
            };

            let display = {
                let mut buf = handles.caption_buffer.lock().unwrap();
                rewrite_embedded_labels(&mut buf, &old_names, &names);
                buf.speaker_names = names.clone();
                buf.display_text()
            };

            let cfg_snapshot = handles.config.lock().unwrap().clone();
            if matches!(
                cfg_snapshot.overlay_mode,
                OverlayMode::Docked | OverlayMode::Floating
            ) {
                panel::set_caption_text(
                    &handles.panel,
                    &handles.label,
                    &display,
                    mtm,
                    &cfg_snapshot,
                );
            }
            transcript_window::rebuild_view(&handles.transcript_state, mtm, &names);
        }
        OverlayCommand::ShowRenameDialog => {
            let current = handles.config.lock().unwrap().speaker_names.clone();
            rename_dialog::show_rename_dialog(current, handles.cmd_tx.clone(), mtm);
        }
    }
}

/// Intentionally mirrors Linux's speaker-name relabel behavior for existing
/// overlay lines: only rewrite a leading `"old label: "` prefix, preserving the
/// rest of the line so live caption layout does not reflow on rename.
fn rewrite_embedded_labels(
    buf: &mut CaptionBuffer,
    old_names: &std::collections::HashMap<u32, String>,
    new_names: &std::collections::HashMap<u32, String>,
) {
    for line in &mut buf.lines {
        let Some(id) = line.speaker_id else { continue };
        let old_label = old_names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("Speaker {}", id + 1));
        let old_prefix = format!("{old_label}: ");
        if line.text.starts_with(&old_prefix) {
            let new_label = new_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("Speaker {}", id + 1));
            let new_prefix = format!("{new_label}: ");
            line.text = format!("{new_prefix}{}", &line.text[old_prefix.len()..]);
        }
    }
}
