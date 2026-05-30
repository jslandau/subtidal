//! Sortformer-based streaming speaker diarization engine.
//!
//! Wraps `parakeet_rs::sortformer::Sortformer` to produce per-chunk dominant
//! speaker labels alongside the STT pipeline.
//!
//! # Architecture
//!
//! The STT pipeline feeds 160ms (2560-sample) chunks of 16kHz mono PCM to both
//! Nemotron and the Sortformer. The Sortformer uses `feed()` which accumulates
//! audio internally and produces segments when enough has been buffered for one
//! inference pass. With `chunk_len=31` (~2.5 seconds of audio per inference),
//! the first speaker label arrives after ~2.5 seconds of audio, then every
//! ~2.5 seconds thereafter.
//!
//! # Platform dispatch
//!
//! CUDA on Linux, WebGPU→CPU fallback on macOS — mirrors `NemotronEngine`.

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::DiarizationPreset;

/// Streaming constants for low-latency real-time diarization.
///
/// This is NVIDIA's shipped "low latency" preset for
/// `diar_streaming_sortformer_4spk-v2.1` (see the model card on HuggingFace):
/// chunk_len=6, right_context=7, fifo_len=188, spkcache_len=188.
/// Latency = (chunk_len + right_context) * 80ms = 13 * 80ms = 1.04s.
///
/// Per Park et al. 2025 (arxiv 2507.18446) Table 2, DER at this preset is
/// flat-to-better vs. the offline 10s preset on DIHARD III. The fifo/spkcache
/// values are NOT scaled down with chunk_len — they hold the model's
/// accumulated speaker context and shrinking them costs accuracy without
/// reducing latency.
const RT_CHUNK_LEN: usize = 6;
const RT_FIFO_LEN: usize = 188;
const RT_SPKCACHE_LEN: usize = 188;
const RT_RIGHT_CONTEXT: usize = 7;

/// A speaker segment within a chunk of audio.
#[derive(Debug, Clone)]
pub struct SpeakerSegment {
    /// Speaker index (0-based, max 3 for 4-speaker model).
    pub speaker_id: u32,
    /// Start sample offset within the chunk.
    pub start_sample: usize,
    /// End sample offset within the chunk.
    pub end_sample: usize,
}

/// Result of a diarization pass over one STT chunk boundary.
#[derive(Debug, Clone)]
pub struct DiarizationResult {
    /// The dominant speaker for this audio window, or None if no speech
    /// was detected (silence or below threshold).
    pub dominant_speaker: Option<u32>,
    /// All segments found in this window.
    pub segments: Vec<SpeakerSegment>,
}

/// Streaming speaker diarization engine backed by Sortformer v2/v2.1.
///
/// Stateful — preserves FIFO, speaker cache, and silence profile across
/// calls. Uses `feed()` which accumulates audio and produces segments when
/// enough has been buffered for one inference pass. With `chunk_len=31`,
/// first result arrives after ~2.5 seconds of audio, then every ~2.5 seconds.
pub struct DiarizationEngine {
    inner: parakeet_rs::sortformer::Sortformer,
}

impl DiarizationEngine {
    /// Load the Sortformer model from the given directory and configure it
    /// for real-time inference.
    ///
    /// Directory must contain: `diar_streaming_sortformer_4spk-v2.1.onnx`
    ///
    /// On Linux, `use_cuda` requests GPU acceleration via CUDA; CPU is used
    /// when `use_cuda` is false. On macOS, `use_cuda` requests WebGPU
    /// (backed by Metal); CPU fallback is automatic on WebGPU init failure.
    pub fn new(model_dir: &Path, use_cuda: bool, preset: &DiarizationPreset) -> Result<Self> {
        let model_path = model_dir.join("diar_streaming_sortformer_4spk-v2.1.onnx");
        if !model_path.exists() {
            anyhow::bail!(
                "Sortformer model not found at {}. Run with diarization enabled to trigger download.",
                model_path.display()
            );
        }

        let diar_config = match preset {
            DiarizationPreset::Callhome => parakeet_rs::sortformer::DiarizationConfig::callhome(),
            DiarizationPreset::Dihard3 => parakeet_rs::sortformer::DiarizationConfig::dihard3(),
            DiarizationPreset::Custom { onset, offset, min_seg_duration } => {
                let mut cfg = parakeet_rs::sortformer::DiarizationConfig::custom(*onset as f32, *offset as f32);
                cfg.min_duration_on = *min_seg_duration as f32;
                cfg
            }
        };

        #[cfg(target_os = "linux")]
        let mut inner = {
            let exec_config = parakeet_rs::ExecutionConfig::new()
                .with_execution_provider(if use_cuda {
                    parakeet_rs::ExecutionProvider::Cuda
                } else {
                    parakeet_rs::ExecutionProvider::Cpu
                });
            let provider = if use_cuda { "Cuda" } else { "Cpu" };
            eprintln!("info: Sortformer using execution provider: {provider}");
            parakeet_rs::sortformer::Sortformer::with_config(
                &model_path,
                Some(exec_config),
                diar_config,
            )
            .with_context(|| format!("loading Sortformer from {} (provider={provider})", model_path.display()))?
        };

        #[cfg(target_os = "macos")]
        let mut inner = build_macos_sortformer(&model_path, use_cuda, diar_config)?;

        // Override streaming constants for real-time latency.
        // Default (124/124/188/1) targets offline accuracy with ~10s latency.
        // Our (31/31/47/1) gives ~2.6s latency, balancing speed with accuracy.
        inner.chunk_len = RT_CHUNK_LEN;
        inner.fifo_len = RT_FIFO_LEN;
        inner.spkcache_len = RT_SPKCACHE_LEN;
        inner.right_context = RT_RIGHT_CONTEXT;
        // Reset state so the new streaming constants take full effect.
        inner.reset_state();

        eprintln!(
            "info: Sortformer real-time config: chunk_len={}, fifo_len={}, spkcache_len={}, right_context={}, latency≈{:.1}s",
            inner.chunk_len,
            inner.fifo_len,
            inner.spkcache_len,
            inner.right_context,
            inner.latency(),
        );

        Ok(DiarizationEngine { inner })
    }

