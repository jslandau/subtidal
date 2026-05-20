// macOS overlay orchestration (NSPanel for caption modes; NSWindow for
// Transcript). Phase 2 ships only the Floating NSPanel + a caption-bridge
// dispatch path. Phase 6 adds Docked geometry, Transcript window, drag, and
// captions-disable surface-clearing.

pub mod panel;
pub mod drag;
pub mod transcript_window;
mod app;

pub use app::run_app;
