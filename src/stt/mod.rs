//! STT engine abstraction and the combined audio-resample-inference thread.

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod nemotron;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod diarization;

use anyhow::Result;
use arc_swap::ArcSwap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use ringbuf::traits::Consumer;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use ringbuf::HeapCons;
use std::collections::VecDeque;
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
    /// Additional caption holdback used for release-time speaker assignment.
    /// Startup-only in this iteration; changing it requires an app restart.
    pub diarization_display_delay_ms: u64,
    /// Estimated STT↔diarization alignment lag used to map emitted text onto
    /// the recent Sortformer segment timeline. Startup-only; requires restart.
    pub diarization_alignment_lag_ms: u64,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
const RELABEL_LOOKBACK_SAMPLES: u64 = 24_000;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const TIMELINE_GAP_TOLERANCE_SAMPLES: u64 = 8_000;
#[cfg(any(target_os = "linux", target_os = "macos"))]
const TIMELINE_RETENTION_MARGIN_SAMPLES: u64 = 8_000;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingCaption {
    text: String,
    emit_sample: u64,
    release_sample: u64,
    release_at: Instant,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct RecentSpeakerSegment {
    speaker_id: u32,
    start_sample: u64,
    end_sample: u64,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ms_to_samples(ms: u64) -> u64 {
    ms.saturating_mul(16)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn assign_speaker_from_timeline(
    target_sample: u64,
    segments: &VecDeque<RecentSpeakerSegment>,
) -> Option<u32> {
    for seg in segments.iter().rev() {
        if seg.start_sample <= target_sample && target_sample < seg.end_sample {
            return Some(seg.speaker_id);
        }
    }

    let mut best: Option<(u64, u64, u32)> = None;
    for seg in segments {
        let gap = if target_sample < seg.start_sample {
            seg.start_sample - target_sample
        } else {
            target_sample.saturating_sub(seg.end_sample)
        };
        if gap > TIMELINE_GAP_TOLERANCE_SAMPLES {
            continue;
        }
        let candidate = (gap, seg.end_sample, seg.speaker_id);
        if best
            .as_ref()
            .map(|(best_gap, best_end, _)| {
                gap < *best_gap || (gap == *best_gap && seg.end_sample >= *best_end)
            })
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, speaker_id)| speaker_id)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn trim_recent_segments(segments: &mut VecDeque<RecentSpeakerSegment>, keep_from_sample: u64) {
    while segments
        .front()
        .map(|seg| seg.end_sample < keep_from_sample)
        .unwrap_or(false)
    {
        segments.pop_front();
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn reset_diarization_state(
    current_speaker: &mut Option<u32>,
    samples_fed_to_diar: &mut u64,
    current_speaker_last_end: &mut u64,
    pending_captions: &mut VecDeque<PendingCaption>,
    recent_segments: &mut VecDeque<RecentSpeakerSegment>,
) {
    *current_speaker = None;
    *samples_fed_to_diar = 0;
    *current_speaker_last_end = 0;
    pending_captions.clear();
    recent_segments.clear();
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn flush_pending_captions(
    caption_tx: &async_channel::Sender<CaptionEvent>,
    pending: &mut VecDeque<PendingCaption>,
    segments: &VecDeque<RecentSpeakerSegment>,
    current_speaker: Option<u32>,
    alignment_lag_samples: u64,
    now: Instant,
    force_all: bool,
) -> bool {
    while pending
        .front()
        .map(|p| force_all || p.release_at <= now)
        .unwrap_or(false)
    {
        let pending_caption = pending.pop_front().expect("front checked above");
        let target_sample = pending_caption
            .emit_sample
            .saturating_sub(alignment_lag_samples);
        let speaker_id = assign_speaker_from_timeline(target_sample, segments).or_else(|| {
            if segments.is_empty() {
                eprintln!(
                    "info: diarization: no segment history for caption at sample {}, falling back to current speaker",
                    pending_caption.emit_sample
                );
            }
            current_speaker
        });
        let event = CaptionEvent::Append {
            text: pending_caption.text,
            speaker_id,
            emit_sample: pending_caption.emit_sample,
        };
        if caption_tx.try_send(event).is_err() {
            return true;
        }
    }
    false
}

/// Spawn the combined audio→resample→inference thread.
///
/// Replaces the old pair (bridge thread + inference thread) and their channel.
/// Reads directly from the ring buffer, resamples, dispatches to the current
/// engine (read lock-free via `ArcSwap`), and sends recognised text via
/// `async-channel` to the UI main loop.
///
/// Engine swap is handled by swapping the `ArcSwap<Engine>` from the tray; this
/// thread notices on each chunk boundary and rebuilds its local engine.
///
/// When diarization is enabled (`cfg.diarization_enabled`), each resampled
/// chunk is also fed to a `DiarizationEngine`. Recognized text is held briefly,
/// then assigned a speaker at release time by matching its estimated acoustic
/// position against a rolling Sortformer segment timeline.
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

            let display_delay_samples = ms_to_samples(cfg.diarization_display_delay_ms);
            let alignment_lag_samples = ms_to_samples(cfg.diarization_alignment_lag_ms);
            let segment_retention_samples = display_delay_samples
                .max(alignment_lag_samples)
                .max(RELABEL_LOOKBACK_SAMPLES)
                .saturating_add(TIMELINE_RETENTION_MARGIN_SAMPLES);

            // Local engine + tracking of the engine choice it was built from.
            let mut engine: Option<Box<dyn SttEngine>> = None;
            let mut engine_built_for: Option<Engine> = None;
            let mut disabled_since: Option<Instant> = None;

            // Diarization engine (lazy — built when first enabled, dropped when disabled).
            let mut diar_engine: Option<diarization::DiarizationEngine> = None;
            let mut diar_was_enabled = cfg.diarization_enabled.load(Ordering::Relaxed);
            let mut current_speaker: Option<u32> = None;
            let mut samples_fed_to_diar: u64 = 0;
            let mut current_speaker_last_end: u64 = 0;
            let mut pending_captions: VecDeque<PendingCaption> = VecDeque::new();
            let mut recent_segments: VecDeque<RecentSpeakerSegment> = VecDeque::new();

            if diar_was_enabled {
                eprintln!("info: diarization enabled at startup, loading Sortformer engine");
                match diarization::DiarizationEngine::new(
                    &cfg.diarization_model_dir,
                    cfg.use_cuda,
                    &cfg.diarization_preset,
                ) {
                    Ok(d) => {
                        diar_engine = Some(d);
                        reset_diarization_state(
                            &mut current_speaker,
                            &mut samples_fed_to_diar,
                            &mut current_speaker_last_end,
                            &mut pending_captions,
                            &mut recent_segments,
                        );
                    }
                    Err(e) => {
                        eprintln!("warn: failed to build Sortformer engine at startup: {e:#}");
                    }
                }
            }

            let mut raw = vec![0f32; 8192];

            loop {
                if wake.is_shutdown() {
                    let _ = flush_pending_captions(
                        &caption_tx,
                        &mut pending_captions,
                        &recent_segments,
                        current_speaker,
                        alignment_lag_samples,
                        Instant::now(),
                        true,
                    );
                    break;
                }

                wake.wait_timeout(Duration::from_millis(250));

                if wake.is_shutdown() {
                    let _ = flush_pending_captions(
                        &caption_tx,
                        &mut pending_captions,
                        &recent_segments,
                        current_speaker,
                        alignment_lag_samples,
                        Instant::now(),
                        true,
                    );
                    break;
                }

                let diar_now_enabled = cfg.diarization_enabled.load(Ordering::Relaxed);
                if diar_now_enabled && !diar_was_enabled {
                    eprintln!("info: diarization enabled, loading Sortformer engine");
                    match diarization::DiarizationEngine::new(
                        &cfg.diarization_model_dir,
                        cfg.use_cuda,
                        &cfg.diarization_preset,
                    ) {
                        Ok(d) => {
                            diar_engine = Some(d);
                            reset_diarization_state(
                                &mut current_speaker,
                                &mut samples_fed_to_diar,
                                &mut current_speaker_last_end,
                                &mut pending_captions,
                                &mut recent_segments,
                            );
                        }
                        Err(e) => {
                            eprintln!("warn: failed to build Sortformer engine: {e:#}");
                        }
                    }
                } else if !diar_now_enabled && diar_was_enabled {
                    eprintln!("info: diarization disabled, unloading Sortformer engine");
                    diar_engine = None;
                    reset_diarization_state(
                        &mut current_speaker,
                        &mut samples_fed_to_diar,
                        &mut current_speaker_last_end,
                        &mut pending_captions,
                        &mut recent_segments,
                    );
                }
                diar_was_enabled = diar_now_enabled;

                loop {
                    let n = ring_consumer.pop_slice(&mut raw);
                    if n == 0 {
                        break;
                    }

                    if !cfg.captions_enabled.load(Ordering::Relaxed) {
                        continue;
                    }

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

                    let mut receiver_dropped = false;
                    let push_result = resampler.push_interleaved(&raw[..n], |chunk| {
                        if receiver_dropped {
                            return;
                        }

                        if let Some(ref mut diar) = diar_engine {
                            samples_fed_to_diar += chunk.len() as u64;
                            match diar.process_chunk(chunk) {
                                Ok(Some(result)) => {
                                    for seg in &result.segments {
                                        recent_segments.push_back(RecentSpeakerSegment {
                                            speaker_id: seg.speaker_id,
                                            start_sample: seg.start_sample as u64,
                                            end_sample: seg.end_sample as u64,
                                        });
                                    }
                                    trim_recent_segments(
                                        &mut recent_segments,
                                        samples_fed_to_diar.saturating_sub(segment_retention_samples),
                                    );

                                    let latest_seg = result.segments.iter().max_by_key(|s| s.end_sample);
                                    if let Some(seg) = latest_seg {
                                        let new_speaker = seg.speaker_id;
                                        let old_speaker_last_end = current_speaker
                                            .and_then(|csid| {
                                                result.segments
                                                    .iter()
                                                    .filter(|s| s.speaker_id == csid)
                                                    .map(|s| s.end_sample as u64)
                                                    .max()
                                            })
                                            .unwrap_or(current_speaker_last_end);
                                        if current_speaker.is_some()
                                            && current_speaker != Some(new_speaker)
                                        {
                                            let raw_from = if old_speaker_last_end > 0 {
                                                old_speaker_last_end.min(seg.start_sample as u64)
                                            } else {
                                                seg.start_sample as u64
                                            };
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
                                        current_speaker_last_end = seg.end_sample as u64;
                                    }

                                    if flush_pending_captions(
                                        &caption_tx,
                                        &mut pending_captions,
                                        &recent_segments,
                                        current_speaker,
                                        alignment_lag_samples,
                                        Instant::now(),
                                        false,
                                    ) {
                                        receiver_dropped = true;
                                        return;
                                    }
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    eprintln!("warn: diarization error (skipping chunk): {err}");
                                }
                            }
                        }

                        match e.process_chunk(chunk) {
                            Ok(Some(text)) if !text.trim().is_empty() => {
                                if diar_engine.is_some() {
                                    let now = Instant::now();
                                    let emit_sample = samples_fed_to_diar;
                                    pending_captions.push_back(PendingCaption {
                                        text,
                                        emit_sample,
                                        release_sample: emit_sample.saturating_add(display_delay_samples),
                                        release_at: now + Duration::from_millis(cfg.diarization_display_delay_ms),
                                    });
                                    if flush_pending_captions(
                                        &caption_tx,
                                        &mut pending_captions,
                                        &recent_segments,
                                        current_speaker,
                                        alignment_lag_samples,
                                        now,
                                        false,
                                    ) {
                                        receiver_dropped = true;
                                    }
                                } else {
                                    let event = CaptionEvent::Append {
                                        text,
                                        speaker_id: None,
                                        emit_sample: samples_fed_to_diar,
                                    };
                                    if caption_tx.try_send(event).is_err() {
                                        receiver_dropped = true;
                                    }
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
                        let _ = flush_pending_captions(
                            &caption_tx,
                            &mut pending_captions,
                            &recent_segments,
                            current_speaker,
                            alignment_lag_samples,
                            Instant::now(),
                            true,
                        );
                        return;
                    }

                    if flush_pending_captions(
                        &caption_tx,
                        &mut pending_captions,
                        &recent_segments,
                        current_speaker,
                        alignment_lag_samples,
                        Instant::now(),
                        false,
                    ) {
                        return;
                    }
                }

                if flush_pending_captions(
                    &caption_tx,
                    &mut pending_captions,
                    &recent_segments,
                    current_speaker,
                    alignment_lag_samples,
                    Instant::now(),
                    false,
                ) {
                    return;
                }

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
        Engine::Nemotron => Ok(Box::new(nemotron::NemotronEngine::new(
            model_dir, use_cuda,
        )?)),
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
        assert!(
            elapsed < Duration::from_millis(500),
            "wait should return promptly on notify"
        );
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

    #[test]
    fn assign_speaker_from_timeline_prefers_containing_segment() {
        let segments = VecDeque::from(vec![
            RecentSpeakerSegment {
                speaker_id: 0,
                start_sample: 0,
                end_sample: 10_000,
            },
            RecentSpeakerSegment {
                speaker_id: 1,
                start_sample: 10_000,
                end_sample: 20_000,
            },
        ]);
        assert_eq!(assign_speaker_from_timeline(15_000, &segments), Some(1));
    }

    #[test]
    fn assign_speaker_from_timeline_uses_nearby_gap_tolerance() {
        let segments = VecDeque::from(vec![RecentSpeakerSegment {
            speaker_id: 2,
            start_sample: 20_000,
            end_sample: 30_000,
        }]);
        assert_eq!(assign_speaker_from_timeline(18_500, &segments), Some(2));
    }

    #[test]
    fn assign_speaker_from_timeline_returns_none_when_gap_too_large() {
        let segments = VecDeque::from(vec![RecentSpeakerSegment {
            speaker_id: 2,
            start_sample: 20_000,
            end_sample: 30_000,
        }]);
        assert_eq!(assign_speaker_from_timeline(5_000, &segments), None);
    }

    #[test]
    fn flush_pending_captions_emits_fifo_with_timeline_speakers() {
        let (tx, rx) = async_channel::unbounded();
        let mut pending = VecDeque::from(vec![
            PendingCaption {
                text: "first".to_string(),
                emit_sample: 10_000,
                release_sample: 28_000,
                release_at: Instant::now() - Duration::from_millis(10),
            },
            PendingCaption {
                text: "second".to_string(),
                emit_sample: 30_000,
                release_sample: 48_000,
                release_at: Instant::now() - Duration::from_millis(10),
            },
        ]);
        let segments = VecDeque::from(vec![
            RecentSpeakerSegment {
                speaker_id: 0,
                start_sample: 0,
                end_sample: 20_000,
            },
            RecentSpeakerSegment {
                speaker_id: 1,
                start_sample: 20_000,
                end_sample: 40_000,
            },
        ]);

        let dropped = flush_pending_captions(
            &tx,
            &mut pending,
            &segments,
            None,
            0,
            Instant::now(),
            false,
        );
        assert!(!dropped);
        assert!(pending.is_empty());

        match rx.try_recv().unwrap() {
            CaptionEvent::Append { text, speaker_id, .. } => {
                assert_eq!(text, "first");
                assert_eq!(speaker_id, Some(0));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        match rx.try_recv().unwrap() {
            CaptionEvent::Append { text, speaker_id, .. } => {
                assert_eq!(text, "second");
                assert_eq!(speaker_id, Some(1));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn flush_pending_captions_falls_back_to_current_speaker_without_history() {
        let (tx, rx) = async_channel::unbounded();
        let mut pending = VecDeque::from(vec![PendingCaption {
            text: "fallback".to_string(),
            emit_sample: 10_000,
            release_sample: 28_000,
            release_at: Instant::now() - Duration::from_millis(10),
        }]);
        let segments = VecDeque::new();

        let dropped = flush_pending_captions(
            &tx,
            &mut pending,
            &segments,
            Some(3),
            0,
            Instant::now(),
            false,
        );
        assert!(!dropped);
        match rx.try_recv().unwrap() {
            CaptionEvent::Append { speaker_id, .. } => assert_eq!(speaker_id, Some(3)),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn trim_recent_segments_discards_old_history() {
        let mut segments = VecDeque::from(vec![
            RecentSpeakerSegment {
                speaker_id: 0,
                start_sample: 0,
                end_sample: 5_000,
            },
            RecentSpeakerSegment {
                speaker_id: 1,
                start_sample: 5_000,
                end_sample: 15_000,
            },
        ]);
        trim_recent_segments(&mut segments, 6_000);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments.front().unwrap().speaker_id, 1);
    }
}
