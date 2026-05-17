//! STT engine abstraction and the combined audio-resample-inference thread.

#[cfg(target_os = "linux")]
pub mod nemotron;

use anyhow::Result;
use arc_swap::ArcSwap;
use ringbuf::HeapCons;
use ringbuf::traits::Consumer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::audio::resampler::AudioResampler;
use crate::config::Engine;

/// Trait implemented by all STT backends.
pub trait SttEngine: Send + 'static {
    fn process_chunk(&mut self, pcm: &[f32]) -> Result<Option<String>>;
}

/// Cross-thread wake primitive: the PipeWire RT callback calls `notify()` after
/// pushing to the ring buffer; the STT consumer thread calls `wait_timeout()`.
///
/// `data_ready` is the boolean predicate the Condvar protects; the RT side sets
/// it with SeqCst and signals without holding the mutex (a missed wakeup is
/// harmless because the consumer re-checks the flag on every timeout).
pub struct AudioWake {
    data_ready: AtomicBool,
    shutdown: AtomicBool,
    mutex: Mutex<()>,
    condvar: Condvar,
}

impl AudioWake {
    pub fn new() -> Self {
        Self {
            data_ready: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            mutex: Mutex::new(()),
            condvar: Condvar::new(),
        }
    }

    /// Called from the PipeWire real-time callback. Sets the ready flag and
    /// signals the consumer. Does not lock the mutex (notify_one is safe to call
    /// without holding it; the consumer's timeout closes the missed-wakeup race).
    #[inline]
    pub fn notify(&self) {
        self.data_ready.store(true, Ordering::Release);
        self.condvar.notify_one();
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.condvar.notify_all();
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    /// Wait for data or timeout. Returns true if data was signalled, false on timeout.
    /// On return, `data_ready` is cleared so the caller should drain whatever is
    /// available.
    fn wait_timeout(&self, timeout: Duration) -> bool {
        if self.data_ready.swap(false, Ordering::AcqRel) {
            return true;
        }
        let guard = self.mutex.lock().unwrap();
        let (_guard, _wait_result) = self
            .condvar
            .wait_timeout_while(guard, timeout, |_| {
                !self.data_ready.load(Ordering::Acquire) && !self.shutdown.load(Ordering::Acquire)
            })
            .unwrap();
        self.data_ready.swap(false, Ordering::AcqRel)
    }
}

/// Parameters for the combined STT pipeline thread.
pub struct PipelineConfig {
    pub engine_choice: Arc<ArcSwap<Engine>>,
    pub captions_enabled: Arc<AtomicBool>,
    pub unload_after: Option<Duration>,
    pub model_dir: std::path::PathBuf,
    pub use_cuda: bool,
}

/// Spawn the combined audio→resample→inference thread.
///
/// Replaces the old pair (bridge thread + inference thread) and their channel.
/// Reads directly from the ring buffer, resamples, dispatches to the current
/// engine (read lock-free via `ArcSwap`), and sends recognised text via
/// `async-channel` to the GTK main loop.
///
/// Engine swap is handled by swapping the `ArcSwap<Engine>` from the tray; this
/// thread notices on each chunk boundary and rebuilds its local engine.
#[cfg(target_os = "linux")]
pub fn spawn_stt_thread(
    mut ring_consumer: HeapCons<f32>,
    wake: Arc<AudioWake>,
    caption_tx: async_channel::Sender<String>,
    cfg: PipelineConfig,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("stt-pipeline".to_string())
        .spawn(move || {
            let mut resampler = match AudioResampler::new() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: failed to create resampler: {e:#}");
                    return;
                }
            };

            // Local engine + tracking of the engine choice it was built from.
            let mut engine: Option<Box<dyn SttEngine>> = None;
            let mut engine_built_for: Option<Engine> = None;
            let mut disabled_since: Option<Instant> = None;

            // Scratch buffer for ring drain; sized generously so one wake typically
            // drains all pending audio in a single pop.
            let mut raw = vec![0f32; 8192];

            loop {
                if wake.is_shutdown() {
                    break;
                }

                // Block until data arrives or we timeout (timeout needed so we can
                // observe shutdown, captions-disabled VRAM unload, and engine swaps
                // even when audio stops).
                wake.wait_timeout(Duration::from_millis(250));

                if wake.is_shutdown() {
                    break;
                }

                // Drain all currently-available samples in a tight inner loop —
                // one wake can correspond to many ring pushes.
                loop {
                    let n = ring_consumer.pop_slice(&mut raw);
                    if n == 0 {
                        break;
                    }

                    if !cfg.captions_enabled.load(Ordering::Relaxed) {
                        // Model stays loaded; we just discard audio.
                        continue;
                    }

                    // Rebuild engine if the tray asked for a different one, or
                    // if we're reloading after a VRAM unload. We do this before
                    // resampling so the closure has a live engine to call into.
                    let desired = Engine::clone(&cfg.engine_choice.load());
                    let needs_rebuild =
                        engine.is_none() || engine_built_for.as_ref() != Some(&desired);
                    if needs_rebuild {
                        eprintln!("info: (re)loading STT engine: {desired:?}");
                        match build_engine(&desired, &cfg.model_dir, cfg.use_cuda) {
                            Ok(e) => {
                                engine = Some(e);
                                engine_built_for = Some(desired);
                            }
                            Err(err) => {
                                eprintln!("warn: failed to build STT engine: {err:#}");
                                continue;
                            }
                        }
                    }

                    let e = engine.as_mut().unwrap();
                    disabled_since = None;

                    // Track whether the caption receiver has dropped so we can exit
                    // the thread cleanly after the current resampler call returns.
                    let mut receiver_dropped = false;
                    let push_result = resampler.push_interleaved(&raw[..n], |chunk| {
                        if receiver_dropped {
                            return;
                        }
                        match e.process_chunk(chunk) {
                            Ok(Some(text)) if !text.trim().is_empty() => {
                                if caption_tx.try_send(text).is_err() {
                                    receiver_dropped = true;
                                }
                            }
                            Ok(_) => {}
                            Err(err) => {
                                eprintln!("warn: inference error (skipping chunk): {err}");
                            }
                        }
                    });
                    if let Err(err) = push_result {
                        eprintln!("warn: resampler error: {err}");
                    }
                    if receiver_dropped {
                        return;
                    }
                }

                // Idle: maybe unload the engine to free VRAM.
                if let Some(unload_dur) = cfg.unload_after.filter(|d| !d.is_zero()) {
                    if cfg.captions_enabled.load(Ordering::Relaxed) {
                        disabled_since = None;
                    } else {
                        let since = disabled_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= unload_dur && engine.is_some() {
                            eprintln!(
                                "info: unloading STT engine from VRAM after {}s idle",
                                unload_dur.as_secs()
                            );
                            engine = None;
                            engine_built_for = None;
                        }
                    }
                }
            }
        })
        .expect("spawning stt-pipeline thread")
}

