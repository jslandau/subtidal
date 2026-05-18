# macOS Port — Phase 0: WebGPU spike

**Goal:** Empirically verify that `parakeet_rs` with the `webgpu` feature loads the Nemotron model and runs at real-time factor ≤ 1.0 on Apple Silicon Metal before committing to the rest of the port.

**Architecture:** Throwaway `examples/macos_webgpu_smoke.rs` binary, gated `#![cfg(target_os = "macos")]`, that bypasses the production `Nemotron::new(model_dir, use_cuda: bool)` wrapper and calls `parakeet_rs::Nemotron` directly with `ExecutionConfig::with_execution_provider(ExecutionProvider::WebGPU)`. Phase 3 later widens the production wrapper.

**Tech Stack:** Rust 2021 edition, `parakeet-rs` 0.3.4 (`webgpu` feature), `ort` 2.0.0-rc.12 (`webgpu` feature, backs WebGPU on Metal via wgpu), `hound` 3.5 (WAV decoding), `tokio` (already in workspace, for `ensure_nemotron_models()`).

**Scope:** Phase 0 of 8 (phases 0–7 of the macOS port design).

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

This phase implements and verifies (operational verification only — Phase 0 is a spike, not test-driven):

### macos-port.AC4: STT engine on macOS (WebGPU primary, CPU fallback)
- **macos-port.AC4.4 Success:** Real-time factor on the WebGPU path measured on the Phase 0 spike is ≤1.0 on the testing M-series machine.

AC4.1, AC4.2, AC4.3, AC4.5 (engine selection, CPU fallback, accuracy parity, ArcSwap safety) are formally implemented and tested in Phase 3. AC4.4 is uniquely satisfiable here because the spike is the only place we measure raw WebGPU RTF in isolation, free of audio capture and overlay overhead.

---

## Implementation Tasks

<!-- START_TASK_1 -->
### Task 1: Add macOS-conditional dependency block to Cargo.toml

**Files:**
- Modify: `Cargo.toml` (insert a new block immediately after the existing `[target.'cfg(target_os = "linux")'.dependencies]` block ending at line 86)

**Implementation:**

Append the macOS target-conditional dependency block after the Linux block. This intentionally scopes Phase 0 narrowly to the inference deps; the `objc2-*` crates and `dispatch` are added in Phase 1.

```toml
# Resolver v2 isolates the `webgpu` feature on `ort` and `parakeet-rs` to
# macOS-target compilations only; this mirrors the Linux/cuda pattern above.
# objc2-* crates and dispatch are added in Phase 1 (skeleton wiring); this
# Phase 0 block intentionally adds ONLY the inference dependencies needed
# for the WebGPU spike, so Phase 0 stays narrowly scoped.
[target.'cfg(target_os = "macos")'.dependencies]
parakeet-rs = { version = "0.3.4", features = ["webgpu"] }
ort = { version = "2.0.0-rc.12", features = ["webgpu"] }
hound = "3.5"  # WAV reader for the spike fixture; tiny pure-Rust crate
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin --verbose
cargo check --lib
```
Expected: both succeed without errors.

**Commit:** `macos: add target-conditional parakeet-rs/ort webgpu deps`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add test fixture WAV

**Files:**
- Create: `tests/fixtures/macos-webgpu-smoke.wav`
- Create: `tests/fixtures/README.md`

**Implementation:**

Create the fixtures directory and add a short (≤10s) clear English speech sample at 16 kHz mono PCM s16le — the format Nemotron consumes after Subtidal's resampler. Source from a public-domain corpus (LibriSpeech `dev-clean` is the standard pick; LibriVox also works). Convert with ffmpeg if needed:

```bash
mkdir -p tests/fixtures
ffmpeg -i source.wav -ar 16000 -ac 1 -sample_fmt s16 tests/fixtures/macos-webgpu-smoke.wav
```

