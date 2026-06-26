//! macOS audio capture using Core Audio Process Taps (Phase 5 revised).
//! Replaces ScreenCaptureKit-based capture with a more direct, lower-latency
//! mechanism that uses system Audio Capture permission instead of Screen Recording.

use anyhow::Result;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use ringbuf::traits::Split;
use ringbuf::HeapRb;

use crate::audio::FallbackEvent;
use crate::stt::AudioWake;

mod notify;
mod tap; // Core Audio process tap RAII (Task 3, Phase 5 revised)
mod tap_processes; // Core Audio process enumeration (Task 2, Phase 5 revised) // UNUserNotificationCenter helper (Task 5, Phase 5 revised)

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
/// `error_caption_tx` is the same caption-event channel the STT pipeline writes
/// into; on TCC denial the audio thread posts a one-shot non-diarized caption to it.
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
    audio_wake: Arc<AudioWake>,
    error_caption_tx: async_channel::Sender<crate::overlay::CaptionEvent>,
) -> Result<(
    SyncSender<AudioCommand>,
    ringbuf::HeapCons<f32>,
    Receiver<FallbackEvent>,
)> {
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
                    let _ = error_caption_tx.send_blocking(crate::overlay::CaptionEvent::Append {
                        text: TCC_DENIED_PANEL_MESSAGE.to_string(),
                        speaker_id: None,
                        emit_sample: 0,
                    });
                }
                Err(CaptureError::RuntimeFailure(e)) => {
                    eprintln!("error: audio tap capture exited: {e:#}");
                    // For runtime failures (rebuild failed, etc.), post a generic error message.
                    let _ = error_caption_tx.send_blocking(crate::overlay::CaptionEvent::Append {
                        text: format!("Audio capture failed: {e}"),
                        speaker_id: None,
                        emit_sample: 0,
                    });
                }
            }
        })?;

    Ok((tx_cmd, ring_consumer, fallback_rx))
}

