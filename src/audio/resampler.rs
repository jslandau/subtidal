//! Audio resampler: 48kHz stereo F32 → 16kHz mono F32 with 160ms chunk output.

use anyhow::Context;
use anyhow::Result;
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

/// Input sample rate from PipeWire (stereo).
pub const INPUT_SAMPLE_RATE: u32 = 48_000;
/// Output sample rate for STT engines.
pub const OUTPUT_SAMPLE_RATE: u32 = 16_000;
/// Output chunk size: 160ms at 16kHz = 2560 samples.
pub const CHUNK_SAMPLES: usize = 2_560;
/// Corresponding input chunk size at 48kHz: 480ms input for 160ms output.
/// Fft with FixedSync::Input requires input size = 7680 frames.
pub const INPUT_FRAMES_PER_CHUNK: usize = 7_680;

/// Resamples 48kHz stereo → 16kHz mono and accumulates 160ms output chunks.
pub struct AudioResampler {
    resampler: Fft<f32>,
    /// Accumulation buffer: mono 16kHz samples waiting to fill a 160ms chunk.
    accumulator: Vec<f32>,
    /// Interleaved stereo input buffer waiting to fill one resampler input chunk.
    input_buf: Vec<f32>,
}

impl AudioResampler {
    /// Create a new resampler for 48kHz stereo → 16kHz mono.
    pub fn new() -> Result<Self> {
        // Fft<f32>: FFT-based synchronous resampler.
        // Parameters: input_rate, output_rate, chunk_size (in input frames), sub_chunks, channels, fixed
        // chunk_size = INPUT_FRAMES_PER_CHUNK frames, 2 channels (stereo input)
        // FixedSync::Input means input size is fixed, output varies naturally
        let resampler = Fft::<f32>::new(
            INPUT_SAMPLE_RATE as usize,
            OUTPUT_SAMPLE_RATE as usize,
            INPUT_FRAMES_PER_CHUNK,
            2, // sub-chunks (1 = no sub-chunking)
            2, // channels: stereo input
            FixedSync::Input,
        )
        .context("creating Fft resampler")?;

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

            // Resample both channels using the process_into_buffer method.
            // Allocate output buffers sized for the expected output.
            let expected_output_frames = (INPUT_FRAMES_PER_CHUNK * OUTPUT_SAMPLE_RATE as usize)
                / INPUT_SAMPLE_RATE as usize;
            let left_out = vec![0.0f32; expected_output_frames];
            let right_out = vec![0.0f32; expected_output_frames];

            // Create adapters from the vector slices
            let input_vecs = vec![left, right];
            let input_adapter = SequentialSliceOfVecs::new(&input_vecs, 2, INPUT_FRAMES_PER_CHUNK)
                .context("creating input adapter")?;

            let mut output_vecs = vec![left_out, right_out];
            let mut output_adapter = SequentialSliceOfVecs::new_mut(
                &mut output_vecs,
                2,
                expected_output_frames,
            )
            .context("creating output adapter")?;

            let (_, output_count) = self.resampler.process_into_buffer(
                &input_adapter,
                &mut output_adapter,
                None,
            )
            .context("resampling audio")?;

            // Downmix to mono by averaging.
            for (l, r) in output_vecs[0][..output_count].iter().zip(&output_vecs[1][..output_count]) {
                self.accumulator.push((l + r) * 0.5);
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
