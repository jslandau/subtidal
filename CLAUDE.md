# Subtidal

Real-time speech-to-text overlay for Linux/Wayland and macOS.

Freshness: 2026-05-30

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
stt/mod.rs                   — SttEngine trait + AudioWake + spawn_stt_thread / build_engine / `mod nemotron` (all platform-neutral; engine selects CUDA vs WebGPU vs CPU at runtime)
stt/nemotron.rs              — Nemotron RNNT engine (ort + parakeet-rs); CUDA on Linux, WebGPU on macOS, CPU fallback on both
stt/diarization.rs           — Streaming Sortformer v2.1 diarization engine (parakeet-rs `sortformer` feature); CUDA on Linux, WebGPU on macOS, CPU fallback. Lazy-loaded when `diarization_enabled` flips true; dropped on toggle off to free VRAM
overlay/mod.rs               — neutral: OverlayCommand, CaptionsEnabled; re-exports overlay/linux on Linux, overlay/macos on macOS
overlay/caption_buffer.rs    — pure text buffer: line-fill, overlap dedup, expiry (GTK-free, well-tested) [neutral]
overlay/transcript_log.rs    — pure data: timestamped fragments, paragraph coalescing, .json serialization (GTK-free, well-tested) [neutral]
overlay/linux/mod.rs         — overlay orchestration, OverlayCommand dispatch, run_gtk_app public API
overlay/linux/window.rs      — GTK4 layer-shell window construction (docked/floating), CSS, caption label
overlay/linux/drag.rs        — floating-mode drag gesture with compositor-quirk coordinate compensation
overlay/linux/input_region.rs — Wayland input region for click-through
overlay/linux/transcript_window.rs — GTK4 toplevel window for transcript mode: scrollable TextView, autoscroll, Save dialog
overlay/linux/rename_dialog.rs — GTK4 modal `show_rename_dialog(parent, current_names, cmd_tx)`; 4 fixed slots (Sortformer max speakers); Apply dispatches `OverlayCommand::SetSpeakerNames`
overlay/macos/mod.rs         — neutral shell for macOS overlay subtree
overlay/macos/app.rs         — NSApplication wiring; OverlayCommand dispatch (SetVisible/SetMode/SetLocked/UpdateAppearance/SetCaptionsEnabled with 4-surface clear); caption bridge routes through CaptionBuffer + TranscriptLog; 1s NSTimer drives caption expiry
overlay/macos/panel.rs       — NSPanel construction + `apply_geometry(panel, mtm, mode, config)` for Docked/Floating/Transcript without rebuild; `SubtidalScreenObserver` for NSApplicationDidChangeScreenParametersNotification
overlay/macos/drag.rs        — `SubtidalDragObserver` on NSWindowDidMoveNotification; persists `panel.frame.origin` → `config.position` via hot-reload-safe `Config::save()`
overlay/macos/transcript_window.rs — NSWindow + NSScrollView + NSTextView + Save NSButton; `SubtidalTranscriptActions: NSObject` save handler (NSSavePanel + `TranscriptLog::to_json`). Returns a `TranscriptWindow { state, actions }` bundle because NSButton holds setTarget weakly
tray/mod.rs                  — neutral shell; re-exports impl_linux on Linux, impl_macos on macOS
tray/impl_linux.rs           — ksni StatusNotifierItem system tray
tray/impl_macos.rs           — NSStatusItem + NSMenu with `SubtidalTrayActions: NSObject` (canonical `objc2 0.6 define_class!` reference for the codebase); holds `TrayState` ivars + `Retained<NSMenuItem>` handles; 5s NSTimer refreshes the dynamic audio-source submenu
```

## Thread Model

**Linux:**
1. **Main/GTK thread** — GTK4 main loop. Consumes `async_channel::Receiver` for captions and overlay commands via `glib::MainContext::spawn_local` futures; no polling.
2. **PipeWire thread** (`pipewire-audio`) — captures audio into the ring buffer and processes AudioCommand. After each successful push, calls `AudioWake::notify()`.
3. **STT pipeline thread** (`stt-pipeline`) — blocks on `AudioWake::wait_timeout(250ms)`, drains the ring buffer, resamples via rubato, reads `ArcSwap<Engine>` to get the current engine choice, builds/rebuilds the engine as needed, runs `SttEngine::process_chunk`, sends captions via `async_channel::Sender`. When `diarization_enabled` is set, the same thread also feeds resampled PCM into a lazily-constructed `DiarizationEngine` (Sortformer), tracks `samples_fed_to_diar` + `current_speaker_last_end`, and emits `CaptionEvent::Append { speaker_id, emit_sample }` plus retroactive `CaptionEvent::Relabel { from_sample, new_speaker_id }` events on speaker transitions.
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
- **Config**: TOML at `~/.config/subtidal/config.toml` (Linux) or `~/Library/Application Support/Subtidal/config.toml` (macOS). Linux hot-reload only sends SetMode/SetLocked/UpdateAppearance when values actually changed (prevents drag feedback loop). macOS hot-reload (`config::start_hot_reload_macos`) takes `(audio_cmd_tx, overlay_tx)` and mirrors the Linux dispatcher: tracks `prev_appearance/prev_mode/prev_locked/prev_above_fullscreen` and emits `AudioCommand::SwitchSource` on `audio_source` change plus `OverlayCommand::{UpdateAppearance, SetMode, SetLocked, SetAboveFullscreen}` on the corresponding field diffs. `position` is intentionally not watched (programmatic drag writes must not feed back). Malformed TOML is warned and ignored.
- **AppearanceConfig.font_family** (`config.rs`): new field, serde-default `"monospace"`; existing configs parse unchanged. macOS resolution lives in `overlay/macos/panel.rs::resolve_font`: `"monospace"`/empty → `NSFont::userFixedPitchFontOfSize`, `"system"` → `NSFont::systemFontOfSize`, otherwise `fontWithName:size:` with monospace fallback. Linux GTK wiring is deferred.
- **macOS overlay panel structure** (`overlay/macos/panel.rs`): the NSPanel's contentView is a layer-backed wrapper `NSView` carrying the rounded translucent `CALayer` (corner radius + background color). The caption `NSTextField` is a subview inset by `pub const INSET: f64 = 14.0`pt with flexible width+height autoresizing. `apply_background_color` operates on the wrapper view; `apply_geometry` takes `&NSTextField` directly (downcasting from contentView is no longer possible and previously contributed to a Docked-switch crash). Panel height uses `font_size * 1.3 * lines + 2 * INSET` — the padding term **must** equal `2 * INSET` so the resulting label height (= panel − 2·INSET) matches the budgeted text rect; any mismatch clips the bottom of line N and produces "line N doesn't appear until full" symptoms. Visual line count comes from `measure_text_height` (NSStringDrawing `boundingRectWithSize` with `UsesLineFragmentOrigin | UsesFontLeading` against the label's actual font), not `\n` counting — so post-wrap visual rows are sized correctly even when the character-width heuristic in CaptionBuffer disagrees with NSTextField's pixel-precise wrap. Docked mode centers a `config.appearance.width`-wide panel at the top of `visibleFrame` with Center alignment; Floating uses Left alignment. `set_caption_text` resizes the panel to `min(visual_lines, max_lines)` with the top edge held fixed *before* `setStringValue` to avoid resize-during-paint flicker. `setIgnoresMouseEvents` in Floating mode is tied to lock state (true when locked).
- **macOS overlay drag persistence** (`overlay/macos/drag.rs`): the `windowDidMove` observer writes to `config.position` only when `mode == Floating`. Programmatic frame moves issued for Docked/Transcript modes therefore do not clobber the user's saved Floating coordinates.
- **macOS SetMode re-entry** (`overlay/macos/app.rs`): the SetMode handler must snapshot-and-drop the config `Mutex` *before* calling `apply_geometry`, because the resulting `windowDidMove` AppKit notification re-enters the same lock and would deadlock. `UpdateAppearance` re-applies font, text color, background color, and geometry (not just `CaptionBuffer` config). An initial `apply_geometry` runs right after `build_overlay_panel` so the startup Floating panel honors `config.locked` and is draggable without a Lock-Position toggle. `derive_max_chars` scales character width by font size.
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
- **macOS tray** (`tray/impl_macos.rs`, Phase 6, extended Phase 7 polish): NSStatusItem hosts an NSMenu driven by `SubtidalTrayActions: NSObject`. Action callbacks read/mutate shared `TrayState` ivars and update `Retained<NSMenuItem>` handles (checkmarks, dynamic source list). A 5s NSTimer refreshes the audio-source submenu to reflect process appearance/disappearance. Phase 7 polish adds Font and Lines submenus: Font shows a curated list (`CURATED_FONTS = ["System", "SF Mono", "Menlo", "Monaco", "Courier New"]`) followed by an "All Fonts ▶" cascading submenu populated from `NSFontManager.availableFontNamesWithTraits(FixedPitchFontMask)`; Lines is a discrete 1–5 picker. Both call `OverlayCommand::UpdateAppearance` and persist via `Config::save()`. Audio-source labels come from `NSRunningApplication::runningApplicationsWithBundleIdentifier(...).localizedName()` (so "Music" instead of "com.apple.Music") with the bundle id as fallback. Tray icon is a template PNG bundled at `resources/macos/tray-icon-template.png` (copied into `Subtidal.app/Contents/Resources` by `scripts/bundle-mac.sh`).
- **macOS overlay geometry** (`overlay/macos/panel.rs`, Phase 6): `apply_geometry(panel, mtm, mode, config)` reshapes the existing NSPanel for Docked / Floating / Transcript transitions without teardown — analogous to the Linux mode-switch path. `SubtidalScreenObserver` listens for `NSApplicationDidChangeScreenParametersNotification` and re-applies geometry on display changes.
- **macOS drag persistence** (`overlay/macos/drag.rs`, Phase 6): `SubtidalDragObserver` observes `NSWindowDidMoveNotification` and writes `panel.frame.origin` back into `config.position` through `Config::save()`. The save path is the same one hot-reload watches, so writes must round-trip without re-triggering the debouncer beyond the normal SetMode-suppression logic.
- **macOS transcript window** (`overlay/macos/transcript_window.rs`, Phase 6): NSWindow + NSScrollView + NSTextView with a Save NSButton wired to `SubtidalTranscriptActions` which spawns an NSSavePanel and writes `TranscriptLog::to_json`. The window is returned as a `TranscriptWindow { state, actions }` bundle so the caller keeps `actions` alive — NSButton's `setTarget:` is weak.
- **Caption bridge ownership** (`overlay/macos/app.rs`, Phase 6): the caption bridge is the sole caller of `TranscriptLog::push()`. `transcript_window::append_fragment` only re-renders the view from existing log state. Splitting these responsibilities prevents the double-push that surfaced during integration.
- **CaptionEvent** (`overlay/mod.rs`, diarization branch — BREAKING vs the earlier struct-shaped scaffold): now an enum.
  - `Append { text: String, speaker_id: Option<u32>, emit_sample: u64 }` — normal caption append. `emit_sample` is in the diarization engine's sample-count frame (matches Sortformer's `elapsed_samples`).
  - `Relabel { from_sample: u64, new_speaker_id: u32 }` — retroactive re-attribution. Ordered ahead of subsequent `Append`s from the new speaker on the same channel so the overlay sees Relabel before the new speaker's first caption.
- **Diarization engine** (`stt/diarization.rs`): wraps `parakeet_rs::sortformer::Sortformer` v2.1 using NVIDIA's shipped low-latency preset (`chunk_len=6, right_context=7, fifo_len=188, spkcache_len=188`) — 1.04 s latency with flat DER vs the offline preset (arxiv 2507.18446 Table 2; HF model card). Max 4 speakers. Model: `diar_streaming_sortformer_4spk-v2.1.onnx` from `altunenes/parakeet-rs` HF repo (~492 MB).
- **Diarized caption attribution** (STT pipeline thread): when diarization is enabled, Nemotron fragments are queued briefly instead of being labeled immediately from `current_speaker`. Release-time speaker assignment uses a rolling Sortformer segment timeline plus `Config.diarization_display_delay_ms` and `Config.diarization_alignment_lag_ms` (startup-only knobs; restart required after editing them). `current_speaker_last_end`, `samples_fed_to_diar`, pending queued captions, and recent segment history all reset on toggle-off or diarizer rebuild.
- **Relabel boundary computation** (STT pipeline thread): first-speaker detection is silent — it just primes `current_speaker` and emits no Relabel. Subsequent transitions still emit fallback `Relabel { from_sample = max(seg.start - RELABEL_LOOKBACK_SAMPLES, min(current_speaker_last_end, new_seg.start)) }`, but delayed release-time attribution is now the normal first-pass path. `RELABEL_LOOKBACK_SAMPLES = 24_000` (1.5 s @ 16 kHz) bounds damage when Sortformer misses a brief in-between speaker entirely.
- **Fragment / CaptionLine schema** (diarization branch):
  - `transcript_log::Fragment` gained `speaker_id: Option<u32>` (skip-serialize-if-none; included in JSON export) and `emit_sample: u64` (not serialized — implementation detail consumed by `relabel_since`).
  - `caption_buffer::CaptionLine` gained `speaker_id: Option<u32>`, `earliest_emit_sample: u64`, `latest_emit_sample: u64`.
- **TranscriptLog speaker APIs** (`overlay/transcript_log.rs`): `push_with_speaker(text, speaker_id)`, `push_with_speaker_and_sample(text, speaker_id, emit_sample)`, `push_at_with_speaker(text, speaker_id, emit_sample, ts)`, `relabel_since(from_sample, new_speaker_id) -> usize`. The paragraph-break rule is extended: a speaker change now forces `AppendKind::NewParagraph` in addition to the existing time-gap rule.
- **CaptionBuffer speaker APIs** (`overlay/caption_buffer.rs`): `push_with_speaker` / `push_with_speaker_and_sample`; `speaker_names: HashMap<u32, String>` field (duplicates `Config.speaker_names`, kept in sync by `SetSpeakerNames`); `relabel_since(from_sample, new_speaker_id) -> usize` with three-case logic — fully-past line (rewrite `speaker_id` AND substitute embedded label text), straddling line (rewrite `speaker_id` only; embedded label text preserved because layout would otherwise re-flow mid-line), older line (skip). `last_speaker_id` only advances when an embedded label substitution actually happens, so the next push falls through to the normal speaker-change labeling path and the new speaker remains visually identifiable. Speaker-change finalises the current line by inserting an empty new line before the labeled push, and clears `last_tail` so overlap-dedup does not eat the start of the new speaker's fragment.
- **OverlayCommand additions**: `SetSpeakerNames(HashMap<u32, String>)` — updates `Config.speaker_names`, `CaptionBuffer.speaker_names`, substitutes embedded label prefixes on existing lines, rebuilds transcript view. `ShowRenameDialog` — opens the speaker rename dialog on Linux (GTK4) and macOS (AppKit).
- **Diarization config** (`config.rs`): `Config.diarization_enabled: bool` (default false); `Config.diarization_preset: DiarizationPreset` enum (`Callhome` default, `Dihard3`, `Custom { onset, offset, min_seg_duration }`); startup-only timing knobs `Config.diarization_display_delay_ms` (default 600) and `Config.diarization_alignment_lag_ms` (default 600), both requiring restart after edits; `Config.speaker_names: HashMap<u32, String>` with `#[serde(skip)]` — session-scoped because Sortformer's 0..3 IDs are not stable across launches.
- **PipelineConfig diarization fields** (`stt/mod.rs`): `diarization_enabled: Arc<AtomicBool>` (shared with the tray for the checkmark item), `diarization_preset: DiarizationPreset`, `diarization_model_dir: PathBuf`, `diarization_display_delay_ms: u64`, `diarization_alignment_lag_ms: u64`.
- **Diarization model download** (`models/mod.rs`): `diarization_model_dir()`, `diarization_models_present()`, `diarization_model_file_in()`, `ensure_diarization_models()` download the Sortformer ONNX into the `diarization/` subdir of the per-OS data dir. Download failure is non-fatal — diarization simply stays disabled.
- **Tray diarization wiring**: `TrayState.diarization_enabled: Arc<AtomicBool>` exists on both platforms. Linux tray has the "Diarization" CheckmarkItem and "Rename Speakers..." StandardItem wired; macOS tray has matching `Diarization` and `Rename Speakers…` NSMenuItem actions wired through retained `SubtidalTrayActions` selectors.
- **Signature changes from this branch**: `run_gtk_app(config, caption_rx, cmd_rx, cmd_tx, captions_enabled)` — the new `cmd_tx` lets the rename dialog dispatch `SetSpeakerNames` back into the same command queue. `transcript_window::append_fragment_to_view(state, fragment, kind, speaker_names: &HashMap<u32, String>)` — old code hardcoded `"Speaker {N+1}"`; new code honors user-provided names. New `transcript_window::rebuild_view(state, log, speaker_names)` for full re-render on `SetSpeakerNames`.
- **Linux AudioSource::App arms**: `audio::impl_linux.rs` now matches `AudioSource::App { .. }` (a macOS-only variant) by falling back to SystemOutput with a warning. Without these arms the Linux build was actually broken on master.

