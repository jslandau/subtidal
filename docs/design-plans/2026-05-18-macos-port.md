# macOS Port Design

## Summary

The macOS port adds a third platform-native implementation layer to Subtidal without modifying any of its neutral, cross-platform contracts. Rather than building a new application, the approach is strictly additive: three sibling implementation files (`audio/impl_macos.rs`, `tray/impl_macos.rs`, `overlay/macos/`) slot into the existing shell-and-re-export module structure alongside their Linux counterparts, while every shared contract — `SttEngine` trait, `AudioWake`, `ArcSwap<Engine>` engine switching, `OverlayCommand`, `CaptionBuffer`, `TranscriptLog`, the rubato resampler, the ring buffer, and the `AudioSource` enumeration — remains untouched. Platform-specific behavior is cfg-gated at the Cargo dependency level (objc2 crates, `ort`/`parakeet-rs` with the `webgpu` feature) and at the source level with `#[cfg(target_os = "macos")]`, following the same patterns already established for Linux. A minimal `.app` bundle with a stable bundle ID solves the macOS TCC permission system, which ties Screen Recording grants to `(CFBundleIdentifier, code signature)` rather than executable path.

The key architectural choices each have a specific rationale. AppKit enforces strict main-thread affinity (enforced at the type level by `objc2`'s `MainThreadMarker`), which inverts the thread model relative to Linux: `NSApplication.run()` owns `main()`, and a dedicated caption bridge thread marshals STT results back onto the main thread via `dispatch::Queue::main().exec_async` — the direct analogue of GTK's `glib::MainContext::spawn_local`. For inference, WebGPU (backed by Metal) replaces CUDA as the primary accelerator; a known ORT bug causes non-deterministic crashes when multiple threads construct WebGPU sessions concurrently, but Subtidal's existing single-threaded `stt-pipeline` design already provides the required isolation as an invariant. The phases are ordered so Phase 0 empirically validates WebGPU on real Apple Silicon hardware before any other work begins, with a documented CPU-only fallback path if it proves unworkable.

## Definition of Done

**Primary deliverable:** Subtidal runs natively on macOS 14.4+ Apple Silicon with feature parity to the Linux build, except where macOS platform constraints force documented behavioral differences.

**Success criteria:**

- `cargo run` on macOS (via a minimal `.app` wrapper script) produces a working live-caption overlay.
- All three overlay modes (Docked, Floating, Transcript) function with macOS-appropriate semantics.
- System and per-app audio capture work via ScreenCaptureKit (per-app at app granularity).
- STT pipeline runs via parakeet-rs (WebGPU primary, CPU fallback) using the same Nemotron model as Linux.
- Tray controls, hot-reload config, click-through, above-fullscreen toggle, drag, and audio-source fallback all functional.
- The `cargo check --lib --target x86_64-apple-darwin` CI check from `.github/workflows/macos-check.yml` continues to pass throughout; no Linux coupling regressions.
- Design doc is self-contained enough for a fresh Mac session (zero conversation context) to execute.

**Out of scope (deferred):**

- App Store distribution, Developer ID signing, notarization, DMG creation.
- Intel Mac support.
- macOS < 14.4 support (no fallback for older systems).
- Window/tab-level per-app audio (app-level only).
- Subregion click-through (window-level only).
- Auto-update mechanism.

**Documented platform parity gaps (intentional, not bugs):**

- Per-app audio capture is at app granularity, not per-window/per-tab (ScreenCaptureKit limitation; Linux PipeWire can route finer).
- Docked mode does not reserve screen space (macOS has no exclusive-zone equivalent of wlr-layer-shell struts); other windows may overlap the overlay. Above-fullscreen visibility is preserved via `NSWindowCollectionBehavior.fullScreenAuxiliary`.
- Click-through is window-level via `NSWindow.ignoresMouseEvents`, not subregion (no functional impact: caption modes have no interactive widgets, and Transcript mode is a regular window).

## Acceptance Criteria

### macos-port.AC1: Three overlay modes function on macOS

- **macos-port.AC1.1 Success:** Selecting Docked from the tray menu positions an NSPanel at the top of `NSScreen.main.visibleFrame` spanning the full screen width.
- **macos-port.AC1.2 Success:** Selecting Floating from the tray menu shows an NSPanel at the position recorded in `config.toml` (or a sensible default for first run), draggable via click-and-drag on the panel background.
- **macos-port.AC1.3 Success:** Selecting Transcript from the tray menu shows a regular NSWindow with a scrollable NSTextView; captions append as timestamped paragraphs and the view autoscrolls to the bottom when the user is at the bottom.
- **macos-port.AC1.4 Success:** Switching modes via the tray is instant and does not require restart; both windows are constructed once at startup and visibility-toggled.
- **macos-port.AC1.5 Failure:** Switching to Transcript while no captions have been received produces an empty (not crashed) NSTextView; the Save dialog still functions and produces a valid (possibly empty-of-fragments) JSON file.
- **macos-port.AC1.6 Edge:** Resizing the screen (external display connect/disconnect) while in Docked mode re-positions the panel to the new `NSScreen.main.visibleFrame`.

### macos-port.AC2: NSPanel renders correctly across Spaces and fullscreen

- **macos-port.AC2.1 Success:** Panel is constructed with `level = .floating`, `collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]`, `isFloatingPanel = true`, `styleMask` including `.borderless` and `.nonactivatingPanel` (verifiable via inspection helper in unit test).
- **macos-port.AC2.2 Success:** Panel is visible on every Space the user switches to (Mission Control verified).
- **macos-port.AC2.3 Success:** Panel is visible above another application's fullscreen window (Safari/Chrome in fullscreen confirmed).
- **macos-port.AC2.4 Success:** Toggling "Show Above Fullscreen" via the tray switches the panel's `level` between `.floating` and `.statusBar` without panel rebuild; change observable within one OverlayCommand cycle.
- **macos-port.AC2.5 Success:** Caption modes (Docked, Floating) have `ignoresMouseEvents = true`; clicks pass through to the window below.
- **macos-port.AC2.6 Failure:** Transcript mode has `ignoresMouseEvents = false`; clicks land on the window and Save button works.
- **macos-port.AC2.7 Edge:** Panel does not collide with the menu bar (positioned via `visibleFrame`, not `frame`).

### macos-port.AC3: ScreenCaptureKit audio capture

- **macos-port.AC3.1 Success:** Selecting "System Output" as the audio source captures all system audio; playing a video produces real-time captions.
- **macos-port.AC3.2 Success:** Selecting a specific running application as the audio source captures only that app's audio.
- **macos-port.AC3.3 Success:** Switching the audio source via the tray uses `SCStream.updateContentFilter` and does not interrupt the caption stream visibly (no panel flicker, no caption gap > 1 sample).
- **macos-port.AC3.4 Success:** When the captured app exits, an `NSUserNotification` is posted and the audio source falls back to System Output automatically.
- **macos-port.AC3.5 Success:** First-run launch surfaces the macOS Screen Recording permission prompt with the text from `NSScreenCaptureUsageDescription`; after granting, captures begin.
- **macos-port.AC3.6 Failure:** Refusing the Screen Recording permission produces a user-visible error (NSUserNotification or in-panel message), not a silent crash.
- **macos-port.AC3.7 Edge:** SCK callback maintains RT-safety discipline: no allocation, no logging, try_lock only, copy-and-return (verified by code review and a debug-build instrumentation that asserts no `Mutex::lock` calls inside the callback).

### macos-port.AC4: STT engine on macOS (WebGPU primary, CPU fallback)

- **macos-port.AC4.1 Success:** On Apple Silicon, `NemotronEngine::new` selects `ExecutionProvider::WebGpu` and engine init succeeds; a log line confirms `WebGpu` as the chosen provider.
- **macos-port.AC4.2 Success:** If `WebGpu` init fails (e.g., simulated by injected fault), `NemotronEngine::new` retries with `ExecutionProvider::Cpu`; a log line confirms fallback occurred.
- **macos-port.AC4.3 Success:** Transcript accuracy on the committed test fixture WAV (`tests/fixtures/macos-webgpu-smoke.wav`) matches the Linux baseline within tokenizer-level tolerance (identical token sequence; small whitespace differences acceptable).
- **macos-port.AC4.4 Success:** Real-time factor on the WebGPU path measured on the Phase 0 spike is ≤1.0 on the testing M-series machine.
- **macos-port.AC4.5 Edge:** Engine swap via the tray (single-engine for now, but the code path exists) reads `ArcSwap<Engine>` on the next chunk boundary; no concurrent session construction occurs (verified by code review of the STT thread).

### macos-port.AC5: Tray (NSStatusItem) controls

- **macos-port.AC5.1 Success:** Tray icon appears in the menu bar with `isTemplate = true`; renders correctly in light and dark mode without code branching.
- **macos-port.AC5.2 Success:** "Captions On/Off" menu item toggles the `CaptionsEnabled` shared state; the checkmark reflects the current state.
- **macos-port.AC5.3 Success:** "Mode" submenu lists Docked, Floating, Transcript; the active mode is checkmarked; selection posts `OverlayCommand::SetMode`.
- **macos-port.AC5.4 Success:** "Audio Source" submenu is populated dynamically from `list_sources()` and lists System Output plus running apps that produce audio (identified by bundle ID).
- **macos-port.AC5.5 Success:** "Show Above Fullscreen" toggles `config.above_fullscreen` and posts `OverlayCommand::SetAboveFullscreen` live (no rebuild).
- **macos-port.AC5.6 Success:** "Lock Position" toggle is only enabled in Floating mode; locks the panel against drag.
- **macos-port.AC5.7 Success:** "Quit Subtidal" (Cmd-Q) terminates the application cleanly via `applicationWillTerminate`, signaling all worker threads to shut down.
- **macos-port.AC5.8 Failure:** Tray menu items reflect disabled state when the underlying feature is unavailable (e.g., "Lock Position" is disabled in Docked/Transcript modes).

### macos-port.AC6: Hot-reload config

- **macos-port.AC6.1 Success:** Editing `~/Library/Application Support/Subtidal/config.toml` and saving triggers a debounced reload; changed values apply within ~500ms.
- **macos-port.AC6.2 Success:** Drag-induced position writes to config do not trigger a config-reload feedback loop (existing change-detection invariant preserved).
- **macos-port.AC6.3 Failure:** Malformed TOML in `config.toml` is warned to stderr and ignored; the application continues with previous config.

### macos-port.AC7: TCC permission stability

- **macos-port.AC7.1 Success:** Granting Screen Recording permission once persists across `cargo build && scripts/bundle-mac.sh && open Subtidal.app` cycles, as long as bundle ID stays `com.subtidal.app` and ad-hoc signing is re-applied.
- **macos-port.AC7.2 Failure:** Modifying `CFBundleIdentifier` or changing the signing identity invalidates the grant and re-prompts (documented expected behavior, not a regression).

### macos-port.AC8: Main-thread caption delivery and shutdown

- **macos-port.AC8.1 Success:** Captions sent from the STT thread arrive at the main thread via the caption bridge (`dispatch::Queue::main().exec_async`) and update the NSTextField; no AppKit thread-affinity panics in the AppKit-warnings log.
- **macos-port.AC8.2 Success:** Cmd-Q triggers a clean shutdown: SCK stream stopped, STT thread exits within 250ms (one `AudioWake::wait_timeout` cycle), audio thread exits, tray thread exits, `NSApplication.run()` returns, `main` exits with code 0.
- **macos-port.AC8.3 Edge:** Force-closing the app via Activity Monitor does not leave orphan SCK streams (verified by inspecting `lsof` for the captured device after kill).

### macos-port.AC9: CI matrix coverage

- **macos-port.AC9.1 Success:** `cargo check --lib --target x86_64-apple-darwin` passes on `ubuntu-latest` (existing check, no regression).
- **macos-port.AC9.2 Success:** `cargo check --lib --target aarch64-apple-darwin` passes on `ubuntu-latest` (new matrix entry).
- **macos-port.AC9.3 Failure:** Accidentally introducing Linux coupling into a notionally-neutral module (e.g., importing `pipewire::*` from `audio/mod.rs`) breaks both cross-target checks.

### macos-port.AC10: Documentation and self-containment

- **macos-port.AC10.1 Success:** `CLAUDE.md` is updated at end of Phase 7 to describe the codebase as cross-platform (Linux + macOS), not "Linux currently with macOS planned".
- **macos-port.AC10.2 Success:** This design document, read in isolation by a fresh Mac session with no prior conversation context, contains sufficient detail to execute every phase (verifiable by handing the doc to a fresh agent and observing that no clarifying questions about the design itself are needed — only codebase-state questions).
- **macos-port.AC10.3 Success:** Newly discovered macOS landmines (analogues of `project_ort_argv0_quirk.md` and `project_gpu_cuda_landmines.md`) are documented in a form suitable for the user's auto-memory.

## Glossary

- **ArcSwap**: A lock-free `Arc`-swapping primitive (from the `arc-swap` crate). Used here so the tray thread can atomically replace the current `SttEngine` choice while the STT pipeline thread reads it on each chunk boundary without taking a mutex.
- **AudioCommand**: An enum that tray code sends to the audio thread to request source changes (e.g., switch to a specific app's audio stream).
- **AudioSourceId**: A neutral enum (`SystemOutput` or `App(bundle_id)`) that identifies an audio source portably across platforms; the macOS implementation maps `App(bundle_id)` to a live PID for SCK filters.
- **AudioWake**: A synchronization primitive (an `AtomicBool` plus `Condvar`) that allows the real-time audio callback to unblock the STT pipeline thread without holding a mutex. The callback calls `notify()`; the STT thread calls `wait_timeout()`.
- **`block_on(tokio)`**: A way to run async code synchronously on the calling thread; used in `main_macos.rs` to await the HuggingFace model download before starting `NSApplication.run()`.
- **`build.rs` target gating**: A pattern where `build.rs` reads the `TARGET` environment variable (the cross-compilation target) rather than using `cfg!(target_os = ...)` (which reflects the build host). Necessary for correct behavior when cross-compiling — e.g., running `cargo check --target x86_64-apple-darwin` from a Linux machine.
- **`CaptionBuffer`**: A pure, GTK-free data structure that manages the line-fill model for caption display: words fill lines up to a character limit, oldest lines expire during silence. Shared across platforms.
- **`CaptionsEnabled`**: A shared boolean flag (wrapped in an `Arc`) indicating whether the caption pipeline is active. Toggled by the tray.
- **`CMSampleBuffer`**: A Core Media type that wraps time-stamped audio (or video) data delivered by ScreenCaptureKit callbacks. Subtidal uses `objc2-core-media` to extract raw PCM from it.
- **`collectionBehavior`**: An `NSWindow`/`NSPanel` property bitmask controlling how a window behaves across macOS Spaces and fullscreen modes. `.canJoinAllSpaces` makes the panel appear on every Space; `.fullScreenAuxiliary` makes it appear above fullscreen apps.
- **Docked mode**: An overlay mode where the caption panel is pinned to the top edge of the screen at full width — analogous to a status bar.
- **`dispatch::Queue::main().exec_async`**: The GCD (Grand Central Dispatch) Rust binding used to post a closure for execution on the main thread. The macOS equivalent of GTK's `glib::MainContext::spawn_local`.
- **ExecutionProvider**: An `ort` (ONNX Runtime) concept identifying the hardware backend for inference: `Cuda` on Linux, `WebGpu` (backed by Metal) on macOS, `Cpu` as universal fallback.
- **Floating mode**: An overlay mode where the caption panel is a free-floating, draggable window at a user-configurable position.
- **`hf-hub`**: The HuggingFace Hub Rust client crate, used to download the Nemotron model weights to a local cache directory.
- **`isMovableByWindowBackground`**: An `NSWindow` property that allows the user to drag the window by clicking anywhere on its background, without requiring a title bar.
- **`isTemplate`**: An `NSImage` property that tells AppKit to treat the image as a monochrome mask, automatically coloring it for light/dark mode. Required for correct tray icon rendering.
- **`MainThreadMarker`**: A zero-sized type from the `objc2` crate that proves at compile time that the current code is running on the main thread. AppKit constructors require it, enforcing thread safety statically.
- **Metal**: Apple's GPU graphics and compute API. Used here implicitly as the backend for ONNX Runtime's WebGPU execution provider on Apple Silicon.
- **Nemotron**: NVIDIA's 600M-parameter RNNT speech-recognition model. Used by Subtidal as the sole STT engine; implemented in `stt/nemotron.rs` using `parakeet-rs` and `ort`.
- **`NSPanel`**: A subclass of `NSWindow` designed for floating utility windows. Used for Docked and Floating caption modes because it supports `isFloatingPanel`, `nonactivatingPanel`, and `canJoinAllSpaces` behaviors that a regular window cannot.
- **`NSStatusItem`**: The macOS system tray icon object. Each `NSStatusItem` gets a button in the menu bar and an associated `NSMenu`.
- **`NSUserNotification`**: A macOS API for posting system-level desktop notifications (e.g., "audio source lost, falling back to System Output").
- **`objc2` crate family**: A set of Rust crates (`objc2`, `objc2-foundation`, `objc2-app-kit`, `objc2-screen-capture-kit`, `objc2-core-media`) that provide safe, typed Rust bindings to Apple's Objective-C frameworks via zero-overhead FFI.
- **`ort`**: The Rust binding to ONNX Runtime, used for neural network inference. Supports multiple `ExecutionProvider` backends.
- **`OverlayCommand`**: An enum sent from the tray thread (and config hot-reload) to the overlay thread to request display changes: mode switches, caption enable/disable, style updates, above-fullscreen toggle, etc.
- **`parakeet-rs`**: A Rust crate implementing the Parakeet/Nemotron RNNT decoder on top of `ort`. Provides the `Nemotron` struct with `from_pretrained` and `transcribe` methods.
- **PipeWire**: Linux's audio server/routing daemon. Used on Linux to capture per-application or system audio. Replaced by ScreenCaptureKit on macOS.
- **`@rpath` / `@loader_path`**: macOS dyld (dynamic linker) tokens that resolve shared library locations relative to the executable or the referencing binary, respectively. This mechanism makes the Linux argv[0] hack for locating CUDA stubs unnecessary on macOS.
- **RNNT**: Recurrent Neural Network Transducer, the architecture underlying the Nemotron model. A streaming-capable ASR architecture that emits tokens incrementally.
- **`rubato`**: A Rust crate for high-quality audio resampling. Used to convert 48kHz stereo PCM (from PipeWire or SCK) to 16kHz mono for the STT engine.
- **`SCContentFilter`**: A ScreenCaptureKit object specifying what to capture: either all displays (system audio) or a specific running application.
- **`SCShareableContent`**: A ScreenCaptureKit API that enumerates what can be captured — displays, windows, and running applications that produce audio.
- **`SCStream`**: The ScreenCaptureKit object that manages an active capture session. Delivers audio (and optionally video) to a delegate callback. Supports live source switching via `updateContentFilter`.
- **ScreenCaptureKit (SCK)**: Apple's high-level framework (macOS 12.3+) for screen and audio capture. Requires user-granted Screen Recording permission via TCC.
- **Shell-and-re-export pattern**: A code organization pattern in Subtidal where `mod.rs` is a thin cfg-gated shell that declares the platform implementation submodule and re-exports its public surface, keeping the module's public API stable across platforms.
- **TCC (Transparency, Consent, and Control)**: macOS's permission system for sensitive capabilities (Screen Recording, Microphone, etc.). Grants are keyed on `(CFBundleIdentifier, code signature)`, which is why a stable `.app` bundle with a fixed bundle ID is required for development.
- **Transcript mode**: An overlay mode using a regular titled window with an append-only, timestamped log of all recognized speech. Persists captions across mode switches and supports JSON export.
- **`TranscriptLog`**: A pure data structure (no GTK or AppKit dependencies) that stores timestamped speech fragments, coalesces them into paragraphs, and serializes to JSON. Shared across platforms.
- **`updateContentFilter`**: An `SCStream` method for live audio source switching without stopping and restarting the stream, avoiding any audible gap or caption delay.
- **`visibleFrame`**: An `NSScreen` property returning the screen area not occupied by the menu bar or Dock. Used for Docked mode positioning to avoid overlapping the menu bar.
- **WebGPU (execution provider)**: The ONNX Runtime backend that delegates computation to the WebGPU graphics API, which on Apple Silicon translates to Metal. Used as the primary accelerator on macOS in place of CUDA.
- **wlr-layer-shell**: A Wayland protocol extension for rendering overlay windows at specific screen layers (above or below normal windows). Used on Linux for Docked/Floating modes. Has no macOS equivalent; `NSPanel` with appropriate `level` and `collectionBehavior` provides similar (though not identical) semantics.

## Architecture

The macOS port preserves every neutral contract in the Subtidal codebase (`SttEngine` trait, `AudioWake`, `Arc<ArcSwap<Engine>>`, `OverlayCommand`, `CaptionsEnabled`, `CaptionBuffer`, `TranscriptLog`, `AudioCommand`, `Config`) and adds three platform-implementation siblings — `audio/impl_macos.rs`, `tray/impl_macos.rs`, an `overlay/macos/` subtree — plus a `main_macos.rs` startup orchestrator. This matches the "Recipe for adding a new platform" in `CLAUDE.md` exactly.

### File map (additions and modifications)

```
src/main_macos.rs                  — NEW: startup orchestration (model download via block_on tokio, spawn workers, NSApplication.run())
src/audio/impl_macos.rs            — NEW: ScreenCaptureKit capture, ring buffer push, SCShareableContent enumeration, AudioWake.notify
src/overlay/macos/mod.rs           — NEW: orchestration, OverlayCommand dispatch, run_app public API
src/overlay/macos/panel.rs         — NEW: NSPanel construction (Docked/Floating), level/collectionBehavior, NSTextField
src/overlay/macos/drag.rs          — NEW: minimal (isMovableByWindowBackground=true); no quirk compensation
src/overlay/macos/transcript_window.rs — NEW: regular NSWindow with NSScrollView+NSTextView, autoscroll, NSSavePanel
src/tray/impl_macos.rs             — NEW: NSStatusItem + NSMenu construction and action handlers
scripts/bundle-mac.sh              — NEW: builds, wraps in .app, ad-hoc codesigns
resources/macos/Info.plist         — NEW: minimal plist with TCC usage descriptions
resources/macos/tray-icon-template.png — NEW: 22x22 monochrome (black + alpha)

src/main.rs                        — MODIFIED: cfg-gate mod main_macos; remove the macOS branch from the compile_error! guard
src/lib.rs                         — MODIFIED: cfg-gate new macOS modules so `cargo check --lib --target x86_64-apple-darwin` exercises them
src/audio/mod.rs                   — MODIFIED: cfg-gate impl_macos and re-export
src/overlay/mod.rs                 — MODIFIED: cfg-gate macos/ subtree
src/tray/mod.rs                    — MODIFIED: cfg-gate impl_macos
src/stt/mod.rs                     — MODIFIED: cfg-gate macOS branch in build_engine to select WebGpu provider
src/stt/nemotron.rs                — MODIFIED: cfg-add ExecutionProvider::WebGpu (macOS) alongside existing Cuda (Linux); Cpu fallback shared
build.rs                           — MODIFIED: extend early-return predicate so macOS path skips CUDA scanning
Cargo.toml                         — MODIFIED: add [target.'cfg(target_os = "macos")'.dependencies] block
.github/workflows/macos-check.yml  — MODIFIED: extend matrix to also run aarch64-apple-darwin
```

### Thread model

Five threads. The Linux thread roles invert on macOS because AppKit enforces main-thread affinity at the OS level (objc2's `MainThreadMarker` is the type-level enforcer), so `NSApplication.run()` must occupy `main()`. The channel topology and inter-thread contracts are otherwise identical to Linux.

| Thread | Role | Notes |
|---|---|---|
| **Main** | Model download via `block_on(tokio)`, then `NSApplication.run()` blocks until Cmd-Q | `MainThreadMarker::new()` acquired early; passed through `NSApplication::sharedApplication(mtm)` |
| **screen-capture-audio** | Owns the `SCStream`; SCK delegate runs on SCK's internal dispatch queue and pushes f32 PCM into the existing ring buffer, then calls `AudioWake::notify()` | Same RT-safety discipline as the PipeWire callback: no allocation, no blocking, try_lock only, copy and return |
| **stt-pipeline** | Identical to Linux: blocks on `AudioWake::wait_timeout(250ms)`, reads `Arc<ArcSwap<Engine>>` on each chunk batch, rebuilds engine if changed, calls `SttEngine::process_chunk`, sends captions via `async_channel::Sender<String>` | **Must stay single-threaded** to avoid the ORT WebGPU concurrent-session race ([microsoft/onnxruntime#27592](https://github.com/microsoft/onnxruntime/issues/27592)). The existing ArcSwap-load-on-chunk-boundary pattern already provides this guarantee — preserved invariant, not a new constraint |
| **Tray** | Hosts NSMenu action handlers; menu actions post `OverlayCommand` / `AudioCommand` through the same channels the Linux ksni tray uses | NSMenu construction occurs on the main thread once at startup; subsequent action callbacks fire on the main thread (AppKit guarantee) |
| **Caption bridge** | Tiny helper thread that blocks on `Receiver<String>::recv()` and wraps each caption in `dispatch::Queue::main().exec_async(\|\| label.setStringValue(...))` | Direct analogue of GTK's `glib::MainContext::spawn_local` future. Same observable behavior as Linux: captions arrive on the main thread for AppKit text updates |

Shutdown: AppKit sends `applicationWillTerminate` on Cmd-Q; the app delegate flips a shared `AtomicBool` and posts wakes to all worker threads. STT thread observes the flag on its next `wait_timeout` tick. SCK stream is stopped via `stopCapture(completionHandler:)` wrapped in `block_on`.

### STT pipeline

The macOS STT path adds a single `ExecutionProvider::WebGpu` branch alongside the existing `Cuda`/`Cpu` branches in `stt/nemotron.rs`. The Nemotron 600M model, parakeet-rs crate, ort version, RNNT decoder, 160ms-chunk contract, and downstream caption pipeline are all unchanged from Linux. WebGPU delegates to Metal under the hood via wgpu on Apple Silicon.

**Engine selection contract:**

```rust
// Conceptual shape — actual cfg-gating per-line
#[cfg(target_os = "linux")]
ExecutionProvider::Cuda    // primary if cuda_available()
ExecutionProvider::Cpu     // fallback

#[cfg(target_os = "macos")]
ExecutionProvider::WebGpu  // primary; always-attempt (Metal is always present on Apple Silicon)
ExecutionProvider::Cpu     // fallback on WebGpu init failure
```

**Probe pattern:** No equivalent of `cuda_available()` is needed. Metal is guaranteed on Apple Silicon, so macOS attempts `WebGpu` directly and falls back to `Cpu` only if `Nemotron::from_pretrained` returns an error. No reexec-with-absolute-argv0 hack is needed (the ORT argv[0] quirk is Linux-specific — macOS uses `@rpath`/`@loader_path` for dylib resolution).

**Plan B if WebGPU proves unworkable in Phase 0 spike:** Ship CPU-only as the macOS default and document `whisper.cpp` (via `whisper-rs` or `whisper-cpp-plus`) as an alternate `SttEngine` implementation behind a new `Engine` variant. This Plan B is documented but not built unless Phase 0 forces it.

### Audio pipeline

ScreenCaptureKit replaces PipeWire. The post-callback pipeline (ring buffer → rubato resampler → 16kHz mono f32 chunks → `SttEngine::process_chunk`) is unchanged.

**SCStream configuration contract:**

```
SCStreamConfiguration:
  capturesAudio                 = true
  excludesCurrentProcessAudio   = true       // defensive against future feedback loops
  sampleRate                    = 48000      // requested; may deliver 44.1 on some HW — normalize at callback boundary
  channelCount                  = 2          // stereo, matches existing pre-resampler contract
  capturesVideo                 = false      // audio-only, minimizes overhead

SCContentFilter:
  System Output:  display(.main, excludingApplications: [])
  Per-App:        display(.main, including: [SCRunningApplication(pid)])
```

**Format normalization** happens at the SCK callback boundary inside `audio/impl_macos.rs`: extract `AudioBufferList` from `CMSampleBuffer` via `objc2-core-media` (unsafe FFI; no safe wrapper exists in the ecosystem yet), validate `(sampleRate, channelCount, format)`, convert to 48kHz stereo f32 if SCK delivered a different shape (typically a no-op on Apple Silicon), push into the existing ring buffer, call `AudioWake::notify()`. Drop samples silently on ring overflow.

**Audio source enumeration** uses a neutral surface so the tray UI code stays cross-platform:

```rust
// audio/mod.rs (neutral)
pub struct AudioSource { pub id: AudioSourceId, pub label: String }
pub enum AudioSourceId { SystemOutput, App(String /* bundle_id */) }

pub fn list_sources() -> Vec<AudioSource>;
```

`impl_macos::list_sources` calls `SCShareableContent.current` and returns `SystemOutput` plus one `App(bundle_id)` per running application that produces audio. **Bundle IDs are the stable identifier across launches** — PIDs change, bundle IDs don't.

**Source switching** uses `SCStream.updateContentFilter(_:completionHandler:)` for live updates (no flicker, no buffer flush). This is *better* than Linux's stop-and-recreate pattern but observably equivalent.

**Source fallback:** when a captured app's audio stops (caught via SCK's stream-stopped delegate callback), the audio thread posts an `NSUserNotification` and falls back to `SystemOutput` via `updateContentFilter`. Same observable behavior as the Linux PipeWire-node-disappeared fallback.

### Overlay (NSPanel for caption modes, NSWindow for Transcript)

Both windows are constructed once at startup and visibility-toggled by `OverlayCommand::SetMode` — same lifecycle pattern as the Linux overlay.

**Caption overlay (single NSPanel, reconfigured per mode):**

```
NSPanel:
  styleMask                     = [.borderless, .nonactivatingPanel]
  level                         = .floating    (or .statusBar when above-fullscreen toggled on)
  collectionBehavior            = [.canJoinAllSpaces, .fullScreenAuxiliary]
  isFloatingPanel               = true
  backgroundColor               = .clear
  hasShadow                     = false (Floating) / true (Docked)
  ignoresMouseEvents            = true         // window-level click-through
  isMovableByWindowBackground   = true (Floating) / false (Docked)

Content view: NSTextField, wrappable, sizeToFit on update
  attributedStringValue = NSAttributedString {
    font:  monospace (matches Linux Pango font choice)
    color: from Config
    per-line max width
  }
```

**Mode geometry:**

- **Docked:** position at top of `NSScreen.main.visibleFrame`, full width, fixed height.
- **Floating:** smaller rect from `Config`, draggable, position persisted back to config via the existing hot-reload-safe write path.
- **Above-fullscreen toggle:** flip `level` between `.floating` and `.statusBar` live via `OverlayCommand::SetAboveFullscreen` — no rebuild needed (matching Linux's `Layer::Overlay`/`Layer::Top` toggle).

**Transcript window** is a regular titled `NSWindow` containing `NSScrollView` → `NSTextView` → `NSTextStorage`. Append paragraphs on receipt; autoscroll to bottom unless the user has scrolled away from the bottom. Save dialog uses `NSSavePanel.runModal`; serialization goes through the neutral `TranscriptLog::to_json`.

**Captions-disable edge** clears all four caption surfaces (`TranscriptLog`, transcript `NSTextView`, `CaptionBuffer`, overlay `NSTextField`) — preserved from Linux.

**Drag mechanics:** `isMovableByWindowBackground = true` is sufficient. No coordinate-quirk compensation, no `is_dragging` flag, no relayout suppression — AppKit handles Y-axis flipping internally and does not exhibit gtk4-layer-shell's drag relayout jitter.

### Tray (NSStatusItem)

```
NSStatusItem from NSStatusBar.system.statusItem(withLength: NSSquareStatusItemLength)
  button.image            = NSImage(byReferencingFile: "<bundle>/Contents/Resources/tray-icon-template.png")
  button.image.isTemplate = true            // monochrome auto-styling for light/dark

NSMenu items:
  - Captions On/Off              (checkmark from CaptionsEnabled)
  - Mode > [Docked | Floating | Transcript]
  - Engine > [Nemotron]          (single-item for now; matches Linux)
  - Audio Source > [System Output | App: <label> | ...]    (dynamic from list_sources())
  - Show Above Fullscreen        (checkmark from config.above_fullscreen)
  - Lock Position                (checkmark; Floating mode only)
  - ---
  - Quit Subtidal                (Cmd-Q)
```

Menu actions post `OverlayCommand`, `AudioCommand`, or mutate `Arc<ArcSwap<Engine>>` through the same channels the Linux ksni tray uses. The `start_tray(...) -> JoinHandle<()>` shape in `tray/mod.rs` is preserved.

**Icon prep (one-time task in Phase 6):** create a 22×22 monochrome (black + alpha) version of the existing Linux tray icon at `resources/macos/tray-icon-template.png`. `isTemplate = true` discards RGB and lets macOS auto-color for light/dark mode.

### TCC permissions and the `.app` wrapper

TCC keys on `(CFBundleIdentifier, code signature)`. A bare `target/release/subtidal` invocation invalidates TCC grants on every `cargo build` (ad-hoc signature changes with binary content). The fix is a minimal `.app` bundle with a stable bundle ID + stable ad-hoc identity:

```
Subtidal.app/
  Contents/
    MacOS/
      subtidal                    (the cargo build output, copied or symlinked)
    Info.plist                    (from resources/macos/Info.plist)
    Resources/
      tray-icon-template.png      (from resources/macos/tray-icon-template.png)
```

**Minimal `Info.plist`:**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>          <string>com.subtidal.app</string>
  <key>CFBundleExecutable</key>          <string>subtidal</string>
  <key>CFBundlePackageType</key>         <string>APPL</string>
  <key>CFBundleVersion</key>             <string>1.0</string>
  <key>NSScreenCaptureUsageDescription</key>
    <string>Subtidal captures system audio to display live captions.</string>
  <key>NSMicrophoneUsageDescription</key>
    <string>Subtidal captures audio for speech-to-text processing.</string>
</dict>
</plist>
```

**`scripts/bundle-mac.sh`** (called manually after `cargo build`):

1. `cargo build --release`
2. Construct `Subtidal.app/Contents/{MacOS,Resources}` skeleton
3. Copy `target/release/subtidal` → `Subtidal.app/Contents/MacOS/subtidal`
4. Copy `resources/macos/Info.plist` → `Subtidal.app/Contents/Info.plist`
5. Copy `resources/macos/tray-icon-template.png` → `Subtidal.app/Contents/Resources/`
6. `codesign --force --deep --sign - Subtidal.app`

In-place binary replacement on subsequent rebuilds preserves the TCC grant as long as the bundle ID and signing identity stay stable.

## Existing Patterns

This design follows established patterns in the Subtidal codebase:

**Platform isolation (`CLAUDE.md` § "Platform Isolation" and § "Recipe for adding a new platform"):**

- **Shell-and-re-export** for `audio/` and `tray/`: `mod.rs` is a thin shell that declares `#[cfg(target_os = "macos")] mod impl_macos;` and re-exports the public surface. The macOS implementation body lives in `impl_macos.rs`. Mirrors the existing `impl_linux.rs` pattern.
- **Subtree-and-re-export** for `overlay/`: `mod.rs` keeps neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, `transcript_log`) at the module root and gates the `macos/` subdirectory holding panel/drag/transcript_window submodules. Mirrors the existing `linux/` subtree.
- **In-place gating** for `stt/`: existing pattern keeps neutral types (`SttEngine` trait, `AudioWake`, `PipelineConfig`) unguarded and gates only platform-specific items (`ExecutionProvider::WebGpu` selection) with `#[cfg(target_os = "macos")]` directly.

**Cargo target-conditional dependencies (`CLAUDE.md` § "Cargo dependencies"):** the existing `[target.'cfg(target_os = "linux")'.dependencies]` block enables `parakeet-rs` and `ort` with the `cuda` feature. The new macOS block follows the same additive-feature-unification pattern (Cargo resolver v2) with `webgpu` instead of `cuda`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
parakeet-rs = { version = "0.3.4", features = ["webgpu"] }
ort = { version = "2.0.0-rc.12", features = ["webgpu"] }
objc2 = "..."
objc2-foundation = "..."
objc2-app-kit = "..."
objc2-screen-capture-kit = "..."
objc2-core-media = "..."
objc2-core-foundation = "..."
dispatch = "..."
```

Resolver v2 keeps `cuda` and `webgpu` features isolated per target — no feature bleeding.

**`build.rs` target gating (`CLAUDE.md` § "Build-script gate"):** existing code uses `env::var("TARGET").unwrap_or_default().contains("linux")` rather than `cfg!(target_os = "linux")` because the macro reflects the build host, not the cross-compilation target. The macOS branch extends this predicate so `cargo check --target x86_64-apple-darwin` from a Linux host correctly skips both CUDA scanning and any future macOS-specific build-time work.

**`compile_error!` placement (`CLAUDE.md` § "`compile_error!` location"):** the existing guard in `main.rs` sits immediately after the `mod main_linux;` declaration. The macOS port adds `#[cfg(target_os = "macos")] mod main_macos;` next to it and refines the `compile_error!` predicate to exclude both Linux and macOS (e.g., trigger only on Windows/BSD/etc.).

**Neutral contracts preserved without modification:** `SttEngine` trait, `AudioWake` notify/wait pattern, `Arc<ArcSwap<Engine>>` engine-switching, 48kHz stereo F32LE pre-resampler format, `OverlayCommand` dispatch, three-mode lifecycle, `TranscriptLog::to_json`, hot-reload `Config` change-detection, 4-surface caption clearing on captions-disable, `AudioCommand` and `AudioSource` neutral types. None of these change.

**CI extension (`.github/workflows/macos-check.yml`):** the existing workflow runs `cargo check --lib --target x86_64-apple-darwin` on `ubuntu-latest`. The macOS port extends the matrix to also run `aarch64-apple-darwin` (Apple Silicon target). Adding targets to the matrix is the established extension pattern — replacing the workflow is not.

**Audio format normalization at platform boundary:** the existing `audio/resampler.rs` (rubato 48kHz stereo → 16kHz mono) is platform-neutral. PipeWire delivers 48kHz stereo F32LE directly; SCK may deliver other shapes, but the SCK callback normalizes at the boundary to the same 48kHz stereo F32LE that the rubato resampler consumes. The contract between audio capture and the resampler is preserved.

## Implementation Phases

Eight phases, ordered so the highest-risk validation runs first and each subsequent phase builds on a working foundation. The `cargo check --lib --target x86_64-apple-darwin` CI check must pass after every phase. Phase counts toward writing-plans' 8-phase hard limit are exact (Phase 0 through Phase 7 = 8 phases).

<!-- START_PHASE_0 -->
### Phase 0: WebGPU spike (de-risk before anything else)
**Goal:** Empirically verify that `parakeet_rs::Nemotron` with `ExecutionProvider::WebGpu` runs correctly on Apple Silicon Metal before committing to the rest of the port.

**Components:**
- New macOS-conditional block in `Cargo.toml` enabling `parakeet-rs`/`ort` with the `webgpu` feature
- `examples/macos_webgpu_smoke.rs` — throwaway example that constructs a `Nemotron` engine with `ExecutionProvider::WebGpu`, runs inference on a committed test WAV fixture, prints the transcription
- `tests/fixtures/macos-webgpu-smoke.wav` — short pre-recorded test audio (≤10s of clear speech)

**Dependencies:** None (first phase)

**Done when:**
- `cargo run --release --example macos_webgpu_smoke` on the target Apple Silicon machine transcribes the fixture WAV correctly
- Measured real-time factor on the test machine is ≤1.0
- If WebGPU init fails: design's Plan B activates (CPU primary; whisper.cpp evaluation deferred to a separate design plan)
- `cargo check --lib --target x86_64-apple-darwin` remains green

**Note:** This phase is infrastructure verification (does the toolchain work?) and a parity AC verification (`macos-port.AC4` family is testable here). Failure here triggers a design re-spin, not a workaround.
<!-- END_PHASE_0 -->

<!-- START_PHASE_1 -->
### Phase 1: Skeleton wiring + `.app` bundle
**Goal:** Produce a launchable `.app` bundle from `cargo build`, with all cfg-gating in place so cross-target CI continues to pass.

**Components:**
- `src/main_macos.rs` — minimal stub: prints "Hello from macOS Subtidal" and exits cleanly
- `src/main.rs` — modify `compile_error!` predicate to exclude macOS; add `#[cfg(target_os = "macos")] mod main_macos;` and entry-point dispatch
- `src/lib.rs` — cfg-gate `audio::impl_macos`, `tray::impl_macos`, `overlay::macos` (modules can be empty `pub mod` stubs at this phase)
- `src/audio/mod.rs`, `src/overlay/mod.rs`, `src/tray/mod.rs`, `src/stt/mod.rs` — cfg-gating updates
- `build.rs` — extend early-return predicate so macOS targets skip CUDA scanning
- `Cargo.toml` — add `[target.'cfg(target_os = "macos")'.dependencies]` block with all objc2-* crates, `dispatch`, parakeet-rs/ort with `webgpu` feature
- `resources/macos/Info.plist` — minimal plist (CFBundleIdentifier, CFBundleExecutable, CFBundlePackageType, CFBundleVersion, NSScreenCaptureUsageDescription, NSMicrophoneUsageDescription)
- `scripts/bundle-mac.sh` — build, construct `.app` skeleton, copy binary + plist, `codesign --force --deep --sign -`

**Dependencies:** Phase 0 (engine choice locked)

**Done when:**
- `cargo build --release` succeeds on macOS
- `scripts/bundle-mac.sh` produces `target/release/Subtidal.app`
- `open target/release/Subtidal.app` (or direct execution of `Subtidal.app/Contents/MacOS/subtidal`) prints the hello message and exits cleanly
- `cargo check --lib --target x86_64-apple-darwin` from a Linux host remains green
- Bundle Info.plist is parseable (`plutil -lint` exits clean)

Covers: infrastructure verification only (no functionality ACs yet).
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: NSApplication startup + minimal NSPanel
**Goal:** Run `NSApplication.run()` on the main thread with worker threads spawned; show an empty NSPanel; demonstrate the caption bridge pattern with hardcoded test captions.

**Components:**
- `src/main_macos.rs` — full startup orchestration: acquire `MainThreadMarker`, build the small `block_on(tokio)` model-download invocation (still stubbed if model not yet wired), spawn placeholder audio/stt/tray threads (no-op stubs), spawn caption bridge thread, call `NSApplication.run()`
- `src/overlay/macos/mod.rs` — `run_app()` public surface; OverlayCommand dispatch loop
- `src/overlay/macos/panel.rs` — `NSPanel` construction with all flags from the Architecture section: `styleMask`, `level`, `collectionBehavior`, `isFloatingPanel`, `ignoresMouseEvents`; NSTextField content view
- Caption bridge: helper thread that blocks on `Receiver<String>::recv()` and wraps each in `dispatch::Queue::main().exec_async(|| label.setStringValue(...))`
- Test harness: a timer-driven hardcoded caption stream ("Hello", "Hello, world", "Hello, world, from macOS") to verify the bridge end-to-end

**Dependencies:** Phase 1 (skeleton wiring complete)

**Done when:**
- `.app` launches; an empty Floating-mode NSPanel appears
- Hardcoded test captions appear in the NSPanel via the caption bridge (verifying main-thread marshaling works)
- Panel is visible above fullscreen apps (manually verify by entering Safari fullscreen)
- Cmd-Q quits cleanly; no worker-thread hang
- Tests: unit test for `overlay/macos/panel.rs` configuration (verify level/collectionBehavior/flags via inspection helpers)

Covers: `macos-port.AC2.1` (panel construction), `macos-port.AC2.4` (above-fullscreen), `macos-port.AC8.1` (main-thread caption delivery), `macos-port.AC8.2` (clean shutdown).
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: STT engine on macOS (WebGPU + CPU fallback)
**Goal:** Wire the real Nemotron engine for macOS with WebGPU primary and CPU fallback.

**Components:**
- `src/stt/nemotron.rs` — add `#[cfg(target_os = "macos")] ExecutionProvider::WebGpu` branch alongside existing Cuda/Cpu branches in `NemotronEngine::new`
- `src/stt/mod.rs` — macOS branch in `build_engine` that attempts WebGPU first, catches init error, retries with Cpu, logs which provider succeeded
- `src/main_macos.rs` — wire the real STT pipeline thread (replacing the Phase 2 stub) using the existing neutral `spawn_stt_thread` shape
- Reuse all neutral STT pipeline code unchanged: ring buffer drain, resampler, AudioWake wait, ArcSwap engine read

**Dependencies:** Phase 0 (WebGPU confirmed working), Phase 2 (caption bridge proven)

**Done when:**
- Feeding pre-recorded f32 PCM (sine wave + the same test WAV from Phase 0) through the in-process pipeline produces captions in the NSPanel
- Engine selection log line shows `WebGpu` as the chosen provider on Apple Silicon
- Forcing WebGPU init failure (e.g., via injected fault) correctly falls back to Cpu and produces captions
- Tests: integration test that drives the pipeline with the test WAV and asserts caption content

Covers: `macos-port.AC4.1` (WebGPU primary), `macos-port.AC4.2` (CPU fallback), `macos-port.AC4.3` (transcript accuracy parity with Linux).
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: ScreenCaptureKit audio capture (SystemOutput only)
**Goal:** Capture system audio via SCK and feed it to the STT pipeline; handle the TCC permission prompt flow.

**Components:**
- `src/audio/impl_macos.rs`: `SCStream` setup with the documented `SCStreamConfiguration` (capturesAudio=true, excludesCurrentProcessAudio=true, sampleRate=48000, channelCount=2, capturesVideo=false); `SCContentFilter` for SystemOutput; `SCStreamOutput` delegate implementation that extracts PCM from `CMSampleBuffer` via `objc2-core-media` and pushes to the ring buffer
- RT-safety discipline enforced in the callback: no allocation, no logging, try_lock only, `AudioWake::notify()` after each push
- Format normalization at the callback boundary (validate 48kHz/stereo/f32; convert if SCK delivered something else)
- TCC handling: `NSScreenCaptureUsageDescription` in Info.plist (already present from Phase 1); first launch surfaces system permission dialog
- Stream lifecycle: `startCapture(completionHandler:)` and `stopCapture(completionHandler:)` wrapped in `block_on` for synchronous worker-thread integration

**Dependencies:** Phase 3 (STT pipeline live), Phase 2 (panel visible)

**Done when:**
- Launch the `.app`, grant Screen Recording permission on first run
- Play a YouTube video or system audio; captions appear in real time in the NSPanel
- Cmd-Q stops the stream cleanly (no dangling `SCStream`)
- Subsequent launches do not re-prompt for permission (verifying TCC stability via bundle ID + ad-hoc sign)
- Tests: unit test for format normalization (mock CMSampleBuffer inputs of various shapes → expected 48kHz stereo f32 output)

Covers: `macos-port.AC3.1` (system audio capture), `macos-port.AC3.5` (TCC permission flow), `macos-port.AC7.1` (TCC stability across rebuilds).
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Per-app capture + source switching
**Goal:** Enumerate running applications as audio sources; switch between sources live; handle source-disappeared fallback.

**Components:**
- `src/audio/impl_macos.rs`: `list_sources()` implementation calling `SCShareableContent.current` and producing the neutral `Vec<AudioSource>` (SystemOutput plus per-app entries keyed by bundle ID)
- Per-app `SCContentFilter` construction: `display(.main, including: [SCRunningApplication(pid)])`
- Source switching via `SCStream.updateContentFilter(_:completionHandler:)` — no stream stop/restart
- Source fallback: SCK stream-stopped delegate callback → post `NSUserNotification` via `objc2-foundation`/`objc2-user-notifications` → call `updateContentFilter` with SystemOutput
- Bundle ID stability across launches: snapshot bundle ID at selection time and re-resolve to current PID on each session (PIDs are not persisted)

**Dependencies:** Phase 4 (SCK capture working)

**Done when:**
- A test driver (no tray yet) sends `AudioCommand::SetSource(App("com.apple.Safari"))` and audio capture switches to Safari without flicker
- Killing Safari while captured triggers a desktop notification and falls back to SystemOutput
- `list_sources()` returns SystemOutput plus a non-empty set of running apps producing audio
- Tests: unit test for `list_sources()` neutral shape; integration test for source switch via injected command

Covers: `macos-port.AC3.2` (per-app capture), `macos-port.AC3.3` (live source switching), `macos-port.AC3.4` (source-disappeared fallback).
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Tray + full overlay modes
**Goal:** Implement the NSStatusItem tray and complete all three overlay modes with drag, above-fullscreen, lock-position, and transcript.

**Components:**
- `src/tray/impl_macos.rs`: NSStatusItem construction with `NSStatusBar.system.statusItem`; `NSImage(byReferencingFile:)` + `isTemplate = true` for the monochrome icon; full `NSMenu` (Captions, Mode, Engine, Audio Source, Show Above Fullscreen, Lock Position, Quit) with action handlers that post commands through the existing neutral channels
- `src/overlay/macos/panel.rs`: Docked geometry (top of `NSScreen.main.visibleFrame`, full width); Floating geometry from `Config` with persisted position; above-fullscreen `level` toggle live via `OverlayCommand::SetAboveFullscreen`
- `src/overlay/macos/drag.rs`: `isMovableByWindowBackground = true` + persisted position write-back to `Config` (using the same hot-reload-safe change-detection path Linux uses, so drag doesn't trigger a config-reload feedback loop)
- `src/overlay/macos/transcript_window.rs`: regular NSWindow with NSScrollView + NSTextView; autoscroll with user-scroll detection; NSSavePanel for `TranscriptLog::to_json` export
- `resources/macos/tray-icon-template.png`: generate the 22×22 monochrome (black + alpha) version of the existing Linux tray icon (manual graphics task; the existing colored icon's alpha channel can be reused if visually adequate)
- 4-surface caption clearing on captions-disable preserved (TranscriptLog, transcript NSTextView, CaptionBuffer, NSPanel NSTextField)

**Dependencies:** Phase 5 (audio sources enumerable for the Audio Source menu)

**Done when:**
- Tray icon appears in the menu bar with correct light/dark mode rendering
- All menu items functional: toggle captions on/off; switch among Docked/Floating/Transcript; switch among audio sources; toggle Show Above Fullscreen; toggle Lock Position (Floating only); Quit
- Floating mode is draggable; new position persists to `~/Library/Application Support/subtidal/config.toml` (or the macOS equivalent of the existing Linux config path)
- Transcript mode shows append-only timestamped paragraphs; Save dialog produces a valid `.json` matching the existing serialization contract
- Captions-disable clears all four surfaces
- Tests: unit tests for tray menu construction; integration test for mode-switching via command injection; integration test for transcript Save round-trip (write → read → parse)

Covers: `macos-port.AC1` family (overlay modes), `macos-port.AC2` family (panel behaviors), `macos-port.AC5` family (tray controls), `macos-port.AC6` family (config hot-reload).
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Polish + CI matrix + AC validation
**Goal:** Extend cross-target CI, walk every Acceptance Criterion manually, and document discovered macOS landmines.

**Components:**
- `.github/workflows/macos-check.yml` — extend the matrix to also run `cargo check --lib --target aarch64-apple-darwin` alongside the existing x86_64 check
- Manual AC walkthrough: every `macos-port.AC*` criterion validated against the running `.app`, recorded in a `docs/macos-port-ac-results.md` checklist (gets deleted before merge — purely a tracking artifact)
- Document discovered macOS-specific landmines (analogues of `project_ort_argv0_quirk.md` and `project_gpu_cuda_landmines.md`) for the user to commit to their auto-memory: e.g., any Metal-specific quirks, parakeet-rs WebGPU empirical findings, TCC re-prompt scenarios encountered during development
- `CHANGELOG.md` (if present) entry for macOS support
- Update `CLAUDE.md`: convert the "Recipe for adding a new platform" prose into a "Platform implementations: Linux and macOS" reality; refresh the freshness date

**Dependencies:** Phase 6 (full feature set built)

**Done when:**
- Both `x86_64-apple-darwin` and `aarch64-apple-darwin` checks pass in CI
- Every `macos-port.AC*.*` criterion has a validation step that was run on the target Mac and passed
- `CLAUDE.md` accurately describes the cross-platform codebase
- Newly discovered landmines documented for user memory ingestion

Covers: cross-cutting `macos-port.AC9` (CI matrix coverage), `macos-port.AC10` (documentation freshness).
<!-- END_PHASE_7 -->

## Additional Considerations

**ORT argv[0] quirk does NOT apply to macOS.** Linux uses argv[0] prefix to locate the CUDA stub library, which is why `main_linux.rs` contains `reexec_with_absolute_argv0_if_needed`. macOS uses `@rpath`/`@loader_path` for dylib resolution via standard dyld mechanisms. The macOS startup path is correspondingly simpler — no reexec hack, no absolute-argv0 handling.

**ORT WebGPU concurrent-session race ([microsoft/onnxruntime#27592](https://github.com/microsoft/onnxruntime/issues/27592)).** A known non-deterministic crash when multiple threads construct ORT WebGPU sessions concurrently on macOS Metal. Subtidal is naturally safe: only the single `stt-pipeline` thread constructs engines, and engine swaps go through `ArcSwap` reads on chunk boundaries — never concurrent. This invariant must be preserved; any future change that allows multi-threaded engine construction needs to re-evaluate this risk.

**Metal VRAM release on engine teardown.** Metal's memory pooling may not release VRAM immediately when an `ort::Session` is dropped. If hot model-reload loops are added later (e.g., engine cycling for testing), insert a small delay between teardown and recreation to allow Metal to release. Not a concern for the current Phase 6 design where the user changes engine occasionally via tray.

**Dylib distribution during development.** parakeet-rs and ort ship their dylibs to `target/release/` alongside the binary. The `.app` bundle script (Phase 1) copies the binary but does not currently relocate dylibs. For development, run via `DYLD_LIBRARY_PATH=target/release ./target/release/Subtidal.app/Contents/MacOS/subtidal` if dylib lookup fails. A future distribution-oriented design plan would handle proper `Contents/Frameworks/` packaging with `install_name_tool` — explicitly out of scope here.

**Config path on macOS.** The Linux config lives at `~/.config/subtidal/config.toml`. The macOS convention is `~/Library/Application Support/Subtidal/config.toml`. The existing `Config::load_path` resolution must be cfg-gated so each platform uses its conventional path. This is a small `config.rs` change not called out separately above; flag during Phase 1 wiring.

**Model directory path on macOS.** Similar story: Linux uses `~/.local/share/subtidal/models/nemotron/`; macOS convention is `~/Library/Application Support/Subtidal/models/nemotron/`. The existing `models::models_dir()` helper must be cfg-gated. Not called out as a separate phase task; folded into Phase 3 (STT wiring).

**TCC permission re-prompt scenarios to watch for during development.**
1. Renaming the `.app` directory or changing `CFBundleIdentifier` will re-prompt — keep `com.subtidal.app` stable.
2. Modifying `Info.plist` content (other than `NSScreenCaptureUsageDescription` text) can change the signature — re-run `codesign --force --deep --sign -` after any plist edit.
3. Replacing the binary in-place inside an existing signed `.app` is generally safe; the system re-verifies the signature on launch.

**Implementation scoping note.** This design has 8 phases (Phase 0 through Phase 7), at the writing-plans 8-phase hard limit. No splitting needed, but no room to add more phases without restructuring. If Phase 0 reveals that WebGPU is unworkable and CPU-with-whisper.cpp becomes the path, a separate follow-up design plan for the whisper.cpp engine should be created rather than expanding this one.
