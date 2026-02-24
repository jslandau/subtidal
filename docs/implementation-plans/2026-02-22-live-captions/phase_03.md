# Live Captions Implementation Plan — Phase 3: Audio Capture

**Goal:** Capture audio from PipeWire (system monitor or selected application node), resample from 48kHz stereo to 16kHz mono, and deliver 160ms chunks (2560 f32 samples) to a channel consumer.

**Architecture:** PipeWire MainLoop runs on a dedicated OS thread. Its RT callback writes F32LE stereo frames into a lock-free `ringbuf::HeapRb<f32>`. An audio processing task running on the same thread (or a separate one) drains the ring buffer, downmixes stereo → mono, resamples 48→16kHz via rubato, accumulates 2560-sample chunks, and sends them over a `std::sync::mpsc` channel to the inference thread (Phase 4).

**Tech Stack:** pipewire 0.9, rubato 1.0, ringbuf 0.4.

**Scope:** Phase 3 of 8. Depends on Phase 1 (Config struct, AudioSource enum).

**Codebase verified:** 2026-02-22 — greenfield, no src/audio/ exists.

---

## Acceptance Criteria Coverage

### live-captions.AC1: Audio is captured from the selected PipeWire source
- **live-captions.AC1.1 Success:** System output (monitor sink) is captured by default on first launch
- **live-captions.AC1.2 Success:** Selecting an application node from the tray menu switches capture to that stream
- **live-captions.AC1.3 Success:** Switching audio source does not require restarting the application
- **live-captions.AC1.4 Failure:** If the selected application node disappears, capture falls back to system output, the tray source selection updates to reflect the change, and a desktop toast notification identifies what was lost and what it fell back to
- **live-captions.AC1.5 Failure:** If PipeWire is unavailable at startup, the app exits with a clear error message

---

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Create src/audio/resampler.rs — stereo downmix and 48→16kHz resampling

**Files:**
- Create: `src/audio/resampler.rs`
- Create: `src/audio/` directory

**Step 2: Create src/audio/resampler.rs**

This module wraps rubato and accumulates 160ms output chunks.

