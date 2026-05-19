//! macOS startup entry point.
//! Phase 2 implementation: NSApplication orchestration with test caption harness.
//! Phases 3-4 wire in the real STT pipeline, audio capture, and config hot-reload.

use objc2::MainThreadMarker;
use subtidal::config::Config;
use subtidal::overlay::{self, OverlayCommand, CaptionsEnabled};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Phase 2 test caption harness: pushes hardcoded captions to prove AC8.1
/// (main-thread caption delivery without AppKit affinity warnings).
/// Remove this when the real STT thread lands in Phase 3.
fn spawn_test_caption_harness(tx: async_channel::Sender<String>) {
    std::thread::Builder::new()
        .name("test-caption-harness".into())
        .spawn(move || {
            let samples = [
                "Hello",
                "Hello, world",
                "Hello, world, from macOS Subtidal",
            ];
            for s in samples {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                if tx.send_blocking(s.to_string()).is_err() {
                    return;
                }
            }
            // Then idle — leaves the panel showing the last caption.
        })
        .expect("spawn test-caption-harness");
}

/// Main entry point for macOS Subtidal.
/// Acquires main-thread proof, loads config, builds shared state, spawns workers,
/// and calls overlay::run_app to block in NSApplication.run() until shutdown.
pub fn main() {
    // 1. Acquire MainThreadMarker at the very top. main() always starts on the main
    // thread, but be explicit.
    let _mtm = MainThreadMarker::new()
        .expect("main_macos::main must run on the main thread");

    // 2. Load config (use the neutral Config::load() which defaults gracefully).
    let config = Config::load();

    // 3. Construct shared state: captions_enabled flag for the caption bridge.
    let captions_enabled: CaptionsEnabled =
        Arc::new(AtomicBool::new(true));

    // 4. Build async channels for captions and overlay commands.
    let (caption_tx, caption_rx) = async_channel::unbounded::<String>();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<OverlayCommand>();

    // 5. Phase 2 only: spawn test caption harness to prove AC8.1 end-to-end.
    // Remove when STT thread lands in Phase 3.
    spawn_test_caption_harness(caption_tx.clone());

    // 6. Install Ctrl-C handler to post OverlayCommand::Quit.
    let cmd_tx_signal = cmd_tx.clone();
    ctrlc::set_handler(move || {
        let _ = cmd_tx_signal.send_blocking(OverlayCommand::Quit);
    })
    .expect("install ctrlc handler");

    // 7. Call overlay::run_app to build the panel and run NSApplication.run().
    // This blocks until Quit is posted (from ctrlc or signal handler).
    overlay::run_app(config, caption_rx, cmd_rx, captions_enabled);

    // 8. After run_app returns, drop the senders so worker threads see closure.
    drop(caption_tx);
    drop(cmd_tx);

    // 9. Exit cleanly with code 0 (no CUDA atexit cleanup on macOS).
    std::process::exit(0);
}
