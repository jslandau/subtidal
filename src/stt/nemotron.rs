//! Streaming Nemotron adapter with explicit CUDA/WebGPU-to-CPU construction fallback.

use super::SttEngine;
use anyhow::{Context, Result};
use parakeet_rs::NemotronMode;
use std::path::Path;

const NEMOTRON_CHUNK_SAMPLES: usize = 8960;

pub struct NemotronEngine {
    inner: parakeet_rs::Nemotron,
    using_gpu: bool,
    chunk_buf: Vec<f32>,
    /// Reused at each 560 ms inference boundary to avoid per-chunk allocation.
    process_buf: Vec<f32>,
}

impl NemotronEngine {
    /// Load the model directory, retrying CPU if requested GPU construction fails.
    pub fn new(model_dir: &Path, use_cuda: bool) -> Result<Self> {
        let build = |gpu| -> Result<parakeet_rs::Nemotron> {
            let exec = super::ort_runtime::execution_config(gpu)?;
            let mut inner = parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec))
                .with_context(|| format!("loading Nemotron from {}", model_dir.display()))?;
            if inner.mode() == NemotronMode::Multilingual {
                inner
                    .set_target_lang("auto")
                    .context("setting Nemotron target language")?;
            }
            Ok(inner)
        };
        let (inner, using_gpu) = super::ort_runtime::build_with_cpu_fallback(
            "Nemotron",
            use_cuda,
            || build(true),
            || build(false),
        )?;
        Ok(Self {
            inner,
            using_gpu,
            chunk_buf: Vec::with_capacity(NEMOTRON_CHUNK_SAMPLES),
            process_buf: Vec::with_capacity(NEMOTRON_CHUNK_SAMPLES),
        })
    }

    /// Whether GPU construction succeeded (CPU-only graph operators remain allowed).
    pub fn using_gpu(&self) -> bool {
        self.using_gpu
    }
}

impl SttEngine for NemotronEngine {
    fn using_gpu(&self) -> bool {
        self.using_gpu
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>> {
        self.chunk_buf.extend_from_slice(pcm);
        if self.chunk_buf.len() < NEMOTRON_CHUNK_SAMPLES {
            return Ok(None);
        }
        self.process_buf.clear();
        self.process_buf
            .extend(self.chunk_buf.drain(..NEMOTRON_CHUNK_SAMPLES));
        let text = self
            .inner
            .transcribe_chunk(&self.process_buf)
            .context("Nemotron transcribe_chunk")?;
        Ok((!text.is_empty()).then_some(text))
    }
}
