//! Platform-isolated overlay subsystem.
//!
//! Neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`,
//! `transcript_log`) live here. The Linux GTK + layer-shell implementation is in
//! `linux/`, gated behind `#[cfg(target_os = "linux")]`. To add a new platform,
//! create a sibling subdirectory (e.g. `macos/`), gate it analogously, and
//! re-export the platform-specific entry points from here.

pub mod caption_buffer;
pub mod transcript_follow;
pub mod transcript_log;
pub mod transcript_presentation;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::run_gtk_app;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::run_app;

#[cfg(all(target_os = "macos", feature = "native-transcript-tests"))]
pub use macos::run_native_transcript_scenarios;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
    /// Restore the floating overlay to its default position.
    ResetFloatingPosition,
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

// ---------------------------------------------------------------------
// Transcript routing (F8): neutral, Linux-testable decision for how the
// macOS transcript view must be updated per caption event, so the per-
// caption path can append incrementally instead of rebuilding the whole
// session history on every fragment.
// ---------------------------------------------------------------------

/// Pre-derived caption-render config cached on the macOS overlay handles so
/// the per-caption path takes exactly one cheap lock instead of cloning the
/// full `Config` two to three times per caption. Refreshed by
/// `UpdateAppearance` / `SetSpeakerNames` / `SetMode`.
///
/// Distinct from [`LogSnapshot`]: this carries caption-render config, that
/// carries transcript-log state.
#[derive(Clone, Debug)]
pub struct AppearanceSnapshot {
    pub font_family: String,
    pub font_size: f64,
    pub max_lines: usize,
    pub overlay_mode: OverlayMode,
    pub speaker_names: std::collections::HashMap<u32, String>,
}

impl AppearanceSnapshot {
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Self {
            font_family: cfg.appearance.font_family.clone(),
            font_size: cfg.appearance.font_size as f64,
            max_lines: cfg.appearance.max_lines as usize,
            overlay_mode: cfg.overlay_mode.clone(),
            speaker_names: cfg.speaker_names.clone(),
        }
    }
}

/// Events consumed by the transcript routing decision: constructed by the
/// macOS caption-event closure (Append/Relabel) and the OverlayCommand
/// handlers (SetSpeakerNames/SetMode/SetCaptionsEnabled → the rest).
#[derive(Clone, Debug)]
pub enum TranscriptRouteEvent {
    Append {
        text: String,
        speaker_id: Option<u32>,
        emit_sample: u64,
    },
    Relabel {
        from_sample: u64,
        new_speaker_id: u32,
    },
    SetSpeakerNames,
    ModeChanged,
    Clear,
}

/// The transcript-log state the paragraph-break and speaker-label rules
/// read, snapshotted **before** the incoming fragment is pushed. Timestamps
/// are `DateTime<Local>` to match `TranscriptLog`'s fragment type.
#[derive(Clone, Debug)]
pub struct LogSnapshot {
    pub last_speaker_id: Option<u32>,
    pub last_emit_sample: u64,
    pub last_timestamp: Option<chrono::DateTime<chrono::Local>>,
    /// Speaker display names for label rendering.
    pub speaker_names: std::collections::HashMap<u32, String>,
    /// Wall clock used to stamp and gap-check the incoming fragment (the
    /// caller pushes with this same timestamp so the incremental view stays
    /// byte-identical to a rebuild).
    pub now: chrono::DateTime<chrono::Local>,
    /// Gap above which a new paragraph starts.
    pub paragraph_gap: std::time::Duration,
}

impl LogSnapshot {
    /// Snapshot the routing-relevant state of a `TranscriptLog` (before a push).
    pub fn from_log(
        log: &transcript_log::TranscriptLog,
        speaker_names: std::collections::HashMap<u32, String>,
        now: chrono::DateTime<chrono::Local>,
        paragraph_gap: std::time::Duration,
    ) -> Self {
        let (last_speaker_id, last_emit_sample, last_timestamp) = match log.fragments().last() {
            Some(f) => (f.speaker_id, f.emit_sample, Some(f.timestamp)),
            None => (None, 0, None),
        };
        LogSnapshot {
            last_speaker_id,
            last_emit_sample,
            last_timestamp,
            speaker_names,
            now,
            paragraph_gap,
        }
    }
}

