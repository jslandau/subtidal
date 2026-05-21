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
    ///
    /// On Linux, `use_cuda` requests GPU acceleration via CUDA; CPU is used when `use_cuda` is false.
    /// On macOS, `use_cuda` requests GPU acceleration via WebGPU (backed by Metal); CPU fallback
    /// is automatic on WebGPU init failure.
    pub fn new(model_dir: &Path, use_cuda: bool) -> Result<Self> {
        #[cfg(target_os = "linux")]
        let inner = {
            let exec_config = parakeet_rs::ExecutionConfig::new()
                .with_execution_provider(if use_cuda {
                    parakeet_rs::ExecutionProvider::Cuda
                } else {
                    parakeet_rs::ExecutionProvider::Cpu
                });
            let provider = if use_cuda { "Cuda" } else { "Cpu" };
            eprintln!("info: Nemotron using execution provider: {provider}");
            parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec_config))
                .with_context(|| format!("loading Nemotron from {} (provider={provider})", model_dir.display()))?
        };

        #[cfg(target_os = "macos")]
        let inner = build_macos(model_dir, use_cuda)?;

        Ok(NemotronEngine {
            inner,
            chunk_buf: Vec::with_capacity(NEMOTRON_CHUNK_SAMPLES),
        })
    }
}

#[cfg(target_os = "macos")]
fn build_macos(model_dir: &Path, use_cuda: bool) -> Result<parakeet_rs::Nemotron> {
    build_macos_with(model_dir, use_cuda, |dir| {
        let exec = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(parakeet_rs::ExecutionProvider::WebGPU);
        parakeet_rs::Nemotron::from_pretrained(dir, Some(exec))
            .map_err(anyhow::Error::from)
    })
}

#[cfg(target_os = "macos")]
fn build_macos_with<F>(model_dir: &Path, use_cuda: bool, try_webgpu: F) -> Result<parakeet_rs::Nemotron>
where
    F: FnOnce(&Path) -> Result<parakeet_rs::Nemotron, anyhow::Error>,
{
    // `use_cuda` here means "request GPU acceleration"; macOS uses WebGPU
    // (backed by Metal via wgpu) as the GPU provider. CPU is the fallback
    // both when the caller explicitly requested CPU AND when WebGPU init fails.
    if use_cuda {
        match try_webgpu(model_dir) {
            Ok(inner) => {
                eprintln!("info: Nemotron using execution provider: WebGPU");
                return Ok(inner);
            }
            Err(e) => {
                eprintln!("warn: WebGPU init failed ({e}); falling back to CPU");
            }
        }
    }
    let exec = parakeet_rs::ExecutionConfig::new()
        .with_execution_provider(parakeet_rs::ExecutionProvider::Cpu);
    eprintln!("info: Nemotron using execution provider: Cpu");
    parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec))
        .with_context(|| format!("loading Nemotron from {} (provider=Cpu)", model_dir.display()))
}

impl SttEngine for NemotronEngine {
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires Nemotron model files at the conventional model dir"]
    fn cpu_fallback_on_simulated_webgpu_failure() {
        let model_dir = crate::models::nemotron_model_dir();
        let result = build_macos_with(&model_dir, true, |_dir| {
            Err(anyhow::anyhow!("simulated WebGPU init failure"))
        });
        assert!(
            result.is_ok(),
            "CPU fallback should produce a working Nemotron, got: {:?}",
            result.err()
        );
    }
}
