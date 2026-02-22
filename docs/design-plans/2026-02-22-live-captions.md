# Live Captions Design

## Summary

Live-captions is a Rust desktop application for Linux that displays real-time speech-to-text transcriptions as a persistent on-screen overlay. It captures audio from PipeWire — either the system-wide output or a specific application's stream — resamples it to the format expected by a local speech recognition model, runs inference on a dedicated thread, and renders the resulting text in a GTK4 window that floats above all other content. No audio or caption data leaves the machine; all transcription is done locally using ONNX model weights downloaded from HuggingFace on first run.

The application is structured as four concurrent domains connected by channels: a real-time PipeWire callback feeds audio into a lock-free ring buffer; an inference thread drains that buffer, resamples the audio, and runs it through whichever STT engine is active (Parakeet TDT 0.6B on GPU, or Moonshine on CPU); the GTK4 main thread receives caption strings and updates the overlay window; and a `ksni`-based system tray thread handles user controls via a right-click menu. Configuration — including overlay appearance, audio source, and engine choice — is stored in a TOML file and hot-reloaded at runtime so appearance changes apply without restarting the application.

## Definition of Done

- A Rust desktop application for Linux (KDE Plasma 6 / Wayland) that captures audio via PipeWire — either system output (monitor sink) or a user-selected application stream — and produces live captions using a pluggable STT engine: Parakeet TDT 0.6B (primary, GPU/CUDA via ONNX) or Moonshine (CPU fallback via ONNX)
- Captions displayed as an on-screen overlay above all other windows, in either docked mode (wlr-layer-shell, anchored to a configurable screen edge) or floating mode (freely positionable, click-through when locked)
- A system tray icon that toggles captions on/off on left-click; right-click menu provides audio source selection (radio group), overlay mode and lock controls, STT engine selection, and access to settings
- Configurable appearance: background color/transparency, text color, font size; configuration persists across sessions
- English-only transcription; no caption persistence to file

## Acceptance Criteria

### live-captions.AC1: Audio is captured from the selected PipeWire source
- **live-captions.AC1.1 Success:** System output (monitor sink) is captured by default on first launch
- **live-captions.AC1.2 Success:** Selecting an application node from the tray menu switches capture to that stream
- **live-captions.AC1.3 Success:** Switching audio source does not require restarting the application
- **live-captions.AC1.4 Failure:** If the selected application node disappears, capture falls back to system output, the tray source selection updates to reflect the change, and a desktop toast notification identifies what was lost and what it fell back to
- **live-captions.AC1.5 Failure:** If PipeWire is unavailable at startup, the app exits with a clear error message

### live-captions.AC2: Live captions are produced with acceptable latency
- **live-captions.AC2.1 Success:** Spoken English produces caption text within 300ms of utterance end (Parakeet engine)
- **live-captions.AC2.2 Success:** Spoken English produces caption text within 400ms of utterance end (Moonshine engine)
- **live-captions.AC2.3 Success:** Captions update continuously during sustained speech without long gaps
- **live-captions.AC2.4 Failure:** Silence produces no spurious caption output

### live-captions.AC3: Overlay displays captions correctly
- **live-captions.AC3.1 Success:** Overlay appears above all other windows in docked mode (anchored to configured edge)
- **live-captions.AC3.2 Success:** Overlay appears above all other windows in floating mode
- **live-captions.AC3.3 Success:** Docked mode passes all pointer and keyboard events through to windows below
- **live-captions.AC3.4 Success:** Floating mode in locked state passes all pointer events through
- **live-captions.AC3.5 Success:** Floating mode in unlocked state can be dragged to any screen position; position persists across restart
- **live-captions.AC3.6 Success:** Switching between docked and floating mode at runtime works without restart
- **live-captions.AC3.7 Success:** Caption text respects configured font size, text color, and background color/transparency
- **live-captions.AC3.8 Failure:** Toggling captions off hides the overlay entirely