/// How the transcript view must be updated for one routed event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TranscriptUpdate {
    /// Apply only this fully formatted fragment text (leading paragraph
    /// break, `[HH:MM:SS] ` timestamp, speaker label included) by appending
    /// at the tail of the view.
    AppendFormatted(String),
    /// Full re-render from log state (Relabel / rename / mode change / clear).
    Rebuild,
}

/// The routing decision: `Append` produces the formatted tail fragment
/// (mirroring `transcript_window::format_fragments`'s paragraph-break and
/// speaker-label rules exactly); every other variant requires a rebuild.
pub fn append_decision_routes_events(
    event: &TranscriptRouteEvent,
    log: &LogSnapshot,
) -> TranscriptUpdate {
    match event {
        TranscriptRouteEvent::Append {
            text,
            speaker_id,
            emit_sample: _,
        } => {
            let speaker_changed = speaker_id.is_some()
                && log.last_speaker_id.is_some()
                && *speaker_id != log.last_speaker_id;
            let gap_exceeded = log.last_timestamp.is_some_and(|prev| {
                log.now
                    .signed_duration_since(prev)
                    .to_std()
                    .unwrap_or(std::time::Duration::ZERO)
                    > log.paragraph_gap
            });
            let new_paragraph = log.last_timestamp.is_none() || speaker_changed || gap_exceeded;

            let mut out = String::new();
            if new_paragraph {
                // A leading '\n' separates from prior content — but the very
                // first fragment of the session starts the document.
                if log.last_timestamp.is_some() {
                    out.push('\n');
                }
                out.push_str(&log.now.format("[%H:%M:%S] ").to_string());
                if let Some(id) = speaker_id {
                    out.push_str(&speaker_display_label(&log.speaker_names, *id));
                    out.push_str(": ");
                }
            }
            out.push_str(text);
            TranscriptUpdate::AppendFormatted(out)
        }
        TranscriptRouteEvent::Relabel { .. }
        | TranscriptRouteEvent::SetSpeakerNames
        | TranscriptRouteEvent::ModeChanged
        | TranscriptRouteEvent::Clear => TranscriptUpdate::Rebuild,
    }
}