#[cfg(target_os = "linux")]
fn build_engine(
    choice: &Engine,
    model_dir: &std::path::Path,
    use_cuda: bool,
) -> Result<Box<dyn SttEngine>> {
    match choice {
        Engine::Nemotron => Ok(Box::new(nemotron::NemotronEngine::new(model_dir, use_cuda)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_wake_notify_wakes_waiter() {
        let w = Arc::new(AudioWake::new());
        let w2 = Arc::clone(&w);
        let handle = thread::spawn(move || {
            let start = Instant::now();
            w2.wait_timeout(Duration::from_secs(2));
            start.elapsed()
        });
        thread::sleep(Duration::from_millis(50));
        w.notify();
        let elapsed = handle.join().unwrap();
        assert!(elapsed < Duration::from_millis(500), "wait should return promptly on notify");
    }

    #[test]
    fn audio_wake_notify_before_wait_is_not_lost() {
        let w = AudioWake::new();
        w.notify();
        let start = Instant::now();
        let got = w.wait_timeout(Duration::from_secs(2));
        assert!(got, "wait should see prior notify");
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn audio_wake_shutdown_wakes_waiter() {
        let w = Arc::new(AudioWake::new());
        let w2 = Arc::clone(&w);
        let handle = thread::spawn(move || {
            w2.wait_timeout(Duration::from_secs(2));
        });
        thread::sleep(Duration::from_millis(50));
        w.shutdown();
        handle.join().unwrap();
        assert!(w.is_shutdown());
    }
}