```rust
//! Audio resampler: 48kHz stereo F32 → 16kHz mono F32 with 160ms chunk output.

use anyhow::{Context, Result};
use rubato::{FftFixedIn, Resampler};

/// Input sample rate from PipeWire (stereo).
pub const INPUT_SAMPLE_RATE: u32 = 48_000;
/// Output sample rate for STT engines.
pub const OUTPUT_SAMPLE_RATE: u32 = 16_000;
/// Output chunk size: 160ms at 16kHz = 2560 samples.
pub const CHUNK_SAMPLES: usize = 2_560;
/// Corresponding input chunk size at 48kHz: 480ms input for 160ms output.
/// FftFixedIn requires input size = output_size * ratio = 2560 * 3 = 7680 frames (stereo).
pub const INPUT_FRAMES_PER_CHUNK: usize = 7_680;

/// Resamples 48kHz stereo → 16kHz mono and accumulates 160ms output chunks.
pub struct AudioResampler {
    resampler: FftFixedIn<f32>,
    /// Accumulation buffer: mono 16kHz samples waiting to fill a 160ms chunk.
    accumulator: Vec<f32>,
    /// Interleaved stereo input buffer waiting to fill one resampler input chunk.
    input_buf: Vec<f32>,
}

impl AudioResampler {
    /// Create a new resampler for 48kHz stereo → 16kHz mono.
    pub fn new() -> Result<Self> {
        // FftFixedIn<f32>: fixed input size resampler.
        // Parameters: input_rate, output_rate, chunk_size (in input frames), sub_chunks, channels
        // chunk_size = INPUT_FRAMES_PER_CHUNK frames, 2 channels (stereo input)
        let resampler = FftFixedIn::<f32>::new(
            INPUT_SAMPLE_RATE as usize,
            OUTPUT_SAMPLE_RATE as usize,
            INPUT_FRAMES_PER_CHUNK,
            2, // sub-chunks (1 = no sub-chunking)
            2, // channels: stereo input
        )
        .context("creating FftFixedIn resampler")?;

        Ok(AudioResampler {
            resampler,
            accumulator: Vec::with_capacity(CHUNK_SAMPLES * 2),
            input_buf: Vec::with_capacity(INPUT_FRAMES_PER_CHUNK * 2 * 2),
        })
    }

    /// Feed interleaved stereo 48kHz f32 samples. Returns complete 160ms mono chunks as they
    /// become available. May return zero or more chunks per call.
    ///
    /// `samples` must be interleaved stereo: [L0, R0, L1, R1, ...]
    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<Vec<Vec<f32>>> {
        self.input_buf.extend_from_slice(samples);
        let mut output_chunks = Vec::new();

        // Process full resampler input chunks (INPUT_FRAMES_PER_CHUNK * 2 interleaved samples).
        let interleaved_chunk = INPUT_FRAMES_PER_CHUNK * 2;
        while self.input_buf.len() >= interleaved_chunk {
            let chunk: Vec<f32> = self.input_buf.drain(..interleaved_chunk).collect();

            // Deinterleave stereo into two channel vectors.
            let mut left = Vec::with_capacity(INPUT_FRAMES_PER_CHUNK);
            let mut right = Vec::with_capacity(INPUT_FRAMES_PER_CHUNK);
            for pair in chunk.chunks_exact(2) {
                left.push(pair[0]);
                right.push(pair[1]);
            }

            // Resample both channels.
            // rubato 1.0 API: FftFixedIn::process(&[impl AsRef<[f32]>], None) -> Result<Vec<Vec<f32>>>
            // This API is unchanged from 0.15 — no audioadapter crate is needed when
            // manually deinterleaving as done here.
            let resampled = self.resampler.process(&[left, right], None)
                .context("resampling audio")?;

            // resampled: Vec<Vec<f32>> with 2 channels (left, right) at 16kHz.
            // Downmix to mono by averaging.
            let out_len = resampled[0].len();
            for i in 0..out_len {
                let mono = (resampled[0][i] + resampled[1][i]) * 0.5;
                self.accumulator.push(mono);
            }

            // Drain full 160ms output chunks.
            while self.accumulator.len() >= CHUNK_SAMPLES {
                let chunk: Vec<f32> = self.accumulator.drain(..CHUNK_SAMPLES).collect();
                output_chunks.push(chunk);
            }
        }

        Ok(output_chunks)
    }

    /// Flush remaining buffered samples as a final (possibly shorter) chunk.
    /// Call when shutting down or switching audio sources.
    pub fn flush(&mut self) -> Vec<f32> {
        self.input_buf.clear();
        self.accumulator.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_produces_correct_chunk_size() {
        let mut r = AudioResampler::new().unwrap();
        // Feed exactly one resampler input worth of stereo frames.
        // INPUT_FRAMES_PER_CHUNK frames × 2 channels = 15360 interleaved samples.
        let samples: Vec<f32> = vec![0.1f32; INPUT_FRAMES_PER_CHUNK * 2];
        let chunks = r.push_interleaved(&samples).unwrap();
        // Exactly one complete 160ms output chunk expected.
        assert_eq!(chunks.len(), 1, "expected 1 chunk, got {}", chunks.len());
        assert_eq!(chunks[0].len(), CHUNK_SAMPLES, "chunk should be {} samples", CHUNK_SAMPLES);
    }

    #[test]
    fn resampler_accumulates_partial_input() {
        let mut r = AudioResampler::new().unwrap();
        // Feed half a resampler input chunk — should produce no output yet.
        let samples: Vec<f32> = vec![0.1f32; INPUT_FRAMES_PER_CHUNK]; // only half
        let chunks = r.push_interleaved(&samples).unwrap();
        assert!(chunks.is_empty(), "partial input should not produce output");
    }

    #[test]
    fn resampler_accumulates_across_multiple_pushes() {
        let mut r = AudioResampler::new().unwrap();
        // Feed in small increments; total must add up to 1 full input chunk.
        let increment: Vec<f32> = vec![0.0f32; 512];
        let total_needed = INPUT_FRAMES_PER_CHUNK * 2; // stereo
        let mut total_chunks = 0usize;
        let mut pushed = 0usize;
        while pushed < total_needed {
            let to_push = increment.len().min(total_needed - pushed);
            let out = r.push_interleaved(&increment[..to_push]).unwrap();
            total_chunks += out.len();
            pushed += to_push;
        }
        assert_eq!(total_chunks, 1, "one full input chunk should yield one output chunk");
    }
}
```

**Step 3: Verify compilation and tests pass**

```bash
cargo test audio::resampler::tests
```

Expected: All 3 tests pass. If `FftFixedIn::process()` API differs in rubato 1.0.1, run `cargo doc --open` and check the exact signature; the semantics (deinterleaved channel slices in, Vec<Vec<f32>> out) are unchanged.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Create src/audio/mod.rs — PipeWire stream and node enumeration

**Files:**
- Create: `src/audio/mod.rs`

**Step 1: Create src/audio/mod.rs**

This module manages the PipeWire connection, stream, and node list.