### live-captions.AC4: System tray controls work correctly
- **live-captions.AC4.1 Success:** Left-clicking tray icon toggles captions on and off
- **live-captions.AC4.2 Success:** Right-click menu reflects current state (active source, engine, overlay mode, lock state)
- **live-captions.AC4.3 Success:** Audio source submenu shows all available PipeWire sinks and application streams as a radio group
- **live-captions.AC4.4 Success:** STT engine can be switched at runtime via tray menu without restart
- **live-captions.AC4.5 Success:** Overlay lock/unlock menu item is greyed out in docked mode

### live-captions.AC5: STT engine management
- **live-captions.AC5.1 Success:** On first run, required model files are downloaded automatically before captions start
- **live-captions.AC5.2 Success:** Subsequent runs skip download when model files are already present
- **live-captions.AC5.3 Success:** If CUDA is unavailable, app automatically falls back to Moonshine (CPU) with a tray tooltip warning
- **live-captions.AC5.4 Failure:** If model download fails, app exits with a clear error message pointing to the missing model

### live-captions.AC6: Configuration persists and hot-reloads
- **live-captions.AC6.1 Success:** Audio source, engine, overlay mode and position persist across restarts
- **live-captions.AC6.2 Success:** Editing background color, text color, or font size in config.toml applies to the live overlay within 1 second
- **live-captions.AC6.3 Failure:** A malformed config.toml causes a warning log but the app starts with defaults

## Glossary

- **PipeWire**: A Linux audio and video routing daemon that replaced PulseAudio and JACK. This application uses it to capture audio streams from individual applications or from the system-wide output.
- **Monitor sink**: A PipeWire/PulseAudio concept for a loopback capture source that records whatever is currently playing through a given audio output device, rather than from a microphone.
- **Application node**: A PipeWire graph node representing a single running application's audio stream. Identified internally by a numeric node ID rather than a name, which may not be unique.
- **STT (Speech-to-Text)**: The process of converting an audio waveform containing speech into a text transcript. Also called automatic speech recognition (ASR).
- **Parakeet TDT 0.6B**: A streaming-capable ASR model from NVIDIA, run here via ONNX on a CUDA-capable GPU. "TDT" refers to Token-and-Duration Transducer, an architecture that enables low-latency streaming output.
- **Moonshine**: A lightweight ASR model from Useful Sensors designed for CPU inference. Used as the fallback engine when CUDA is unavailable.
- **ONNX / ONNX Runtime (`ort`)**: Open Neural Network Exchange — a standard file format for trained ML models, and a runtime library for executing them. Allows models trained in Python frameworks to run in Rust without those frameworks.
- **CUDA**: NVIDIA's parallel computing platform and API, used here to run GPU-accelerated inference with the Parakeet engine. Requires `libcudart` to be present at runtime.
- **EOU (End of Utterance)**: A signal from the Parakeet model indicating that a complete speech segment has been recognized and a final transcript is ready. `ParakeetEOU` is the specific API surface used here.
- **VAD (Voice Activity Detection)**: A pre-processing step that classifies audio frames as speech or silence. Used by the Moonshine engine to avoid running inference on silent audio.
- **wlr-layer-shell**: A Wayland protocol extension (originating from wlroots compositors) that allows applications to place surfaces at fixed positions relative to the screen edge, above or below normal windows. Used here for the docked overlay mode.
- **xdg_toplevel**: The standard Wayland protocol surface type for normal application windows. Used for the floating overlay mode.
- **`gtk4-layer-shell`**: A library that wraps the wlr-layer-shell Wayland protocol for use with GTK4 applications.
- **`wl_surface` input region**: A Wayland concept defining which area of a window surface receives pointer and keyboard events. Setting this to an empty region makes a window fully click-through.
- **`ksni`**: A Rust library for creating system tray icons via the StatusNotifierItem D-Bus protocol, used by KDE Plasma and other compliant desktop environments.
- **D-Bus**: A Linux inter-process communication system. Used here by `ksni` to register and communicate with the desktop environment's system tray.
- **`rubato`**: A Rust audio resampling library. Used here to convert 48kHz stereo PCM from PipeWire to 16kHz mono PCM expected by both STT engines.
- **`ringbuf::HeapRb`**: A heap-allocated lock-free ring buffer from the `ringbuf` crate. Used to pass audio from the real-time PipeWire callback to the inference thread without blocking or allocating memory in the callback.
- **`glib::MainContext` channel**: GTK's thread-safe channel primitive for sending messages from background threads to the GTK main thread, which owns the GUI.
- **`hf-hub`**: A Rust client for the HuggingFace Hub API, used here to download ONNX model weight files on first run.
- **`notify`**: A Rust file-system event library. Used to watch `config.toml` for changes and trigger hot-reload of appearance settings without restarting.
- **F32LE**: A raw PCM audio sample format: 32-bit IEEE 754 floating-point, little-endian. The format PipeWire delivers audio in to this application.
- **PCM (Pulse-Code Modulation)**: The standard uncompressed digital audio representation — a sequence of amplitude samples at a fixed sample rate. All audio in this pipeline is PCM.
- **Exclusive zone**: A wlr-layer-shell property specifying how much screen space a layer surface reserves for itself, preventing other windows from overlapping that region. Set to zero here so the overlay does not displace other content.
- **KDE Plasma 6 / Wayland**: The target desktop environment. KDE Plasma 6 is the sixth major version of the KDE desktop; Wayland is the Linux display server protocol it runs on in this configuration.
- **TOML**: A human-readable configuration file format. Used for the application's `config.toml`.
- **`anyhow`**: A Rust crate for ergonomic error handling that produces rich, chainable error messages. Used throughout for propagating errors.
- **`notify-rust`**: A Rust crate for sending desktop notifications via the `org.freedesktop.Notifications` D-Bus interface. Used to alert the user when audio source fallback occurs.

