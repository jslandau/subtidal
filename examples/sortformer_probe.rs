// examples/sortformer_probe.rs
//
// Spike: measure whether NVIDIA Sortformer v2.1 streaming diarization can run
// alongside Nemotron in real time, on Linux+CUDA and macOS+WebGPU. Throwaway.
//
// Usage:
//   # Grab the pyannote-rs 6_speakers.wav fixture into
//   # tests/fixtures/diarization/. One-time.
//   cargo run --release --example sortformer_probe -- --download-fixtures
//
//   # Recommended "real" test: AMI Meeting Corpus headset-mix wavs (16 kHz
//   # mono Int16, 4 speakers per meeting, conversational, overlapping).
//   # https://groups.inf.ed.ac.uk/ami/corpus/ — files named like
//   # IB4001.Mix-Headset.wav. Trim to ~2 min for fast iteration.
//
//   # Input wav must be 16 kHz mono f32/i16 (use `ffmpeg -ac 1 -ar 16000`).
//   cargo run --release --example sortformer_probe -- <audio.wav>
//
// Reports:
//   - cold-load time for each model
//   - p50 / p95 / max wall time for Sortformer::feed() and Nemotron::transcribe_chunk()
//     over the audio, fed in 160 ms ticks (matches the live STT pipeline cadence)
//   - speaker segment list + concatenated transcript
//
// Memory delta is intentionally not measured in-process — run with
// `/usr/bin/time -l` (macOS) or `/usr/bin/time -v` (Linux) externally.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::env;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result};
use hound::SampleFormat;
use parakeet_rs::sortformer::{DiarizationConfig, Sortformer};
use parakeet_rs::{ExecutionConfig, ExecutionProvider, Nemotron};
use subtidal::models::{ensure_nemotron_models, ensure_sortformer_model, nemotron_model_dir};

const TICK_SAMPLES: usize = 2560; // 160 ms @ 16 kHz — same cadence as the live pipeline
const NEMOTRON_CHUNK: usize = 8960; // 560 ms @ 16 kHz

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--download-fixtures") {
        return download_fixtures();
    }
    let offline = args.iter().any(|a| a == "--offline");
    let cfg_name = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("callhome");
    let diar_config = match cfg_name {
        "callhome" => DiarizationConfig::callhome(),
        "dihard3" => DiarizationConfig::dihard3(),
        other => anyhow::bail!("unknown --config {other} (callhome|dihard3)"),
    };
    let wav_path = args
        .iter()
        .find(|a| !a.starts_with("--") && !["callhome", "dihard3"].contains(&a.as_str()))
        .context("usage: sortformer_probe [--config callhome|dihard3] [--offline] <audio.wav>")?
        .clone();
    eprintln!(
        "info: mode={} config={cfg_name}",
        if offline { "offline" } else { "streaming" }
    );

    eprintln!("info: ensuring models present...");
    ensure_nemotron_models().await?;
    let sortformer_path = ensure_sortformer_model().await?;

    let audio = load_wav_16k_mono(&wav_path)?;
    let duration_s = audio.len() as f32 / 16_000.0;
    eprintln!("info: loaded {duration_s:.2}s of audio from {wav_path}");

    // --- Load Sortformer ---------------------------------------------------
    let t0 = Instant::now();
    let mut sortformer =
        Sortformer::with_config(&sortformer_path, Some(default_provider()), diar_config)
            .context("loading Sortformer")?;
    let sortformer_load_ms = t0.elapsed().as_secs_f64() * 1e3;
    eprintln!(
        "info: sortformer cold-load: {sortformer_load_ms:.1} ms; chunk_len={}, right_context={}, latency={:.2}s",
        sortformer.chunk_len, sortformer.right_context, sortformer.latency()
    );

    // --- Load Nemotron -----------------------------------------------------
    let t0 = Instant::now();
    let mut nemotron = Nemotron::from_pretrained(nemotron_model_dir(), Some(default_provider()))
        .context("loading Nemotron")?;
    let nemotron_load_ms = t0.elapsed().as_secs_f64() * 1e3;
    eprintln!("info: nemotron cold-load: {nemotron_load_ms:.1} ms");

    // --- Stream both models in lock-step at 160 ms ticks -------------------
    let mut sortformer_times: Vec<f64> = Vec::new();
    let mut nemotron_times: Vec<f64> = Vec::new();
    let mut nem_buf: Vec<f32> = Vec::with_capacity(NEMOTRON_CHUNK);
    let mut all_segments: Vec<parakeet_rs::sortformer::SpeakerSegment> = Vec::new();
    let mut transcript = String::new();

    let wall_start = Instant::now();
    if offline {
        // Offline diarization: one shot over the whole buffer.
        let t = Instant::now();
        let segs = sortformer
            .diarize(audio.clone(), 16_000, 1)
            .context("Sortformer::diarize")?;
        sortformer_times.push(t.elapsed().as_secs_f64() * 1e3);
        all_segments.extend(segs);
    }
    for tick in audio.chunks(TICK_SAMPLES) {
        if !offline {
            let t = Instant::now();
            let segs = sortformer.feed(tick).context("Sortformer::feed")?;
            sortformer_times.push(t.elapsed().as_secs_f64() * 1e3);
            all_segments.extend(segs);
        }
        // Nemotron expects 560 ms chunks; accumulate ticks.
        nem_buf.extend_from_slice(tick);
        while nem_buf.len() >= NEMOTRON_CHUNK {
            let chunk: Vec<f32> = nem_buf.drain(..NEMOTRON_CHUNK).collect();
            let t = Instant::now();
            let text = nemotron
                .transcribe_chunk(&chunk)
                .context("Nemotron::transcribe_chunk")?;
            nemotron_times.push(t.elapsed().as_secs_f64() * 1e3);
            if !text.is_empty() {
                transcript.push_str(&text);
            }
        }
    }
    if !offline {
        let flush_segs = sortformer.flush().context("Sortformer::flush")?;
        all_segments.extend(flush_segs);
    }
    let wall_elapsed_s = wall_start.elapsed().as_secs_f64();

    // --- Report ------------------------------------------------------------
    println!(
        "\n=== Sortformer feed() (160 ms ticks, {} samples) ===",
        sortformer_times.len()
    );
    print_stats(&sortformer_times);
    println!(
        "\n=== Nemotron transcribe_chunk() (560 ms chunks, {} chunks) ===",
        nemotron_times.len()
    );
    print_stats(&nemotron_times);

    println!("\n=== Realtime factor ===");
    println!(
        "audio = {duration_s:.2}s, processed in {wall_elapsed_s:.2}s, RTF = {:.3}",
        wall_elapsed_s / duration_s as f64
    );

    let distinct: std::collections::BTreeSet<usize> =
        all_segments.iter().map(|s| s.speaker_id).collect();
    println!(
        "\n=== Speaker segments ({} segs, {} distinct speakers: {:?}) ===",
        all_segments.len(),
        distinct.len(),
        distinct
    );
    for seg in &all_segments {
        println!(
            "  [{:6.2}s - {:6.2}s] Speaker {}",
            seg.start as f64 / 16_000.0,
            seg.end as f64 / 16_000.0,
            seg.speaker_id
        );
    }

    println!("\n=== Transcript ===\n{transcript}");

    Ok(())
}

