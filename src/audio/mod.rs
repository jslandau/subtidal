//! Platform-isolated audio subsystem.
//!
//! Public types (`AudioCommand`, `AudioNode`, `NodeList`) and entry points
//! (`start_audio_thread`, `validate_audio_source`) are re-exported from the
//! platform implementations. To add a new platform, create a sibling
//! `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export the same
//! public surface here.

pub mod resampler;
pub mod validation;

pub use validation::{
    audio_buffer_f32, extract_f32, push_pw_chunk, tap_format_ok, tap_format_violation,
};

/// Neutral identifier surfaced to the tray menu. Phase 5 introduces this for
/// macOS; Linux's existing `AudioNode` continues alongside.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSourceInfo {
    pub source: crate::config::AudioSource,
    pub label: String,
}

/// Sent from the audio thread when a captured source disappears and the
/// thread has auto-switched to SystemOutput.
#[derive(Debug, Clone)]
pub struct FallbackEvent {
    pub previous_label: String,
    pub new_source: crate::config::AudioSource,
}

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{
    start_audio_thread, validate_audio_source, AudioCommand, AudioNode, NodeList,
};

#[cfg(target_os = "macos")]
mod impl_macos;

#[cfg(target_os = "macos")]
pub use impl_macos::{
    bundle_to_label, enumerate_process_snapshot, list_sources,
    notify_request_authorization_best_effort, start_audio_thread, AudioCommand,
};
