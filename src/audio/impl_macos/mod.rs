//! macOS audio capture using Core Audio Process Taps (Phase 5 revised).
//! Replaces ScreenCaptureKit-based capture with a more direct, lower-latency
//! mechanism that uses system Audio Capture permission instead of Screen Recording.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use ringbuf::HeapRb;
use ringbuf::traits::Split;

use crate::stt::AudioWake;
use crate::audio::FallbackEvent;

mod tap_processes;    // Core Audio process enumeration (Task 2, Phase 5 revised)
mod tap;              // Core Audio process tap RAII (Task 3, Phase 5 revised)
mod notify;           // UNUserNotificationCenter helper (Task 5, Phase 5 revised)

/// Commands sent to the audio thread.
pub enum AudioCommand {
    Shutdown,
    /// Switch to a different audio source, rebuilding the tap and aggregate device.
    SwitchSource(crate::config::AudioSource),
}

/// Error type distinguishing initial tap build failure from runtime failures.
#[derive(Debug)]
enum CaptureError {
    /// Initial tap construction failed (typically permission denied).
    InitialBuildFailed(anyhow::Error),
    /// Tap was running but failed later (app disappeared, rebuild failed, etc.).
    RuntimeFailure(anyhow::Error),
}

impl From<anyhow::Error> for CaptureError {
    fn from(e: anyhow::Error) -> Self {
        CaptureError::RuntimeFailure(e)
    }
}

/// User-visible message posted to the NSPanel when Audio Capture permission is denied.
/// Satisfies AC3.6's "in-panel message" branch.
const TCC_DENIED_PANEL_MESSAGE: &str =
    "Grant Audio Capture permission in System Settings → Privacy & Security, then relaunch.";

/// Public entry point — takes an initial source and returns a 3-tuple:
/// (command sender, ring consumer, fallback event receiver).
///
/// The fallback receiver emits `FallbackEvent` when a captured app exits and
/// the audio thread auto-switches to SystemOutput. The caller should spawn a
/// thread to drain this receiver for logging and future tray-state updates.
///
/// `error_caption_tx` is the same caption channel the STT pipeline writes
/// into; on TCC denial the audio thread posts a one-shot caption to it.
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
    audio_wake: Arc<AudioWake>,
    error_caption_tx: async_channel::Sender<String>,
) -> Result<(SyncSender<AudioCommand>, ringbuf::HeapCons<f32>, Receiver<FallbackEvent>)> {
    // Same capacity as Linux: 48000 frames × 2 channels = 96_000 f32 elements.
    const RING_BUF_CAPACITY: usize = 96_000;
    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(RING_BUF_CAPACITY).split();
    let (tx_cmd, rx_cmd) = sync_channel::<AudioCommand>(8);
    let (fallback_tx, fallback_rx) = sync_channel::<FallbackEvent>(8);

    // Wrap producer in Arc<Mutex<>> so the IOProc callback and the worker
    // thread can both reference it. RT-SAFE: the IOProc uses try_lock only.
    let ring_producer = Arc::new(Mutex::new(ring_producer));

    let producer_for_thread = Arc::clone(&ring_producer);
    let wake_for_thread = Arc::clone(&audio_wake);
    let initial_source_for_thread = initial_source.clone();
    std::thread::Builder::new()
        .name("audio-tap-worker".into())
        .spawn(move || {
            match run_tap_capture(
                initial_source_for_thread,
                producer_for_thread,
                wake_for_thread,
                rx_cmd,
                fallback_tx,
            ) {
                Ok(()) => {}
                Err(CaptureError::InitialBuildFailed(e)) => {
                    eprintln!("error: initial audio tap construction failed: {e:#}");
                    // Surface TCC denial message in the NSPanel.
                    let _ = error_caption_tx.send_blocking(TCC_DENIED_PANEL_MESSAGE.to_string());
                }
                Err(CaptureError::RuntimeFailure(e)) => {
                    eprintln!("error: audio tap capture exited: {e:#}");
                    // For runtime failures (rebuild failed, etc.), post a generic error message.
                    let _ = error_caption_tx.send_blocking(
                        format!("Audio capture failed: {e}")
                    );
                }
            }
        })?;

    Ok((tx_cmd, ring_consumer, fallback_rx))
}

