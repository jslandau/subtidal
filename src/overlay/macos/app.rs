//! macOS overlay application orchestration (NSApplication startup + caption bridge).
//! Phase 2 implementation: caption bridge + OverlayCommand dispatch loop.
//! Phase 3+ wire in real STT pipeline and system tray.

#[allow(dead_code)]
use crate::config::Config;
use crate::overlay::{OverlayCommand, CaptionsEnabled};

/// Run the macOS overlay application.
///
/// Entry point for the overlay subsystem. Acquires MainThreadMarker,
/// initializes NSApplication, builds the overlay panel, spawns caption bridge
/// and command dispatch threads, and blocks in NSApplication.run() until shutdown.
#[allow(dead_code)]
pub fn run_app(
    _config: Config,
    _caption_rx: async_channel::Receiver<String>,
    _cmd_rx: async_channel::Receiver<OverlayCommand>,
    _captions_enabled: CaptionsEnabled,
) {
    // Phase 2 placeholder. Phase 3 implements full startup orchestration.
    todo!("Task 3: implement run_app");
}
