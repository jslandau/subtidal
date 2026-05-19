//! Subtidal library crate.
//!
//! The binary at `src/main.rs` is a thin orchestrator; all subsystem code lives here.
//! Linux-bound subsystems are cfg-gated: `audio/`, `tray/`, `stt/nemotron`,
//! `overlay/linux/`, and `models/` (the last because its `hf-hub` dependency
//! transitively pulls in `ring`, whose build script doesn't cross-compile from
//! Linux to darwin). Neutral items (`config`, `overlay::caption_buffer`,
//! `overlay::transcript_log`, `audio::resampler`, `stt::SttEngine`/`AudioWake`/
//! `PipelineConfig`) compile on all targets.

pub mod audio;
pub mod config;
pub mod models;
pub mod overlay;
pub mod stt;
pub mod tray;