fn run_tap_capture(
    initial_source: crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    audio_wake: Arc<AudioWake>,
    rx_cmd: Receiver<AudioCommand>,
    fallback_tx: SyncSender<FallbackEvent>,
) -> Result<(), CaptureError> {
    let mut current_source = initial_source.clone();
    let mut current_label = source_label(&current_source);

    // Build the initial tap.
    let mut tap = tap::AudioTap::build(
        tap_target_for(&current_source)?,
        Arc::clone(&ring_producer),
        Arc::clone(&audio_wake),
    ).map_err(|e| CaptureError::InitialBuildFailed(e.context("initial tap construction (Audio Capture permission denied?)")))?;

    // Watchdog: every 1 second, if we're capturing a specific process,
    // check if it's still running. On disappearance, fall back to SystemOutput.
    let watchdog_tick = std::time::Duration::from_secs(1);
    let mut last_tick = std::time::Instant::now();

    loop {
        // Short timeout (250ms) so we can interleave watchdog ticks.
        match rx_cmd.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(AudioCommand::Shutdown) => break,
            Ok(AudioCommand::SwitchSource(new_source)) => {
                // Validate the new source and build a tap for it.
                let new_target = match tap_target_for(&new_source) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("warn: cannot switch to {new_source:?}: {e}; staying on current");
                        continue;
                    }
                };
                // Rebuild: drop old tap (Drop tears down in correct order), build new.
                drop(tap);
                match tap::AudioTap::build(
                    new_target,
                    Arc::clone(&ring_producer),
                    Arc::clone(&audio_wake),
                ) {
                    Ok(new_tap) => {
                        tap = new_tap;
                        current_source = new_source;
                        current_label = source_label(&current_source);
                    }
                    Err(e) => {
                        eprintln!("error: tap rebuild on source switch failed: {e:#}");
                        let _ = fallback_tx.send(FallbackEvent {
                            previous_label: "Source switch".to_string(),
                            new_source: crate::config::AudioSource::SystemOutput,
                        });
                        // Rebuild for SystemOutput as fallback.
                        match tap::AudioTap::build(
                            tap::TapTarget::SystemMix,
                            Arc::clone(&ring_producer),
                            Arc::clone(&audio_wake),
                        ) {
                            Ok(new_tap) => {
                                tap = new_tap;
                                current_label = "System Output".to_string();
                            }
                            Err(e) => {
                                eprintln!("error: SystemOutput fallback rebuild failed: {e:#}; audio thread exiting");
                                return Err(CaptureError::RuntimeFailure(e));
                            }
                        }
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Run watchdog tick if we're capturing a specific process.
        if last_tick.elapsed() >= watchdog_tick {
            last_tick = std::time::Instant::now();
            if let Some(pid) = tap.captured_pid() {
                // Check if the process is still running via Core Audio registry.
                // Only trigger fallback if IsRunning is positively false, not on translate errors.
                let is_running = match tap_processes::translate_pid_to_process_object(pid) {
                    Ok(obj_id) => tap_processes::process_is_running(obj_id),
                    Err(e) => {
                        // Transient HAL hiccup; log and skip this tick rather than triggering fallback.
                        eprintln!("warn: translate_pid_to_process_object failed: {e}; skipping watchdog tick");
                        continue;
                    }
                };

                if !is_running {
                    // Source disappeared; fall back to SystemOutput.
                    eprintln!(
                        "info: audio source '{}' disappeared; switched to SystemOutput",
                        current_label
                    );

                    // Post a user notification about the disappearance.
                    let notification_msg = format!("'{}' stopped producing audio. Falling back to System Output.", current_label);
                    let _ = notify::post_user_notification(
                        "Subtidal: audio source unavailable",
                        &notification_msg,
                    );

                    // Notify the caller.
                    let _ = fallback_tx.send(FallbackEvent {
                        previous_label: current_label.clone(),
                        new_source: crate::config::AudioSource::SystemOutput,
                    });

                    // Try to rebuild for SystemOutput. If it fails, log and keep the old (now-dead)
                    // tap rather than killing the thread. The next SwitchSource has another chance.
                    drop(tap);
                    match tap::AudioTap::build(
                        tap::TapTarget::SystemMix,
                        Arc::clone(&ring_producer),
                        Arc::clone(&audio_wake),
                    ) {
                        Ok(new_tap) => {
                            tap = new_tap;
                            current_label = "System Output".to_string();
                        }
                        Err(e) => {
                            eprintln!("error: SystemMix rebuild after disappearance failed: {e:#}; audio thread will be silent until next SwitchSource");
                            let _ = fallback_tx.send(FallbackEvent {
                                previous_label: current_label.clone(),
                                new_source: crate::config::AudioSource::SystemOutput,
                            });
                            // Rebuild for the next attempt. If this also fails, the thread exits
                            // with an error rather than running with a dead tap forever.
                            match tap::AudioTap::build(
                                tap::TapTarget::SystemMix,
                                Arc::clone(&ring_producer),
                                Arc::clone(&audio_wake),
                            ) {
                                Ok(new_tap) => tap = new_tap,
                                Err(e) => {
                                    eprintln!("error: retry rebuild also failed: {e:#}; exiting audio thread");
                                    return Err(CaptureError::RuntimeFailure(e));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Map an AudioSource to a TapTarget (the raw capture target).
fn tap_target_for(src: &crate::config::AudioSource) -> Result<tap::TapTarget> {
    match src {
        crate::config::AudioSource::SystemOutput => Ok(tap::TapTarget::SystemMix),
        crate::config::AudioSource::Application { .. } => {
            // Linux variant; should not appear on macOS. Treat as SystemOutput.
            eprintln!("warn: AudioSource::Application is Linux-only; treating as SystemOutput");
            Ok(tap::TapTarget::SystemMix)
        }
        crate::config::AudioSource::App { bundle_id, .. } => {
            // macOS variant: enumerate processes and find the one matching bundle_id.
            let procs = tap_processes::enumerate_audio_processes()?;
            let proc = procs
                .iter()
                .find(|p| p.bundle_id.as_deref() == Some(bundle_id))
                .with_context(|| format!("app '{}' is not running", bundle_id))?;
            Ok(tap::TapTarget::Process { pid: proc.pid })
        }
    }
}

/// Convert an AudioSource to a user-visible label.
fn source_label(src: &crate::config::AudioSource) -> String {
    match src {
        crate::config::AudioSource::SystemOutput => "System Output".to_string(),
        crate::config::AudioSource::App { label, .. } => label.clone(),
        crate::config::AudioSource::Application { node_name, .. } => node_name.clone(),
    }
}

/// Public wrapper around notify::request_authorization_best_effort.
/// Exposed at the audio module level for main_macos.rs to call at startup.
pub fn notify_request_authorization_best_effort() {
    notify::request_authorization_best_effort();
}

/// Enumerate audio sources visible in the tray menu.
/// Returns SystemOutput plus one entry per running app with a non-nil bundle ID,
/// populated from Core Audio's process registry.
pub fn list_sources() -> Result<Vec<crate::audio::AudioSourceInfo>> {
    let mut out = vec![crate::audio::AudioSourceInfo {
        source: crate::config::AudioSource::SystemOutput,
        label: "System Output".to_string(),
    }];

    for proc in tap_processes::enumerate_audio_processes()? {
        if let Some(bundle) = proc.bundle_id {
            let label = bundle_to_label(&bundle);
            out.push(crate::audio::AudioSourceInfo {
                source: crate::config::AudioSource::App {
                    bundle_id: bundle,
                    label: label.clone(),
                },
                label,
            });
        }
    }

    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok(out)
}

/// Translate a bundle ID to a user-visible label.
/// For now, returns the bundle ID as-is. Future versions (Phase 6+) can
/// resolve to CFBundleName from the bundle's Info.plist for prettier labels.
fn bundle_to_label(bundle_id: &str) -> String {
    bundle_id.to_string()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::config::AudioSource;

    #[test]
    fn start_audio_thread_returns_three_tuple() {
        // Verify the signature of start_audio_thread: it returns a 3-tuple
        // (SyncSender<AudioCommand>, HeapCons<f32>, Receiver<FallbackEvent>).
        // This is a compile-time check; the function requires Audio Capture permission
        // to actually run, so we verify the types here.

        // If this compiles, the types are correct. We use a const function pointer
        // to enforce signature checking at compile time.
        const _: fn(
            crate::config::AudioSource,
            std::sync::Arc<crate::stt::AudioWake>,
            async_channel::Sender<String>,
        ) -> anyhow::Result<(
            std::sync::mpsc::SyncSender<crate::audio::AudioCommand>,
            ringbuf::HeapCons<f32>,
            std::sync::mpsc::Receiver<crate::audio::FallbackEvent>,
        )> = start_audio_thread;
    }

    #[test]
    #[ignore = "requires Screen Recording permission and a running graphical session"]
    fn list_sources_returns_system_output_plus_running_apps() {
        let sources = list_sources().expect("list_sources should succeed");
        assert!(
            sources.iter().any(|s| matches!(s.source, AudioSource::SystemOutput)),
            "SystemOutput must always appear",
        );
        assert!(
            sources.iter().any(|s| matches!(s.source, AudioSource::App { .. })),
            "at least one App entry expected on a typical desktop session",
        );
        assert!(sources.len() >= 2);
    }
}