fn speaker_display_label(names: &std::collections::HashMap<u32, String>, id: u32) -> String {
    names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("Speaker {}", id + 1))
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use chrono::TimeZone;

    fn base_snapshot(now: chrono::DateTime<chrono::Local>) -> LogSnapshot {
        LogSnapshot {
            last_speaker_id: Some(0),
            last_emit_sample: 16_000,
            last_timestamp: Some(now - chrono::Duration::milliseconds(200)),
            speaker_names: std::collections::HashMap::new(),
            now,
            paragraph_gap: std::time::Duration::from_millis(1500),
        }
    }

    /// AC.8: every `TranscriptRouteEvent` variant routes correctly — Append
    /// to the incremental path, everything else to a rebuild.
    /// (Calls go through `super::` because the test shares the function's
    /// name.)
    #[test]
    fn append_decision_routes_events() {
        let now = chrono::Local.timestamp_opt(10_000, 0).unwrap();

        // Append → AppendFormatted; continuation within the gap.
        let ev = TranscriptRouteEvent::Append {
            text: " hello".to_string(),
            speaker_id: Some(0),
            emit_sample: 32_000,
        };
        match super::append_decision_routes_events(&ev, &base_snapshot(now)) {
            TranscriptUpdate::AppendFormatted(frag) => {
                assert_eq!(frag, " hello", "continuation appends bare text");
            }
            other => panic!("Append must route to AppendFormatted, got {other:?}"),
        }

        // Append with a speaker change → new paragraph with timestamp+label.
        let ev = TranscriptRouteEvent::Append {
            text: " hi".to_string(),
            speaker_id: Some(1),
            emit_sample: 32_000,
        };
        match super::append_decision_routes_events(&ev, &base_snapshot(now)) {
            TranscriptUpdate::AppendFormatted(frag) => {
                assert!(
                    frag.starts_with('\n'),
                    "new paragraph breaks the line: {frag:?}"
                );
                assert!(frag.starts_with("\n["), "timestamp prefix: {frag:?}");
                assert!(frag.ends_with("Speaker 2:  hi"), "speaker label: {frag:?}");
            }
            other => panic!("Append must route to AppendFormatted, got {other:?}"),
        }

        // Gap-exceeded → new paragraph even for the same speaker.
        let mut snap = base_snapshot(now);
        snap.last_timestamp = Some(now - chrono::Duration::seconds(5));
        let ev = TranscriptRouteEvent::Append {
            text: " later".to_string(),
            speaker_id: Some(0),
            emit_sample: 32_000,
        };
        match super::append_decision_routes_events(&ev, &snap) {
            TranscriptUpdate::AppendFormatted(frag) => {
                assert!(
                    frag.starts_with('\n'),
                    "gap forces a new paragraph: {frag:?}"
                );
            }
            other => panic!("Append must route to AppendFormatted, got {other:?}"),
        }

        // First fragment of the session: new paragraph WITHOUT leading '\n'.
        let mut snap = base_snapshot(now);
        snap.last_timestamp = None;
        snap.last_speaker_id = None;
        let ev = TranscriptRouteEvent::Append {
            text: " first".to_string(),
            speaker_id: Some(0),
            emit_sample: 1,
        };
        match super::append_decision_routes_events(&ev, &snap) {
            TranscriptUpdate::AppendFormatted(frag) => {
                assert!(
                    frag.starts_with('['),
                    "first fragment starts the document: {frag:?}"
                );
            }
            other => panic!("Append must route to AppendFormatted, got {other:?}"),
        }

        // Custom speaker names are honored.
        let mut snap = base_snapshot(now);
        snap.speaker_names.insert(1, "Bob".to_string());
        let ev = TranscriptRouteEvent::Append {
            text: " x".to_string(),
            speaker_id: Some(1),
            emit_sample: 2,
        };
        match super::append_decision_routes_events(&ev, &snap) {
            TranscriptUpdate::AppendFormatted(frag) => {
                assert!(frag.contains("Bob:  x"), "{frag:?}");
            }
            other => panic!("Append must route to AppendFormatted, got {other:?}"),
        }

        // Every other variant → Rebuild.
        for ev in [
            TranscriptRouteEvent::Relabel {
                from_sample: 1,
                new_speaker_id: 2,
            },
            TranscriptRouteEvent::SetSpeakerNames,
            TranscriptRouteEvent::ModeChanged,
            TranscriptRouteEvent::Clear,
        ] {
            assert_eq!(
                super::append_decision_routes_events(&ev, &base_snapshot(now)),
                TranscriptUpdate::Rebuild,
                "{ev:?} must route to Rebuild"
            );
        }
    }

    /// AC.8: the single-snapshot struct supplies everything the caption path
    /// needs — font, lines, mode, speaker names — so the multi-lock pattern
    /// cannot silently return.
    #[test]
    fn caption_snapshot_contains_required_fields() {
        let cfg = crate::config::Config::default();
        let snap = AppearanceSnapshot::from_config(&cfg);
        // Fields the caption path reads (set_caption_text + transcript
        // routing): font_size, max_lines, overlay_mode, speaker_names, and
        // font_family for appearance refreshes.
        assert_eq!(snap.font_size, cfg.appearance.font_size as f64);
        assert_eq!(snap.max_lines, cfg.appearance.max_lines as usize);
        assert_eq!(snap.overlay_mode, cfg.overlay_mode);
        assert_eq!(snap.speaker_names, cfg.speaker_names);
        assert_eq!(snap.font_family, cfg.appearance.font_family);
    }
}
