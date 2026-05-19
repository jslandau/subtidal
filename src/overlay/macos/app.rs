//! macOS overlay application orchestration (NSApplication startup + caption bridge).
//! Phase 2 implementation: caption bridge + OverlayCommand dispatch loop.
//! Phase 3+ wire in real STT pipeline and system tray.

use std::sync::Arc;
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::NSString;
use objc2::{msg_send};
use crate::config::Config;
use crate::overlay::{OverlayCommand, CaptionsEnabled};
use super::panel;

/// Sendable wrapper for raw pointers passed to worker threads using usize encoding.
/// SAFETY: The wrapped address outlives the dispatch2 callbacks that dereference it.
/// Both the caption-bridge and overlay-command threads only dereference their pointers
/// within dispatch2::DispatchQueue::main().exec_async closures, and the main queue is AppKit-affine.
/// The panel and label are retained for the lifetime of NSApplication.run(), which only
/// returns after termination (triggered by Quit handler or signal).
#[derive(Copy, Clone)]
struct SendablePtr(usize);

/// Run the macOS overlay application.
///
/// Entry point for the overlay subsystem. Acquires MainThreadMarker,
/// initializes NSApplication, builds the overlay panel, spawns caption bridge
/// and command dispatch threads, and blocks in NSApplication.run() until shutdown.
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
    // NSApplicationActivationPolicy values: Regular=0, Accessory=1, Prohibited=2.
    // Accessory matches LSUIElement=true in Info.plist: no Dock icon, UI allowed.
    let activation_policy: isize = 1;
    unsafe { let _: () = msg_send![&app, setActivationPolicy: activation_policy]; }

    // 3. Build the overlay panel and retain both panel and content label.
    let (panel, label) = panel::build_overlay_panel(mtm, &config);
    unsafe { let _: () = msg_send![&panel, orderFront: std::ptr::null::<std::ffi::c_void>()]; }

    // 4. Spawn the caption-bridge thread. It blocks on caption_rx.recv_blocking()
    // and posts each caption onto the main run loop via dispatch2 for UI update.
    let label_ptr_value = SendablePtr(objc2::rc::Retained::as_ptr(&label) as usize);
    let caption_captions_enabled = Arc::clone(&captions_enabled);
    std::thread::Builder::new()
        .name("caption-bridge".into())
        .spawn(move || {
            while let Ok(text) = caption_rx.recv_blocking() {
                if !caption_captions_enabled.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let text_copy = text.clone();
                let label_ptr = label_ptr_value;
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let _mtm = MainThreadMarker::new()
                        .expect("dispatch main queue runs on main thread");
                    unsafe {
                        let label_ref: &objc2_app_kit::NSTextField = &*(label_ptr.0 as *const objc2_app_kit::NSTextField);
                        let ns_text = NSString::from_str(&text_copy);
                        let _: () = msg_send![label_ref, setStringValue: &*ns_text];
                        // CaptionBuffer / TranscriptLog integration deferred to Phase 6.
                    }
                });
            }
        })
        .expect("spawn caption-bridge thread");

    // 5. Spawn the OverlayCommand dispatch loop. It blocks on cmd_rx.recv_blocking()
    // and posts each command onto the main run loop for execution.
    let panel_ptr_value = SendablePtr(objc2::rc::Retained::as_ptr(&panel) as usize);
    let label_ptr_value2 = SendablePtr(objc2::rc::Retained::as_ptr(&label) as usize);
    let cmd_captions_enabled = Arc::clone(&captions_enabled);
    std::thread::Builder::new()
        .name("overlay-cmd".into())
        .spawn(move || {
            while let Ok(cmd) = cmd_rx.recv_blocking() {
                let captions_enabled = Arc::clone(&cmd_captions_enabled);
                let p_ptr = panel_ptr_value;
                let l_ptr = label_ptr_value2;
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let mtm = MainThreadMarker::new()
                        .expect("main queue runs on main thread");
                    match cmd {
                        OverlayCommand::Quit => {
                            let app = NSApplication::sharedApplication(mtm);
                            unsafe { let _: () = msg_send![&app, terminate: std::ptr::null::<std::ffi::c_void>()]; }
                        }
                        OverlayCommand::SetAboveFullscreen(on) => {
                            let panel_ref = unsafe { &*(p_ptr.0 as *const objc2_app_kit::NSPanel) };
                            panel::set_above_fullscreen(panel_ref, mtm, on);
                        }
                        OverlayCommand::SetCaptionsEnabled(on) => {
                            captions_enabled.store(on, std::sync::atomic::Ordering::Relaxed);
                            if !on {
                                // Phase 2: clear the label only.
                                // Phase 6 extends to all 4 surfaces.
                                unsafe {
                                    let label_ref: &objc2_app_kit::NSTextField = &*(l_ptr.0 as *const objc2_app_kit::NSTextField);
                                    let ns_empty = NSString::from_str("");
                                    let _: () = msg_send![label_ref, setStringValue: &*ns_empty];
                                }
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

    // 6. Call NSApplication.run() — blocks until terminate(None) is called from
    // any dispatched closure (e.g. the Quit handler above).
    unsafe { let _: () = msg_send![&app, run]; }

    // 7. After run() returns, workers exit on next iteration when the channels close.
}
