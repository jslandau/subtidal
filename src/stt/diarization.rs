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
//! inference pass. With the v3 low-latency profile (`chunk_len=9`,
//! `right_context=4`), algorithmic diarization latency is about 1.04 seconds;
//! larger chunk/context settings trade more delay for potentially better
//! speaker-boundary stability.
//!
//! # Platform dispatch
//!
//! CUDA on Linux, WebGPU on macOS, with explicit CPU construction fallback on both.

use anyhow::{Context, Result};
use std::path::Path;

use crate::config::DiarizationPreset;

/// Sortformer v3 supports eight session-local speaker IDs (0 through 7).
pub const MAX_SPEAKERS: usize = crate::config::MAX_DIARIZATION_SPEAKERS;

/// NVIDIA's v3 low-latency profile has 9 emitted and 4 lookahead frames,
/// retaining a 264-frame FIFO/cache and a 222-frame cache update period.
/// Apply it through `set_profile` so all streaming state is reset together.
const STREAMING_PROFILE: parakeet_rs::sortformer::StreamingProfile =
    parakeet_rs::sortformer::StreamingProfile::low_latency();

/// A speaker segment in the diarizer's absolute sample-count frame.
#[derive(Debug, Clone)]
pub struct SpeakerSegment {
    /// Speaker index (0-based, 0 through 7 for the eight-speaker model).
    pub speaker_id: u32,
    /// Absolute start sample offset since the last state reset.
    pub start_sample: usize,
    /// Absolute end sample offset since the last state reset.
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

/// Streaming speaker diarization engine backed by Sortformer v3.
///
/// Stateful — preserves FIFO, speaker cache, and silence profile across
/// calls. Uses `feed()` which accumulates audio and produces segments when
/// enough has been buffered for one inference pass. With the current
/// low-latency settings, diarization latency is approximately
/// `(chunk_len + right_context) * 80ms`; increasing those values trades
/// responsiveness for potentially better turn-boundary accuracy.
pub struct DiarizationEngine {
    inner: parakeet_rs::sortformer::Sortformer,
    using_gpu: bool,
}

impl DiarizationEngine {
    /// Load the Sortformer model from the given directory and configure it
    /// for real-time inference.
    ///
    /// The model filename is resolved by `models::diarization_model_file_in`.
    ///
    /// `use_cuda` requests CUDA on Linux or WebGPU on macOS. Both platforms
    /// explicitly retry CPU if GPU registration or model construction fails.
    /// `using_gpu()` reports the successful construction path, not the request.
    pub fn new(model_dir: &Path, use_cuda: bool, preset: &DiarizationPreset) -> Result<Self> {
        let model_path = model_file_in_directory(model_dir)?;
        if !model_path.exists() {
            anyhow::bail!(
                "Sortformer model not found at {}. Run with diarization enabled to trigger download.",
                model_path.display()
            );
        }

        let diar_config = postprocessing_config(preset);

        let build = |gpu| -> Result<parakeet_rs::sortformer::Sortformer> {
            let exec = super::ort_runtime::execution_config(gpu)?;
            let mut inner = parakeet_rs::sortformer::Sortformer::with_config(
                &model_path,
                Some(exec),
                diar_config.clone(),
            )
            .with_context(|| format!("loading Sortformer from {}", model_path.display()))?;
            inner
                .set_profile(STREAMING_PROFILE)
                .context("applying Sortformer v3 low-latency profile")?;
            Ok(inner)
        };
        let (inner, using_gpu) = super::ort_runtime::build_with_cpu_fallback(
            "Sortformer",
            use_cuda,
            || build(true),
            || build(false),
        )?;

        eprintln!(
            "info: Sortformer real-time config: chunk_len={}, fifo_len={}, spkcache_len={}, right_context={}, latency≈{:.1}s",
            inner.chunk_len,
            inner.fifo_len,
            inner.spkcache_len,
            inner.right_context,
            inner.latency(),
        );

        Ok(DiarizationEngine { inner, using_gpu })
    }

    /// Feed a chunk of 16kHz mono f32 PCM and collect diarization results.
    ///
    /// Uses `feed()` which accumulates audio internally and produces segments
    /// only when a full inference window is ready. The effective algorithmic
    /// latency is approximately `(chunk_len + right_context) * 80ms`.
    ///
    /// The `dominant_speaker` field is the speaker with the most samples in
    /// the returned segments, or None if no segments were produced.
    pub fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<DiarizationResult>> {
        let parakeet_segments = self.inner.feed(pcm).context("Sortformer feed error")?;

        Ok(collect_segments(parakeet_segments))
    }

    /// Whether requested GPU construction succeeded, rather than falling back to CPU.
    pub fn using_gpu(&self) -> bool {
        self.using_gpu
    }

