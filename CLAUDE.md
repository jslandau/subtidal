# Subtidal

Real-time speech-to-text overlay for Linux/Wayland and macOS.

Freshness: 2026-05-20

## Purpose

Captures system or per-application audio via PipeWire (Linux) or Core Audio Taps (macOS), runs local STT inference (Nemotron GPU or CPU), and displays live captions in a GTK4 layer-shell overlay (Linux) or NSPanel (macOS) with system tray controls.

## Architecture

Platform-bound code is cfg-gated; see `## Platform Isolation` below for the gating patterns. File map:

```
lib.rs                       — library crate root; re-exports modules for cross-target `cargo check --lib`
main.rs                      — bin entry point; platform-agnostic `compile_error!` guard; dispatches to main_linux/main_macos
main_linux.rs                — Linux startup orchestration, thread wiring, CUDA probe/reexec helpers (cuda_available, run_cuda_probe, cuda_status_message, reexec_with_absolute_argv0_if_needed, …)
main_macos.rs                — macOS startup entry point (stub in Phase 1; Phase 2+ has NSApplication orchestration)
config.rs                    — TOML config with hot-reload (notify/debouncer); cfg-gated per-OS config paths
models/mod.rs                — HuggingFace model download (hf-hub + tokio); cross-platform with cfg-gated per-OS data dirs
audio/mod.rs                 — neutral shell; re-exports impl_linux on Linux, impl_macos on macOS
audio/impl_linux.rs          — PipeWire capture thread, node enumeration, source switching
audio/impl_macos/mod.rs      — macOS audio capture orchestration via Core Audio Taps (Phase 5 revised)
audio/impl_macos/tap.rs      — RAII wrapper for Core Audio process tap + aggregate device + IOProc (Task 3, Phase 5 revised)
audio/impl_macos/tap_processes.rs — safe wrappers for Core Audio process enumeration (Phase 5 revised)
audio/impl_macos/notify.rs   — UNUserNotificationCenter helper for source-disappeared/TCC-denied alerts (Phase 5 revised)
audio/resampler.rs           — rubato 48kHz stereo -> 16kHz mono resampler (platform-neutral)
stt/mod.rs                   — SttEngine trait + AudioWake (neutral) + Linux-gated spawn_stt_thread / build_engine / `mod nemotron`
stt/nemotron.rs              — Nemotron RNNT engine (ort + parakeet-rs, CUDA) [Linux-only]
overlay/mod.rs               — neutral: OverlayCommand, CaptionsEnabled; re-exports overlay/linux on Linux, overlay/macos on macOS
overlay/caption_buffer.rs    — pure text buffer: line-fill, overlap dedup, expiry (GTK-free, well-tested) [neutral]
overlay/transcript_log.rs    — pure data: timestamped fragments, paragraph coalescing, .json serialization (GTK-free, well-tested) [neutral]
overlay/linux/mod.rs         — overlay orchestration, OverlayCommand dispatch, run_gtk_app public API
overlay/linux/window.rs      — GTK4 layer-shell window construction (docked/floating), CSS, caption label
overlay/linux/drag.rs        — floating-mode drag gesture with compositor-quirk coordinate compensation
overlay/linux/input_region.rs — Wayland input region for click-through
overlay/linux/transcript_window.rs — GTK4 toplevel window for transcript mode: scrollable TextView, autoscroll, Save dialog
overlay/macos/mod.rs         — macOS overlay orchestration (NSPanel/NSWindow); skeleton in Phase 1, populated Phase 2+ (panel + caption bridge)
tray/mod.rs                  — neutral shell; re-exports impl_linux on Linux, impl_macos on macOS
tray/impl_linux.rs           — ksni StatusNotifierItem system tray
tray/impl_macos.rs           — macOS tray implementation (NSStatusItem); skeleton in Phase 1, populated Phase 6
```

## Thread Model