## Dependencies (key crates)

**Cross-platform:**
- rubato 1.0 — sample rate conversion
- ort 2.0.0-rc.12 — ONNX Runtime inference (`cuda` on Linux, `webgpu` on macOS)
- parakeet-rs 0.3 — Nemotron RNNT decoder + streaming Sortformer diarizer. Features: `cuda` + `sortformer` on Linux, `webgpu` + `sortformer` on macOS.
- hf-hub 0.5 — model download. Uses `tokio` + `default-tls` features: `tokio` pulls in reqwest with `default-features = false`, which strips reqwest's TLS too. Re-enabling `default-tls` (native-tls / system OpenSSL on Linux, Secure Transport on macOS) avoids the ring cross-compilation issue that breaks `macos-check` CI.
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
- objc2 0.6 + objc2-foundation 0.3 (`NSTimer`, `NSDate` enabled in Phase 6 for tray/caption-expiry timers) + objc2-app-kit 0.3 (Phase 6 adds `NSMenu`, `NSMenuItem`, `NSStatusBar`, `NSStatusItem`, `NSScrollView`, `NSTextView`, `NSTextStorage`, `NSSavePanel`, `NSImage`, `NSControl`, `NSCell`) — Obj-C runtime and AppKit bindings. `CATapDescription` has no binding crate; it is driven through raw `objc2::msg_send!` against the Apple-named initializers `initStereoMixdownOfProcesses:` and `initStereoGlobalTapButExcludeProcesses:`.
- objc2-user-notifications 0.3 — UNUserNotificationCenter for source-disappeared alerts
- objc2-quartz-core 0.3 (features `CALayer`, `objc2-core-foundation`) — CALayer bindings for the overlay panel's rounded translucent background (corner radius + background color on the layer-backed wrapper view)
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
- **AppKit `setTarget:` is weak.** Any `define_class!` action target (tray actions, transcript Save button, drag/screen observers) must be owned by something that outlives the menu/button/notification — return it in a bundle (e.g. `TranscriptWindow { state, actions }`) or stash it in a `let _binding = ...` in `main_macos.rs`. Pointing `setTarget:` at an unrelated long-lived object (NSStatusItem, the panel) raises `NSInvalidArgumentException` on first click — a real bug we hit and reverted twice.
- **First speaker detection is silent.** The diarization pipeline never emits `Relabel` for the *first* detected speaker — it just primes `current_speaker`. Only subsequent transitions emit Relabel events. Violating this would retroactively re-attribute the entire transcript on the first detected segment boundary.
- **`CaptionBuffer.last_speaker_id` advances conditionally.** It only updates when an embedded label substitution actually fires inside `relabel_since`'s fully-past branch. If it advanced unconditionally, the next speaker-change push would skip the labeling path and the new speaker would be visually indistinguishable in the overlay.
- **Canonical `objc2 0.6 define_class!` pattern lives at the top of `src/tray/impl_macos.rs`.** New macOS Obj-C subclasses (`SubtidalDragObserver`, `SubtidalScreenObserver`, `SubtidalTranscriptActions`, future ones) should mirror that structure rather than reinvent it — `objc2` 0.6 changed the macro shape vs. older examples on the web.

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

