# Subtidal

Real-time speech-to-text overlay for Linux/Wayland.

Freshness: 2026-02-27

## Purpose

Captures system or per-application audio via PipeWire, runs local STT inference (Nemotron GPU or CPU), and displays live captions in a GTK4 layer-shell overlay with system tray controls.

## Architecture

```
main.rs           — CLI args, startup orchestration, thread wiring
config.rs         — TOML config with hot-reload (notify/debouncer)
models/mod.rs     — HuggingFace model download (hf-hub + tokio)
audio/mod.rs      — PipeWire capture thread, node enumeration, source switching
audio/resampler.rs — rubato 48kHz stereo -> 16kHz mono resampler
stt/mod.rs        — SttEngine trait + inference thread management
stt/nemotron.rs   — Nemotron RNNT engine (ort + parakeet-rs, CUDA)
overlay/mod.rs    — GTK4 layer-shell overlay window (docked/floating)
overlay/input_region.rs — Wayland input region for click-through
tray/mod.rs       — ksni StatusNotifierItem system tray
```

## Thread Model

Five long-lived threads communicate via typed channels:

1. **Main/GTK thread** — GTK4 main loop, polls caption_rx and cmd_rx via glib::timeout_add_local (100ms)
2. **PipeWire thread** (`pipewire-audio`) — captures audio into ring buffer, processes AudioCommand
3. **Audio bridge thread** — drains ring buffer, resamples via rubato, sends 160ms chunks to inference
4. **Inference thread** (`stt-inference`) — runs SttEngine::process_chunk, sends caption strings
5. **Engine switch thread** — listens for EngineCommand, atomically swaps chunk_tx in Arc<Mutex<>>

The system tray runs on the tokio runtime (required by ksni).

## Key Contracts

- **SttEngine trait** (`stt/mod.rs`): `process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>` — 160ms chunks of 16kHz mono f32 PCM. Returns Some(text) on recognized utterance, None when buffering.
- **Audio pipeline**: PipeWire captures 48kHz stereo F32LE -> ring buffer -> resampler produces 16kHz mono -> 160ms (2560 sample) chunks to inference.
- **Engine switching**: Arc<Mutex<SyncSender<Vec<f32>>>> is atomically replaced; old inference thread exits when its Receiver is dropped.
- **Config**: TOML at `~/.config/subtidal/config.toml`. Hot-reload only sends SetMode/SetLocked/UpdateAppearance when values actually changed (prevents drag feedback loop). Malformed TOML is warned and ignored.
- **Models**: Downloaded from HuggingFace to `~/.local/share/subtidal/models/nemotron/`. Hardlinked from HF cache when possible.
- **Nemotron engine**: 600M param RNNT model using parakeet-rs::Nemotron. Uses CUDA when available, falls back to CPU. Internally buffers 160ms chunks and emits results on 560ms boundaries.
- **Caption fragments**: Engine whitespace is preserved for word boundary detection; fragments are not trimmed/joined with spaces (fixes split words like "del ve" -> "delve").
- **Overlay drag**: Uses accumulated offset tracking to compensate for layer-shell coordinate system shift. During drag, all GTK mutations (captions, CSS, commands) are suppressed via is_dragging flag to prevent relayout jitter.
- **Audio source fallback**: When a captured PipeWire node disappears, automatically falls back to SystemOutput with desktop notification.

## Dependencies (key crates)

- gtk4 0.10 + gtk4-layer-shell 0.7 — Wayland overlay
- pipewire 0.9 — audio capture
- rubato 1.0 — sample rate conversion
- ort 2.0.0-rc.11 (cuda feature) — ONNX Runtime inference
- parakeet-rs 0.3 — Nemotron RNNT decoder
- ksni 0.3 — D-Bus StatusNotifierItem tray
- hf-hub 0.5 — model download
- notify 6 + notify-debouncer-mini 0.4 — config file watching

## Invariants

- PipeWire stream callback is real-time safe: no allocation, no blocking, try_lock only.
- GTK4 calls happen only on the main thread; channels bridge other threads.
- CUDA unavailability triggers automatic fallback to CPU execution (Nemotron runs on both GPU and CPU).
- Config save failures are warned but never fatal.
- Ring buffer overflow drops samples silently (preferred over blocking RT callback).

## Build & Run

```bash
cargo build --release
./target/release/subtidal [--engine nemotron|parakeet] [--config path] [--reset-config]
```

Requires: PipeWire running, Wayland compositor with wlr-layer-shell support. CUDA optional (GPU acceleration for Nemotron).