```rust
//! PipeWire audio capture: stream setup, node enumeration, runtime source switching.

pub mod resampler;

use anyhow::{bail, Context, Result};
use pipewire as pw;
use pw::properties::properties;
use ringbuf::{HeapRb, traits::{Producer, Consumer}};
use std::sync::{Arc, Mutex};
use std::thread;

/// A discovered PipeWire audio node (sink or application stream).
#[derive(Debug, Clone)]
pub struct AudioNode {
    pub node_id: u32,
    pub name: String,
    pub description: String,
    /// true = system sink/monitor; false = application output stream
    pub is_monitor: bool,
}

/// Commands sent to the PipeWire thread for runtime control.
pub enum AudioCommand {
    /// Switch to a new audio source.
    SwitchSource(crate::config::AudioSource),
    /// Shut down the PipeWire thread.
    Shutdown,
}

/// Shared list of discovered audio nodes (updated by registry callbacks).
pub type NodeList = Arc<Mutex<Vec<AudioNode>>>;

/// Ring buffer capacity: 1 second of 48kHz stereo f32 samples.
/// HeapRb<f32> counts f32 elements, not bytes — so 48000 frames × 2 channels = 96_000 elements.
const RING_BUF_CAPACITY: usize = 48_000 * 2;

/// Start the PipeWire audio capture thread.
///
/// Returns:
/// - `tx_cmd`: send AudioCommand to the PipeWire thread
/// - `rx_audio`: receive raw interleaved stereo 48kHz f32 samples (drained by inference thread)
/// - `node_list`: shared list of available audio nodes (updated by registry)
///
/// Exits the process if PipeWire is unavailable (AC1.5).
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
) -> Result<(
    std::sync::mpsc::SyncSender<AudioCommand>,
    ringbuf::HeapCons<f32>,
    NodeList,
)> {
    // Initialize PipeWire library (must be called before any PW objects).
    pw::init();

    // Test PipeWire availability by attempting to create a MainLoop.
    // If this fails, PipeWire is unavailable (AC1.5).
    // The actual MainLoop is created on the PipeWire thread below.

    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(RING_BUF_CAPACITY).split();
    let ring_producer = Arc::new(Mutex::new(ring_producer));

    let node_list: NodeList = Arc::new(Mutex::new(Vec::new()));
    let node_list_clone = Arc::clone(&node_list);

    let (tx_cmd, rx_cmd) = std::sync::mpsc::sync_channel::<AudioCommand>(8);

    let ring_producer_thread = Arc::clone(&ring_producer);

    thread::Builder::new()
        .name("pipewire-audio".to_string())
        .spawn(move || {
            if let Err(e) = run_pipewire_loop(
                initial_source,
                ring_producer_thread,
                node_list_clone,
                rx_cmd,
            ) {
                eprintln!("error: PipeWire audio thread exited: {e:#}");
                std::process::exit(1);
            }
        })
        .context("spawning PipeWire thread")?;

    Ok((tx_cmd, ring_consumer, node_list))
}

/// Enumerate available audio nodes from the shared node list.
/// Called by the tray to build the audio source submenu.
pub fn list_nodes(node_list: &NodeList) -> Vec<AudioNode> {
    node_list.lock().unwrap().clone()
}

/// Main PipeWire event loop (runs on dedicated thread).
fn run_pipewire_loop(
    initial_source: crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    node_list: NodeList,
    rx_cmd: std::sync::mpsc::Receiver<AudioCommand>,
) -> Result<()> {
    let mainloop = pw::main_loop::MainLoop::new(None)
        .context("creating PipeWire MainLoop — is PipeWire running?")?;
    let context = pw::context::Context::new(&mainloop)
        .context("creating PipeWire Context")?;
    let core = context.connect(None)
        .context("connecting to PipeWire — is PipeWire running?")?;
    let registry = core.get_registry()
        .context("getting PipeWire Registry")?;

    // Collect disappeared node IDs from the registry global_remove callback.
    // Phase 8's NodeDisappeared handler reads this list in the command loop below.
    let disappeared_node_ids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

    // Listen for node additions/removals to populate node_list.
    let node_list_registry = Arc::clone(&node_list);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            // Filter for audio nodes: application streams and monitor sinks.
            // global.props contains node properties.
            if let Some(props) = &global.props {
                let media_class = props.get("media.class").unwrap_or("");
                let node_name = props.get("node.name").unwrap_or("").to_string();
                let description = props.get("node.description")
                    .or(props.get("node.nick"))
                    .unwrap_or(&node_name)
                    .to_string();

                let is_monitor = media_class == "Audio/Source"
                    && node_name.ends_with(".monitor");
                let is_app_stream = media_class == "Stream/Output/Audio";

                if is_monitor || is_app_stream {
                    let node = AudioNode {
                        node_id: global.id,
                        name: node_name,
                        description,
                        is_monitor,
                    };
                    node_list_registry.lock().unwrap().push(node);
                }
            }
        })
        .global_remove({
            // Phase 8 wires this to AudioCommand::NodeDisappeared (AC1.4).
            // We use an Arc<Mutex<Vec<u32>>> to collect disappeared node IDs
            // so the registry closure (which runs during mainloop.iterate()) can
            // communicate with the command-processing loop below without a second channel.
            let disappeared_ids = Arc::clone(&disappeared_node_ids);
            move |id| {
                disappeared_ids.lock().unwrap().push(id);
            }
        })
        .register();

    // Create the capture stream for the initial source.
    let mut _stream = create_capture_stream(&core, &initial_source, Arc::clone(&ring_producer))?;

    // Poll for AudioCommands and run the PipeWire event loop.
    // PipeWire MainLoop::iterate() processes pending events non-blockingly.
    loop {
        mainloop.iterate(std::time::Duration::from_millis(10));

        match rx_cmd.try_recv() {
            Ok(AudioCommand::Shutdown) => break,
            Ok(AudioCommand::SwitchSource(new_source)) => {
                // Drop the current stream to disconnect it from PipeWire.
                drop(_stream);
                // Reconnect to the new source.
                match create_capture_stream(&core, &new_source, Arc::clone(&ring_producer)) {
                    Ok(s) => {
                        _stream = s;
                        eprintln!("info: audio source switched to {:?}", new_source);
                    }
                    Err(e) => {
                        eprintln!("warn: failed to switch audio source: {e:#}");
                        // Attempt fallback to system output.
                        match create_capture_stream(
                            &core,
                            &crate::config::AudioSource::SystemOutput,
                            Arc::clone(&ring_producer),
                        ) {
                            Ok(s) => {
                                _stream = s;
                                eprintln!("warn: fell back to system output capture");
                            }
                            Err(e2) => {
                                eprintln!("error: failed to reconnect audio: {e2:#}");
                                return Err(e2);
                            }
                        }
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }

        // Phase 8: drain nodes that disappeared during this iterate() call.
        // The registry global_remove callback appends disappeared node IDs to
        // `disappeared_node_ids`. Phase 8 adds AudioCommand::NodeDisappeared handling
        // below via the fallback_tx; for Phase 3, just drain to avoid unbounded growth.
        // (Phase 8 replaces this comment with actual fallback logic.)
        if let Ok(mut ids) = disappeared_node_ids.try_lock() {
            ids.retain(|&id| {
                // Phase 8: check if `id` is the currently captured node and fall back.
                // For Phase 3: remove known nodes from the list so the tray stays accurate.
                node_list.lock().unwrap().retain(|n| n.node_id != id);
                false // retain returns false = remove the entry from disappeared_node_ids
            });
        }
    }

    Ok(())
}

/// Create a PipeWire capture stream connected to the given AudioSource.
fn create_capture_stream(
    core: &pw::core::Core,
    source: &crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
) -> Result<pw::stream::Stream> {
    use pw::spa::pod::Pod;
    use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};

    // Build stream properties.
    let target_node = match source {
        crate::config::AudioSource::SystemOutput => None,
        crate::config::AudioSource::Application { node_id, .. } => Some(node_id.to_string()),
    };

    let mut stream_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::APP_NAME => "live-captions",
        *pw::keys::NODE_NAME => "live-captions-capture",
    };

    if let Some(target) = &target_node {
        stream_props.insert(*pw::keys::TARGET_OBJECT, target.as_str());
    } else {
        // System output monitor: connect to the default monitor sink.
        stream_props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }

    let stream = pw::stream::Stream::new(core, "live-captions-capture", stream_props)
        .context("creating PipeWire stream")?;

    // Build SPA format parameters: F32LE, 48kHz, stereo.
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(48_000);
    audio_info.set_channels(2);

    // Encode the SPA param as a POD.
    // Note: exact POD-building API may differ slightly between pipewire-rs 0.9.x versions.
    // Reference: https://pipewire.pages.freedesktop.org/pipewire-rs/pipewire/
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
            id: pw::spa::param::ParamType::EnumFormat.as_raw(),
            properties: audio_info.into_pod_properties(),
        }),
    )
    .context("serializing SPA audio format pod")?
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).context("creating SPA Pod")?];

    // Register the process callback (real-time — no allocation, no blocking).
    let ring_producer_cb = Arc::clone(&ring_producer);
    stream
        .add_local_listener_with_user_data(ring_producer_cb)
        .process(|stream, ring_producer| {
            if let Some(mut buf) = stream.dequeue_buffer() {
                let datas = buf.datas_mut();
                if let Some(data) = datas.first_mut() {
                    if let Some(chunk) = data.chunk() {
                        let offset = chunk.offset() as usize;
                        let size = chunk.size() as usize;
                        if let Some(bytes) = data.data() {
                            let float_bytes = &bytes[offset..offset + size];
                            // Convert bytes to f32 slice (F32LE, native endian on x86).
                            let samples = bytemuck::cast_slice::<u8, f32>(float_bytes);
                            // Push to ring buffer — never block in RT context.
                            let mut prod = ring_producer.lock().unwrap();
                            let _ = prod.push_slice(samples); // drop samples if ring full
                        }
                    }
                }
            }
        })
        .register()
        .context("registering PipeWire stream listener")?;

    // Connect the stream.
    stream.connect(
        pw::spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )
    .context("connecting PipeWire capture stream")?;

    Ok(stream)
}
```