**Linux:**
1. **Main/GTK thread** — GTK4 main loop. Consumes `async_channel::Receiver` for captions and overlay commands via `glib::MainContext::spawn_local` futures; no polling.
2. **PipeWire thread** (`pipewire-audio`) — captures audio into the ring buffer and processes AudioCommand. After each successful push, calls `AudioWake::notify()`.
3. **STT pipeline thread** (`stt-pipeline`) — blocks on `AudioWake::wait_timeout(250ms)`, drains the ring buffer, resamples via rubato, reads `ArcSwap<Engine>` to get the current engine choice, builds/rebuilds the engine as needed, runs `SttEngine::process_chunk`, sends captions via `async_channel::Sender`.
4. **Tray thread** — ksni runs on the tokio runtime.

**macOS:**
1. **Main/AppKit thread** — NSApplication main loop.
2. **Audio worker thread** (`audio-tap-worker`) — Core Audio IOProc callback pushes samples to ring buffer; a 1 Hz polling loop in the worker uses POSIX `kill(pid, 0)` to detect process death and triggers fallback to SystemMix. (Core Audio's `kAudioProcessPropertyIsRunning` is *not* used for liveness — it tracks "audio I/O active right now" and false-positives on every playback pause.)
3. **STT pipeline thread** (`stt-pipeline`) — same as Linux.
4. **Tray thread** — tokio runtime (Phase 6).

Engine changes are a lock-free `ArcSwap::store` read at the next chunk boundary.

## Key Contracts

- **SttEngine trait** (`stt/mod.rs`): `process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>` — 160ms chunks of 16kHz mono f32 PCM. Returns Some(text) on recognized utterance, None when buffering.
- **Audio pipeline**: 
  - Linux: PipeWire captures 48kHz stereo F32LE -> ring buffer -> STT pipeline thread resamples to 16kHz mono -> 160ms (2560 sample) chunks fed directly into the engine.
  - macOS: Core Audio Tap captures 48kHz stereo f32 interleaved via RT-safe IOProc -> ring buffer -> same resampling and STT pipeline as Linux.
  - No inter-thread channel between resampler and engine.
- **Audio wake**: `stt::AudioWake` is an `AtomicBool` + `Condvar` pair. RT callback (PipeWire or Core Audio IOProc) calls `notify()` without holding any mutex; consumer uses `wait_timeout_while` with the flag as predicate. The timeout handles VRAM unload and shutdown observation when audio is silent.
- **Audio source fallback**: 
  - Linux: When a captured PipeWire node disappears, automatically falls back to SystemOutput with desktop notification.
  - macOS: 1 Hz `kill(pid, 0)` watchdog over `TapTarget::Processes { watchdog_pids }` detects process death; on any watched PID disappearing, falls back to SystemMix with a `UNUserNotification`. (Multi-PID is the common case: browsers split audio across WebKit/GPU helpers sharing one bundle id.)
- **Engine switching**: `Arc<ArcSwap<Engine>>`. Tray calls `store()`, STT thread reads via `load()` on each chunk batch and rebuilds the engine if the choice changed. Only Nemotron is currently implemented.
- **Config**: TOML at `~/.config/subtidal/config.toml` (Linux) or `~/Library/Application Support/Subtidal/config.toml` (macOS). Linux hot-reload only sends SetMode/SetLocked/UpdateAppearance when values actually changed (prevents drag feedback loop). macOS hot-reload (`config::start_hot_reload_macos`) watches `audio_source` only and emits `AudioCommand::SwitchSource`; appearance/mode fields are ignored until Phase 6 wires the overlay surface. Malformed TOML is warned and ignored.
- **Models**: Downloaded from HuggingFace to `~/.local/share/subtidal/models/nemotron/` (Linux) or `~/Library/Application Support/Subtidal/models/nemotron/` (macOS). Hardlinked from HF cache when possible.
- **Nemotron engine**: 600M param RNNT model using parakeet-rs::Nemotron. Uses CUDA when available on Linux, WebGPU on macOS, falls back to CPU. Internally buffers 160ms chunks and emits results on 560ms boundaries.
- **Caption display**: Line-fill model — text fills lines word-by-word up to a character limit (0.85× estimated max chars), then shifts oldest line off when all lines are full. During silence, lines expire one at a time after `expire_secs` (default 8s). Engine whitespace signals word boundaries: leading space = new word, no space = continuation of previous word. RNNT overlap deduplication is preserved.
- **Overlay drag** (Linux): Uses accumulated offset tracking to compensate for layer-shell coordinate system shift. During drag, all GTK mutations (captions, CSS, commands) are suppressed via is_dragging flag to prevent relayout jitter.
- **Above-fullscreen toggle**: `config.above_fullscreen` (tray: "Show Above Fullscreen") selects `Layer::Overlay` vs `Layer::Top` for the layer-shell overlay (Linux only). Overlay layer renders above compositor-fullscreened clients (e.g. browser video); Top does not. Live-applied via `OverlayCommand::SetAboveFullscreen` (no rebuild). No-op in Transcript mode (regular toplevel).
- **Overlay modes**: Three modes — `Docked` and `Floating` use the gtk4-layer-shell overlay (Linux); `Transcript` uses a regular toplevel window with append-only timestamped paragraphs. Both windows are constructed at startup and visibility-toggled by mode. Captions always append to `TranscriptLog` regardless of mode (mid-session switch reveals full history). On captions-disable edge, all caption surfaces are cleared (TranscriptLog, transcript view, CaptionBuffer, overlay label).
- **Core Audio Tap lifecycle** (macOS, Phase 5 revised): `AudioTap` RAII type owns process tap + aggregate device + IOProc. Drop impl tears down in correct order: Stop → DestroyIOProc → DestroyAggregateDevice → DestroyProcessTap. During `AudioTap::build`, a local `TapGuard` scope-guard destroys a freshly-created tap on any error path until IOProc creation succeeds, at which point it is `defuse()`d and ownership transfers to `AudioTap`. Source switching rebuilds the tap + aggregate device (sub-100ms latency; AC3.3 target ≤1 second caption gap).
- **TapTarget shape**: `TapTarget::SystemMix | TapTarget::Processes { object_ids: Vec<AudioObjectID>, watchdog_pids: Vec<c_int> }`. `object_ids` are the per-process Core Audio objects the tap mixes; `watchdog_pids` are the POSIX PIDs the worker thread polls. Multi-PID is the rule, not the exception, because browsers split audio across helper processes that share a single bundle id.
- **Tap format guard** (`verify_tap_format`): immediately after `AudioHardwareCreateProcessTap` returns, the tap's stream format is read and rejected unless it is exactly 48 kHz stereo f32. No format coercion is implemented; downstream resampling assumes this layout.
- **CaptureError discrimination** (`src/audio/impl_macos/mod.rs`): the worker returns `CaptureError::InitialBuildFailed` for first-build failures (mapped to a TCC-denied user-notification message) and `CaptureError::RuntimeFailure` for in-flight failures (generic message). Anything else converts via `From<anyhow::Error> for CaptureError` as `RuntimeFailure`.
- **Process source enumeration** (`list_sources` on macOS): deduplicates by `bundle_id` and **does not** filter by `kAudioProcessPropertyIsRunning` — the process list reflects "has ever instantiated an audio engine this session" rather than "currently producing audio". Filtering by IsRunning would hide apps mid-pause.
- **macOS TCC boundary**: capture requires the **Audio Capture** TCC service (declared via `NSAudioCaptureUsageDescription` in `Info.plist`), *not* Screen Recording. Grant persists across launches without stable codesigning — the headline payoff vs Phase 4's ScreenCaptureKit approach.

## Dependencies (key crates)

**Cross-platform:**
- rubato 1.0 — sample rate conversion
- ort 2.0.0-rc.12 — ONNX Runtime inference (`cuda` on Linux, `webgpu` on macOS)
- parakeet-rs 0.3 — Nemotron RNNT decoder (`cuda` on Linux, `webgpu` on macOS)
- hf-hub 0.5 — model download
- notify 6 + notify-debouncer-mini 0.4 — config file watching
- chrono 0.4 — timestamps for transcript fragments and Save filenames
- serde_json 1 — transcript .json export sidecar

**Linux-specific:**
- gtk4 0.10 + gtk4-layer-shell 0.7 — Wayland overlay
- pipewire 0.9 — audio capture
- ksni 0.3 — D-Bus StatusNotifierItem tray
- notify-rust 4 — desktop notifications

**macOS-specific:**
- coreaudio-sys 0.2 — Core Audio FFI. Note: `AudioHardwareCreateProcessTap` / `AudioHardwareDestroyProcessTap` are declared inline (`extern "C"`) in `src/audio/impl_macos/tap.rs` because the `CoreAudio.h` umbrella header excludes `AudioHardwareTapping.h` and the 0.2.17 bindgen output omits them.
- core-foundation 0.10 — CFString, CFNumber, CFDictionary, CFArray
- objc2 0.6 + objc2-foundation 0.3 + objc2-app-kit 0.3 — Obj-C runtime and AppKit bindings. `CATapDescription` has no binding crate; it is driven through raw `objc2::msg_send!` against the Apple-named initializers `initStereoMixdownOfProcesses:` and `initStereoGlobalTapButExcludeProcesses:`.
- objc2-user-notifications 0.3 — UNUserNotificationCenter for source-disappeared alerts
- dispatch2 0.3 + block2 0.6 — Grand Central Dispatch for main-thread marshaling
- libc 0.2 — `kill(pid, 0)` for the process-liveness watchdog

## Invariants

- PipeWire stream callback is real-time safe: no allocation, no blocking, try_lock only (Linux).
- Core Audio IOProc callback is real-time safe: no allocation, no blocking, try_lock only (macOS).
- GTK4 calls happen only on the main thread; channels bridge other threads (Linux).
- AppKit calls happen only on the main NSApplication thread; channels bridge other threads (macOS).
- CUDA unavailability triggers automatic fallback to CPU execution (Nemotron on Linux).
- WebGPU unavailability triggers automatic fallback to CPU execution (Nemotron on macOS).
- Config save failures are warned but never fatal.
- Ring buffer overflow drops samples silently (preferred over blocking RT callback).

## Build & Run

**Linux:**
```bash
cargo build --release
./target/release/subtidal [--engine nemotron|parakeet] [--config path] [--reset-config]
```
Requires: PipeWire running, Wayland compositor with wlr-layer-shell support. CUDA optional (GPU acceleration for Nemotron).

**macOS (Phase 5 revised onwards):**
```bash
scripts/bundle-mac.sh
open target/release/Subtidal.app
```
Requires: macOS 14.4+, Audio Capture permission granted via System Settings → Privacy & Security. WebGPU available on Apple Silicon (M1+). CPU fallback for older hardware.

**Cross-target check (macOS):**
```bash
cargo check --lib --target aarch64-apple-darwin
```
Verifies audio/overlay/tray modules compile on macOS targets without accidentally coupling Linux code.

## Platform Isolation

Subtidal's source tree is structured so that all Linux-specific code is gated behind `#[cfg(target_os = "linux")]`. The crate exposes both a `[lib]` (`src/lib.rs`) and a `[[bin]]` (`src/main.rs`); the binary additionally carries a `#[cfg(not(target_os = "linux"))] compile_error!` guard that hard-fails non-Linux binary builds with a clear "macOS support is planned" message.

**Cfg-gating boundaries.** Each platform-bound subsystem follows one of three patterns:

- **Shell-and-re-export** (`audio/`, `tray/`): `mod.rs` is a thin shell that declares `#[cfg(target_os = "linux")] mod impl_linux;` and re-exports the public surface. The Linux implementation body lives in `impl_linux.rs`.
- **Subtree-and-re-export** (`overlay/`): `mod.rs` keeps neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, `transcript_log`) at the module root and gates a `linux/` subdirectory holding the GTK orchestration (`run_gtk_app`, `handle_overlay_command`) and per-window submodules (`window`, `drag`, `input_region`, `transcript_window`).
- **In-place gating** (`stt/`): the module mixes neutral types (`SttEngine` trait, `AudioWake`, `PipelineConfig`) with Linux-only items (`mod nemotron`, `spawn_stt_thread`, `build_engine`). Linux-only items carry `#[cfg(target_os = "linux")]` directly; neutral items are unguarded.