Subtidal supports Linux and macOS. All platform-specific code is cfg-gated; the crate exposes both a `[lib]` (`src/lib.rs`) and a `[[bin]]` (`src/main.rs`). The binary carries a `#[cfg(not(any(target_os = "linux", target_os = "macos")))] compile_error!` guard that hard-fails builds on any other OS with a clear message.

**Cfg-gating boundaries.** Each platform-bound subsystem follows one of three patterns:

- **Shell-and-re-export** (`audio/`, `tray/`): `mod.rs` is a thin shell that declares `#[cfg(target_os = "linux")] mod impl_linux;` and `#[cfg(target_os = "macos")] mod impl_macos;` and re-exports the public surface. Implementation bodies live in `impl_linux.rs` / `impl_macos.rs` (or `impl_macos/` subdir for audio).
- **Subtree-and-re-export** (`overlay/`): `mod.rs` keeps neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, `transcript_log`) at the module root and gates `linux/` (GTK orchestration: `window`, `drag`, `input_region`, `transcript_window`) and `macos/` (AppKit orchestration: `app`, `panel`, `drag`, `transcript_window`) subdirectories.
- **In-place gating** (`stt/`): the module exposes platform-neutral types and functions (`SttEngine` trait, `AudioWake`, `PipelineConfig`, `spawn_stt_thread`, `build_engine`, `mod nemotron`); the underlying engine selects CUDA vs WebGPU vs CPU at runtime based on what's available.

