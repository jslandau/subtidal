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
///
/// All working buffers are owned as fields and reused across calls; steady-state
/// operation performs no heap allocation.
pub struct AudioResampler {
    resampler: Fft<f32>,
    /// Accumulator for produced mono 16kHz samples; chunks of CHUNK_SAMPLES are
    /// handed to the caller callback and removed via rotate/truncate.
    accumulator: Vec<f32>,
    /// Interleaved stereo input samples not yet fed to rubato.
    input_buf: Vec<f32>,
    /// Deinterleaved channel buffers (reused; cleared + refilled per rubato call).
    /// Held as `Vec<Vec<f32>>` because `SequentialSliceOfVecs` borrows a slice of
    /// vecs; the outer Vec is allocated once and never grown.
    input_vecs: Vec<Vec<f32>>,
    output_vecs: Vec<Vec<f32>>,
    /// Number of output frames rubato produces per input chunk.
    expected_output_frames: usize,
}

impl AudioResampler {
    pub fn new() -> Result<Self> {
        // Fft<f32>: FFT-based synchronous resampler.
        // FixedSync::Input means input size is fixed, output varies naturally.
        let resampler = Fft::<f32>::new(
            INPUT_SAMPLE_RATE as usize,
            OUTPUT_SAMPLE_RATE as usize,
            INPUT_FRAMES_PER_CHUNK,
            2, // sub-chunks
            2, // stereo input channels
            FixedSync::Input,
        )
        .context("creating Fft resampler")?;

        let expected_output_frames =
            (INPUT_FRAMES_PER_CHUNK * OUTPUT_SAMPLE_RATE as usize) / INPUT_SAMPLE_RATE as usize;

        // Pre-size the reusable channel buffers to their exact steady-state length.
        let input_vecs = vec![
            Vec::with_capacity(INPUT_FRAMES_PER_CHUNK),
            Vec::with_capacity(INPUT_FRAMES_PER_CHUNK),
        ];
        let output_vecs = vec![
            vec![0.0f32; expected_output_frames],
            vec![0.0f32; expected_output_frames],
        ];

        Ok(AudioResampler {
            resampler,
            accumulator: Vec::with_capacity(CHUNK_SAMPLES * 2),
            input_buf: Vec::with_capacity(INPUT_FRAMES_PER_CHUNK * 2 * 2),
            input_vecs,
            output_vecs,
            expected_output_frames,
        })
    }

    /// Feed interleaved stereo 48kHz f32 samples. For each complete 160ms mono chunk
    /// produced, `on_chunk` is invoked with a borrowed slice of CHUNK_SAMPLES f32s.
    ///
    /// `samples` must be interleaved stereo: [L0, R0, L1, R1, ...].
    ///
    /// Zero heap allocations in the steady state: all working buffers are owned by
    /// `self` and cleared/refilled in place.
    pub fn push_interleaved<F>(&mut self, samples: &[f32], mut on_chunk: F) -> Result<()>
    where
        F: FnMut(&[f32]),
    {
        self.input_buf.extend_from_slice(samples);

        let interleaved_chunk = INPUT_FRAMES_PER_CHUNK * 2;
        let mut consumed = 0usize;

        while self.input_buf.len() - consumed >= interleaved_chunk {
            let chunk = &self.input_buf[consumed..consumed + interleaved_chunk];
            consumed += interleaved_chunk;

            // Deinterleave stereo into the reusable channel vecs.
            let (left, right) = self.input_vecs.split_at_mut(1);
            let left = &mut left[0];
            let right = &mut right[0];
            left.clear();
            right.clear();
            for pair in chunk.chunks_exact(2) {
                left.push(pair[0]);
                right.push(pair[1]);
            }

            let input_adapter =
                SequentialSliceOfVecs::new(&self.input_vecs, 2, INPUT_FRAMES_PER_CHUNK)
                    .context("creating input adapter")?;

            let mut output_adapter = SequentialSliceOfVecs::new_mut(
                &mut self.output_vecs,
                2,
                self.expected_output_frames,
            )
            .context("creating output adapter")?;

            let (_, output_count) = self
                .resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, None)
                .context("resampling audio")?;

            // Downmix stereo to mono by averaging, appending to the accumulator.
            let left_out = &self.output_vecs[0][..output_count];
            let right_out = &self.output_vecs[1][..output_count];
            for (l, r) in left_out.iter().zip(right_out) {
                self.accumulator.push((l + r) * 0.5);
            }

            // Emit every complete 160ms output chunk. We slice into the accumulator
            // in place and compact it with drain(..emitted) at the end — no per-chunk
            // allocation, no Vec<Vec<f32>> return.
            let mut emitted = 0usize;
            while self.accumulator.len() - emitted >= CHUNK_SAMPLES {
                let start = emitted;
                let end = emitted + CHUNK_SAMPLES;
                on_chunk(&self.accumulator[start..end]);
                emitted = end;
            }
            if emitted > 0 {
                self.accumulator.drain(..emitted);
            }
        }

        if consumed > 0 {
            self.input_buf.drain(..consumed);
        }

        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_chunks(r: &mut AudioResampler, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        r.push_interleaved(samples, |chunk| out.push(chunk.to_vec())).unwrap();
        out
    }

    #[test]
    fn resampler_produces_correct_chunk_size() {
        let mut r = AudioResampler::new().unwrap();
        let samples: Vec<f32> = vec![0.1f32; INPUT_FRAMES_PER_CHUNK * 2];
        let chunks = collect_chunks(&mut r, &samples);
        assert_eq!(chunks.len(), 1, "expected 1 chunk, got {}", chunks.len());
        assert_eq!(chunks[0].len(), CHUNK_SAMPLES);
    }

    #[test]
    fn resampler_accumulates_partial_input() {
        let mut r = AudioResampler::new().unwrap();
        let samples: Vec<f32> = vec![0.1f32; INPUT_FRAMES_PER_CHUNK];
        let chunks = collect_chunks(&mut r, &samples);
        assert!(chunks.is_empty(), "partial input should not produce output");
    }

    #[test]
    fn resampler_accumulates_across_multiple_pushes() {
        let mut r = AudioResampler::new().unwrap();
        let increment: Vec<f32> = vec![0.0f32; 512];
        let total_needed = INPUT_FRAMES_PER_CHUNK * 2;
        let mut total_chunks = 0usize;
        let mut pushed = 0usize;
        while pushed < total_needed {
            let to_push = increment.len().min(total_needed - pushed);
            r.push_interleaved(&increment[..to_push], |_| total_chunks += 1).unwrap();
            pushed += to_push;
        }
        assert_eq!(total_chunks, 1);
    }
}