    /// Reset internal state. Call this when diarization is toggled off and
    /// back on to avoid stale speaker assignments.
    pub fn reset_state(&mut self) {
        self.inner.reset_state();
    }
}

fn model_file_in_directory(model_dir: &Path) -> Result<std::path::PathBuf> {
    // The models helper accepts a base directory; this adapter accepts the
    // diarization directory itself. Reuse only the helper's model filename.
    let conventional_path = crate::models::diarization_model_file_in(Path::new(""));
    Ok(model_dir.join(
        conventional_path
            .file_name()
            .context("Sortformer model filename missing")?,
    ))
}

fn postprocessing_config(preset: &DiarizationPreset) -> parakeet_rs::sortformer::DiarizationConfig {
    use parakeet_rs::sortformer::DiarizationConfig;
    match preset {
        DiarizationPreset::Default => DiarizationConfig::default(),
        // Preserve explicitly selected legacy postprocessing settings.
        DiarizationPreset::Callhome => DiarizationConfig {
            onset: 0.641,
            offset: 0.561,
            pad_onset: 0.229,
            pad_offset: 0.079,
            min_duration_on: 0.511,
            min_duration_off: 0.296,
            median_window: 11,
        },
        DiarizationPreset::Dihard3 => DiarizationConfig {
            onset: 0.56,
            offset: 1.0,
            pad_onset: 0.063,
            pad_offset: 0.002,
            min_duration_on: 0.007,
            min_duration_off: 0.151,
            median_window: 11,
        },
        DiarizationPreset::Custom {
            onset,
            offset,
            min_seg_duration,
        } => {
            let mut cfg = DiarizationConfig::custom(*onset as f32, *offset as f32);
            cfg.min_duration_on = *min_seg_duration as f32;
            cfg
        }
    }
}

fn collect_segments(
    input: Vec<parakeet_rs::sortformer::SpeakerSegment>,
) -> Option<DiarizationResult> {
    let mut speaker_samples = [0_u64; MAX_SPEAKERS];
    let segments: Vec<_> = input
        .into_iter()
        .filter_map(|seg| {
            // Check the upstream usize ID before narrowing; large IDs must not
            // wrap into a legitimate speaker slot.
            let samples = speaker_samples.get_mut(seg.speaker_id)?;
            let start_sample = usize::try_from(seg.start).ok()?;
            let end_sample = usize::try_from(seg.end).ok()?;
            let duration = seg.end.checked_sub(seg.start).filter(|&n| n > 0)?;
            *samples = samples.saturating_add(duration);
            Some(SpeakerSegment {
                speaker_id: u32::try_from(seg.speaker_id).ok()?,
                start_sample,
                end_sample,
            })
        })
        .collect();
    if segments.is_empty() {
        return None;
    }
    let dominant_speaker = speaker_samples
        .iter()
        .enumerate()
        .max_by_key(|(_, samples)| *samples)
        .filter(|(_, samples)| **samples > 0)
        .map(|(id, _)| id as u32);
    Some(DiarizationResult {
        dominant_speaker,
        segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diarization_preset_matches_docs() {
        let profile = STREAMING_PROFILE;
        assert_eq!((profile.chunk_len, profile.right_context), (9, 4));
        assert_eq!((profile.fifo_len, profile.spkcache_len), (264, 264));
        assert_eq!(profile.spkcache_update_period, 222);
        assert_eq!((profile.chunk_len + profile.right_context) * 80, 1040);
        assert_eq!(MAX_SPEAKERS, 8);
    }

    #[test]
    fn postprocessing_preserves_explicit_legacy_presets_and_v3_default() {
        for (preset, expected, median) in [
            (
                DiarizationPreset::Default,
                [0.5, 0.5, 0.0, 0.0, 0.0, 0.0],
                1,
            ),
            (
                DiarizationPreset::Callhome,
                [0.641, 0.561, 0.229, 0.079, 0.511, 0.296],
                11,
            ),
            (
                DiarizationPreset::Dihard3,
                [0.56, 1.0, 0.063, 0.002, 0.007, 0.151],
                11,
            ),
        ] {
            let config = postprocessing_config(&preset);
            assert_eq!(
                [
                    config.onset,
                    config.offset,
                    config.pad_onset,
                    config.pad_offset,
                    config.min_duration_on,
                    config.min_duration_off
                ],
                expected
            );
            assert_eq!(config.median_window, median);
        }
    }

    #[test]
    #[ignore = "requires installed Nemotron English and Sortformer v3 models and ORT runtime"]
    fn production_cpu_adapters_fixture_smoke() -> anyhow::Result<()> {
        production_adapters_fixture_smoke(false)
    }

    #[test]
    #[cfg(target_os = "linux")]
    #[ignore = "requires matching staged ORT CUDA providers, GPU, and installed production models"]
    fn production_cuda_adapters_fixture_smoke() -> anyhow::Result<()> {
        // A CUDA test must fail, not silently exercise the intentional CPU fallback.
        super::super::ort_runtime::require_gpu_registration()?;
        production_adapters_fixture_smoke(true)
    }

    #[test]
    fn model_directory_contract_does_not_duplicate_diarization_subdirectory() {
        let base = Path::new("arbitrary-model-base");
        let expected = crate::models::diarization_model_file_in(base);
        assert_eq!(
            model_file_in_directory(&base.join("diarization")).unwrap(),
            expected
        );
        let custom = Path::new("custom-sortformer-directory");
        assert_eq!(
            model_file_in_directory(custom).unwrap().parent(),
            Some(custom)
        );
    }

    fn production_adapters_fixture_smoke(gpu: bool) -> anyhow::Result<()> {
        use crate::stt::{nemotron::NemotronEngine, SttEngine};
        let mut reader = hound::WavReader::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/macos-webgpu-smoke.wav"
        ))?;
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        let mut pcm: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.map(|value| value as f32 / 32768.0))
            .collect::<Result<_, _>>()?;
        // Drain buffered speech through the adapters' ordinary streaming path.
        pcm.extend(std::iter::repeat_n(0.0, 16_000 * 3));
        let mut asr = NemotronEngine::new(&crate::models::nemotron_model_dir(), gpu)?;
        let mut diar = DiarizationEngine::new(
            &crate::models::diarization_model_dir(),
            gpu,
            &DiarizationPreset::Default,
        )?;
        assert_eq!(asr.using_gpu(), gpu, "Nemotron fell back during GPU smoke");
        assert_eq!(
            diar.using_gpu(),
            gpu,
            "Sortformer fell back during GPU smoke"
        );
        let mut text = String::new();
        let mut count = 0;
        for chunk in pcm.chunks(2560) {
            if let Some(fragment) = asr.process_chunk(chunk)? {
                text.push_str(&fragment);
            }
            if let Some(result) = diar.process_chunk(chunk)? {
                count += result.segments.len();
                assert!(result
                    .segments
                    .iter()
                    .all(|seg| seg.speaker_id < MAX_SPEAKERS as u32
                        && seg.end_sample > seg.start_sample));
            }
        }
        assert!(!text.trim().is_empty(), "fixture produced no recognition");
        assert!(count > 0, "fixture produced no diarization segments");
        diar.reset_state();
        assert!(diar.process_chunk(&[0.0; 2560])?.is_none());
        eprintln!("production adapter smoke (gpu={gpu}): {count} segments; text={text}");
        Ok(())
    }

