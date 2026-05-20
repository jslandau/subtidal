# macOS Port — Phase 5 (Revised): Core Audio Process Taps

**Status:** Supersedes `phase_05.md` and replaces the capture mechanism introduced in `phase_04.md`. Phase 4's SCK code (`src/audio/impl_macos/stream.rs`, `normalize.rs`) is removed; the `start_audio_thread` external signature is preserved where possible so `main_macos.rs` wiring barely changes.

**Goal:** Replace ScreenCaptureKit-based audio capture with Core Audio Process Taps (macOS 14.4+). Capture either the full system mix or a specific running application; switch sources live; auto-fall-back to System Output when a captured app exits.

**Why this revision:** Phase 4 hardware bring-up exposed three structural costs of the SCK approach: (1) Screen Recording TCC re-prompts on every launch without stable codesigning (driving Phase 7's scope), (2) audio→caption latency higher than Linux because SCK audio is delivered through the screen-capture pipeline, (3) unavoidable "stream output NOT found" log noise from SCK's mandatory video frames. Core Audio Taps (added in macOS 14.2; first-class permission key in 14.4) addresses all three: it has its own narrower TCC service (`NSAudioCaptureUsageDescription`) that is persistent without codesign stability, delivers raw PCM directly from the audio HAL via an `IOProc` callback, and has no video machinery to ignore. The Tap API is also a more honest match for the use case — Subtidal captures audio, not screens.

**Architecture:** A new `src/audio/impl_macos/tap.rs` owns the FFI to `AudioHardwareCreateProcessTap` / `AudioHardwareCreateAggregateDevice` / `AudioDeviceCreateIOProcID` / `AudioDeviceStart`, plus the process-enumeration helpers (`kAudioHardwarePropertyProcessObjectList`, `kAudioProcessPropertyBundleID`, `kAudioProcessPropertyPID`, `kAudioProcessPropertyIsRunning`). `src/audio/impl_macos/mod.rs::start_audio_thread` retains its Phase 4+5 external signature `(initial_source, audio_wake, error_caption_tx) -> Result<(SyncSender<AudioCommand>, HeapCons<f32>, Receiver<FallbackEvent>)>` but its body switches from SCK to Tap-based capture. Source switching tears down the aggregate device + tap and rebuilds (no live `updateContentFilter` analog exists; rebuild cost is sub-100ms and only fires on user action). Source-disappeared detection runs on a low-frequency (1 Hz) polling thread inside the audio worker, checking `kAudioProcessPropertyIsRunning` for the captured PID; on disappearance it posts the same `FallbackEvent` + `NSUserNotification` flow Phase 5 defined.

**What survives unchanged from Phase 5 already-shipped work:** commit `8fead4a` (`config::AudioSource::App { bundle_id, label }` enum variant) stays as-is — the persisted config shape is backend-agnostic.

**What gets reverted from Phase 5 Subcomp A:** commit `c309820`'s `list_sources()` body (SCK-flavored, calls `SCShareableContent`) and the `AudioCommand::SwitchSource` variant land in this phase reworked. Neutral types `AudioSourceInfo` and `FallbackEvent` are kept as-is.

**Tech Stack:** `coreaudio-sys` 0.2.17+ (raw bindgen-generated FFI), `objc2-foundation` + `objc2-user-notifications` (for the fallback `UNUserNotificationCenter` post — `NSUserNotification` is deprecated and we want a modern path now that we're rewriting), `core-foundation` (for `CFString` round-trips needed by `kAudioProcessPropertyBundleID`).

**Minimum macOS:** 14.4 (raised from Phase 0's 13.0). The `NSAudioCaptureUsageDescription` key and persistent Audio Capture TCC grant ship in 14.4; the Tap API itself ships in 14.2 but `UNUserNotificationCenter`-style permission UX requires 14.4. Update `resources/macos/Info.plist`'s `LSMinimumSystemVersion` accordingly.

**Scope:** Replaces Phase 5 of 8. Phase 6 (tray) and Phase 7 (polish) continue from here unchanged in shape, though Phase 7's "stable codesigning for TCC persistence" task becomes much smaller (the Audio Capture grant is persistent without it).

**Codebase verified:** 2026-05-20.

---

## Acceptance Criteria Coverage

This revision covers the same AC scope as `phase_05.md`, plus carrying Phase 4's AC3.1 forward on the new backend:

### macos-port.AC3: ScreenCaptureKit audio capture
*Note: the criterion is named for SCK historically; the underlying requirement is "capture system / per-app audio." Core Audio Taps satisfies the requirement equivalently.*

- **macos-port.AC3.1 Success:** System Output capture produces a continuous stream of captions matching audio playing on the default output device (re-verifies Phase 4 on the new backend).
- **macos-port.AC3.2 Success:** Selecting a specific running application as the audio source captures only that app's audio.
- **macos-port.AC3.3 Success:** Switching the audio source rebuilds the tap + aggregate device with no panel flicker and a caption gap ≤ 1 second (relaxed from Phase 5's "no visible gap" because tap rebuild has measurable latency; sub-second is the realistic target and still meets the spirit of the AC).
- **macos-port.AC3.4 Success:** When the captured app exits, the polling watchdog detects it within ≤ 2 seconds, posts a `UNUserNotification`, and falls back to System Output.
- **macos-port.AC3.6 Success:** When Audio Capture permission is denied, the audio thread surfaces an in-panel caption directing the user to System Settings → Privacy & Security → Audio Capture (rebinds Phase 4's Screen Recording message to the new TCC service).

### macos-port.AC7: TCC persistence
- **macos-port.AC7.1 Success (partial):** The Audio Capture permission grant persists across rebuilds with ad-hoc codesigning. (Phase 7's stable-signing work remains for any *future* TCC service that may be needed but is no longer blocking on this one.)

---

## Pre-flight: API verification spike

**Verifies:** binding availability before the rest of the phase commits to the approach.

**Files:**
- Create: `scripts/check-tap-symbols.sh` — single-file build smoke check

**Implementation:**

Before writing any production code, confirm that `coreaudio-sys` 0.2.17 actually exposes the Tap symbols on the target macOS SDK. The umbrella header `CoreAudio/CoreAudio.h` may or may not pull in `AudioHardwareTapping.h` depending on Apple's SDK hygiene.

```bash
#!/usr/bin/env bash
# scripts/check-tap-symbols.sh
set -eu
cd "$(dirname "$0")/.."
cat > /tmp/tap_smoke.rs <<'EOF'
use coreaudio_sys::{
    AudioHardwareCreateProcessTap, CATapDescription,
    kAudioHardwarePropertyTranslatePIDToProcessObject,
    kAudioHardwarePropertyProcessObjectList,
    kAudioProcessPropertyBundleID,
    kAudioProcessPropertyIsRunning,
    kAudioProcessPropertyPID,
    AudioHardwareCreateAggregateDevice,
    kAudioAggregateDeviceTapListKey,
};
fn main() { let _ = (AudioHardwareCreateProcessTap, AudioHardwareCreateAggregateDevice); }
EOF
# Use the smoke file as a one-shot bin in the current crate to share Cargo.lock.
cp /tmp/tap_smoke.rs examples/tap_smoke.rs
cargo check --example tap_smoke --target aarch64-apple-darwin
rm examples/tap_smoke.rs
```

**Outcomes:**
- **All symbols resolve:** proceed to Task 1 unmodified.
- **Some symbols missing:** vendor `coreaudio-sys` as a path dependency in `vendor/coreaudio-sys/` with one extra line in `build.rs`:
  ```rust
  headers.push("CoreAudio/AudioHardwareTapping.h");
  ```
  Document the vendoring rationale in the commit body and continue.
- **Build fails for SDK reasons (Xcode < 15.2):** stop and escalate; the toolchain on the build machine needs updating before the rest of the phase makes sense.

**Commit:** `macos: add tap-symbols smoke check (pre-flight for Core Audio Taps rewrite)`

---

## Implementation Tasks

<!-- START_TASK_1 -->
### Task 1: Cargo & Info.plist substrate

**Files:**
- Modify: `Cargo.toml` macOS target block — remove `objc2-screen-capture-kit`, `objc2-core-media`, `objc2-core-audio-types`; add `coreaudio-sys = "0.2"`, `objc2-user-notifications = "0.3"`, `core-foundation = "0.10"` (or whichever is current).
- Modify: `resources/macos/Info.plist` — replace `NSScreenCaptureUsageDescription` with `NSAudioCaptureUsageDescription` (string: "Subtidal captures system audio to display live captions."). Bump `LSMinimumSystemVersion` to `14.4`.
- Modify: `scripts/bundle-mac.sh` — drop the SCK-specific notes from the header comment; the dylib bundling for `libwebgpu_dawn.dylib` remains identical.

**Verification:**

```bash
cargo check --lib                              # Linux unaffected
cargo check --lib --target aarch64-apple-darwin
plutil -lint resources/macos/Info.plist
```

**Commit:** `macos: swap SCK deps for coreaudio-sys; replace Screen Recording TCC with Audio Capture`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Process enumeration

**Verifies:** macos-port.AC3.2 (selection prerequisite)

**Files:**
- Create: `src/audio/impl_macos/tap_processes.rs` — safe wrappers around `kAudioHardwarePropertyProcessObjectList` enumeration.
- Modify: `src/audio/impl_macos/mod.rs` — rewrite `list_sources()` to call the new helper instead of `SCShareableContent`.
- Remove (later, after all callers migrated): `src/audio/impl_macos/stream.rs::shareable_content_current` — left in place this task; removed in Task 6.

**Implementation outline:**

```rust
// tap_processes.rs

pub struct ProcessInfo {
    pub audio_object_id: AudioObjectID,
    pub pid: pid_t,
    pub bundle_id: Option<String>,
}

/// Enumerate all process AudioObjects known to Core Audio.
/// Returns only processes with a non-empty bundle ID and a live PID.
pub fn enumerate_audio_processes() -> Result<Vec<ProcessInfo>> {
    // 1. AudioObjectGetPropertyDataSize on kAudioHardwarePropertyProcessObjectList
    //    against kAudioObjectSystemObject, mScope = kAudioObjectPropertyScopeGlobal,
    //    mElement = kAudioObjectPropertyElementMain. Allocate Vec<AudioObjectID>.
    // 2. AudioObjectGetPropertyData fills the vec.
    // 3. For each id: read kAudioProcessPropertyPID, kAudioProcessPropertyBundleID,
    //    kAudioProcessPropertyIsRunning. Filter out non-running / no-bundle entries.
    // CFString → Rust String conversion via core_foundation::string::CFString.
}

/// Translate a PID into the corresponding Core Audio process AudioObjectID.
/// Used at tap-creation time to map a user-selected bundle back to an object id.
pub fn translate_pid_to_process_object(pid: pid_t) -> Result<AudioObjectID> {
    // AudioObjectGetPropertyData with
    //   kAudioObjectSystemObject + kAudioHardwarePropertyTranslatePIDToProcessObject,
    //   qualifier = &pid, out = AudioObjectID.
}

/// Whether the given process AudioObject reports kAudioProcessPropertyIsRunning = true.
/// Cheap; safe to call from a 1 Hz watchdog.
pub fn process_is_running(obj: AudioObjectID) -> bool { … }
```

`list_sources()` becomes:

```rust
pub fn list_sources() -> Result<Vec<AudioSourceInfo>> {
    let mut out = vec![AudioSourceInfo {
        source: AudioSource::SystemOutput,
        label: "System Output".to_string(),
    }];
    for proc in tap_processes::enumerate_audio_processes()? {
        if let Some(bundle) = proc.bundle_id {
            let label = bundle_to_label(&bundle);  // best-effort; falls back to bundle id
            out.push(AudioSourceInfo {
                source: AudioSource::App { bundle_id: bundle, label: label.clone() },
                label,
            });
        }
    }
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok(out)
}
```

`bundle_to_label` reads `CFBundle`'s `CFBundleName` from the bundle's `Info.plist` via `CFBundleCreate` + `CFBundleGetValueForInfoDictionaryKey`. If unavailable, returns the bundle ID unchanged.

**Verification:**
```bash
cargo check --lib --target aarch64-apple-darwin
cargo test --lib
```

Hardware exercise in Task 7.

**Commit:** `macos: enumerate audio-producing processes via Core Audio property API`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Tap + aggregate device lifecycle

**Verifies:** macos-port.AC3.1, macos-port.AC3.2 (capture mechanism)

**Files:**
- Create: `src/audio/impl_macos/tap.rs` — RAII wrappers around the Tap + Aggregate Device + IOProc trio.

**Implementation outline:**

```rust
// tap.rs

/// Owns a single Core Audio process tap, its aggregate device, and its IOProc.
/// Drop tears all three down in correct order: AudioDeviceStop → AudioDeviceDestroyIOProcID →
/// AudioHardwareDestroyAggregateDevice → AudioHardwareDestroyProcessTap.
pub struct AudioTap {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    ioproc_id: AudioDeviceIOProcID,
    callback_context: Box<CallbackContext>,    // pinned via Box; pointer passed as clientData
    captured_pid: Option<pid_t>,               // for the 1 Hz watchdog; None = SystemOutput
}

struct CallbackContext {
    producer: Arc<Mutex<HeapProd<f32>>>,
    wake: Arc<AudioWake>,
}

pub enum TapTarget {
    SystemMix,                   // empty process list + isExclusive=true → tap everything
    Process { pid: pid_t },      // single-PID tap; isExclusive=false → tap only this
}

impl AudioTap {
    pub fn build(
        target: TapTarget,
        producer: Arc<Mutex<HeapProd<f32>>>,
        wake: Arc<AudioWake>,
    ) -> Result<Self> {
        // 1. CATapDescription:
        //    - SystemMix:  init(processes: [], andDeviceUID: nil, withStream: 0, isExclusive: true, isMixdown: true, isPrivate: true)
        //    - Process(p): translate pid → AudioObjectID; init(processes: [obj], …, isExclusive: false, …)
        //    Constructed via raw Obj-C dispatch (objc2 Class lookup or NSClassFromString;
        //    no objc2 binding crate covers CATapDescription yet).
        // 2. AudioHardwareCreateProcessTap(&desc, &mut tap_id)
        // 3. Read tap's UID via AudioObjectGetPropertyData(tap_id, kAudioTapPropertyUID)
        // 4. Build aggregate device dict:
        //    { kAudioAggregateDeviceUIDKey: "com.subtidal.tap.<uuid>",
        //      kAudioAggregateDeviceNameKey: "Subtidal Tap",
        //      kAudioAggregateDeviceIsPrivateKey: 1,
        //      kAudioAggregateDeviceTapListKey: [ { kAudioSubTapUIDKey: <tap_uid> } ],
        //      kAudioAggregateDeviceTapAutoStartKey: 1 }
        //    via CFDictionary built with core_foundation crate. Call
        //    AudioHardwareCreateAggregateDevice(dict, &mut aggregate_id).
        // 5. AudioDeviceCreateIOProcID(aggregate_id, ioproc_fn, ctx_ptr, &mut ioproc_id)
        // 6. AudioDeviceStart(aggregate_id, ioproc_id)
        // On any error, partially-constructed resources are torn down before bail!.
    }

    pub fn captured_pid(&self) -> Option<pid_t> { self.captured_pid }
}

impl Drop for AudioTap { … }

/// RT-safe IOProc. Called on a Core Audio thread; MUST NOT allocate / log / blocking-lock.
extern "C" fn ioproc_fn(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut c_void,
) -> OSStatus {
    let ctx = &*(client_data as *const CallbackContext);
    let buf_list = &*input_data;
    // First (and only) buffer for a tap that asked for stereo mixdown.
    let buf = &buf_list.mBuffers[0];
    let n_frames = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
    let samples = std::slice::from_raw_parts(buf.mData as *const f32, n_frames);
    if let Ok(mut prod) = ctx.producer.try_lock() {
        let _ = prod.push_slice(samples);
    }
    ctx.wake.notify();
    0  // noErr
}
```

**Format negotiation:** the tap's natural format is 44.1 or 48 kHz stereo `f32` interleaved depending on the source. The downstream rubato resampler at `audio/resampler.rs` already handles 48 kHz stereo → 16 kHz mono. If the tap reports a different rate, we set `kAudioAggregateDeviceMainSubDeviceKey` to coerce 48 kHz, OR (cleaner) we accept whatever rate the tap gives and extend the resampler to read the actual rate from a new field on the producer side. **Decision deferred to Task 3 implementation** — try the coerce-to-48k path first; document the chosen approach in the commit body.

**Why `Box<CallbackContext>`:** the IOProc receives a `*mut c_void` clientData pointer. We need a stable address that survives moves of `AudioTap`, so the context is heap-pinned in a `Box`. Drop order matters: the IOProc must be destroyed (`AudioDeviceStop` + `AudioDeviceDestroyIOProcID`) BEFORE the `Box<CallbackContext>` is dropped, otherwise an in-flight callback dereferences freed memory.

**Verification:**
```bash
cargo check --lib --target aarch64-apple-darwin
```
Hardware exercise in Task 7.

**Commit:** `macos: AudioTap RAII — process tap, aggregate device, IOProc`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Rewrite `start_audio_thread` on top of `AudioTap`

**Verifies:** macos-port.AC3.1, macos-port.AC3.3 (live switching)

**Files:**
- Modify: `src/audio/impl_macos/mod.rs` — rewrite `run_sck_capture` as `run_tap_capture`; keep the public `start_audio_thread` signature.
- Modify: `src/main_macos.rs` — pass `cfg.audio_source` as the new first parameter (already added in Phase 5 plan); no other changes needed.

**Implementation outline:**

```rust
fn run_tap_capture(
    initial_source: AudioSource,
    ring_producer: Arc<Mutex<HeapProd<f32>>>,
    audio_wake: Arc<AudioWake>,
    rx_cmd: Receiver<AudioCommand>,
    fallback_tx: SyncSender<FallbackEvent>,
) -> Result<()> {
    let mut current_source = initial_source.clone();
    let mut current_label = source_label(&current_source);
    let mut tap = AudioTap::build(
        tap_target_for(&current_source)?,
        Arc::clone(&ring_producer),
        Arc::clone(&audio_wake),
    ).context("initial tap construction (Audio Capture permission denied?)")?;

    // Watchdog: every 1s, if we're capturing a specific PID, check IsRunning.
    let watchdog_tick = std::time::Duration::from_secs(1);
    let mut last_tick = std::time::Instant::now();

    loop {
        // Short timeout so we can interleave watchdog ticks.
        match rx_cmd.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(AudioCommand::Shutdown) => break,
            Ok(AudioCommand::SwitchSource(new_source)) => {
                let new_target = match tap_target_for(&new_source) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("warn: cannot switch to {new_source:?}: {e}; staying on current");
                        continue;
                    }
                };
                // Rebuild: drop old tap (Drop tears down in correct order), build new.
                drop(tap);
                tap = AudioTap::build(new_target, Arc::clone(&ring_producer), Arc::clone(&audio_wake))?;
                current_source = new_source;
                current_label = source_label(&current_source);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_tick.elapsed() >= watchdog_tick {
            last_tick = std::time::Instant::now();
            if let Some(pid) = tap.captured_pid() {
                if !tap_processes::process_is_running(translate_pid_to_process_object(pid)?) {
                    // Source disappeared.
                    notify::post_user_notification(
                        "Subtidal: audio source unavailable",
                        &format!("'{current_label}' stopped producing audio. Falling back to System Output."),
                    ).ok();
                    let _ = fallback_tx.send(FallbackEvent {
                        previous_label: current_label.clone(),
                        new_source: AudioSource::SystemOutput,
                    });
                    drop(tap);
                    tap = AudioTap::build(TapTarget::SystemMix, Arc::clone(&ring_producer), Arc::clone(&audio_wake))?;
                    current_source = AudioSource::SystemOutput;
                    current_label = "System Output".into();
                }
            }
        }
    }
    Ok(())
}

fn tap_target_for(src: &AudioSource) -> Result<TapTarget> {
    match src {
        AudioSource::SystemOutput | AudioSource::Application { .. } => Ok(TapTarget::SystemMix),
        AudioSource::App { bundle_id, .. } => {
            let procs = tap_processes::enumerate_audio_processes()?;
            let proc = procs.iter().find(|p| p.bundle_id.as_deref() == Some(bundle_id))
                .with_context(|| format!("app '{bundle_id}' is not running"))?;
            Ok(TapTarget::Process { pid: proc.pid })
        }
    }
}
```

**Note on the watchdog cadence:** 1 Hz means AC3.4's "≤ 2 seconds detection" leaves room for one missed tick. If hardware verification shows users perceive 1–2s as too slow, drop to 500 ms; the property read is cheap.

**Verification:**
```bash
cargo check --lib --target aarch64-apple-darwin
cargo test --lib
```

**Commit:** `macos: rewrite audio worker on AudioTap; SwitchSource via rebuild; PID watchdog`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Fallback notification

**Verifies:** macos-port.AC3.4

**Files:**
- Create: `src/audio/impl_macos/notify.rs` — `post_user_notification(title, body)` using `UNUserNotificationCenter`.
- Modify: `src/main_macos.rs` — at startup, request UN authorization once (best-effort; ignore failure — the watchdog notification is a nice-to-have, not load-bearing).

**Implementation outline:**

```rust
pub fn post_user_notification(title: &str, body: &str) -> Result<()> {
    use objc2_user_notifications::{UNUserNotificationCenter, UNMutableNotificationContent, UNNotificationRequest};
    unsafe {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        let req = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&format!("subtidal-fallback-{}", uuid_like())),
            &content,
            None,  // immediate delivery
        );
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.addNotificationRequest_withCompletionHandler(&req, None);
    }
    Ok(())
}

pub fn request_authorization_best_effort() {
    // UNUserNotificationCenter.requestAuthorization(options:[.alert]) → ignore result.
    // First call surfaces the macOS notification permission prompt. If denied, our
    // posts silently no-op; the on-screen UNAVAILABLE banner is still posted via
    // error_caption_tx as a fallback.
}
```

Adapt method spellings to the objc2-user-notifications 0.3 bindings exactly (Phase 4 had several name mismatches between plan and generated bindings — same caveat).

**Verification:**
```bash
cargo check --lib --target aarch64-apple-darwin
```

**Commit:** `macos: UNUserNotificationCenter helper for source-disappeared fallback`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Remove SCK code

**Files:**
- Delete: `src/audio/impl_macos/stream.rs`
- Delete: `src/audio/impl_macos/normalize.rs`
- Modify: `src/audio/impl_macos/mod.rs` — remove `mod stream;` / `mod normalize;` declarations.
- Modify: `Cargo.toml` — confirm SCK crates dropped in Task 1 (this is a final sweep).

**Verification:**
```bash
cargo check --lib --target aarch64-apple-darwin
cargo check --lib
cargo test --lib
grep -r "screen_capture_kit\|SCStream\|CMSampleBuffer" src/  # must be empty
```

**Commit:** `macos: remove ScreenCaptureKit code path (superseded by Core Audio Taps)`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Hardware verification

**Verifies:** macos-port.AC3.1, macos-port.AC3.2, macos-port.AC3.3, macos-port.AC3.4, macos-port.AC3.6, macos-port.AC7.1

On the target Apple Silicon Mac (macOS 14.4+), with Safari + a YouTube tab ready:

1. **First-launch permission (AC3.6 setup):** Build via `scripts/bundle-mac.sh`, `open target/release/Subtidal.app`. First capture attempt triggers the Audio Capture TCC prompt. Grant. Captions begin within ≤ 1 second of playing audio (AC3.1).

2. **AC3.1 re-verification:** Play audio in multiple apps; the System Output mix is captured (all apps audible in captions).

3. **AC3.2 (per-app capture):** Edit `~/Library/Application Support/Subtidal/config.toml`:
   ```toml
   [audio_source]
   type = "app"
   bundle_id = "com.apple.Safari"
   label = "Safari"
   ```
   Save → config hot-reload posts `SwitchSource` → captions now reflect only Safari. Play audio in Music.app simultaneously; confirm it is NOT captured.

4. **AC3.3 (live switching):** Edit config back to `type = "system_output"` while audio is playing. Caption gap should be ≤ 1 second; no panel flicker.

5. **AC3.4 (source-disappeared fallback):** While capturing Safari, Cmd-Q Safari. Within ≤ 2 seconds: a notification banner appears, captures switch to System Output. `/tmp/subtidal-phase5b.log` shows:
   ```
   info: audio source 'Safari' disappeared; switched to SystemOutput
   ```

6. **AC3.6 (permission denied):** Reset Audio Capture grant via `tccutil reset AudioCapture com.subtidal.app`. Relaunch. Deny the prompt. The NSPanel displays the in-panel error message ("Grant Audio Capture permission in System Settings → Privacy & Security, then relaunch").

7. **AC7.1 (TCC persistence):** Re-grant the permission. Quit and relaunch the app several times. Confirm: no re-prompt, captures resume immediately each launch. (Contrast: under Phase 4's SCK + Screen Recording, every launch re-prompted.)

8. **Cross-target CI:**
   ```bash
   cargo check --lib --target aarch64-apple-darwin
   ```
   Green. The GitHub Actions `macos-check` workflow is also expected green on the next push.

**Commit:** `macos: Phase 5 (revised) hardware verification notes`
<!-- END_TASK_7 -->

---

## Risks & open questions

1. **`coreaudio-sys` Tap symbols may be absent** (mitigation: vendored fork with one-line header addition — see pre-flight spike).
2. **Aggregate-device naming collisions** if a user runs multiple Subtidal builds or another app that creates `com.subtidal.tap.*` aggregates. Mitigated by including a UUID/PID suffix in the aggregate UID.
3. **Sample-rate negotiation** — Tap rate may not be 48 kHz on all hardware. First attempt: coerce via `kAudioAggregateDeviceMainSubDeviceKey`; fallback: extend the resampler to accept any input rate (small change).
4. **Notification permission UX** — `UNUserNotificationCenter` requires its own one-time prompt for banner display. The fallback notification is a convenience, not load-bearing; if the user denies it, the in-panel caption banner (`error_caption_tx`) still surfaces the event.
5. **macOS 14.4 minimum** raised from Phase 0's 13.0. Acceptable given the project's current single-user scope; revisit if external users on older macOS appear.

---

## Carry-forward note for Phase 6 and Phase 7

- **Phase 6 (tray):** the tray menu's "Audio source" submenu calls `audio::list_sources()` and posts `AudioCommand::SwitchSource` — both APIs are unchanged in shape from `phase_05.md`, so Phase 6's plan needs no revision.
- **Phase 7 (polish):** the "stable codesigning for TCC persistence" subtask becomes optional. The Audio Capture grant persists under ad-hoc signing in our hardware testing. Keep the stable-signing task for future-proofing but reduce its priority from blocking to optional.
