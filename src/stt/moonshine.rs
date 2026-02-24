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
/// Maximum speech buffer size: 480,000 samples = 30 seconds at 16kHz
const MAX_SPEECH_SAMPLES: usize = 480_000;

pub struct MoonshineEngine {
    #[allow(dead_code)]
    encoder: Session,
    #[allow(dead_code)]
    decoder: Session,
    /// Tokenizer for converting token IDs to text (Phase 8).
    tokenizer: tokenizers::Tokenizer,
    /// Accumulated speech audio (16kHz mono) pending inference.
    speech_buf: Vec<f32>,
    /// Count of consecutive silent chunks since last speech.
    silent_chunks: usize,
    /// True if we are currently accumulating speech.
    in_speech: bool,
}

impl MoonshineEngine {
    /// Load Moonshine ONNX models from the given directory.
    /// Directory must contain: encoder_model_quantized.onnx, decoder_model_merged_quantized.onnx, tokenizer.json
    pub fn new(model_dir: &Path) -> Result<Self> {
        let encoder_path = model_dir.join("encoder_model_quantized.onnx");
        let decoder_path = model_dir.join("decoder_model_merged_quantized.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

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

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("loading Moonshine tokenizer from {}: {}", tokenizer_path.display(), e))?;

        Ok(MoonshineEngine {
            encoder,
            decoder,
            tokenizer,
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

        // ⚠️ KNOWN TECH DEBT: This function currently uses placeholder token generation instead of
        // actual ONNX encoder/decoder inference. The encoder and decoder ONNX sessions are loaded
        // but not wired into this function. The placeholder generates fake token IDs (101, 102, ...)
        // based on buffer length, which are then decoded by the tokenizer for testing.
        //
        // TODO: Replace with actual ONNX inference:
        // 1. Convert self.speech_buf (Vec<f32>) to the correct ONNX input format (likely mel-spectrogram)
        // 2. Run self.encoder.run() with the prepared input
        // 3. Run self.decoder.run() on encoder output
        // 4. Extract token IDs from decoder output
        // 5. Decode tokens to text using self.tokenizer
        //
        // This requires understanding the Moonshine ONNX model's input/output signatures,
        // which will be completed in a future phase.
        let num_tokens = (self.speech_buf.len() / 1000).clamp(1, 50);
        let output_tokens: Vec<u32> = (1..=num_tokens)
            .map(|i| (100 + i) as u32)
            .collect();

        // Decode token IDs to text.
        let decoded = self.tokenizer
            .decode(&output_tokens, true)
            .map_err(|e| anyhow::anyhow!("decoding Moonshine output tokens: {}", e))?;

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

            // Check if buffer exceeds maximum size
            if self.speech_buf.len() >= MAX_SPEECH_SAMPLES {
                // Buffer full: run inference on accumulated audio and clear to continue accumulating
                let text = self.run_inference()?;
                self.speech_buf.clear();
                if text.is_empty() {
                    return Ok(None);
                } else {
                    return Ok(Some(text));
                }
            }
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
