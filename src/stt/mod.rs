//! STT engine abstraction and inference thread management.

pub mod parakeet;
pub mod moonshine;

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
/// - `engine`: boxed SttEngine (Parakeet or Moonshine)
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

/// Detect CUDA availability via ort.
/// Returns true if a CUDA-capable GPU is accessible.
pub fn cuda_available() -> bool {
    ort::execution_providers::CUDAExecutionProvider::default().is_available().unwrap_or(false)
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

    /// AC5.3: CUDA unavailable triggers Moonshine fallback.
    /// Test that cuda_available() returns a bool without panicking.
    /// (The actual fallback logic is in main.rs and is inherently hard to unit test.)
    #[test]
    fn cuda_available_detection_does_not_panic() {
        // This should not panic. The result depends on the system,
        // so we only verify the function completes successfully.
        let _result = cuda_available();
        // Test passes if we reach this point without panicking.
    }
}
