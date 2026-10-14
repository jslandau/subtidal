# Changelog

All notable changes to Subtidal are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] — 2026-10-14

### Changed
- **ORT inference update:** pinned parakeet-rs 0.4.1 and ONNX Runtime rc.14, retaining ORT rather than switching to Burn. The English ASR model files and digest pins are unchanged.
- **Eight-speaker diarization:** independently pinned Nemotron-3 diarization v3, enabled its low-latency profile and default postprocessing, and expanded Linux/macOS speaker rename dialogs to eight slots. Explicit legacy postprocessing presets remain supported.
- **Linux GPU requirements:** the pinned ORT CUDA distribution requires CUDA 13 runtime libraries and cuDNN 9; CPU fallback remains available.

### Fixed
- **Runtime initialization and fallback:** share one ORT environment, retry CPU after GPU construction failure, invalidate stale GPU verdicts, and update the Linux tray indicator to reflect the loaded ASR backend.
- **Provider compatibility:** stage only the matching ORT CUDA/shared provider pair; failed staging cannot reuse a cached positive GPU verdict or unrelated cached libraries.

### Added
- **Reproducible inference comparison:** isolated locked ORT/Burn runners, verified AMI fixtures, quality scoring, timing/memory reports, and documented measurements supporting the decision to retain ORT.

### Validation
- Linux locked release build, all-target check, 157 library tests, and 18 CLI/static/migration/provider tests passed. Production CPU and CUDA-requested adapter smoke tests passed.
- Native macOS compilation, bundle launch, WebGPU inference, and eight-slot dialog interaction still require native validation; Linux cross-compilation is blocked by Apple C compiler/SDK requirements.

## [0.3.3] — 2026-10-13

### Added
- **Linux floating drag override:** `floating_drag_coordinate_handling` accepts `automatic`, `cumulative`, or `compensated`. Changes apply on the next drag, including with custom config paths.

### Fixed
- **Linux floating positioning:** corrected accelerated dragging on Umbriel and bounded dragging and restored positions to the output.
- **Reproducible installation:** pinned the tested parakeet-rs 0.3.6 and ORT rc.12 inference dependencies and documented installation with `--locked` to avoid incompatible dependency resolution.

### Performance & security hardening

### Fixed
- **Panic and process-abort boundaries:** made multilingual caption overlap/tail handling UTF-8 safe; validated PipeWire chunk bounds/alignment and Core Audio IOProc buffers; and enforced the complete 48 kHz stereo f32 interleaved tap format.
- **Model supply-chain integrity:** pinned the Hugging Face revision and SHA-256 digests, verified existing and fetched files, quarantined mismatches, and installed atomically without replacing a trusted destination. Nemotron model downloads now run concurrently.
- **Engine and runtime performance:** added engine-build failure backoff with one actionable notice per failure streak, corrected pre-switch audio discard, fixed Core Foundation Create-rule ownership, reduced macOS caption/tray overhead, and cached the CUDA probe verdict.
- **Hardening and test safety:** made config temporary files unique and symlink-safe, flushed output before raw exit, reused the Nemotron drain buffer, mitigated watchdog PID reuse, synchronized the macOS bundle version, sandboxed the CLI test, moved macOS notifications onto the main queue, fixed aggregate-dictionary callbacks and UID uniqueness, and lengthened the PipeWire command-loop timeout.

### Audit status
- `cargo audit --deny warnings` was run against the updated lockfile. Fixable advisories were addressed by upgrading `crossbeam-epoch` to 0.9.20, `h2` to 0.4.16, `ringbuf` to 0.5.2, `rustls` to 0.23.45, and `rustls-webpki` to 0.103.15.
- The remaining audit findings are explicitly acknowledged because they are transitive and not independently upgradeable without a broader dependency or platform change: **RUSTSEC-2026-0194** and **RUSTSEC-2026-0195** affect `quick-xml 0.37.5` required by the Windows-only `tauri-winrt-notification 0.7.2` path in `notify-rust`; **RUSTSEC-2024-0436** affects unmaintained `paste 1.0.15` through `parakeet-rs` → `tokenizers`; **RUSTSEC-2026-0190** affects `anyhow 1.0.102` (used directly and by PipeWire); **RUSTSEC-2026-0221** affects `event-listener 5.4.1` through `async-channel`/`zbus`; **RUSTSEC-2024-0429** affects Linux `glib 0.19.9`; **RUSTSEC-2026-0097** affects `rand 0.9.2` through `hf-hub`/`tokenizers`; and the yanked-package warnings for **js-sys 0.3.88** and **wasm-bindgen 0.2.111** arise from the wasm-capable transitive dependency graph. None is used as an externally exposed parser or unsafe callback boundary by Subtidal; each remains a tracked dependency-maintenance item.