Target characteristics:
- Format: WAV, PCM s16le, 16000 Hz, mono
- Duration: 3–10 seconds
- Content: clear English speech, ideally a short sentence with no proper nouns so transcription is stable
- Size: well under 1 MB

Document provenance in `tests/fixtures/README.md`:

```markdown
# Test fixtures

## macos-webgpu-smoke.wav

Short English speech clip used by `examples/macos_webgpu_smoke.rs` (Phase 0 of
the macOS port) to verify that `parakeet_rs::ExecutionProvider::WebGPU` runs
on Apple Silicon Metal and produces a reasonable transcription.

- Source: [fill in: e.g., LibriSpeech dev-clean utterance <id>, CC BY 4.0]
- Format: 16 kHz mono PCM s16le, ~Ns
- Expected transcription (approximate): "<the spoken text>"

Used only for the spike; not exercised by `cargo test`.
```

**Verification:**

```bash
file tests/fixtures/macos-webgpu-smoke.wav
```
Expected: `RIFF ... WAVE audio, Microsoft PCM, 16 bit, mono 16000 Hz`.

**Commit:** `macos: add Phase 0 webgpu spike audio fixture`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Write the macOS WebGPU smoke example

**Files:**
- Create: `examples/macos_webgpu_smoke.rs`

**Implementation:**

Create the example, gated `#![cfg(target_os = "macos")]` so it's a no-op on non-macOS targets. It mirrors the prod loop (560 ms / 8960-sample chunks at 16 kHz) but skips the audio capture, ring buffer, and overlay — just decode a WAV with `hound`, stream chunks through `parakeet_rs::Nemotron`, print transcription + RTF.

```rust
// examples/macos_webgpu_smoke.rs
//
// Phase 0 WebGPU spike: empirically verify that parakeet-rs with the `webgpu`
// feature (backed by ONNX Runtime on Metal via wgpu) loads the Nemotron model,
// transcribes a short fixture WAV, and runs at real-time factor <= 1.0 on
// Apple Silicon. Throwaway code — superseded by Phase 3's production wiring.
//
// Run (on Apple Silicon):
//   cargo run --release --example macos_webgpu_smoke
//
// Compiles on macOS targets only; on other platforms the file becomes empty.

#![cfg(target_os = "macos")]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use hound::SampleFormat;
use parakeet_rs::{ExecutionConfig, ExecutionProvider, Nemotron};
use subtidal::models::{ensure_nemotron_models, nemotron_model_dir};

const FIXTURE: &str = "tests/fixtures/macos-webgpu-smoke.wav";

// Nemotron expects 560ms chunks at 16kHz mono = 8960 samples (mirrors the
// constant in src/stt/nemotron.rs).
const CHUNK_SAMPLES: usize = 8960;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // 1. Ensure model weights are present (download via hf-hub if missing).
    ensure_nemotron_models()
        .await
        .context("downloading Nemotron model weights")?;
    let model_dir: PathBuf = nemotron_model_dir();

    // 2. Build the WebGPU-backed Nemotron engine directly via parakeet-rs.
    //    Bypasses src/stt/nemotron.rs::Nemotron::new (still bool-cuda-only);
    //    that production surface is widened in Phase 3.
    let exec = ExecutionConfig::new().with_execution_provider(ExecutionProvider::WebGPU);
    let t_init = Instant::now();
    let mut engine = Nemotron::from_pretrained_with_config(&model_dir, exec)
        .context("constructing Nemotron with WebGPU execution provider")?;
    eprintln!("[init] WebGPU engine ready in {:.2}s", t_init.elapsed().as_secs_f32());

    // 3. Decode the fixture WAV into mono f32 PCM at 16kHz.
    let pcm = read_wav_16k_mono_f32(FIXTURE)?;
    let audio_secs = pcm.len() as f32 / 16_000.0;
    eprintln!("[fixture] {} samples, {:.2}s", pcm.len(), audio_secs);

    // 4. Stream the PCM into Nemotron in 560ms chunks, mirroring the prod loop.
    let t_infer = Instant::now();
    let mut transcript = String::new();
    for chunk in pcm.chunks(CHUNK_SAMPLES) {
        // Pad the final chunk with silence so Nemotron always sees its full
        // expected window; prod code does the same via the ring buffer.
        let mut buf;
        let slice: &[f32] = if chunk.len() == CHUNK_SAMPLES {
            chunk
        } else {
            buf = vec![0.0f32; CHUNK_SAMPLES];
            buf[..chunk.len()].copy_from_slice(chunk);
            &buf
        };
        if let Some(text) = engine.transcribe_chunk(slice)? {
            transcript.push_str(&text);
        }
    }
    let infer_secs = t_infer.elapsed().as_secs_f32();
    let rtf = infer_secs / audio_secs;

    // 5. Print results — operator eyeballs correctness against fixture README.
    println!("transcription: {}", transcript.trim());
    println!("audio_secs:    {:.3}", audio_secs);
    println!("infer_secs:    {:.3}", infer_secs);
    println!("rtf:           {:.3}  (must be <= 1.0 to satisfy macos-port.AC4.4)", rtf);

    if rtf > 1.0 {
        eprintln!("WARN: real-time factor exceeds 1.0 — design Plan B may need to activate");
    }
    Ok(())
}

fn read_wav_16k_mono_f32(path: &str) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("opening {path}"))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == 16_000 && spec.channels == 1,
        "fixture must be 16kHz mono; got {} Hz, {} ch",
        spec.sample_rate,
        spec.channels
    );
    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
            .collect::<std::result::Result<_, _>>()?,
        SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()?,
    };
    Ok(samples)
}
```