## Architecture

Live-captions is a single Rust binary with four concurrent domains connected by channels. GTK4 owns the main thread. PipeWire manages its own real-time callback thread. A dedicated inference thread runs STT processing. `ksni` handles tray events over D-Bus.

```
PipeWire RT callback
    │  F32LE 48kHz stereo frames
    ▼
[ringbuf::HeapRb<f32>]  (lock-free, no allocation in callback)
    │
Inference thread
    │  rubato: 48kHz stereo → 16kHz mono
    │  accumulate 160ms chunks
    │  SttEngine::process_chunk() → Option<String>
    ▼
[glib::MainContext channel]
    │
GTK4 main thread ──── overlay window (wlr-layer-shell or xdg_toplevel)
    │                    gtk::Label::set_text()
    │                    gtk::Window::set_visible()
    │
ksni tray thread ──── left-click: toggle captions (Arc<AtomicBool>)
                  └─── right-click: menu → send commands to GTK4/inference
```

**STT abstraction:**

```rust
trait SttEngine: Send + 'static {
    fn sample_rate(&self) -> u32;  // both engines: 16000
    fn process_chunk(&mut self, pcm: &[f32]) -> anyhow::Result<Option<String>>;
}
```

`ParakeetEngine` wraps `parakeet_rs::ParakeetEOU`, feeding 160ms chunks (2560 samples). `MoonshineEngine` wraps an `ort::Session` over Moonshine ONNX weights, using a sliding window with simple energy VAD. Engine selection is determined at startup from config; switching via tray tears down and respawns the inference thread.

**Audio capture:**

PipeWire connection opens a capture stream in `F32LE` stereo 48kHz against the selected sink monitor or application node. The real-time callback writes into a `ringbuf::HeapRb<f32>` without blocking or allocating. The inference thread drains the ring buffer, passes frames through `rubato` (48kHz stereo → 16kHz mono), and accumulates chunks until a full 160ms window is ready.

**Overlay:**

Two modes share an `Arc<AtomicBool>` for caption visibility and the same `glib::MainContext` channel for caption text and command events.

- *Docked*: `gtk4_layer_shell` surface anchored to a configured screen edge, zero exclusive zone, empty input region + `keyboard_interactivity: None`. Always click-through.
- *Floating*: Borderless `gtk::Window` (xdg_toplevel). Locked state: empty `wl_surface` input region (click-through). Unlocked state: full input region, drag handle visible. Position saved to config on drag release.