## [0.3.2] — 2026-06-26

### Changed
- **Nemotron engine now points to Nemotron 3.5 streaming multilingual.** The temporary dual-engine surface was collapsed so the canonical `nemotron` engine now resolves to the 3.5 model, while legacy `nemotron35` config/CLI spellings continue to map to the same engine for compatibility.
- **Engine-switch model preparation.** Tray-triggered engine changes now ensure the target model files are present before switching, preventing bad runtime behavior when selecting an engine whose model family has not yet been downloaded.
- **Linux rename dialog behavior.** The speaker rename dialog is now non-modal so it no longer blocks interaction with the floating overlay underneath.

### Fixed
- **macOS Audio Source tray cleanup.** The tray now shows one `System Output`
  row followed by user-facing app names instead of raw Core Audio helper and
  background process bundle IDs. Helper-heavy apps such as Discord, Slack,
  Zoom, and browsers are grouped under the visible parent app while preserving
  their helper bundle IDs for reliable capture.
- **macOS audio-source persistence compatibility.** Existing app-source configs
  without grouped capture metadata still load and capture the saved bundle ID;
  grouped app selections now persist `capture_bundle_ids` so relaunch and
  hot-reload can restore helper-aware capture.
- **macOS startup source normalization.** Legacy/helper app selections are
  normalized to the current grouped source at launch so the audio thread, tray
  checkmark, and config stay coherent.

## [0.3.1] — 2026-06-15

### Fixed
- **macOS caption visibility reconciliation.** Caption on/off and mode switching now
  consistently enforce a single visibility rule: when captions are off, no caption
  display surface is shown; when captions are on, exactly the currently selected
  surface is shown (Docked/Floating panel or Transcript window).
- **macOS Transcript mode relaunch/re-entry behavior.** Transcript windows now
  reappear correctly after caption toggles, close/reopen cycles, and mode switches.
- **macOS app presentation.** Transcript mode now participates in normal Dock/Finder
  app behavior, and the macOS bundle now includes an app icon generated from the
  canonical SVG asset.

## [0.3.0] — 2026-05-21

### Added
- **macOS 14.4+ Apple Silicon support.** Full feature parity with the Linux build:
  - Audio capture via Core Audio Process Taps (`AudioHardwareCreateProcessTap` +
    aggregate device + IOProc), gated by the standard **Audio Capture** TCC
    permission (not Screen Recording).
  - Per-application audio source enumeration via `kAudioProcessPropertyBundleID`,
    with multi-PID watchdog (browsers split audio across helper processes that
    share a bundle id) and 1 Hz POSIX `kill(pid, 0)` liveness check; auto-falls
    back to SystemMix with a `UNUserNotification` when the captured app exits.
  - WebGPU (Metal) Nemotron inference with automatic CPU fallback.
  - Overlay panel via NSPanel — Docked, Floating, and Transcript modes with
    drag persistence, rounded translucent background, configurable font/colors,
    and pixel-accurate line counting via NSStringDrawing.
  - System tray via NSStatusItem + NSMenu, with submenus for Mode, Engine,
    Audio Source (pretty names via `NSRunningApplication.localizedName`), Font
    (curated picks + every installed monospace font), Lines (1–5), Above
    Fullscreen, and Lock Position. Dynamic 5 s refresh of the audio source list.
  - Config + hot-reload at `~/Library/Application Support/Subtidal/config.toml`.
  - `.app` bundle script at `scripts/bundle-mac.sh` (ad-hoc codesigned).
- **`appearance.font_family` config field** (default `"monospace"`) selectable
  from the new tray Font submenu. Both `"system"` and any installed font family
  name are accepted; resolves identically on Linux and macOS in spirit (Linux
  GTK wiring of the field is deferred — Linux currently still uses GTK's
  default font).

### Changed
- README rewritten to describe the cross-platform (Linux + macOS) codebase.
- `CLAUDE.md` rewritten to drop "Linux currently, macOS planned" framing.
- Cargo dependency layout split into per-OS conditional blocks; Resolver v2
  keeps `cuda` (Linux) and `webgpu` (macOS) features from bleeding cross-target.

### Removed
- The earlier `compile_error!("Subtidal currently only supports Linux. macOS
  support is planned.")` guard in `src/main.rs`. The guard now fires only on
  OSes other than Linux and macOS.

## [0.2.2] and earlier

Pre-changelog. See `git log` for history.