#[allow(unused_assignments)]
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
    )
    .map_err(|e| {
        CaptureError::InitialBuildFailed(
            e.context("initial tap construction (Audio Capture permission denied?)"),
        )
    })?;

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
                                current_source = crate::config::AudioSource::SystemOutput;
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

        // Run watchdog tick if we're capturing specific processes.
        if last_tick.elapsed() >= watchdog_tick {
            last_tick = std::time::Instant::now();
            let pids = tap.captured_pids();
            if !pids.is_empty() {
                // Use POSIX kill(pid, 0) to test process liveness — Core Audio's
                // kAudioProcessPropertyIsRunning means "audio I/O active right now",
                // not "process exists", so it false-positives on every pause.
                // Process is gone iff kill returns -1 with errno == ESRCH.
                let any_alive = pids.iter().any(|&pid| {
                    // SAFETY: kill is async-signal-safe and reading errno is fine.
                    let rc = unsafe { libc::kill(pid, 0) };
                    rc == 0 || unsafe { *libc::__error() } != libc::ESRCH
                });

                if !any_alive {
                    // Source disappeared; fall back to SystemOutput.
                    eprintln!(
                        "info: audio source '{}' disappeared; switched to SystemOutput",
                        current_label
                    );

                    // Post a user notification about the disappearance.
                    let notification_msg = format!(
                        "'{}' stopped producing audio. Falling back to System Output.",
                        current_label
                    );
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
                            current_source = crate::config::AudioSource::SystemOutput;
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
                                Ok(new_tap) => {
                                    tap = new_tap;
                                    current_source = crate::config::AudioSource::SystemOutput;
                                    current_label = "System Output".to_string();
                                }
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
        crate::config::AudioSource::App {
            bundle_id,
            capture_bundle_ids,
            ..
        } => {
            // A tray row may represent a user-facing app plus multiple raw Core
            // Audio helper bundle IDs. Legacy configs have an empty
            // capture_bundle_ids list, which means “capture bundle_id”.
            let target_bundle_ids: Vec<&str> = if capture_bundle_ids.is_empty() {
                vec![bundle_id.as_str()]
            } else {
                capture_bundle_ids.iter().map(String::as_str).collect()
            };

            let procs = tap_processes::enumerate_audio_processes()?;
            let matches: Vec<_> = procs
                .into_iter()
                .filter(|p| {
                    p.bundle_id
                        .as_deref()
                        .is_some_and(|id| target_bundle_ids.contains(&id))
                })
                .collect();
            if matches.is_empty() {
                anyhow::bail!(
                    "no audio-producing process for app '{}' (requested capture bundle IDs: [{}])",
                    bundle_id,
                    target_bundle_ids.join(", ")
                );
            }
            let object_ids = matches.iter().map(|p| p.audio_object_id).collect();
            let watchdog_pids = matches.iter().map(|p| p.pid).collect();
            Ok(tap::TapTarget::Processes {
                object_ids,
                watchdog_pids,
            })
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
///
/// Returns exactly one SystemOutput row followed by user-facing grouped app
/// choices. Core Audio still reports raw helper/background processes; this
/// function filters obvious non-user-selectable system/self entries and groups
/// helper bundle IDs into the visible parent app while preserving those raw IDs
/// in `AudioSource::App::capture_bundle_ids` for reliable tap construction.
pub fn list_sources() -> Result<Vec<crate::audio::AudioSourceInfo>> {
    let raw_bundle_ids: Vec<String> = tap_processes::enumerate_audio_processes()?
        .into_iter()
        .filter_map(|p| p.bundle_id)
        .collect();
    Ok(build_source_infos(raw_bundle_ids, bundle_to_label))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MacSourceGroup {
    label: String,
    primary_bundle_id: String,
    capture_bundle_ids: Vec<String>,
}

fn build_source_infos(
    raw_bundle_ids: Vec<String>,
    label_resolver: impl Fn(&str) -> String,
) -> Vec<crate::audio::AudioSourceInfo> {
    let mut groups = build_mac_source_groups(raw_bundle_ids, label_resolver);
    disambiguate_duplicate_labels(&mut groups);
    groups.sort_by(|a, b| {
        cmp_label_then_bundle(
            &a.label,
            &a.primary_bundle_id,
            &b.label,
            &b.primary_bundle_id,
        )
    });

    let mut out = vec![crate::audio::AudioSourceInfo {
        source: crate::config::AudioSource::SystemOutput,
        label: "System Output".to_string(),
    }];
    out.extend(
        groups
            .into_iter()
            .map(|group| crate::audio::AudioSourceInfo {
                source: crate::config::AudioSource::App {
                    bundle_id: group.primary_bundle_id,
                    label: group.label.clone(),
                    capture_bundle_ids: group.capture_bundle_ids,
                },
                label: group.label,
            }),
    );
    out
}

fn build_mac_source_groups(
    raw_bundle_ids: Vec<String>,
    label_resolver: impl Fn(&str) -> String,
) -> Vec<MacSourceGroup> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut groups: BTreeMap<String, MacSourceGroup> = BTreeMap::new();
    let mut seen_raw = BTreeSet::new();
    for bundle_id in raw_bundle_ids {
        if !seen_raw.insert(bundle_id.clone()) || is_filtered_bundle_id(&bundle_id) {
            continue;
        }
        let raw_label = label_resolver(&bundle_id);
        if is_filtered_label(&raw_label) {
            continue;
        }

        let (primary_bundle_id, label) = canonical_app_identity(&bundle_id, &raw_label);
        if is_filtered_bundle_id(&primary_bundle_id) || is_filtered_label(&label) {
            continue;
        }

        let entry = groups
            .entry(primary_bundle_id.clone())
            .or_insert_with(|| MacSourceGroup {
                label,
                primary_bundle_id,
                capture_bundle_ids: Vec::new(),
            });
        entry.capture_bundle_ids.push(bundle_id);
    }

    let mut out: Vec<_> = groups
        .into_values()
        .map(|mut group| {
            group
                .capture_bundle_ids
                .sort_by_key(|s| s.to_ascii_lowercase());
            group.capture_bundle_ids.dedup();
            group
        })
        .collect();
    out.sort_by(|a, b| {
        cmp_label_then_bundle(
            &a.label,
            &a.primary_bundle_id,
            &b.label,
            &b.primary_bundle_id,
        )
    });
    out
}

fn canonical_app_identity(bundle_id: &str, raw_label: &str) -> (String, String) {
    let lower_bundle = bundle_id.to_ascii_lowercase();
    let helper_trimmed = strip_helper_suffix(raw_label).trim().to_string();

    if lower_bundle.starts_with("com.hnc.discord") || helper_trimmed.eq_ignore_ascii_case("Discord")
    {
        return ("com.hnc.Discord".to_string(), "Discord".to_string());
    }
    if lower_bundle.starts_with("com.tinyspeck.slackmacgap")
        || helper_trimmed.eq_ignore_ascii_case("Slack")
    {
        return ("com.tinyspeck.slackmacgap".to_string(), "Slack".to_string());
    }
    if lower_bundle.starts_with("us.zoom.") || helper_trimmed.eq_ignore_ascii_case("zoom.us") {
        return ("us.zoom.xos".to_string(), "zoom.us".to_string());
    }
    if lower_bundle.contains("firefox") || helper_trimmed.eq_ignore_ascii_case("Firefox") {
        return ("org.mozilla.firefox".to_string(), "Firefox".to_string());
    }
    if lower_bundle.contains("google.chrome")
        || helper_trimmed.eq_ignore_ascii_case("Google Chrome")
    {
        return ("com.google.Chrome".to_string(), "Google Chrome".to_string());
    }
    if lower_bundle.contains("microsoft.edgemac")
        || helper_trimmed.eq_ignore_ascii_case("Microsoft Edge")
    {
        return (
            "com.microsoft.edgemac".to_string(),
            "Microsoft Edge".to_string(),
        );
    }
    if lower_bundle.starts_with("com.apple.webkit") || helper_trimmed.eq_ignore_ascii_case("Safari")
    {
        return ("com.apple.Safari".to_string(), "Safari".to_string());
    }

    let label = if helper_trimmed.is_empty() || helper_trimmed == raw_label {
        raw_label.to_string()
    } else {
        helper_trimmed
    };
    (bundle_id.to_string(), label)
}

fn strip_helper_suffix(label: &str) -> &str {
    let mut out = label.trim();
    for suffix in [
        " Helper (Renderer)",
        " Helper (Plugin)",
        " Helper (GPU)",
        " Helper",
        " Graphics and Media",
        " Renderer",
        " GPU",
    ] {
        if let Some(stripped) = out.strip_suffix(suffix) {
            out = stripped.trim();
            break;
        }
    }
    out
}

fn is_filtered_bundle_id(bundle_id: &str) -> bool {
    let id = bundle_id.to_ascii_lowercase();
    if id.contains("subtidal") {
        return true;
    }

    let apple_allow = [
        "com.apple.music",
        "com.apple.messages",
        "com.apple.safari",
        "com.apple.tv",
        "com.apple.podcasts",
        "com.apple.books",
        "com.apple.facetime",
        "com.apple.quicktimeplayerx",
    ];
    if apple_allow.iter().any(|allowed| id == *allowed) {
        return false;
    }

    if id.starts_with("com.apple.webkit") {
        return false;
    }

    let denied_exact = [
        "com.apple.assistantd",
        "com.apple.audiomxd",
        "com.apple.mediaanalysisd",
        "com.apple.mediaremoted",
        "com.apple.replayd",
        "com.apple.cloudpaird",
        "com.apple.universalaccessd",
        "com.apple.caphost",
        "com.apple.callservicesd",
        "com.apple.systemsoundserverd",
        "com.apple.loginwindow",
        "com.apple.powerchime",
        "com.apple.sirincservice",
        "com.apple.controlcenter",
        "com.apple.sidebar",
    ];
    if denied_exact.iter().any(|denied| id == *denied) {
        return true;
    }

    let denied_fragments = [
        "quicklook",
        "accessibility",
        "continuitycapture",
        "controlcenter",
        "systemsound",
        "loginwindow",
        "assistantd",
        "audiomxd",
        "mediaanalysisd",
        "mediaremoted",
        "replayd",
        "cloudpaird",
        "universalaccessd",
        "caphost",
        "callservicesd",
        "powerchime",
        "sirincservice",
    ];
    denied_fragments.iter().any(|frag| id.contains(frag))
}

fn is_filtered_label(label: &str) -> bool {
    let label = label.trim();
    if label.eq_ignore_ascii_case("Subtidal") {
        return true;
    }
    let lower = label.to_ascii_lowercase();
    let denied = [
        "assistantd",
        "audiomxd",
        "mediaanalysisd",
        "mediaremoted",
        "replayd",
        "cloudpaird",
        "universalaccessd",
        "caphost",
        "callservicesd",
        "systemsoundserverd",
        "loginwindow",
        "powerchime",
        "sirincservice",
        "control center",
        "sidebar",
        "quicklook",
        "accessibility",
        "continuity capture",
    ];
    denied.iter().any(|needle| lower.contains(needle))
}

fn disambiguate_duplicate_labels(groups: &mut [MacSourceGroup]) {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for group in groups.iter() {
        *counts.entry(group.label.to_ascii_lowercase()).or_default() += 1;
    }
    for group in groups.iter_mut() {
        if counts
            .get(&group.label.to_ascii_lowercase())
            .copied()
            .unwrap_or_default()
            > 1
        {
            group.label = format!("{} ({})", group.label, group.primary_bundle_id);
        }
    }
}

fn cmp_label_then_bundle(
    a_label: &str,
    a_bundle: &str,
    b_label: &str,
    b_bundle: &str,
) -> std::cmp::Ordering {
    a_label
        .to_ascii_lowercase()
        .cmp(&b_label.to_ascii_lowercase())
        .then_with(|| a_bundle.cmp(b_bundle))
}

/// Translate a bundle ID to a user-visible label.
///
/// Looks up the first `NSRunningApplication` with the given bundle id and
/// returns its `localizedName`. This is what the user sees in the Dock /
/// Finder — e.g. "Music" instead of "com.apple.Music", "Firefox" instead
/// of "org.mozilla.firefox". Falls back to the bundle id if no running
/// instance is found.
fn bundle_to_label(bundle_id: &str) -> String {
    use objc2::rc::Retained;
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::{NSArray, NSString};

    let id_ns = NSString::from_str(bundle_id);
    let apps: Retained<NSArray<NSRunningApplication>> =
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&id_ns);
    if apps.count() == 0 {
        return bundle_id.to_string();
    }
    let app = apps.objectAtIndex(0);
    match app.localizedName() {
        Some(name) => name.to_string(),
        None => bundle_id.to_string(),
    }
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
            async_channel::Sender<crate::overlay::CaptionEvent>,
        ) -> anyhow::Result<(
            std::sync::mpsc::SyncSender<crate::audio::AudioCommand>,
            ringbuf::HeapCons<f32>,
            std::sync::mpsc::Receiver<crate::audio::FallbackEvent>,
        )> = start_audio_thread;
    }

    #[test]
    fn grouped_source_infos_have_single_system_output_first() {
        let sources = build_source_infos(vec!["com.apple.Music".to_string()], |id| {
            if id == "com.apple.Music" {
                "Music".to_string()
            } else {
                id.to_string()
            }
        });

        assert!(matches!(sources[0].source, AudioSource::SystemOutput));
        assert_eq!(
            sources
                .iter()
                .filter(|s| matches!(s.source, AudioSource::SystemOutput))
                .count(),
            1
        );
    }

    #[test]
    fn known_background_and_self_sources_are_filtered() {
        let sources = build_source_infos(
            vec![
                "com.apple.assistantd".to_string(),
                "com.apple.audiomxd".to_string(),
                "com.apple.systemsoundserverd".to_string(),
                "com.subtidal.Subtidal".to_string(),
                "com.apple.Music".to_string(),
            ],
            |id| match id {
                "com.apple.Music" => "Music".to_string(),
                "com.subtidal.Subtidal" => "Subtidal".to_string(),
                _ => id.to_string(),
            },
        );

        assert_eq!(sources.len(), 2);
        assert!(sources.iter().any(|s| s.label == "Music"));
        assert!(!sources.iter().any(|s| s.label.contains("assistantd")));
        assert!(!sources.iter().any(|s| s.label == "Subtidal"));
    }

    #[test]
    fn discord_slack_and_zoom_helpers_group_under_parent_rows() {
        let sources = build_source_infos(
            vec![
                "com.hnc.Discord.helper.Renderer".to_string(),
                "com.hnc.Discord".to_string(),
                "com.tinyspeck.slackmacgap.helper".to_string(),
                "com.tinyspeck.slackmacgap".to_string(),
                "us.zoom.xos".to_string(),
                "us.zoom.ZoomClips".to_string(),
            ],
            |id| match id {
                "com.hnc.Discord.helper.Renderer" => "Discord Helper (Renderer)".to_string(),
                "com.hnc.Discord" => "Discord".to_string(),
                "com.tinyspeck.slackmacgap.helper" => "Slack Helper".to_string(),
                "com.tinyspeck.slackmacgap" => "Slack".to_string(),
                "us.zoom.xos" => "zoom.us".to_string(),
                "us.zoom.ZoomClips" => "zoom.us Graphics and Media".to_string(),
                _ => id.to_string(),
            },
        );

        let app_rows: Vec<_> = sources
            .iter()
            .filter(|s| matches!(s.source, AudioSource::App { .. }))
            .collect();
        assert_eq!(app_rows.len(), 3);

        let discord = app_rows.iter().find(|s| s.label == "Discord").unwrap();
        assert!(
            matches!(&discord.source, AudioSource::App { bundle_id, capture_bundle_ids, .. }
            if bundle_id == "com.hnc.Discord"
                && capture_bundle_ids == &vec![
                    "com.hnc.Discord".to_string(),
                    "com.hnc.Discord.helper.Renderer".to_string(),
                ])
        );

        let slack = app_rows.iter().find(|s| s.label == "Slack").unwrap();
        assert!(
            matches!(&slack.source, AudioSource::App { bundle_id, capture_bundle_ids, .. }
            if bundle_id == "com.tinyspeck.slackmacgap"
                && capture_bundle_ids == &vec![
                    "com.tinyspeck.slackmacgap".to_string(),
                    "com.tinyspeck.slackmacgap.helper".to_string(),
                ])
        );

        let zoom = app_rows.iter().find(|s| s.label == "zoom.us").unwrap();
        assert!(
            matches!(&zoom.source, AudioSource::App { bundle_id, capture_bundle_ids, .. }
            if bundle_id == "us.zoom.xos" && capture_bundle_ids.len() == 2)
        );
    }

    #[test]
    fn duplicate_visible_labels_are_disambiguated_deterministically() {
        let sources = build_source_infos(
            vec!["com.example.one".to_string(), "com.example.two".to_string()],
            |_| "Example".to_string(),
        );
        let labels: Vec<_> = sources.into_iter().map(|s| s.label).collect();
        assert_eq!(
            labels,
            vec![
                "System Output".to_string(),
                "Example (com.example.one)".to_string(),
                "Example (com.example.two)".to_string(),
            ]
        );
    }

    #[test]
    #[ignore = "requires Audio Capture permission and a running graphical session"]
    fn list_sources_returns_system_output_plus_user_facing_apps() {
        let sources = list_sources().expect("list_sources should succeed");
        assert!(
            matches!(
                sources.first().map(|s| &s.source),
                Some(AudioSource::SystemOutput)
            ),
            "SystemOutput must always appear first",
        );
        assert!(
            sources.len() >= 1,
            "at least SystemOutput should be present"
        );
    }
}
