//! Platform-isolated overlay subsystem.
//!
//! Neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`,
//! `transcript_log`) live here. The Linux GTK + layer-shell implementation is in
//! `linux/`, gated behind `#[cfg(target_os = "linux")]`. To add a new platform,
//! create a sibling subdirectory (e.g. `macos/`), gate it analogously, and
//! re-export the platform-specific entry points from here.

pub mod caption_buffer;
pub mod transcript_log;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::run_gtk_app;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::run_app;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::config::{AppearanceConfig, OverlayMode};

/// An event emitted by the STT pipeline to the overlay.
///
/// The pipeline emits `Append` for each Nemotron output and `Relabel` when
/// Sortformer reports a speaker switch that began earlier than the most
/// recent appends (retroactive attribution). Ordering on the channel is
/// preserved, so a `Relabel` always arrives before subsequent `Append`s
/// from the new speaker.
#[derive(Debug, Clone)]
pub enum CaptionEvent {
    /// A new recognised caption fragment.
    ///
    /// `text`: Nemotron output (may include leading/trailing whitespace
    /// that signals word boundaries).
    /// `speaker_id`: dominant speaker for this audio window when
    /// diarization is active, otherwise `None`. May be later corrected by
    /// a `Relabel` event whose `from_sample <= emit_sample`.
    /// `emit_sample`: count of 16 kHz mono samples fed to the diarization
    /// engine at the time this fragment was emitted. `0` when diarization
    /// is off — used by `Relabel` to identify which captions to rewrite.
    Append {
        text: String,
        speaker_id: Option<u32>,
        emit_sample: u64,
    },
    /// Retroactively re-attribute captions emitted at or after `from_sample`
    /// to `new_speaker_id`. Issued when Sortformer reveals (at most a few
    /// captions late) that the speaker actually switched at `from_sample`.
    Relabel {
        from_sample: u64,
        new_speaker_id: u32,
    },
}

#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Show or hide the overlay.
    SetVisible(bool),
    /// Switch overlay mode (docked ↔ floating).
    SetMode(OverlayMode),
    /// Lock or unlock the floating overlay.
    SetLocked(bool),
    /// Toggle whether the overlay renders above fullscreen windows
    /// (switches the layer-shell layer between Top and Overlay).
    SetAboveFullscreen(bool),
    /// Update appearance from config.
    UpdateAppearance(AppearanceConfig),
    /// Update caption text (also sent as plain String via glib channel in normal flow).
    #[allow(dead_code)]
    SetCaption(String),
    /// Enable or disable caption emission. On the disable edge the overlay
    /// clears all four caption surfaces (TranscriptLog, transcript view's
    /// TextBuffer, CaptionBuffer, overlay label). On the enable edge no
    /// action is needed — the prior disable already cleared everything.
    SetCaptionsEnabled(bool),
    /// Update speaker display names. The HashMap maps 0-based speaker IDs
    /// to user-chosen display names (e.g. 0 → "Alice"). Unmapped speakers
    /// render as "Speaker {id+1}".
    SetSpeakerNames(std::collections::HashMap<u32, String>),
    /// Show the speaker rename dialog (triggered from tray).
    ShowRenameDialog,
    /// Quit the application cleanly (sent by tray Quit and SIGTERM handler).
    Quit,
}

pub type CaptionsEnabled = Arc<AtomicBool>;