**Cargo dependencies.** Linux-only crates (`pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc`, `notify-rust`) live in `[target.'cfg(target_os = "linux")'.dependencies]`. macOS-only crates (`coreaudio-sys`, `core-foundation`, `objc2*`, `objc2-app-kit`, `objc2-foundation`, `objc2-quartz-core`, `objc2-user-notifications`, `dispatch2`, `block2`, `libc`) live in `[target.'cfg(target_os = "macos")'.dependencies]`. The `cuda` feature on `ort` and `parakeet-rs` is Linux-conditional and the `webgpu` feature is macOS-conditional via additive feature unification: each crate appears once in `[dependencies]` (without backend features) and once in each per-OS block (with the appropriate feature). Resolver v2 (edition 2021 default) keeps backend features from bleeding cross-target.

**Verification mechanism.** A GitHub Actions workflow at `.github/workflows/macos-check.yml` runs `cargo check --lib --target aarch64-apple-darwin` on `macos-latest` for every push and pull request. The aarch64 target reflects the supported macOS surface (Apple Silicon only; Intel Mac is explicitly out of scope). The workflow uses `--lib` (not bare `cargo check`) so the binary's `compile_error!` guard does not fire. Any future commit that accidentally introduces Linux coupling into a notionally-neutral module — or vice versa — fails the check.