**Cargo dependencies.** Linux-only crates (`pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc`) live in `[target.'cfg(target_os = "linux")'.dependencies]`. The `cuda` feature on `ort` and `parakeet-rs` is Linux-conditional via additive feature unification: each crate appears once in `[dependencies]` (without `cuda`) and once in the Linux-conditional block (with `cuda`). Resolver v2 (edition 2021 default) keeps the `cuda` feature from bleeding onto non-Linux targets.

**Verification mechanism.** A GitHub Actions workflow at `.github/workflows/macos-check.yml` runs `cargo check --lib --target aarch64-apple-darwin` on `macos-latest` for every push and pull request. The aarch64 target is used because ort 2.0.0-rc.12 provides wgpu prebuilts for aarch64 but not x86_64 (Phase 7 adds x86_64 once ort supplies those prebuilts). The workflow uses `--lib` (not bare `cargo check`) so the binary's `compile_error!` guard does not fire. Any future commit that accidentally introduces Linux coupling into a notionally-neutral module fails the check.

**Build-script gate.** `build.rs` early-returns on non-Linux targets via `env::var("TARGET").unwrap_or_default().contains("linux")`. The `cfg!(target_os = "linux")` macro is intentionally NOT used here — it reflects the build host, not the cross-compilation target, and would silently fail to skip CUDA-provider scanning during `cargo check --target x86_64-apple-darwin` from a Linux host.

