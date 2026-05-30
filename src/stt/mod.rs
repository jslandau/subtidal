//! STT engine abstraction and the combined audio-resample-inference thread.

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod nemotron;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod diarization;

use anyhow::Result;
use arc_swap::ArcSwap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use ringbuf::HeapCons;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use ringbuf::traits::Consumer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::thread;
use std::time::Duration;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Instant;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::audio::resampler::AudioResampler;
use crate::config::Engine;

use crate::overlay::CaptionEvent;

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
#[derive(Default)]
pub struct AudioWake {
    data_ready: AtomicBool,
    shutdown: AtomicBool,
    // `mutex` and `wait_timeout` are only invoked from the cfg-gated STT
    // pipeline thread; on non-Linux they're carried for structural parity but
    // never touched. Tests on Linux exercise them.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    mutex: Mutex<()>,
    condvar: Condvar,
}

impl AudioWake {
    pub fn new() -> Self {
        Self::default()
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
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
    /// Whether speaker diarization is enabled. The Sortformer engine is
    /// built lazily when this is first set to true.
    pub diarization_enabled: Arc<AtomicBool>,
    /// Diarization quality preset (callhome, dihard3, custom).
    pub diarization_preset: crate::config::DiarizationPreset,
    /// Directory for Sortformer model files (separate from STT model dir).
    pub diarization_model_dir: std::path::PathBuf,
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
///
/// When diarization is enabled (`cfg.diarization_enabled`), each resampled
/// chunk is also fed to a `DiarizationEngine`. The dominant speaker from the
/// most recent Sortformer output is attached to every caption as
/// `CaptionEvent::speaker_id`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn spawn_stt_thread(
    mut ring_consumer: HeapCons<f32>,
    wake: Arc<AudioWake>,
    caption_tx: async_channel::Sender<CaptionEvent>,
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

            // Diarization engine (lazy — built when first enabled, dropped when disabled).
            let mut diar_engine: Option<diarization::DiarizationEngine> = None;
            // Track whether we were previously diarizing so we can detect toggle-on/off.
            let mut diar_was_enabled = cfg.diarization_enabled.load(Ordering::Relaxed);
            // The most recent dominant speaker from Sortformer, applied to each caption.
            let mut current_speaker: Option<u32> = None;
            // Running count of 16kHz mono samples fed to the diarization engine
            // (in the engine's own reference frame — resets to 0 whenever
            // diar_engine is rebuilt, mirroring Sortformer's `elapsed_samples`).
            // Stamped onto every emitted CaptionEvent::Append so a later
            // CaptionEvent::Relabel can identify which captions to rewrite.
            let mut samples_fed_to_diar: u64 = 0;
            // Sample offset (in Sortformer's frame) of the END of the current
            // speaker's most recent observed segment. When the speaker changes,
            // this is the earliest sample at which the NEW speaker can have
            // started — anything later than this and before the new segment's
            // reported start belongs to the new speaker (or silence, which we
            // attribute to the new speaker as the best available guess).
            //
            // Callhome's `min_duration_on=0.511s` filter drops sub-half-second
            // speaker turns, so the new speaker's first reported segment can
            // arrive ~1s after they actually started talking. Using
            // current_speaker_last_end as the relabel boundary captures the
            // captions emitted during that gap; using only seg.start_sample
            // misses them.
            let mut current_speaker_last_end: u64 = 0;

            // If diarization is enabled at startup, build the Sortformer engine immediately.
            if diar_was_enabled {
                eprintln!("info: diarization enabled at startup, loading Sortformer engine");
                match diarization::DiarizationEngine::new(
                    &cfg.diarization_model_dir,
                    cfg.use_cuda,
                    &cfg.diarization_preset,
                ) {
                    Ok(d) => {
                        diar_engine = Some(d);
                    }
                    Err(e) => {
                        eprintln!("warn: failed to build Sortformer engine at startup: {e:#}");
                        // Leave diar_engine as None; captions will have no speaker labels.
                    }
                }
            }

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

                // Check for diarization toggle changes.
                let diar_now_enabled = cfg.diarization_enabled.load(Ordering::Relaxed);
                if diar_now_enabled && !diar_was_enabled {
                    // Diarization just turned on — build the Sortformer engine.
                    eprintln!("info: diarization enabled, loading Sortformer engine");
                    match diarization::DiarizationEngine::new(
                        &cfg.diarization_model_dir,
                        cfg.use_cuda,
                        &cfg.diarization_preset,
                    ) {
                        Ok(d) => {
                            diar_engine = Some(d);
                            current_speaker = None;
                        }
                        Err(e) => {
                            eprintln!("warn: failed to build Sortformer engine: {e:#}");
                            // Leave diar_engine as None; captions will have no speaker labels.
                        }
                    }
                } else if !diar_now_enabled && diar_was_enabled {
                    // Diarization just turned off — drop the engine to free VRAM.
                    eprintln!("info: diarization disabled, unloading Sortformer engine");
                    diar_engine = None;
                    current_speaker = None;
                    samples_fed_to_diar = 0;
                    current_speaker_last_end = 0;
                }
                if diar_now_enabled && diar_engine.is_some() && !diar_was_enabled {
                    // Engine was just (re)built — its elapsed_samples is 0; mirror that.
                    samples_fed_to_diar = 0;
                }
                diar_was_enabled = diar_now_enabled;

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

                        // Feed diarization engine if active. Sortformer accumulates
                        // audio internally and only emits segments when it has a
                        // full window — so this is conceptually "advance the
                        // sample clock by `chunk.len()` and maybe receive a verdict
                        // about a past window."
                        if let Some(ref mut diar) = diar_engine {
                            // Account for the samples about to be fed BEFORE the
                            // call, so the post-call counter matches Sortformer's
                            // elapsed_samples (segment offsets are in the same
                            // frame of reference).
                            samples_fed_to_diar += chunk.len() as u64;
                            match diar.process_chunk(chunk) {
                                Ok(Some(result)) => {
                                    // Pick the speaker whose segment END is
                                    // most recent — i.e. who was talking last.
                                    // Using end (not start) handles the case
                                    // where the new speaker's segment is short
                                    // and starts after a long old-speaker
                                    // segment in the same window.
                                    let latest_seg = result.segments
                                        .iter()
                                        .max_by_key(|s| s.end_sample);
                                    if let Some(seg) = latest_seg {
                                        let new_speaker = seg.speaker_id;
                                        // Update old speaker's last-known end
                                        // sample BEFORE deciding the relabel
                                        // boundary. Find the latest end of
                                        // ANY segment belonging to the
                                        // previously-current speaker in this
                                        // window — that's the boundary past
                                        // which captions belong to the new
                                        // speaker.
                                        let old_speaker_last_end = current_speaker
                                            .and_then(|csid| {
                                                result.segments
                                                    .iter()
                                                    .filter(|s| s.speaker_id == csid)
                                                    .map(|s| s.end_sample as u64)
                                                    .max()
                                            })
                                            .unwrap_or(current_speaker_last_end);
                                        // Only emit Relabel for transitions
                                        // between detected speakers — NOT for
                                        // the first detection. The first
                                        // detection sets current_speaker; from
                                        // then on, captions arrive tagged with
                                        // a speaker_id and get labeled at push
                                        // time by CaptionBuffer's speaker-change
                                        // detection. A first-detection Relabel
                                        // would silently rewrite all pre-
                                        // diarization captions to the detected
                                        // speaker (without adding labels,
                                        // because the old "speaker" is None and
                                        // there's nothing to substitute), and
                                        // then prime last_speaker_id so no
                                        // subsequent caption ever sees a
                                        // speaker change — wiping out all
                                        // labels.
                                        if current_speaker.is_some()
                                            && current_speaker != Some(new_speaker)
                                        {
                                            // Boundary: prefer the OLD speaker's
                                            // last-seen end if it's a tighter
                                            // bound than seg.start (it usually
                                            // is, because min_duration_on
                                            // discards the new speaker's first
                                            // ~500ms — the actual switch happened
                                            // when the old speaker stopped, not
                                            // when the new speaker became
                                            // detectable).
                                            let raw_from = if old_speaker_last_end > 0 {
                                                old_speaker_last_end.min(seg.start_sample as u64)
                                            } else {
                                                seg.start_sample as u64
                                            };
                                            // Clamp the relabel window to
                                            // RELABEL_LOOKBACK_SAMPLES (~2s at
                                            // 16kHz) before the new segment's
                                            // start. Bounds damage when
                                            // Sortformer misses an in-between
                                            // speaker entirely.
                                            const RELABEL_LOOKBACK_SAMPLES: u64 = 32_000;
                                            let lookback_floor = (seg.start_sample as u64)
                                                .saturating_sub(RELABEL_LOOKBACK_SAMPLES);
                                            let from_sample = raw_from.max(lookback_floor);
                                            eprintln!(
                                                "info: diarization: speaker {} -> {} \
                                                 (new segment start {}, old speaker last end {}, relabel from {})",
                                                current_speaker.map(|s| (s + 1) as i64).unwrap_or(-1),
                                                new_speaker + 1,
                                                seg.start_sample,
                                                old_speaker_last_end,
                                                from_sample,
                                            );
                                            let relabel = CaptionEvent::Relabel {
                                                from_sample,
                                                new_speaker_id: new_speaker,
                                            };
                                            if caption_tx.try_send(relabel).is_err() {
                                                receiver_dropped = true;
                                                return;
                                            }
                                        } else if current_speaker.is_none() {
                                            eprintln!(
                                                "info: diarization: first speaker detected as Speaker {}",
                                                new_speaker + 1,
                                            );
                                        }
                                        current_speaker = Some(new_speaker);
                                        // Record this segment's end as the new
                                        // current speaker's last-known end.
                                        current_speaker_last_end = seg.end_sample as u64;
                                    }
                                }
                                Ok(None) => {
                                    // Not enough audio yet — keep the previous speaker label.
                                }
                                Err(err) => {
                                    eprintln!("warn: diarization error (skipping chunk): {err}");
                                }
                            }
                        }

                        match e.process_chunk(chunk) {
                            Ok(Some(text)) if !text.trim().is_empty() => {
                                let speaker_id = if diar_engine.is_some() {
                                    current_speaker
                                } else {
                                    None
                                };
                                let event = CaptionEvent::Append {
                                    text,
                                    speaker_id,
                                    emit_sample: samples_fed_to_diar,
                                };
                                if caption_tx.try_send(event).is_err() {
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn build_engine(
    choice: &Engine,
    model_dir: &std::path::Path,
    use_cuda: bool,
) -> Result<Box<dyn SttEngine>> {
    match choice {
        Engine::Nemotron => Ok(Box::new(nemotron::NemotronEngine::new(model_dir, use_cuda)?)),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Instant;

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