**Build-script gate.** `build.rs` early-returns on non-Linux targets via `env::var("TARGET").unwrap_or_default().contains("linux")`. The `cfg!(target_os = "linux")` macro is intentionally NOT used here — it reflects the build host, not the cross-compilation target, and would silently fail to skip CUDA-provider scanning during `cargo check --target aarch64-apple-darwin` from a Linux host.

**`compile_error!` location.** `src/main.rs` contains the platform-availability guard immediately after all `mod` declarations (`mod main_linux;` and `mod main_macos;`) and before any `use` or other logic. Placement before `mod` declarations causes confusing cascading errors from rustc's mod-resolution pass.

**Platform implementations: Linux and macOS.** The cfg-gating patterns above are realized by two concrete platform implementations:

- **Linux:** `impl_linux.rs` / `linux/` subtrees under `audio/`, `overlay/`, `tray/`; `main_linux.rs`. Audio via PipeWire, overlay via GTK4 layer-shell, tray via ksni StatusNotifierItem. CUDA on `ort` + `parakeet-rs` when available; CPU fallback automatic.
- **macOS:** `impl_macos.rs` / `impl_macos/` / `macos/` subtrees under the same; `main_macos.rs`. Audio via Core Audio Process Taps (`AudioHardwareCreateProcessTap` + aggregate device + IOProc), overlay via NSPanel (Floating/Docked with rounded translucent CALayer) and NSWindow+NSScrollView (Transcript), tray via NSStatusItem + NSMenu. WebGPU on `ort` + `parakeet-rs` for Apple Silicon GPU; CPU fallback automatic. TCC service required: **Audio Capture** (declared via `NSAudioCaptureUsageDescription` in `Info.plist`).