**`compile_error!` location.** `src/main.rs` contains the platform-availability guard immediately after all `mod` declarations (`mod main_linux;` and `mod main_macos;`) and before any `use` or other logic. Placement before `mod` declarations causes confusing cascading errors from rustc's mod-resolution pass.

**Recipe for adding a new platform (e.g., macOS).**

1. Remove the line `compile_error!("Subtidal currently only supports Linux. macOS support is planned.");` from `src/main.rs` (or refine its cfg predicate to exclude the new platform).
2. For each cfg-gated subsystem, mirror the Linux structure with a sibling implementation:
   - `audio/`: add `src/audio/impl_macos.rs` and gate it from `src/audio/mod.rs` with `#[cfg(target_os = "macos")]`.
   - `tray/`: same shape — add `src/tray/impl_macos.rs`.
   - `overlay/`: add `src/overlay/macos/` subdirectory and gate `mod macos;` from `src/overlay/mod.rs`.
   - `stt/`: add a `mod coreml;` (or analogous) and gate the Linux-specific items behind their existing cfgs.
3. Add a `[target.'cfg(target_os = "macos")'.dependencies]` block in `Cargo.toml` listing macOS-only crates (e.g., `core-foundation`, `cocoa`).
4. Move the Linux-specific main helpers analogously into `src/main_macos.rs` and add the corresponding `#[cfg(target_os = "macos")] mod main_macos;` declaration in `src/main.rs`.
5. Add the new target as an entry (or matrix value) in `.github/workflows/macos-check.yml` (which can be renamed to e.g. `cross-target-check.yml` once it serves multiple targets).