Switching modes destroys and recreates the window; caption text state is preserved in memory.

**Config and model storage:**

- Config: `~/.config/live-captions/config.toml` (TOML, watched with `notify` for hot-reload of appearance fields)
- Models: `~/.local/share/live-captions/models/` (downloaded via `hf-hub` on first run with progress to stderr)

## Existing Patterns

This is a greenfield project with no existing codebase. The design introduces the following patterns, chosen to match common Rust desktop application conventions:

- **Thread-per-domain with channels** — standard for Rust GTK4 apps where the GUI must own the main thread and audio callbacks must be real-time safe
- **Trait-based backend abstraction** (`SttEngine`) — allows engine switching without touching pipeline code; follows the same pattern used by projects like `transcribe-rs`
- **Lock-free ring buffer for audio** — standard practice for bridging real-time audio callbacks to non-real-time processing threads, avoiding priority inversion
- **TOML config with `notify` hot-reload** — common in Rust desktop tools (e.g. Alacritty)

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Project Scaffolding and Configuration

**Goal:** Establish project structure, dependency declarations, and the configuration layer. No runtime audio or GUI yet.

**Components:**
- `Cargo.toml` — all dependencies declared: `gtk4`, `gtk4-layer-shell`, `ksni`, `pipewire`, `rubato`, `ringbuf`, `parakeet-rs`, `ort`, `hf-hub`, `notify`, `serde`, `toml`, `anyhow`, `tokio` (for hf-hub async)
- `src/config.rs` — `Config` struct with all fields (engine, audio source, overlay mode/geometry/appearance, font settings); load, save, and file-watch logic
- `src/main.rs` — argument parsing (`--config`, `--engine`, `--reset-config`), config load/create, and placeholder stubs for all subsystem init
- `src/models/mod.rs` — path resolution for model directories (`~/.local/share/live-captions/models/`)

**Dependencies:** None

**Done when:** `cargo build` succeeds; `cargo run` loads or creates a default config file and exits cleanly
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Model Management

**Goal:** Auto-download STT model weights from HuggingFace on first run; verify presence on subsequent runs.

**Components:**
- `src/models/mod.rs` (extended) — download logic via `hf-hub`; checks for expected ONNX files (Parakeet: encoder, decoder, joiner; Moonshine: single ONNX file); progress reporting to stderr; resumable downloads

**Dependencies:** Phase 1

**Done when:** Running the app with no models present downloads Parakeet INT8 ONNX files to `~/.local/share/live-captions/models/parakeet/`; subsequent runs skip download; missing model for the selected engine exits with a clear error message
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Audio Capture

**Goal:** Capture audio from PipeWire (system monitor or selected application node) and deliver 16kHz mono PCM chunks to a consumer thread.

**Components:**
- `src/audio/mod.rs` — PipeWire stream setup; sink/application node enumeration; runtime source switching; lock-free ring buffer handoff
- `src/audio/resampler.rs` — `rubato`-based 48kHz stereo → 16kHz mono downsampler; chunk accumulator producing 160ms windows

**Dependencies:** Phase 1

**Done when:** With a selected audio source active, 160ms chunks of 16kHz mono f32 PCM are delivered to a channel consumer; node enumeration returns available sinks and application streams; source switching reconnects without restart
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: STT Engine Layer

**Goal:** Define the `SttEngine` trait and implement both backends. Wire the inference thread to consume audio chunks and emit caption strings.

**Components:**
- `src/stt/mod.rs` — `SttEngine` trait; inference thread spawn/teardown; channel to GTK4 main context
- `src/stt/parakeet.rs` — `ParakeetEngine`: wraps `parakeet_rs::ParakeetEOU`; feeds 160ms chunks; returns caption strings on EOU events
- `src/stt/moonshine.rs` — `MoonshineEngine`: wraps `ort::Session` over Moonshine ONNX; sliding-window VAD; returns caption strings

**Dependencies:** Phases 2, 3

