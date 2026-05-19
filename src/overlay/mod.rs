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
// No re-exports yet; macOS public surface is built up in Phase 2.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::config::{AppearanceConfig, OverlayMode};

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
    /// Quit the application cleanly (sent by tray Quit and SIGTERM handler).
    Quit,
}

pub type CaptionsEnabled = Arc<AtomicBool>;
