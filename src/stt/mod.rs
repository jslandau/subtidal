//! STT engine abstraction and inference thread management.

pub mod nemotron;

use anyhow::Result;
use ort::ep::ExecutionProvider as _;
use std::sync::mpsc;
use std::thread;

/// Trait implemented by all STT backends.
///
/// Both methods are called from the inference thread. Implementors must be `Send + 'static`.
pub trait SttEngine: Send + 'static {
    /// The sample rate this engine expects. Both engines return 16000.
    /// Note: Currently unused but will be used in future phases for runtime validation.
    #[allow(dead_code)]
    fn sample_rate(&self) -> u32;

    /// Process one 160ms chunk of 16kHz mono PCM.
    ///
    /// Returns `Ok(Some(text))` when a complete utterance has been recognized,
    /// `Ok(None)` when more audio is needed, or an error if inference failed
    /// (caller should log and skip the chunk).
    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>;
}

/// Spawn the inference thread.
///
/// Parameters:
/// - `engine`: boxed SttEngine (Nemotron via parakeet-rs)
/// - `audio_rx`: receives 160ms chunks from the audio processing thread
/// - `caption_tx`: sends recognized text to the GTK4 main thread
///
/// Returns the thread JoinHandle for clean shutdown.
pub fn spawn_inference_thread(
    mut engine: Box<dyn SttEngine>,
    audio_rx: mpsc::Receiver<Vec<f32>>,
    caption_tx: mpsc::SyncSender<String>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("stt-inference".to_string())
        .spawn(move || {
            for chunk in audio_rx.iter() {
                match engine.process_chunk(&chunk) {
                    Ok(Some(text)) if !text.trim().is_empty() => {
                        if caption_tx.send(text).is_err() {
                            break; // receiver dropped — shutdown
                        }
                    }
                    Ok(Some(_)) | Ok(None) => {} // no output yet
                    Err(e) => {
                        eprintln!("warn: inference error (skipping chunk): {e}");
                    }
                }
            }
        })
        .expect("spawning inference thread")
}

/// Restart the inference thread with a new engine.
/// Drops the old chunk_rx (causing the old thread to exit when its sender is replaced).
/// Returns new chunk_tx for the audio bridge thread.
pub fn restart_inference_thread(
    engine: Box<dyn SttEngine>,
    caption_tx: mpsc::SyncSender<String>,
) -> (mpsc::SyncSender<Vec<f32>>, thread::JoinHandle<()>) {
    let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(32);
    let handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
    (chunk_tx, handle)
}

/// Detect CUDA usability by loading the model with CUDA in a subprocess.
///
/// The ort CUDA provider can segfault during dlopen or session creation if there's
/// a version mismatch between the provider .so and the system CUDA libraries. By
/// doing the full model load in a subprocess, we ensure that if CUDA causes a
/// segfault at any point (probe, session creation, or kernel load), the parent
/// process survives and falls back to CPU.
///
/// Returns true only if the child process exits successfully with a "cuda:ok" signal.
pub fn cuda_available(model_dir: &std::path::Path) -> bool {
    use std::io::Read as _;
    use std::process::{Command, Stdio};

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let result = Command::new(exe)
        .env("__SUBTIDAL_CUDA_PROBE", "1")
        .env("__SUBTIDAL_CUDA_PROBE_MODEL_DIR", model_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            let mut output = String::new();
            if let Some(ref mut stdout) = child.stdout {
                let _ = stdout.read_to_string(&mut output);
            }
            let status = child.wait()?;
            Ok((status, output))
        });

    match result {
        Ok((status, output)) => status.success() && output.trim() == "cuda:ok",
        Err(_) => false,
    }
}

/// Called when __SUBTIDAL_CUDA_PROBE env var is set.
/// Attempts to load the Nemotron model with CUDA EP, prints "cuda:ok" on success,
/// then exits. If this segfaults at any stage (EP probe, session creation, kernel
/// load), the parent process sees a non-zero/signal exit and falls back to CPU.
pub fn run_cuda_probe() -> ! {
    // First check if CUDA EP is even available.
    let available = ort::execution_providers::CUDAExecutionProvider::default()
        .is_available()
        .unwrap_or(false);
    if !available {
        std::process::exit(0);
    }

    // Actually attempt to load the model with CUDA — this is where the segfault
    // typically occurs due to CUDA version mismatches during session creation.
    if let Some(model_dir) = std::env::var_os("__SUBTIDAL_CUDA_PROBE_MODEL_DIR") {
        let config = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(parakeet_rs::ExecutionProvider::Cuda);
        if parakeet_rs::Nemotron::from_pretrained(std::path::Path::new(&model_dir), Some(config)).is_err() {
            std::process::exit(1);
        }
    }

    print!("cuda:ok");
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    struct MockEngine {
        responses: Vec<Option<String>>,
        call_index: usize,
    }

    impl SttEngine for MockEngine {
        fn sample_rate(&self) -> u32 { 16_000 }
        fn process_chunk(&mut self, _pcm: &[f32]) -> Result<Option<String>> {
            let resp = self.responses.get(self.call_index).cloned().flatten();
            self.call_index += 1;
            Ok(resp)
        }
    }

    #[test]
    fn inference_thread_forwards_recognized_text() {
        let engine = Box::new(MockEngine {
            responses: vec![Some("hello world".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap();
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["hello world"]);
    }

    #[test]
    fn inference_thread_suppresses_none_responses() {
        let engine = Box::new(MockEngine {
            responses: vec![None, Some("world".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // None
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // Some("world")
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["world"]);
    }

    #[test]
    fn inference_thread_suppresses_whitespace_only_text() {
        let engine = Box::new(MockEngine {
            responses: vec![Some("   ".to_string()), Some("hi".to_string())],
            call_index: 0,
        });
        let (chunk_tx, chunk_rx) = mpsc::sync_channel(4);
        let (caption_tx, caption_rx) = mpsc::sync_channel(4);
        let _handle = spawn_inference_thread(engine, chunk_rx, caption_tx);
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // whitespace only
        chunk_tx.send(vec![0.0f32; 2560]).unwrap(); // "hi"
        drop(chunk_tx);
        let received: Vec<String> = caption_rx.iter().collect();
        assert_eq!(received, vec!["hi"]);
    }

    /// AC5.3: CUDA probe subprocess returns a bool without crashing the parent.
    ///
    /// Note: This test spawns the release binary (not the test binary) as a subprocess.
    /// The test binary doesn't have the probe entry point, so we can't test the
    /// subprocess mechanism in unit tests. Integration testing requires the full binary.
    #[test]
    fn cuda_probe_returns_bool_without_crashing() {
        // cuda_available() spawns the main binary as a subprocess.
        // In the test environment, current_exe() returns the test runner,
        // which doesn't have the __SUBTIDAL_CUDA_PROBE handler — so it will
        // return false (child exits without printing "cuda:ok").
        // We just verify the parent doesn't crash or hang.
        let result = cuda_available(std::path::Path::new("/nonexistent"));
        // Result depends on system — we only verify the parent survived.
        let _ = result;
    }
}