**Done when:** With audio capture running, spoken English produces caption strings on the channel within ~200ms (Parakeet) or ~300ms (Moonshine); both engines pass correctness tests on known audio fixtures; engine selection from config determines which backend starts
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Overlay Window

**Goal:** Display captions in a GTK4 overlay window supporting docked (wlr-layer-shell) and floating (xdg_toplevel) modes with configurable appearance and input region management.

**Components:**
- `src/overlay/mod.rs` — GTK4 window creation; mode dispatch; `glib::MainContext` channel receiver; `gtk::Label` caption display (configurable font, color, max lines); caption text state
- Docked mode: `gtk4_layer_shell` surface anchored to configured edge; zero exclusive zone; empty input region
- Floating mode: borderless `gtk::Window`; drag handle widget; `wl_surface_set_input_region` toggle (locked/unlocked); position save on drag release

**Dependencies:** Phase 1

**Done when:** Overlay displays caption text received via channel; docked mode anchors to bottom by default and passes all pointer events through; floating mode can be unlocked via tray, dragged to any position, re-locked to click-through; background color, text color, and font size apply from config; mode switch works at runtime
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: System Tray

**Goal:** System tray icon with full right-click menu providing all runtime controls.

**Components:**
- `src/tray/mod.rs` — `ksni` tray registration; menu construction and refresh; event dispatch to overlay (via glib channel) and inference thread (via mpsc); audio source list fetched from PipeWire on menu open

**Menu structure:**
- Captions on/off (checkable)
- Audio Source submenu (radio group: System Output + live application nodes)
- Overlay submenu: Docked/Floating radio, Lock overlay position (checkable, floating only)
- STT Engine submenu (radio group: Parakeet / Moonshine)
- Settings (opens config file via `xdg-open`)
- Quit

**Dependencies:** Phases 3, 4, 5

**Done when:** Left-click toggles caption visibility; right-click menu reflects current state; audio source selection reconnects PipeWire stream; STT engine switch tears down and restarts inference thread; overlay lock toggle changes input region; Quit exits cleanly
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Config Persistence and Hot-reload

**Goal:** All runtime state changes persist to config; appearance changes apply immediately without restart.

**Components:**
- `src/config.rs` (extended) — `notify` file watcher; hot-reload handler applies appearance fields (background color, text color, font size, max lines) to live overlay; tray menu writes engine/audio/overlay state to config on change

**Dependencies:** Phases 5, 6

**Done when:** Editing `config.toml` background color updates the overlay within 1 second without restart; selecting a different audio source via tray persists to config and is restored on next launch; all config fields round-trip correctly through save/load
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Full Integration and Error Handling

**Goal:** Wire all subsystems into a complete startup sequence with robust error handling and graceful shutdown.

**Components:**
- `src/main.rs` (completed) — full startup sequence: config load → model check/download → PipeWire init → inference thread spawn → GTK4 main loop start → tray registration
- Error handling: PipeWire disconnection retries after 5s and falls back to system output; inference errors log and skip the chunk; config parse errors warn and use defaults; SIGTERM/SIGINT trigger graceful shutdown (inference thread join, PipeWire disconnect, GTK4 quit)

**Dependencies:** All previous phases

**Done when:** App starts, shows overlay, and captions live audio end-to-end; losing and regaining the PipeWire source recovers without restart; Ctrl-C exits cleanly; all Phase 1–7 acceptance criteria pass together in an integrated run
<!-- END_PHASE_8 -->

## Additional Considerations

**Wayland input region on GTK4:** `wl_surface_set_input_region` must be called after the surface is realized and mapped. The floating mode unlock/lock sequence must account for this by deferring region changes to a `gtk::Widget::map` signal handler on first show.

**PipeWire node names:** Application node names can be non-unique (e.g. two Firefox instances). The tray menu should display `application.name` with PID disambiguation where needed, and store the PipeWire node ID (not name) in config for reliable restoration.

**ONNX Runtime CUDA initialization:** `ort` with the CUDA execution provider requires `libcudart` to be present at runtime. The app should detect CUDA availability at startup and automatically fall back to Moonshine (CPU) if CUDA is unavailable, with a tray tooltip warning.