    /// Feed a chunk of 16kHz mono f32 PCM and collect diarization results.
    ///
    /// Uses `feed()` which accumulates audio internally and produces segments
    /// only when a full inference window is ready. With `chunk_len=31`, the
    /// first result arrives after ~2.5s of audio, then every ~2.5s.
    ///
    /// The `dominant_speaker` field is the speaker with the most samples in
    /// the returned segments, or None if no segments were produced.
    pub fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<DiarizationResult>> {
        let parakeet_segments = self.inner.feed(pcm)
            .context("Sortformer feed error")?;

        if parakeet_segments.is_empty() {
            return Ok(None);
        }

        // Convert parakeet segments to our format and determine dominant speaker.
        let mut speaker_samples: [u64; 4] = [0; 4]; // max 4 speakers
        let segments: Vec<SpeakerSegment> = parakeet_segments
            .into_iter()
            .filter_map(|seg| {
                let speaker_id = seg.speaker_id as u32;
                let start_sample = seg.start as usize;
                let end_sample = seg.end as usize;
                let duration = end_sample.saturating_sub(start_sample);
                if duration > 0 && speaker_id < 4 {
                    speaker_samples[speaker_id as usize] += duration as u64;
                    Some(SpeakerSegment {
                        speaker_id,
                        start_sample,
                        end_sample,
                    })
                } else {
                    None
                }
            })
            .collect();

        if segments.is_empty() {
            return Ok(None);
        }

        // Dominant speaker is the one with the most samples.
        let dominant = speaker_samples
            .iter()
            .enumerate()
            .max_by_key(|(_, &samples)| samples)
            .filter(|(_, &samples)| samples > 0)
            .map(|(id, _)| id as u32);

        Ok(Some(DiarizationResult {
            dominant_speaker: dominant,
            segments,
        }))
    }

    /// Reset internal state. Call this when diarization is toggled off and
    /// back on to avoid stale speaker assignments.
    pub fn reset_state(&mut self) {
        self.inner.reset_state();
    }
}

#[cfg(target_os = "macos")]
fn build_macos_sortformer(
    model_path: &Path,
    use_cuda: bool,
    diar_config: parakeet_rs::sortformer::DiarizationConfig,
) -> Result<parakeet_rs::sortformer::Sortformer> {
    build_macos_sortformer_with(model_path, use_cuda, diar_config, |path, cfg| {
        let exec = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(parakeet_rs::ExecutionProvider::WebGPU);
        parakeet_rs::sortformer::Sortformer::with_config(path, Some(exec), cfg)
            .map_err(anyhow::Error::from)
    })
}

#[cfg(target_os = "macos")]
fn build_macos_sortformer_with<F>(
    model_path: &Path,
    use_cuda: bool,
    diar_config: parakeet_rs::sortformer::DiarizationConfig,
    try_webgpu: F,
) -> Result<parakeet_rs::sortformer::Sortformer>
where
    F: FnOnce(&Path, parakeet_rs::sortformer::DiarizationConfig) -> Result<parakeet_rs::sortformer::Sortformer, anyhow::Error>,
{
    // `use_cuda` means "request GPU acceleration" — on macOS this means WebGPU.
    if use_cuda {
        match try_webgpu(model_path, diar_config.clone()) {
            Ok(inner) => {
                eprintln!("info: Sortformer using execution provider: WebGPU");
                return Ok(inner);
            }
            Err(e) => {
                eprintln!("warn: Sortformer WebGPU init failed ({e}); falling back to CPU");
            }
        }
    }
    let exec = parakeet_rs::ExecutionConfig::new()
        .with_execution_provider(parakeet_rs::ExecutionProvider::Cpu);
    eprintln!("info: Sortformer using execution provider: Cpu");
    let cfg = diar_config;
    parakeet_rs::sortformer::Sortformer::with_config(model_path, Some(exec), cfg)
        .with_context(|| format!("loading Sortformer from {} (provider=Cpu)", model_path.display()))
}