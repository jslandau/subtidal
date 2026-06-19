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

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos_webgpu_smoke is macOS-only");
}

#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::time::Instant;

#[cfg(target_os = "macos")]
use anyhow::{Context, Result};
#[cfg(target_os = "macos")]
use hound::SampleFormat;
#[cfg(target_os = "macos")]
use parakeet_rs::{ExecutionConfig, ExecutionProvider, Nemotron};
#[cfg(target_os = "macos")]
use subtidal::models::{ensure_nemotron_models, nemotron_model_dir};

const FIXTURE: &str = "tests/fixtures/macos-webgpu-smoke.wav";

// Nemotron expects 560ms chunks at 16kHz mono = 8960 samples (mirrors the
// constant in src/stt/nemotron.rs).
#[cfg(target_os = "macos")]
const CHUNK_SAMPLES: usize = 8960;

#[cfg(target_os = "macos")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // 1. Ensure model weights are present (download via hf-hub if missing).
    ensure_nemotron_models()
        .await
        .context("downloading Nemotron model weights")?;
    let model_dir: PathBuf = nemotron_model_dir();

    // 2. Build the WebGPU-backed Nemotron engine directly via parakeet-rs.
    //    Bypasses src/stt/nemotron.rs::NemotronEngine::new (still bool-cuda-only);
    //    that production surface is widened in Phase 3.
    let exec = ExecutionConfig::new().with_execution_provider(ExecutionProvider::WebGPU);
    let t_init = Instant::now();
    let mut engine = Nemotron::from_pretrained(&model_dir, Some(exec))
        .context("constructing Nemotron with WebGPU execution provider")?;
    eprintln!(
        "[init] WebGPU engine ready in {:.2}s",
        t_init.elapsed().as_secs_f32()
    );

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
        let text = engine
            .transcribe_chunk(slice)
            .context("Nemotron transcribe_chunk")?;
        if !text.is_empty() {
            transcript.push_str(&text);
        }
    }
    let infer_secs = t_infer.elapsed().as_secs_f32();
    let rtf = infer_secs / audio_secs;

    // 5. Print results — operator eyeballs correctness against fixture README.
    println!("transcription: {}", transcript.trim());
    println!("audio_secs:    {:.3}", audio_secs);
    println!("infer_secs:    {:.3}", infer_secs);
    println!(
        "rtf:           {:.3}  (must be <= 1.0 to satisfy macos-port.AC4.4)",
        rtf
    );

    if rtf > 1.0 {
        eprintln!("WARN: real-time factor exceeds 1.0 — design Plan B may need to activate");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
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
