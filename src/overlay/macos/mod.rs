// macOS overlay orchestration (NSPanel for caption modes; NSWindow for
// Transcript). Phase 2 ships only the Floating NSPanel + a caption-bridge
// dispatch path. Phase 6 adds Docked geometry, Transcript window, drag, and
// captions-disable surface-clearing.

mod app;
pub mod drag;
pub mod panel;
pub mod rename_dialog;
#[cfg(feature = "native-transcript-tests")]
mod transcript_native_tests;
mod transcript_routing;
pub mod transcript_window;
#[cfg(feature = "native-transcript-tests")]
pub use transcript_native_tests::run as run_native_transcript_scenarios;

pub use app::run_app;