**Diarization platform parity.** The diarization engine (`stt/diarization.rs`) is cross-platform and works on both Linux (CUDA/CPU) and macOS (WebGPU/CPU). Linux exposes a GTK4 rename dialog plus tray "Diarization" and "Rename Speakers..." items. macOS exposes matching NSStatusItem menu actions and an AppKit rename dialog with four Sortformer speaker slots. macOS transcript rendering rebuilds from `TranscriptLog` fragments plus current speaker names, so custom names and retroactive `Relabel` corrections are reflected in visible transcript text as well as saved JSON.

**Diarization known limitations.** A continuation fragment that lands inside a line straddling the relabel boundary keeps the old label visible at the line's start (metadata is corrected; layout is not re-flowed). Speakers shorter than `min_duration_on=0.511s` (callhome preset) are dropped by Sortformer entirely; the 2 s `RELABEL_LOOKBACK_SAMPLES` clamp bounds damage but does not eliminate it.

To add a third platform (e.g., Windows):
1. Refine the `compile_error!` predicate in `src/main.rs` to exclude the new OS.
2. Add `impl_<os>.rs` (or `<os>/` subtree) siblings under `audio/`, `overlay/`, `tray/`, and add corresponding gated `mod` declarations + re-exports in each `mod.rs`.
3. Add a `[target.'cfg(target_os = "<os>")'.dependencies]` block with that platform's crates and (if applicable) the matching backend feature on `ort`/`parakeet-rs`.
4. Add `src/main_<os>.rs` and the corresponding `#[cfg(target_os = "<os>")] mod main_<os>;` declaration in `src/main.rs`, plus a dispatch arm in `main()`.
5. Add the new target to `.github/workflows/macos-check.yml` (or rename it to e.g. `cross-target-check.yml` and convert to a matrix) so cross-coupling regressions break CI.
