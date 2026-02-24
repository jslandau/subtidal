//! Parakeet EOU STT engine: wraps parakeet_rs::ParakeetEOU.

use anyhow::{Context, Result};
use std::path::Path;
use super::SttEngine;

pub struct ParakeetEngine {
    inner: parakeet_rs::ParakeetEOU,
}

impl ParakeetEngine {
    /// Load the Parakeet EOU model from the given directory.
    /// Directory must contain: encoder.onnx, decoder_joint.onnx, tokenizer.json
    /// (downloaded in Phase 2 to ~/.local/share/live-captions/models/parakeet/).
    pub fn new(model_dir: &Path) -> Result<Self> {
        // Build execution config with CUDA (falls through to CPU if CUDA unavailable).
        // Try CUDA if available via ort feature; otherwise defaults to CPU.
        #[cfg(feature = "cuda")]
        let exec_config = {
            parakeet_rs::ExecutionConfig::new()
                .with_execution_provider(parakeet_rs::ExecutionProvider::Cuda)
        };

        #[cfg(not(feature = "cuda"))]
        let exec_config = parakeet_rs::ExecutionConfig::new();

        let inner = parakeet_rs::ParakeetEOU::from_pretrained(model_dir, Some(exec_config))
            .with_context(|| format!("loading ParakeetEOU from {}", model_dir.display()))?;

        Ok(ParakeetEngine { inner })
    }
}

impl SttEngine for ParakeetEngine {
    fn sample_rate(&self) -> u32 {
        16_000
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>> {
        // Feed 160ms chunk (2560 samples at 16kHz).
        // reset_on_eou=true: decoder state resets after each complete utterance.
        let text = self.inner.transcribe(pcm, true)
            .context("ParakeetEOU transcribe")?;

        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    }
}
