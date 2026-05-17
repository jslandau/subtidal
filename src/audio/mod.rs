//! Platform-isolated audio subsystem.
//!
//! Public types (`AudioCommand`, `AudioNode`, `FallbackEvent`, `NodeList`) and entry
//! points (`start_audio_thread`, `validate_audio_source`) are re-exported from the
//! Linux implementation in `impl_linux.rs`. To add a new platform, create a sibling
//! `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export the same
//! public surface here.

pub mod resampler;

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{
    start_audio_thread, validate_audio_source, AudioCommand, AudioNode, FallbackEvent,
    NodeList,
};
