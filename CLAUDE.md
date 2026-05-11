# Subtidal

Real-time speech-to-text overlay for Linux/Wayland.

Freshness: 2026-05-11

## Purpose

Captures system or per-application audio via PipeWire, runs local STT inference (Nemotron GPU or CPU), and displays live captions in a GTK4 layer-shell overlay with system tray controls.

## Architecture

```
main.rs           — CLI args, startup orchestration, thread wiring
config.rs         — TOML config with hot-reload (notify/debouncer)
models/mod.rs     — HuggingFace model download (hf-hub + tokio)
audio/mod.rs      — PipeWire capture thread, node enumeration, source switching
audio/resampler.rs — rubato 48kHz stereo -> 16kHz mono resampler
stt/mod.rs        — SttEngine trait, AudioWake (condvar), combined STT pipeline thread
stt/nemotron.rs   — Nemotron RNNT engine (ort + parakeet-rs, CUDA)
overlay/mod.rs    — overlay orchestration, OverlayCommand dispatch, run_gtk_app public API
overlay/window.rs — GTK4 layer-shell window construction (docked/floating), CSS, caption label
overlay/drag.rs   — floating-mode drag gesture with compositor-quirk coordinate compensation
overlay/caption_buffer.rs — pure text buffer: line-fill, overlap dedup, expiry (GTK-free, well-tested)
overlay/input_region.rs — Wayland input region for click-through
overlay/transcript_log.rs   — pure data: timestamped fragments, paragraph coalescing, .json serialization (GTK-free, well-tested)
overlay/transcript_window.rs — GTK4 toplevel window for transcript mode: scrollable TextView, autoscroll, Save dialog
tray/mod.rs       — ksni StatusNotifierItem system tray
```

## Thread Model

1. **Main/GTK thread** — GTK4 main loop. Consumes `async_channel::Receiver` for captions and overlay commands via `glib::MainContext::spawn_local` futures; no polling.
2. **PipeWire thread** (`pipewire-audio`) — captures audio into the ring buffer and processes AudioCommand. After each successful push, calls `AudioWake::notify()`.
3. **STT pipeline thread** (`stt-pipeline`) — blocks on `AudioWake::wait_timeout(250ms)`, drains the ring buffer, resamples via rubato, reads `ArcSwap<Engine>` to get the current engine choice, builds/rebuilds the engine as needed, runs `SttEngine::process_chunk`, sends captions via `async_channel::Sender`.
4. **Tray thread** — ksni runs on the tokio runtime.

The old "audio bridge" and "engine switch" threads, and the `Arc<Mutex<SyncSender>>` sender-swap dance, are gone. Engine changes are a lock-free `ArcSwap::store` read at the next chunk boundary.

## Key Contracts

- **SttEngine trait** (`stt/mod.rs`): `process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>` — 160ms chunks of 16kHz mono f32 PCM. Returns Some(text) on recognized utterance, None when buffering.
- **Audio pipeline**: PipeWire captures 48kHz stereo F32LE -> ring buffer -> STT pipeline thread resamples to 16kHz mono -> 160ms (2560 sample) chunks fed directly into the engine. No inter-thread channel between resampler and engine.
- **Audio wake**: `stt::AudioWake` is an `AtomicBool` + `Condvar` pair. RT callback calls `notify()` without holding any mutex; consumer uses `wait_timeout_while` with the flag as predicate. The timeout handles VRAM unload and shutdown observation when audio is silent.
- **Engine switching**: `Arc<ArcSwap<Engine>>`. Tray calls `store()`, STT thread reads via `load()` on each chunk batch and rebuilds the engine if the choice changed. Only Nemotron is currently implemented.
- **Config**: TOML at `~/.config/subtidal/config.toml`. Hot-reload only sends SetMode/SetLocked/UpdateAppearance when values actually changed (prevents drag feedback loop). Malformed TOML is warned and ignored.
- **Models**: Downloaded from HuggingFace to `~/.local/share/subtidal/models/nemotron/`. Hardlinked from HF cache when possible.
- **Nemotron engine**: 600M param RNNT model using parakeet-rs::Nemotron. Uses CUDA when available, falls back to CPU. Internally buffers 160ms chunks and emits results on 560ms boundaries.
- **Caption display**: Line-fill model — text fills lines word-by-word up to a character limit (0.85× estimated max chars), then shifts oldest line off when all lines are full. During silence, lines expire one at a time after `expire_secs` (default 8s). Engine whitespace signals word boundaries: leading space = new word, no space = continuation of previous word. RNNT overlap deduplication is preserved.
- **Overlay drag**: Uses accumulated offset tracking to compensate for layer-shell coordinate system shift. During drag, all GTK mutations (captions, CSS, commands) are suppressed via is_dragging flag to prevent relayout jitter.
- **Audio source fallback**: When a captured PipeWire node disappears, automatically falls back to SystemOutput with desktop notification.
- **Overlay modes**: Three modes — `Docked` and `Floating` use the gtk4-layer-shell overlay; `Transcript` uses a regular GTK toplevel window with append-only timestamped paragraphs. Both windows are constructed at startup and visibility-toggled by mode. Captions always append to `TranscriptLog` regardless of mode (mid-session switch reveals full history). On captions-disable edge, all four caption surfaces are cleared (TranscriptLog, transcript view, CaptionBuffer, overlay label).

## Dependencies (key crates)

- gtk4 0.10 + gtk4-layer-shell 0.7 — Wayland overlay
- pipewire 0.9 — audio capture
- rubato 1.0 — sample rate conversion
- ort 2.0.0-rc.12 (cuda feature) — ONNX Runtime inference
- parakeet-rs 0.3 — Nemotron RNNT decoder
- ksni 0.3 — D-Bus StatusNotifierItem tray
- hf-hub 0.5 — model download
- notify 6 + notify-debouncer-mini 0.4 — config file watching
- chrono 0.4 — timestamps for transcript fragments and Save filenames
- serde_json 1 — transcript .json export sidecar

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