// Only 6_speakers.wav is published on pyannote-rs releases; the 2/4 variants
// don't exist. For a 2-speaker test, capture ~30 s through Subtidal's own audio
// path and re-encode to 16 kHz mono: `ffmpeg -i in.wav -ac 1 -ar 16000 out.wav`.
const FIXTURE_URLS: &[&str] =
    &["https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/6_speakers.wav"];

fn download_fixtures() -> Result<()> {
    let dest_dir = Path::new("tests/fixtures/diarization");
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;
    for url in FIXTURE_URLS {
        let name = url.rsplit('/').next().unwrap();
        let dest = dest_dir.join(name);
        if dest.exists() {
            eprintln!("info: already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {url} -> {}", dest.display());
        let status = Command::new("curl")
            .args(["-L", "--fail", "-o"])
            .arg(&dest)
            .arg(url)
            .status()
            .context("invoking curl (install curl, or download fixtures manually)")?;
        anyhow::ensure!(status.success(), "curl failed for {url}");
    }
    eprintln!("info: fixtures ready in {}", dest_dir.display());
    Ok(())
}

fn default_provider() -> ExecutionConfig {
    #[cfg(target_os = "linux")]
    let p = ExecutionProvider::Cuda;
    #[cfg(target_os = "macos")]
    let p = ExecutionProvider::WebGPU;
    ExecutionConfig::new().with_execution_provider(p)
}

fn load_wav_16k_mono(path: &str) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("opening {path}"))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.sample_rate == 16_000,
        "expected 16 kHz wav, got {} Hz — re-encode with `ffmpeg -ac 1 -ar 16000 -i in.wav out.wav`",
        spec.sample_rate
    );
    let mut samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| s.map(|s| s as f32 / 32768.0))
            .collect::<Result<_, _>>()?,
    };
    if spec.channels > 1 {
        samples = samples
            .chunks(spec.channels as usize)
            .map(|c| c.iter().sum::<f32>() / spec.channels as f32)
            .collect();
    }
    Ok(samples)
}

fn print_stats(times_ms: &[f64]) {
    if times_ms.is_empty() {
        println!("  (no samples)");
        return;
    }
    let mut sorted = times_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| sorted[((sorted.len() as f64 - 1.0) * q).round() as usize];
    let sum: f64 = sorted.iter().sum();
    println!(
        "  n={}  mean={:.2}ms  p50={:.2}ms  p95={:.2}ms  p99={:.2}ms  max={:.2}ms",
        sorted.len(),
        sum / sorted.len() as f64,
        p(0.50),
        p(0.95),
        p(0.99),
        sorted[sorted.len() - 1],
    );
}