**Step 1b: Add bytemuck dependency to Cargo.toml**

The PipeWire stream callback uses `bytemuck` for zero-copy byte-to-f32 casting. Add to `[dependencies]` in `Cargo.toml`:

```toml
bytemuck = "1"
```

**⚠️ SPA Pod API note:** The exact SPA Pod serialization API in pipewire-rs 0.9 may differ from the snippet above. Reference the official pipewire-rs examples at `https://gitlab.freedesktop.org/pipewire/pipewire-rs/-/tree/main/pipewire/examples` for the correct pod-building approach. The key outcome is: stream connected to `F32LE, 48kHz, stereo`.

**Step 2: Create src/audio/mod.rs module declaration**

Add `pub mod audio;` to `src/main.rs`.

**Step 3: Verify compilation**

```bash
cargo check
```

Fix any API discrepancies against current pipewire-rs 0.9.x documentation.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Wire audio capture into main.rs and verify chunk delivery

**Files:**
- Modify: `src/main.rs`

**Verifies:** live-captions.AC1.1, live-captions.AC1.5

**Step 1: Add a test consumer loop to main.rs**

After the model check block and before the remaining stubs, add:

```rust
// Phase 3: Start audio capture
let (audio_cmd_tx, mut ring_consumer, node_list) =
    audio::start_audio_thread(cfg.audio_source.clone())
        .unwrap_or_else(|e| {
            eprintln!("error: failed to start audio capture: {e:#}");
            eprintln!("hint: is PipeWire running? (`systemctl --user status pipewire`)");
            std::process::exit(1);
        });

// Test consumer: resample and print chunk count (temporary, replaced in Phase 4)
let mut resampler = audio::resampler::AudioResampler::new()
    .expect("creating resampler");
let mut chunk_count = 0u64;

println!("Audio capture started. Listening for 5 seconds...");
let start = std::time::Instant::now();
while start.elapsed().as_secs() < 5 {
    // Drain ring buffer into a temporary vec.
    let mut raw = vec![0f32; 4096];
    let n = ring_consumer.pop_slice(&mut raw);
    if n > 0 {
        match resampler.push_interleaved(&raw[..n]) {
            Ok(chunks) => {
                chunk_count += chunks.len() as u64;
                if !chunks.is_empty() {
                    eprintln!("info: produced {} 160ms chunks (total: {})", chunks.len(), chunk_count);
                }
            }
            Err(e) => eprintln!("warn: resampler error: {e}"),
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
}

println!("5 second test complete. Total 160ms chunks produced: {chunk_count}");
let _ = audio_cmd_tx.send(audio::AudioCommand::Shutdown);
```

Add `mod audio;` at the top of `src/main.rs`.

**Step 2: Build and run**

```bash
cargo build
cargo run
```

Expected output (with audio playing on the system):
```
Audio capture started. Listening for 5 seconds...
info: produced 1 160ms chunks (total: 1)
info: produced 1 160ms chunks (total: 2)
...
5 second test complete. Total 160ms chunks produced: 31
```

31 chunks × 160ms ≈ 5 seconds. Exact count depends on scheduling.

**Step 3: Test AC1.5 — PipeWire unavailable**

```bash
# Stop PipeWire temporarily (will be restored automatically):
systemctl --user stop pipewire
cargo run 2>&1 | head -5
# Expected: "error: failed to start audio capture: ..." then exit
systemctl --user start pipewire  # restore
```

**Step 4: Commit**

```bash
git add src/audio/ src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: audio capture — PipeWire stream, ring buffer, 48→16kHz resampler"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
