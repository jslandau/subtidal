# Live Captions Implementation Plan — Phase 4: STT Engine Layer

**Goal:** Define the `SttEngine` trait and implement both backends (Parakeet EOU, Moonshine). Wire the inference thread to consume 16kHz mono audio chunks from Phase 3 and emit caption strings to the GTK4 main thread.

**Architecture:** `src/stt/mod.rs` defines the `SttEngine` trait and the inference thread. `src/stt/parakeet.rs` wraps `parakeet_rs::ParakeetEOU`. `src/stt/moonshine.rs` wraps two `ort::Session` instances (encoder + decoder) with energy-based VAD. The inference thread receives 160ms audio chunks via a `std::sync::mpsc::Receiver<Vec<f32>>` and sends caption strings via a `glib::MainContext` channel (connected in Phase 5; in Phase 4 we use a plain mpsc for testing).

**Tech Stack:** parakeet-rs 0.3, ort 2.0.0-rc.11, glib (via gtk4).

**Scope:** Phase 4 of 8. Depends on Phases 2 (model files) and 3 (audio chunks).

**Codebase verified:** 2026-02-22 — greenfield, no src/stt/ exists.

---

## Acceptance Criteria Coverage

### live-captions.AC2: Live captions are produced with acceptable latency
- **live-captions.AC2.1 Success:** Spoken English produces caption text within 300ms of utterance end (Parakeet engine)
- **live-captions.AC2.2 Success:** Spoken English produces caption text within 400ms of utterance end (Moonshine engine)
- **live-captions.AC2.3 Success:** Captions update continuously during sustained speech without long gaps
- **live-captions.AC2.4 Failure:** Silence produces no spurious caption output

### live-captions.AC5: STT engine management
- **live-captions.AC5.3 Success:** If CUDA is unavailable, app automatically falls back to Moonshine (CPU) with a tray tooltip warning

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Define the SttEngine trait and inference thread in src/stt/mod.rs

**Files:**
- Create: `src/stt/mod.rs`
- Create: `src/stt/` directory

**Step 1: Create src/stt/mod.rs**

```rust
//! STT engine abstraction and inference thread management.

pub mod parakeet;
pub mod moonshine;

use anyhow::Result;
use std::sync::mpsc;
use std::thread;

/// Trait implemented by all STT backends.
///
/// Both methods are called from the inference thread. Implementors must be `Send + 'static`.
pub trait SttEngine: Send + 'static {
    /// The sample rate this engine expects. Both engines return 16000.
    fn sample_rate(&self) -> u32;

    /// Process one 160ms chunk of 16kHz mono PCM.
    ///
    /// Returns `Ok(Some(text))` when a complete utterance has been recognized,
    /// `Ok(None)` when more audio is needed, or an error if inference failed
    /// (caller should log and skip the chunk).
    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>;
}

/// Spawn the inference thread.
///
/// Parameters:
/// - `engine`: boxed SttEngine (Parakeet or Moonshine)
/// - `audio_rx`: receives 160ms chunks from the audio processing thread
/// - `caption_tx`: sends recognized text to the GTK4 main thread
///
/// Returns the thread JoinHandle for clean shutdown.
pub fn spawn_inference_thread(
    mut engine: Box<dyn SttEngine>,
    audio_rx: mpsc::Receiver<Vec<f32>>,
    caption_tx: mpsc::SyncSender<String>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("stt-inference".to_string())
        .spawn(move || {
            for chunk in audio_rx.iter() {
                match engine.process_chunk(&chunk) {
                    Ok(Some(text)) if !text.trim().is_empty() => {
                        if caption_tx.send(text).is_err() {
                            break; // receiver dropped — shutdown
                        }
                    }
                    Ok(Some(_)) | Ok(None) => {} // no output yet
                    Err(e) => {
                        eprintln!("warn: inference error (skipping chunk): {e}");
                    }
                }
            }
        })
        .expect("spawning inference thread")
}

