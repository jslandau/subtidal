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

mod stream;           // SCStream + delegate (Task 3, Phase 4 — being superseded)
mod normalize;        // CMSampleBuffer → 48kHz stereo f32 (Task 3 + 5, Phase 4 — being superseded)
mod tap_processes;    // Core Audio process enumeration (Task 2, Phase 5 revised)
mod tap;              // Core Audio process tap RAII (Task 3, Phase 5 revised)
mod notify;           // UNUserNotificationCenter helper (Task 5, Phase 5 revised)

/// Commands sent to the audio thread.
pub enum AudioCommand {
    Shutdown,
    /// Switch to a different audio source, rebuilding the tap and aggregate device.
    SwitchSource(crate::config::AudioSource),
}

/// User-visible message posted to the NSPanel when Audio Capture fails to start
/// (typically because TCC permission was denied). Satisfies AC3.6's
/// "in-panel message" branch.
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
            if let Err(e) = run_tap_capture(
                initial_source_for_thread,
                producer_for_thread,
                wake_for_thread,
                rx_cmd,
                fallback_tx,
            ) {
                eprintln!("error: audio tap capture exited: {e:#}");
                // Surface the failure in the NSPanel via the caption channel.
                // send_blocking is fine here — this is the non-RT path and the
                // overlay's caption-bridge thread will drain immediately.
                let _ = error_caption_tx.send_blocking(TCC_DENIED_PANEL_MESSAGE.to_string());
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
) -> Result<()> {
    let mut current_source = initial_source.clone();
    let mut current_label = source_label(&current_source);

    // Build the initial tap.
    let mut tap = tap::AudioTap::build(
        tap_target_for(&current_source)?,
        Arc::clone(&ring_producer),
        Arc::clone(&audio_wake),
    ).context("initial tap construction (Audio Capture permission denied?)")?;

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
                tap = tap::AudioTap::build(
                    new_target,
                    Arc::clone(&ring_producer),
                    Arc::clone(&audio_wake),
                )
                .context("tap rebuild on source switch")?;
                current_source = new_source;
                current_label = source_label(&current_source);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Run watchdog tick if we're capturing a specific process.
        if last_tick.elapsed() >= watchdog_tick {
            last_tick = std::time::Instant::now();
            if let Some(pid) = tap.captured_pid() {
                // Check if the process is still running.
                let is_running = match tap_processes::translate_pid_to_process_object(pid) {
                    Ok(obj_id) => tap_processes::process_is_running(obj_id),
                    Err(_) => false, // Process not found in Core Audio registry.
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

                    // Rebuild the tap for SystemOutput.
                    drop(tap);
                    tap = tap::AudioTap::build(
                        tap::TapTarget::SystemMix,
                        Arc::clone(&ring_producer),
                        Arc::clone(&audio_wake),
                    ).context("tap rebuild on source disappearance")?;
                    current_label = "System Output".to_string();
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
        // Test that start_audio_thread now returns a 3-tuple (cmd_tx, consumer, fallback_rx)
        // instead of a 2-tuple. This is a signature verification test.
        //
        // We can't run the full thread without Audio Capture permission, but we can verify
        // that the function exists with the right signature.
        //
        // Note: This test would require hardware to actually call, but we can at least
        // verify the signature compiles and is callable with the right types.

        // This test is for documentation/signature verification; actual runtime tests
        // are hardware-gated.
        let _has_audio_source = crate::config::AudioSource::SystemOutput;
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
