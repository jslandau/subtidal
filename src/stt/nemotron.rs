//! Nemotron STT engine: wraps parakeet_rs::Nemotron (RNNT-based, 600M params).
//!
//! Nemotron provides built-in punctuation and capitalization.
//! It requires 560ms (8960 sample) chunks at 16kHz, so this engine
//! internally buffers the 160ms (2560 sample) chunks from the audio bridge
//! until a full 560ms chunk is accumulated.

use anyhow::{Context, Result};
use std::path::Path;
use super::SttEngine;

/// Nemotron expects 560ms chunks = 8960 samples at 16kHz.
const NEMOTRON_CHUNK_SAMPLES: usize = 8960;

pub struct NemotronEngine {
    inner: parakeet_rs::Nemotron,
    /// Internal buffer to accumulate 160ms chunks until 560ms is reached.
    chunk_buf: Vec<f32>,
}

impl NemotronEngine {
    /// Load the Nemotron model from the given directory.
    /// Directory must contain: encoder.onnx, encoder.onnx.data, decoder_joint.onnx, tokenizer.model
    pub fn new(model_dir: &Path, use_cuda: bool) -> Result<Self> {
        let exec_config = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(if use_cuda {
                parakeet_rs::ExecutionProvider::Cuda
            } else {
                parakeet_rs::ExecutionProvider::Cpu
            });

        let inner = parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec_config))
            .with_context(|| format!("loading Nemotron from {}", model_dir.display()))?;

        Ok(NemotronEngine {
            inner,
            chunk_buf: Vec::with_capacity(NEMOTRON_CHUNK_SAMPLES),
        })
    }
}

impl SttEngine for NemotronEngine {
    fn sample_rate(&self) -> u32 {
        16_000
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>> {
        self.chunk_buf.extend_from_slice(pcm);

        if self.chunk_buf.len() < NEMOTRON_CHUNK_SAMPLES {
            return Ok(None); // Still accumulating
        }

        // Drain exactly NEMOTRON_CHUNK_SAMPLES and process.
        let chunk: Vec<f32> = self.chunk_buf.drain(..NEMOTRON_CHUNK_SAMPLES).collect();

        let text = self.inner.transcribe_chunk(&chunk)
            .context("Nemotron transcribe_chunk")?;

        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }
}