**Note on `Nemotron::from_pretrained_with_config`:** the exact constructor name on parakeet-rs 0.3.4 must be confirmed against `src/stt/nemotron.rs:21-39` in the live repo — the production code there builds an `ExecutionConfig` and passes it to `Nemotron`, so mirror that exact call shape (substituting `ExecutionProvider::WebGPU` for `Cuda`/`Cpu`). If the constructor name differs, adjust accordingly.

**Verification:**

Cross-target check from Linux host:
```bash
cargo check --lib --target x86_64-apple-darwin --verbose
cargo check --lib
```
Expected: both succeed (the example is cfg-gated and excluded from the Linux build).

**Commit:** `macos: add Phase 0 WebGPU smoke example`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Run the spike on Apple Silicon

**Files:** none (operational verification only)

**Implementation:**

On the target Apple Silicon Mac:

```bash
cargo run --release --example macos_webgpu_smoke
```

Expected output (shape, not exact text):
```
[init] WebGPU engine ready in <N>s
[fixture] <N> samples, <N.NN>s
transcription: <recognizable English approximating the fixture content>
audio_secs:    <X>
infer_secs:    <Y>
rtf:           <Z>  (must be <= 1.0 to satisfy macos-port.AC4.4)
```

**Pass criteria (macos-port.AC4.4):**

- Transcription is recognizable English matching the fixture (eyeball check vs. `tests/fixtures/README.md` expected text).
- Printed `rtf` is ≤ 1.0.

**If WebGPU init fails on Apple Silicon:**

Activate the design's documented Plan B (design doc §"Additional Considerations", "Plan B if WebGPU proves unworkable"):

- Stop. Do not proceed to Phase 1.
- Report the failure mode to the user (error text + ort/parakeet log lines).
- Per the design: a separate follow-up design plan for the whisper.cpp / CPU-primary path must be created before continuing the port.

**Verification:**

Confirm CI still passes:
```bash
cargo check --lib --target x86_64-apple-darwin
```
Expected: green on Linux host (unchanged from before Task 1).

**Commit:**

If the spike required adjustments (constructor names, feature flags, PCM handling) discovered on real hardware:
```bash
git add -- examples/macos_webgpu_smoke.rs Cargo.toml
git commit -m "macos: phase 0 spike adjustments from real-hardware run"
```
Otherwise no commit.
<!-- END_TASK_4 -->
