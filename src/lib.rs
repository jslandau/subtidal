//! Subtidal library crate.
//!
//! The binary at `src/main.rs` is a thin orchestrator; all subsystem code lives here.
//! Linux-bound subsystems are cfg-gated: `audio/`, `tray/`, `stt/nemotron`,
//! and `overlay/linux/`. Neutral items (`config`, `models`, `overlay::caption_buffer`,
//! `overlay::transcript_log`, `audio::resampler`, `stt::SttEngine`/`AudioWake`/
//! `PipelineConfig`) compile on all targets.

pub mod audio;
pub mod config;
pub mod models;
pub mod overlay;
pub mod stt;
pub mod tray;
