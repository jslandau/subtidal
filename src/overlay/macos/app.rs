//! macOS overlay application orchestration (NSApplication startup + caption bridge).
//! Phase 2 implementation: caption bridge + OverlayCommand dispatch loop.
//! Phase 3+ wire in real STT pipeline and system tray.

use std::sync::Arc;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSPanel, NSTextField, NSApplicationActivationPolicy};
use objc2::rc::Retained;
use objc2_foundation::NSString;
use objc2::msg_send;
use crate::config::Config;
use crate::overlay::{OverlayCommand, CaptionsEnabled};
use super::panel;

/// Handles to overlay UI elements (panel and caption label).
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
    panel.orderFront(None);

    // Wrap panel and label in Arc so workers can share ownership.
    let handles = Arc::new(OverlayHandles { panel, label });

    // 4. Spawn the caption-bridge thread. It blocks on caption_rx.recv_blocking()
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
                    let _mtm = MainThreadMarker::new()
                        .expect("dispatch main queue runs on main thread");
                    let ns_text = NSString::from_str(&text);
                    unsafe { let _: () = msg_send![&*handles_closure.label, setStringValue: &*ns_text]; }
                    // CaptionBuffer / TranscriptLog integration deferred to Phase 6.
                });
            }
        })
        .expect("spawn caption-bridge thread");

    // 5. Spawn the OverlayCommand dispatch loop. It blocks on cmd_rx.recv_blocking()
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
                    match cmd {
                        OverlayCommand::Quit => {
                            let app = NSApplication::sharedApplication(mtm);
                            app.terminate(None);
                        }
                        OverlayCommand::SetAboveFullscreen(on) => {
                            panel::set_above_fullscreen(&handles_closure.panel, mtm, on);
                        }
                        OverlayCommand::SetCaptionsEnabled(on) => {
                            captions_enabled.store(on, std::sync::atomic::Ordering::Relaxed);
                            if !on {
                                // Phase 2: clear the label only.
                                // Phase 6 extends to all 4 surfaces.
                                let ns_empty = NSString::from_str("");
                                unsafe { let _: () = msg_send![&*handles_closure.label, setStringValue: &*ns_empty]; }
                            }
                        }
                        // Phase 2 stubs — Phase 6 fills in real handlers.
                        OverlayCommand::SetVisible(_)
                        | OverlayCommand::SetMode(_)
                        | OverlayCommand::SetLocked(_)
                        | OverlayCommand::UpdateAppearance(_)
                        | OverlayCommand::SetCaption(_) => {
                            eprintln!("info: OverlayCommand {:?} deferred to Phase 6", cmd);
                        }
                    }
                });
            }
        })
        .expect("spawn overlay-cmd thread");

    // 6. Call NSApplication.run() — blocks until terminate() is called from
    // any dispatched closure (e.g. the Quit handler above).
    app.run();

    // 7. After run() returns, workers exit on next iteration when the channels close.
    // The OverlayHandles Arc is dropped when the last worker closes.
}