    fn segment(id: usize, start: u64, end: u64) -> parakeet_rs::sortformer::SpeakerSegment {
        parakeet_rs::sortformer::SpeakerSegment {
            speaker_id: id,
            start,
            end,
        }
    }

    #[test]
    fn accepts_all_eight_speakers_and_accumulates_duration() {
        let mut input: Vec<_> = (0..8).map(|id| segment(id, 100, 110)).collect();
        input.push(segment(7, 1000, 2000));
        let result = collect_segments(input).unwrap();
        assert_eq!(result.segments.len(), 9);
        assert_eq!(result.dominant_speaker, Some(7));
        assert_eq!(result.segments[8].start_sample, 1000);
        assert_eq!(result.segments[8].end_sample, 2000);
        let result = collect_segments(vec![
            segment(4, 0, 60),
            segment(4, 60, 120),
            segment(7, 0, 100),
        ])
        .unwrap();
        assert_eq!(result.dominant_speaker, Some(4));
    }

    #[test]
    fn rejects_invalid_ids_and_empty_or_reversed_intervals() {
        assert!(collect_segments(vec![]).is_none());
        assert!(collect_segments(vec![
            segment(8, 0, 100),
            segment(usize::MAX, 0, 100),
            segment(0, 5, 5),
            segment(1, 10, 5)
        ])
        .is_none());
        let result =
            collect_segments(vec![segment(usize::MAX, 0, 1000), segment(6, 10, 20)]).unwrap();
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.dominant_speaker, Some(6));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn large_invalid_id_cannot_wrap_and_duration_sum_saturates() {
        let wrapped_id = u32::MAX as usize + 1;
        assert!(collect_segments(vec![segment(wrapped_id, 0, 100)]).is_none());
        let result =
            collect_segments(vec![segment(7, 0, u64::MAX), segment(7, 0, u64::MAX)]).unwrap();
        assert_eq!(result.dominant_speaker, Some(7));
        assert_eq!(result.segments.len(), 2);
    }
}
