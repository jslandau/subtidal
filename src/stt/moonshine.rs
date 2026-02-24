//! Moonshine STT engine: ort ONNX inference with energy VAD.
//!
//! ⚠️ SIMPLIFIED IMPLEMENTATION: This Phase 4 stub implements VAD and buffer management.
//! Actual ONNX encoder/decoder inference will be completed in Phase 8 with proper
//! tokenizer integration. For now, this returns placeholder token IDs.

use anyhow::{Context, Result};
use ort::session::Session;
use std::path::Path;
use super::SttEngine;

/// Energy VAD threshold: RMS below this level is treated as silence.
const VAD_RMS_THRESHOLD: f32 = 0.002;
/// Number of consecutive silent 160ms chunks before triggering inference.
const SILENCE_CHUNKS_BEFORE_INFER: usize = 5; // 5 × 160ms = 800ms silence

pub struct MoonshineEngine {
    #[allow(dead_code)]
    encoder: Session,
    #[allow(dead_code)]
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
            .with_execution_providers([ort::ep::CPU::default().build()])
            .context("setting CPU EP (encoder)")?
            .commit_from_file(&encoder_path)
            .with_context(|| format!("loading encoder from {}", encoder_path.display()))?;

        let decoder = Session::builder()
            .context("ort session builder (decoder)")?
            .with_execution_providers([ort::ep::CPU::default().build()])
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
    ///
    /// ⚠️ PHASE 4 STUB: Returns placeholder token IDs. Full ONNX inference
    /// with proper tokenizer integration happens in Phase 8.
    fn run_inference(&mut self) -> Result<String> {
        if self.speech_buf.is_empty() {
            return Ok(String::new());
        }

        // ⚠️ KNOWN INTERMEDIATE STATE: Moonshine output is numeric token IDs (not text)
        // until Phase 8 Task 1 integrates the tokenizer. The tokenizer.json file is already
        // downloaded by Phase 2, but loading it into MoonshineEngine happens in Phase 8.
        // Between Phases 4–7, Moonshine captions will display as e.g. "12 45 67 89".
        // This is expected and does NOT indicate a bug.

        // Generate placeholder output tokens based on buffer length
        // This demonstrates the STT engine architecture without requiring
        // fully functional ONNX inference during Phase 4
        let num_tokens = (self.speech_buf.len() / 1000).max(1).min(50);
        let output_tokens: Vec<String> = (1..=num_tokens)
            .map(|i| (100 + i).to_string())
            .collect();

        let decoded = output_tokens.join(" ");
        eprintln!("debug: Moonshine raw token IDs (placeholder): {decoded}");
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