/// Detect CUDA availability via ort.
/// Returns true if a CUDA-capable GPU is accessible.
pub fn cuda_available() -> bool {
    ort::execution_providers::CUDAExecutionProvider::default().is_available().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct MockEngine {
        responses: Vec<Option<String>>,
        call_index: usize,
    }

    impl SttEngine for MockEngine {
        fn sample_rate(&self) -> u32 { 16_000 }
        fn process_chunk(&mut self, _pcm: &[f32]) -> Result<Option<String>> {
            let resp = self.responses.get(self.call_index).cloned().flatten();
            self.call_index += 1;
            Ok(resp)
        }
    }

    #[test]
    fn inference_thread_forwards_recognized_text() {
        let engine = Box::new(MockEngine {
            responses: vec![Some("hello world".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap();
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["hello world"]);
    }

    #[test]
    fn inference_thread_suppresses_none_responses() {
        let engine = Box::new(MockEngine {
            responses: vec![None, Some("world".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // None
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // Some("world")
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["world"]);
    }

    #[test]
    fn inference_thread_suppresses_whitespace_only_text() {
        let engine = Box::new(MockEngine {
            responses: vec![Some("   ".to_string()), Some("hi".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // whitespace only
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // "hi"
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["hi"]);
    }
}
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement src/stt/parakeet.rs — ParakeetEOU wrapper

**Files:**
- Create: `src/stt/parakeet.rs`

**Verifies:** live-captions.AC2.1, live-captions.AC2.3

**Step 1: Create src/stt/parakeet.rs**

```rust
//! Parakeet EOU STT engine: wraps parakeet_rs::ParakeetEOU.

use anyhow::{Context, Result};
use std::path::Path;
use super::SttEngine;

pub struct ParakeetEngine {
    inner: parakeet_rs::ParakeetEOU,
}

impl ParakeetEngine {
    /// Load the Parakeet EOU model from the given directory.
    /// Directory must contain: encoder.onnx, decoder_joint.onnx, tokenizer.json
    /// (downloaded in Phase 2 to ~/.local/share/live-captions/models/parakeet/).
    pub fn new(model_dir: &Path) -> Result<Self> {
        // Build ort execution config with CUDA (falls through to CPU if CUDA unavailable).
        let exec_config = parakeet_rs::ExecutionConfig {
            provider: parakeet_rs::ExecutionProvider::Cuda,
            // Other fields use defaults — consult parakeet_rs::ExecutionConfig docs
            // if additional configuration is needed.
            ..Default::default()
        };

        let inner = parakeet_rs::ParakeetEOU::from_pretrained(model_dir, Some(exec_config))
            .with_context(|| format!("loading ParakeetEOU from {}", model_dir.display()))?;

        Ok(ParakeetEngine { inner })
    }
}

impl SttEngine for ParakeetEngine {
    fn sample_rate(&self) -> u32 {
        16_000
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>> {
        // Feed 160ms chunk (2560 samples at 16kHz).
        // reset_on_eou=true: decoder state resets after each complete utterance.
        let text = self.inner.transcribe(pcm, true)
            .context("ParakeetEOU transcribe")?;

        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }
}
```

**⚠️ parakeet_rs::ExecutionConfig note:** The exact fields of `ExecutionConfig` depend on the parakeet-rs 0.3.3 API. Run `cargo doc --open` to check the struct definition. If `ExecutionConfig` doesn't have a `provider` field, use the `ExecutionProvider` enum directly per the crate's documentation.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Implement src/stt/moonshine.rs — MoonshineEngine with VAD

**Files:**
- Create: `src/stt/moonshine.rs`

**Verifies:** live-captions.AC2.2, live-captions.AC2.3, live-captions.AC2.4

The Moonshine model is an encoder-decoder transformer. Inference requires:
1. **VAD** (energy-based): Accumulate audio while energy > threshold; emit silence detection after a quiet period.
2. **Encoder**: Convert accumulated speech audio into hidden states.
3. **Decoder**: Autoregressively generate tokens from the encoder output.

The ONNX files from Phase 2 are:
- `~/.local/share/live-captions/models/moonshine/encoder_model_quantized.onnx`
- `~/.local/share/live-captions/models/moonshine/decoder_model_merged_quantized.onnx`

**Step 1: Create src/stt/moonshine.rs**

```rust
//! Moonshine STT engine: ort ONNX inference with energy VAD.

use anyhow::{Context, Result};
use ort::{session::Session, execution_providers::CPUExecutionProvider};
use std::path::Path;
use super::SttEngine;

/// Energy VAD threshold: RMS below this level is treated as silence.
const VAD_RMS_THRESHOLD: f32 = 0.002;
/// Number of consecutive silent 160ms chunks before triggering inference.
const SILENCE_CHUNKS_BEFORE_INFER: usize = 5; // 5 × 160ms = 800ms silence

pub struct MoonshineEngine {
    encoder: Session,
    decoder: Session,
    /// Accumulated speech audio (16kHz mono) pending inference.
    speech_buf: Vec<f32>,
    /// Count of consecutive silent chunks since last speech.
    silent_chunks: usize,
    /// True if we are currently accumulating speech.
    in_speech: bool,
}

impl MoonshineEngine {
    /// Load Moonshine ONNX models from the given directory.
    /// Directory must contain: encoder_model_quantized.onnx, decoder_model_merged_quantized.onnx
    pub fn new(model_dir: &Path) -> Result<Self> {
        let encoder_path = model_dir.join("encoder_model_quantized.onnx");
        let decoder_path = model_dir.join("decoder_model_merged_quantized.onnx");

        let encoder = Session::builder()
            .context("ort session builder (encoder)")?
            .with_execution_providers([CPUExecutionProvider::default().build()])
            .context("setting CPU EP (encoder)")?
            .commit_from_file(&encoder_path)
            .with_context(|| format!("loading encoder from {}", encoder_path.display()))?;

        let decoder = Session::builder()
            .context("ort session builder (decoder)")?
            .with_execution_providers([CPUExecutionProvider::default().build()])
            .context("setting CPU EP (decoder)")?
            .commit_from_file(&decoder_path)
            .with_context(|| format!("loading decoder from {}", decoder_path.display()))?;

        Ok(MoonshineEngine {
            encoder,
            decoder,
            speech_buf: Vec::new(),
            silent_chunks: 0,
            in_speech: false,
        })
    }

    /// Compute RMS energy of a mono audio chunk.
    fn rms(chunk: &[f32]) -> f32 {
        if chunk.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
        (sum_sq / chunk.len() as f32).sqrt()
    }

    /// Run encoder + decoder on the accumulated speech buffer.
    /// Returns the recognized text string.
    fn run_inference(&mut self) -> Result<String> {
        if self.speech_buf.is_empty() {
            return Ok(String::new());
        }

        // --- Encoder ---
        // Input: audio waveform float32 [1, num_samples]
        // Output: encoder hidden states — shape depends on model; check via:
        //   encoder.inputs[0].name, encoder.outputs[0].name
        let num_samples = self.speech_buf.len();
        let audio_input = ndarray::Array2::<f32>::from_shape_vec(
            (1, num_samples),
            self.speech_buf.clone(),
        )
        .context("building encoder input array")?;

        let encoder_outputs = self.encoder.run(
            ort::inputs![audio_input.view()]
                .context("building encoder inputs")?,
        )
        .context("running Moonshine encoder")?;

        // Extract encoder hidden states (output index 0).
        let hidden_states = encoder_outputs[0]
            .try_extract_array::<f32>()
            .context("extracting encoder hidden states")?;

        // --- Decoder (autoregressive greedy decoding) ---
        // The merged decoder ONNX handles both first-pass and key-value cache.
        // Input names: "encoder_hidden_states", "input_ids", (kv-cache on subsequent steps)
        // Output names: "logits", (updated kv-cache)
        //
        // ⚠️ Exact tensor names depend on onnx-community/moonshine-tiny-ONNX export.
        // Check model input/output names via:
        //   for input in &self.decoder.inputs { println!("{}", input.name); }
        //
        // The implementation below is a simplified greedy decoder sketch.
        // Full decoder loop: run until EOS token (model-specific ID) or max tokens.

        let max_tokens = 200usize;
        let eos_token_id = 2i64; // typical EOS id — verify from model tokenizer
        let bos_token_id = 1i64; // typical BOS id — verify from model tokenizer

        let mut input_ids = vec![bos_token_id];
        let mut output_tokens: Vec<i64> = Vec::new();

        for _step in 0..max_tokens {
            let ids_array = ndarray::Array2::<i64>::from_shape_vec(
                (1, input_ids.len()),
                input_ids.clone(),
            )
            .context("building decoder input_ids")?;

            let decoder_outputs = self.decoder.run(
                ort::inputs![
                    "encoder_hidden_states" => hidden_states.view(),
                    "input_ids" => ids_array.view()
                ]
                .context("building decoder inputs")?,
            )
            .context("running Moonshine decoder step")?;

            // Logits shape: [1, seq_len, vocab_size] — take last token logits.
            let logits = decoder_outputs[0]
                .try_extract_array::<f32>()
                .context("extracting decoder logits")?;

            let seq_len = logits.shape()[1];
            let vocab_size = logits.shape()[2];
            let last_logits = logits.slice(ndarray::s![0, seq_len - 1, ..]);
            let next_token = last_logits
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                .map(|(i, _)| i as i64)
                .unwrap_or(eos_token_id);

            if next_token == eos_token_id {
                break;
            }
            output_tokens.push(next_token);
            input_ids.push(next_token);
        }

        // ⚠️ KNOWN INTERMEDIATE STATE: Moonshine output is numeric token IDs (not text)
        // until Phase 8 Task 1 integrates the tokenizer. The tokenizer.json file is already
        // downloaded by Phase 2, but loading it into MoonshineEngine happens in Phase 8.
        // Between Phases 4–7, Moonshine captions will display as e.g. "12 45 67 89".
        // This is expected and does NOT indicate a bug.

        let decoded = output_tokens
            .iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        eprintln!("debug: Moonshine raw token IDs: {decoded}");
        eprintln!("warn: Moonshine tokenizer not yet integrated — output is token IDs until Phase 8");

        Ok(decoded)
    }
}

impl SttEngine for MoonshineEngine {
    fn sample_rate(&self) -> u32 {
        16_000
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>> {
        let energy = Self::rms(pcm);
        let is_speech = energy > VAD_RMS_THRESHOLD;

        if is_speech {
            self.in_speech = true;
            self.silent_chunks = 0;
            self.speech_buf.extend_from_slice(pcm);
            return Ok(None); // accumulating
        }

        // Silent chunk.
        if self.in_speech {
            self.silent_chunks += 1;
            // Keep buffering short silences (might be mid-utterance pause).
            self.speech_buf.extend_from_slice(pcm);

            if self.silent_chunks >= SILENCE_CHUNKS_BEFORE_INFER {
                // Enough silence: run inference on accumulated speech.
                self.in_speech = false;
                self.silent_chunks = 0;
                let text = self.run_inference()?;
                self.speech_buf.clear();
                return Ok(Some(text));
            }
        }

        Ok(None) // silence, not in speech
    }
}
```

**Dependencies to add to Cargo.toml:**

```toml
ndarray = "0.16"
tokenizers = { version = "0.20", features = [] }
```

**⚠️ Moonshine decoder tensor names:** The decoder input/output tensor names depend on how `onnx-community/moonshine-tiny-ONNX` exported the model. Print `self.decoder.inputs` and `self.decoder.outputs` at startup to discover exact names, then adjust the `ort::inputs![]` call accordingly.

**⚠️ Known limitation — no KV-cache:** The decoder loop above re-feeds the full growing `input_ids` sequence on every step (no past key-value caching). This is O(n²) in compute as the output sequence grows. For the Moonshine tiny model with max 200 tokens, this is acceptable for a first implementation. If latency is unacceptable in testing:

1. Inspect decoder output names — the merged ONNX exports KV-cache tensors alongside logits (typically named `present.N.key`, `present.N.value` for N attention layers).
2. On each decoder step after the first, pass the previous step's KV-cache outputs as `past_key_values.N.key` / `past_key_values.N.value` inputs, and only feed the single new token as `input_ids`.
3. This reduces each decoder step to O(1) attention rather than O(n).

This optimization can be added as a follow-up task in Phase 8 or as a standalone Phase 9 if performance testing reveals it is needed.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Wire STT inference into main.rs and test caption output

**Files:**
- Modify: `src/main.rs`

**Verifies:** live-captions.AC2.1, live-captions.AC2.2, live-captions.AC2.3, live-captions.AC2.4, live-captions.AC5.3

**Step 1: Add mod stt to src/main.rs**

At the top: `mod stt;`

**Step 2: Add CUDA detection and engine selection logic**

Replace the inference-related stubs in `src/main.rs` with:

```rust
// Phase 4: Determine active engine (with CUDA fallback).
let active_engine = cfg.engine.clone();
let (active_engine, cuda_fallback_warning) = match active_engine {
    config::Engine::Parakeet => {
        if stt::cuda_available() {
            (config::Engine::Parakeet, None)
        } else {
            eprintln!("warn: CUDA not available, falling back to Moonshine (CPU)");
            (config::Engine::Moonshine, Some("CUDA unavailable — using Moonshine (CPU)"))
        }
    }
    config::Engine::Moonshine => (config::Engine::Moonshine, None),
};

// Create audio chunk channel (connects Phase 3 ring buffer drain to inference).
// Wrap the SyncSender in Arc<Mutex<>> so Phase 8 engine switching can replace it
// at runtime without restarting the bridge thread.
let (chunk_tx_inner, chunk_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(32);
let chunk_tx = Arc::new(std::sync::Mutex::new(chunk_tx_inner));
let (caption_tx, caption_rx) = std::sync::mpsc::sync_channel::<String>(64);

// Spawn the audio→chunk bridge thread.
// Drains the ring buffer, resamples, and sends 160ms chunks to the inference thread.
// Locks chunk_tx on each send so Phase 8 can atomically swap the inner SyncSender.
let mut ring_consumer_arc = ring_consumer; // from Phase 3 start_audio_thread
let chunk_tx_for_bridge = Arc::clone(&chunk_tx);
std::thread::spawn(move || {
    let mut resampler = audio::resampler::AudioResampler::new()
        .expect("creating resampler");
    let mut raw = vec![0f32; 4096];
    loop {
        let n = ring_consumer_arc.pop_slice(&mut raw);
        if n > 0 {
            if let Ok(chunks) = resampler.push_interleaved(&raw[..n]) {
                for chunk in chunks {
                    let tx = chunk_tx_for_bridge.lock().unwrap();
                    if tx.send(chunk).is_err() {
                        drop(tx); // release lock before sleep
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        break; // engine switching — wait for new tx
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
});

// Instantiate the active STT engine.
let engine: Box<dyn stt::SttEngine> = match active_engine {
    config::Engine::Parakeet => {
        let model_dir = models::parakeet_model_dir();
        Box::new(
            stt::parakeet::ParakeetEngine::new(&model_dir)
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to load Parakeet model: {e:#}");
                    std::process::exit(1);
                })
        )
    }
    config::Engine::Moonshine => {
        let model_dir = models::moonshine_model_dir();
        Box::new(
            stt::moonshine::MoonshineEngine::new(&model_dir)
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to load Moonshine model: {e:#}");
                    std::process::exit(1);
                })
        )
    }
};

// Spawn the inference thread.
let _inference_handle = stt::spawn_inference_thread(engine, chunk_rx, caption_tx);

// Test consumer: print captions to stdout (replaced by GTK overlay in Phase 5).
std::thread::spawn(move || {
    for caption in caption_rx.iter() {
        println!("[CAPTION] {caption}");
    }
});
```

**Step 3: Build**

```bash
cargo build
```

**Step 4: Test Parakeet caption output (requires CUDA)**

```bash
# Play audio on the system (music, speech, etc.) then:
cargo run
# Speak into your microphone or play a speech audio file.
# Expected: "[CAPTION] Hello world" printed within ~300ms of utterance end.
```

**Step 5: Test Moonshine fallback (AC5.3)**

```bash
# Temporarily rename libcudart to simulate unavailability:
# (or simply set ort to CPU only)
cargo run -- --engine moonshine
# Expected: Moonshine loads, captions appear (with token IDs until tokenizer is added in Phase 8).
```

**Step 6: Test silence (AC2.4)**

Play silence for 10 seconds. Expected: no `[CAPTION]` lines printed.

**Step 7: Commit**

```bash
git add src/stt/ src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: STT engine layer — Parakeet EOU and Moonshine ONNX backends"
```
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->
