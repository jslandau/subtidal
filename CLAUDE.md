# Live Captions

Real-time speech-to-text overlay for Linux/Wayland.

Freshness: 2026-02-24

## Purpose

Captures system or per-application audio via PipeWire, runs local STT inference (Parakeet GPU or Moonshine CPU), and displays live captions in a GTK4 layer-shell overlay with system tray controls.

## Architecture

```
main.rs           — CLI args, startup orchestration, thread wiring
config.rs         — TOML config with hot-reload (notify/debouncer)
models/mod.rs     — HuggingFace model download (hf-hub + tokio)
audio/mod.rs      — PipeWire capture thread, node enumeration, source switching
audio/resampler.rs — rubato 48kHz stereo -> 16kHz mono resampler
stt/mod.rs        — SttEngine trait + inference thread management
stt/parakeet.rs   — Parakeet RNNT engine (ort + parakeet-rs, CUDA)
stt/moonshine.rs  — Moonshine encoder-decoder engine (ort, CPU)
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
- **Config**: TOML at `~/.config/live-captions/config.toml`. Hot-reload watches the file and sends UpdateAppearance to overlay. Malformed TOML is warned and ignored.
- **Models**: Downloaded from HuggingFace to `~/.local/share/live-captions/models/{parakeet,moonshine}/`. Hardlinked from HF cache when possible.
- **Audio source fallback**: When a captured PipeWire node disappears, automatically falls back to SystemOutput with desktop notification.

## Dependencies (key crates)

- gtk4 0.10 + gtk4-layer-shell 0.7 — Wayland overlay
- pipewire 0.9 — audio capture
- rubato 1.0 — sample rate conversion
- ort 2.0.0-rc.11 (cuda feature) — ONNX Runtime inference
- parakeet-rs 0.3 — Parakeet RNNT decoder
- ksni 0.3 — D-Bus StatusNotifierItem tray
- hf-hub 0.5 — model download
- notify 6 + notify-debouncer-mini 0.4 — config file watching

## Invariants

- PipeWire stream callback is real-time safe: no allocation, no blocking, try_lock only.
- GTK4 calls happen only on the main thread; channels bridge other threads.
- CUDA unavailability triggers automatic fallback from Parakeet to Moonshine.
- Config save failures are warned but never fatal.
- Ring buffer overflow drops samples silently (preferred over blocking RT callback).

## Build & Run

```bash
cargo build --release
./target/release/live-captions [--engine parakeet|moonshine] [--config path] [--reset-config]
```

Requires: PipeWire running, Wayland compositor with wlr-layer-shell support. CUDA optional (for Parakeet).
