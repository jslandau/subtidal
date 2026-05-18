# macOS Port — Phase 4: ScreenCaptureKit audio capture (SystemOutput)

**Goal:** Capture system audio via ScreenCaptureKit and feed it through the existing STT pipeline. Handle the macOS TCC permission flow on first launch and verify the grant persists across rebuilds.

**Architecture:** `audio::start_audio_thread(audio_wake)` on macOS creates a ring buffer (same `HeapRb<f32>` capacity as Linux — 96 000 elements), spawns a `screen-capture-audio` worker thread that constructs an `SCStream` with SystemOutput filter, attaches an Obj-C `SCStreamOutput`/`SCStreamDelegate` subclass (built via `objc2::define_class!`) whose audio callback extracts PCM from `CMSampleBuffer` → `CMBlockBuffer` and pushes into the ring buffer with RT-safety discipline (no allocation, no logging, `try_lock` only, `audio_wake.notify()`). `startCapture`/`stopCapture` are awaited via `tokio` `block_on`. `main_macos` replaces the Phase-3 fixture harness with this real capture.

**Tech Stack:** `objc2-screen-capture-kit` 0.3 (`SCStream`, `SCShareableContent`, `SCError`, `block2`, `dispatch2`, `objc2-core-media` features), `objc2-core-media` 0.3 (`CMSampleBuffer`, `CMBlockBuffer`, `CMFormatDescription`, `CMTime`, `CMAttachment`), `objc2-core-audio-types` 0.3 (`AudioStreamBasicDescription`, `AudioBufferList`), `ringbuf` (re-used), `tokio` current-thread for SCK async bridging, neutral `AudioWake`.

**Scope:** Phase 4 of 8. Phase 5 adds per-app capture, live source switching, source-disappeared fallback (extends `start_audio_thread` return tuple and adds neutral `AudioSource`/`AudioSourceId` types in `audio/mod.rs`).

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

### macos-port.AC3: ScreenCaptureKit audio capture
- **macos-port.AC3.1 Success:** Selecting "System Output" as the audio source captures all system audio; playing a video produces real-time captions.
- **macos-port.AC3.5 Success:** First-run launch surfaces the macOS Screen Recording permission prompt with the text from `NSScreenCaptureUsageDescription`; after granting, captures begin.
- **macos-port.AC3.6 Failure:** Refusing the Screen Recording permission produces a user-visible error (NSUserNotification or in-panel message), not a silent crash.
- **macos-port.AC3.7 Edge:** SCK callback maintains RT-safety discipline: no allocation, no logging, try_lock only, copy-and-return (verified by code review and a debug-build instrumentation that asserts no `Mutex::lock` calls inside the callback).

### macos-port.AC7: TCC permission stability
- **macos-port.AC7.1 Success:** Granting Screen Recording permission once persists across `cargo build && scripts/bundle-mac.sh && open Subtidal.app` cycles, as long as bundle ID stays `com.subtidal.app` and ad-hoc signing is re-applied.

AC3.2 / AC3.3 / AC3.4 (per-app capture, live source switching, source-disappeared fallback) land in Phase 5. AC7.2 (TCC re-prompt on bundle ID change — a "Failure" criterion verified by deliberately altering the bundle ID once) is part of Phase 7's AC walkthrough.

**On AC3.7's debug-build instrumentation (user-approved deviation, 2026-05-18):** the design wording calls for "a debug-build instrumentation that asserts no `Mutex::lock` calls inside the callback." Practically, `std::sync::Mutex::lock` is not overridable from outside std; the runtime guard would require either an interception shim or a custom mutex type. Phase 4 ships a documented code-review verification + a `// RT-SAFE: ...` header comment on the callback enumerating the rules, matching the existing Linux PipeWire-callback precedent. The executor does NOT need to re-surface this scope decision — it was explicitly accepted during plan finalization. A future hardening could swap `std::sync::Mutex` for a custom `RtMutex` wrapper that records call-sites in debug builds; that work belongs in a separate design plan covering both platforms.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add SCK + CoreMedia + CoreAudioTypes deps

**Files:**
- Modify: `Cargo.toml` (macOS target block)

**Implementation:**

Append to the macOS-conditional dep block (after the objc2-app-kit entry from Phase 2):

```toml
objc2-screen-capture-kit = { version = "0.3", features = [
    "SCStream", "SCShareableContent", "SCError",
    "objc2-core-media", "block2", "dispatch2",
] }
objc2-core-media = { version = "0.3", features = [
    "CMSampleBuffer", "CMBlockBuffer", "CMFormatDescription", "CMTime", "CMAttachment",
] }
objc2-core-audio-types = "0.3"  # AudioStreamBasicDescription, AudioBufferList
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin --verbose
```
Expected: green.

**Commit:** `macos: add ScreenCaptureKit, CoreMedia, CoreAudioTypes deps`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Audio impl_macos skeleton + start_audio_thread

**Files:**
- Modify: `src/audio/impl_macos.rs` (convert to `src/audio/impl_macos/mod.rs` directory module — see note below)
- Modify: `src/audio/mod.rs` — add macOS re-exports

**Implementation:**

**File restructure:** since Phase 4 introduces submodules (`stream`, `normalize` in Task 3), convert `src/audio/impl_macos.rs` to a directory:
1. `git mv src/audio/impl_macos.rs src/audio/impl_macos/mod.rs`
2. Future submodules live as siblings of `mod.rs`.

**`src/audio/mod.rs`** — extend the macOS gate from Phase 1:

```rust
#[cfg(target_os = "macos")]
mod impl_macos;

#[cfg(target_os = "macos")]
pub use impl_macos::{start_audio_thread, AudioCommand};
```

Linux re-exports stay; cfg makes the names non-overlapping.

**`src/audio/impl_macos/mod.rs`:**

```rust
//! macOS audio capture (ScreenCaptureKit). Phase 4 ships SystemOutput-only
//! capture; per-app capture and source switching land in Phase 5.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use ringbuf::HeapRb;
use ringbuf::traits::Split;

use crate::stt::AudioWake;

mod stream;     // SCStream + delegate (Task 3)
mod normalize;  // CMSampleBuffer → 48kHz stereo f32 (Task 3 + 5)

/// Commands sent to the audio thread. Phase 4 ships only `Shutdown`; Phase 5
/// adds `SwitchSource(AudioSourceId)`.
pub enum AudioCommand {
    Shutdown,
}

/// Public entry point — symmetric with `audio::impl_linux::start_audio_thread`.
/// Phase 4 tuple has 2 elements; Phase 5 widens to add a fallback-event
/// receiver and surfaces neutral `AudioSource` types.
pub fn start_audio_thread(
    audio_wake: Arc<AudioWake>,
) -> Result<(SyncSender<AudioCommand>, ringbuf::HeapCons<f32>)> {
    // Same capacity as Linux: 48000 frames × 2 channels = 96_000 f32 elements.
    const RING_BUF_CAPACITY: usize = 96_000;
    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(RING_BUF_CAPACITY).split();
    let (tx_cmd, rx_cmd) = sync_channel::<AudioCommand>(8);

    // Wrap producer in Arc<Mutex<>> so the SCK delegate (running on SCK's
    // internal dispatch queue) and the worker thread can both reference it.
    // RT-SAFE: the delegate uses try_lock only — see stream::Delegate doc.
    let ring_producer = Arc::new(Mutex::new(ring_producer));

    let producer_for_thread = Arc::clone(&ring_producer);
    let wake_for_thread = Arc::clone(&audio_wake);
    std::thread::Builder::new()
        .name("screen-capture-audio".into())
        .spawn(move || {
            if let Err(e) = run_sck_capture(producer_for_thread, wake_for_thread, rx_cmd) {
                eprintln!("error: SCK capture exited: {e:#}");
            }
        })?;

    Ok((tx_cmd, ring_consumer))
}

fn run_sck_capture(
    _ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    _audio_wake: Arc<AudioWake>,
    _rx_cmd: Receiver<AudioCommand>,
) -> Result<()> {
    // Task 3 fills this in.
    anyhow::bail!("run_sck_capture not yet implemented (Phase 4 Task 3)")
}
```

(Returning an `Err` instead of `todo!()` keeps the binary launchable for cargo check / cross-target verification without crashing if accidentally exercised at runtime between Task 2 and Task 3.)

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```
Cross-target green.

**Commit:** `macos: audio impl_macos skeleton + AudioCommand + start_audio_thread`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: SCStream construction + RT-safe audio callback

**Verifies:** macos-port.AC3.7 (RT-safety contract; runtime instrumentation deferred per scope decision above)

**Files:**
- Create: `src/audio/impl_macos/stream.rs`
- Create: `src/audio/impl_macos/normalize.rs`
- Modify: `src/audio/impl_macos/mod.rs` — replace the `bail!` body of `run_sck_capture` with the real flow

**Implementation:**

**`stream.rs`** — define an Obj-C subclass conforming to `SCStreamOutput` + `SCStreamDelegate`:

```text
use objc2::define_class;
use objc2::rc::Retained;
use objc2_screen_capture_kit::{SCStream, SCStreamOutput, SCStreamOutputType, ...};
use objc2_core_media::CMSampleBuffer;

define_class!(
    /// Subtidal SCK delegate. Stores a clone of the producer Arc + wake.
    /// All UI work (none here) routed via dispatch2; this lives entirely
    /// on SCK's internal dispatch queue.
    pub struct Delegate {
        producer: Arc<Mutex<HeapProd<f32>>>,
        wake: Arc<AudioWake>,
    }

    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl SCStreamOutput for Delegate {
        // RT-SAFE callback. Rules enforced by review (see header comment above):
        //  * no allocation in hot path
        //  * no eprintln!/println!/log/tracing
        //  * try_lock only on the ring producer; drop sample on contention
        //  * return promptly (target ~50us)
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_didOutput(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Audio { return; }
            // Borrow PCM as &[f32]; None on format mismatch.
            let Some(pcm) = normalize::extract_pcm(sample_buffer) else { return; };
            if let Ok(mut prod) = self.producer.try_lock() {
                use ringbuf::traits::Producer;
                let _ = prod.push_slice(pcm);  // overflow → drop silently
            }
            self.wake.notify();
        }
    }
    unsafe impl SCStreamDelegate for Delegate {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_didStopWithError(&self, _stream: &SCStream, error: &NSError) {
            // Phase 4: log only. Phase 5 posts a fallback event.
            eprintln!("warn: SCStream stopped: {}", error.localizedDescription());
        }
    }
);

impl Delegate {
    pub fn new(producer: Arc<Mutex<HeapProd<f32>>>, wake: Arc<AudioWake>) -> Retained<Self> {
        let mtm = ... // delegate is created off-main-thread; but Obj-C alloc/init
                       // doesn't require MainThreadMarker for NSObject subclasses.
        // Use Self::alloc().set_ivars(...).init_with_...() per objc2 0.6 idiom.
        unimplemented!("executor: follow objc2 0.6 ivars-init pattern")
    }
}
```

**`stream::build_stream`** — produces the configured `Retained<SCStream>`:

```text
pub fn build_stream(
    content: &SCShareableContent,
    producer: Arc<Mutex<HeapProd<f32>>>,
    wake: Arc<AudioWake>,
) -> Result<Retained<SCStream>> {
    let display = content.displays().first().context("no displays available")?;
    let filter = SCContentFilter::alloc().initWithDisplay_excludingApplications_exceptingWindows(
        display, &NSArray::new(), &NSArray::new(),
    );
    let config = SCStreamConfiguration::new();
    unsafe {
        config.setCapturesAudio(true);
        config.setExcludesCurrentProcessAudio(true);
        // Omit setCapturesVideo — defaults to true on some SCK versions; explicitly disable:
        config.setCapturesVideo(false);
        config.setSampleRate(48_000);
        config.setChannelCount(2);
    }
    let delegate = Delegate::new(producer, wake);
    let stream = SCStream::alloc().initWithFilter_configuration_delegate(&filter, &config, Some(&delegate))
        .context("SCStream init")?;
    // Add the same delegate as the audio output sink, on SCK's dispatch queue.
    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            &delegate, SCStreamOutputType::Audio, None, /* error out-param */
        )?;
    }
    Ok(stream)
}
```

**`normalize::extract_pcm`** — `CMSampleBuffer` → `Option<&[f32]>`:

```text
pub fn extract_pcm(sb: &CMSampleBuffer) -> Option<&[f32]> {
    unsafe {
        let fd = sb.formatDescription()?;
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(fd.as_ptr())?;
        let asbd = &*asbd;
        // Validate: 48000 Hz, 2 channels, f32 little-endian, packed.
        if asbd.mSampleRate as u32 != 48_000 { return None; }
        if asbd.mChannelsPerFrame != 2 { return None; }
        if asbd.mFormatID != kAudioFormatLinearPCM { return None; }
        let flags = asbd.mFormatFlags;
        let is_float = flags & kAudioFormatFlagIsFloat != 0;
        let is_packed = flags & kAudioFormatFlagIsPacked != 0;
        if !is_float || !is_packed || asbd.mBitsPerChannel != 32 { return None; }
        // Extract the data buffer.
        let bb = sb.dataBuffer()?;
        let mut len: usize = 0;
        let mut ptr: *mut std::os::raw::c_char = std::ptr::null_mut();
        let status = CMBlockBufferGetDataPointer(
            bb.as_ptr(), 0, std::ptr::null_mut(), &mut len, &mut ptr,
        );
        if status != 0 || ptr.is_null() { return None; }
        let n_samples = len / 4;  // 4 bytes per f32
        Some(std::slice::from_raw_parts(ptr as *const f32, n_samples))
    }
}
```

(The exact API names/casing differ per the objc2 binding version; executor adapts.)

**`run_sck_capture` body** in `impl_macos/mod.rs`:

```text
fn run_sck_capture(
    ring_producer: Arc<Mutex<HeapProd<f32>>>,
    audio_wake: Arc<AudioWake>,
    rx_cmd: Receiver<AudioCommand>,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

    let stream = rt.block_on(async {
        let content = stream::shareable_content_current().await
            .context("SCShareableContent — is Screen Recording permission granted?")?;
        stream::build_stream(&content, Arc::clone(&ring_producer), Arc::clone(&audio_wake))
    })?;

    rt.block_on(async {
        stream::start_capture(&stream).await
            .context("SCStream.startCapture — TCC denied?")
    })?;

    loop {
        match rx_cmd.recv() {
            Ok(AudioCommand::Shutdown) | Err(_) => break,
        }
    }

    rt.block_on(async {
        let _ = stream::stop_capture(&stream).await;  // best-effort
    });
    Ok(())
}
```

`shareable_content_current`, `start_capture`, `stop_capture` are async wrappers bridging SCK's completion-handler APIs to Rust futures (use `tokio::sync::oneshot` + `block2`-closured handlers).

**TCC failure handling (AC3.6):** if `shareable_content_current` or `start_capture` returns `Err(SCError::userDeclined)` (or equivalent), post an `NSUserNotification`:
- Title: "Subtidal needs Screen Recording permission"
- Body: "Open System Settings → Privacy & Security → Screen Recording, enable Subtidal, then relaunch."

Then propagate the `Err` up so the audio thread exits cleanly. The pipeline starves silently (no captions); the NSPanel stays visible but empty. Phase 6's tray surfaces a more prominent "no audio" indicator.

For Phase 4, `NSUserNotification` requires either the `objc2-foundation` `NSUserNotification` feature (deprecated in macOS 11+ but still works) or migration to `UserNotifications.framework` (`objc2-user-notifications`). Phase 4 uses whichever is simpler — surface the choice in the commit body.

**Header comment on the callback** (required for AC3.7 code-review verification):

```rust
// RT-SAFE: this callback runs on ScreenCaptureKit's internal dispatch queue.
// MUST NOT:
//   - allocate (no Vec::push beyond ring capacity, no String::new, no Box::new)
//   - call eprintln!/println!/log/tracing
//   - call Mutex::lock (use try_lock only)
//   - call any blocking I/O
// Target callback duration: <50us. Drop samples silently on contention.
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```
Cross-target green. Hardware exercise in Task 6.

**Commit:** `macos: SCStream construction + RT-safe audio callback`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Wire SCK capture into main_macos.rs

**Verifies:** macos-port.AC3.1, macos-port.AC3.5, macos-port.AC3.6, macos-port.AC7.1

**Files:**
- Modify: `src/main_macos.rs` — replace Phase 3's fixture harness with SCK capture

**Implementation:**

Remove the `phase3-wav-harness` thread and its dependency on the fixture WAV. Replace with:

```text
let (audio_cmd_tx, ring_consumer) = audio::start_audio_thread(Arc::clone(&audio_wake))
    .unwrap_or_else(|e| {
        eprintln!("error: failed to start audio capture: {e:#}");
        eprintln!("hint: did you grant Screen Recording permission to Subtidal.app?");
        std::process::exit(1);
    });
```

(Tuple has 2 elements on macOS in Phase 4; 4 on Linux. Phase 5 widens macOS to add the fallback rx.)

Pass `ring_consumer` to `stt::spawn_stt_thread(...)` — unchanged from Phase 3 except the source.

Update the Ctrl-C handler to also signal the audio thread (mirrors `src/main.rs:287-300`):

```text
let cmd_tx_signal = cmd_tx.clone();
let audio_tx_signal = audio_cmd_tx.clone();
let wake_for_signal = Arc::clone(&audio_wake);
ctrlc::set_handler(move || {
    wake_for_signal.shutdown();
    let _ = audio_tx_signal.send(audio::AudioCommand::Shutdown);
    let _ = cmd_tx_signal.send_blocking(overlay::OverlayCommand::Quit);
})
.expect("install ctrlc handler");
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
cargo check --lib
```

Hardware verification in Task 6.

**Commit:** `macos: wire SCK capture into main_macos`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Unit test for format normalization

**Verifies:** macos-port.AC3.7 (callback contract — input/output shape correctness)

**Files:**
- Modify: `src/audio/impl_macos/normalize.rs` — add `#[cfg(all(test, target_os = "macos"))] mod tests`

**Implementation:**

Test `normalize::extract_pcm` by constructing a `CMSampleBuffer` programmatically via `CMSampleBufferCreate` with controlled `AudioStreamBasicDescription` and `CMBlockBuffer` contents. Read docs.rs/objc2-core-media/0.3 for exact constructor signatures.

Cases:
1. **48 kHz stereo f32 packed:** returns a `&[f32]` slice of the expected sample count (e.g., 1024 frames × 2 ch = 2048 samples).
2. **44.1 kHz stereo:** returns `None`.
3. **48 kHz mono:** returns `None`.
4. **48 kHz stereo i16:** returns `None` (Phase 4 doesn't convert formats; logs once and drops via the `None` path).

**If constructing a `CMSampleBuffer` programmatically is impractical** (requires Core Audio scaffolding beyond reasonable Phase 4 effort): skip the unit test. Document this in the commit body and rely on Task 6's operational verification. Surface the decision to the user before committing.

**Verification:**

On macOS:
```bash
cargo test --lib --target aarch64-apple-darwin -- normalize::tests
```

**Commit:** `macos: unit test for SCK PCM format normalization` (or `macos: defer normalize unit test to operational verification` if skipped)
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: End-to-end hardware verification

**Verifies:** macos-port.AC3.1, macos-port.AC3.5, macos-port.AC3.6, macos-port.AC3.7 (operational), macos-port.AC7.1

**Files:** none (operational verification only)

**Implementation:**

On the target Apple Silicon Mac:

```bash
scripts/bundle-mac.sh
open target/release/Subtidal.app
```

Walk each criterion:

1. **AC3.5 (first-run TCC prompt):** the macOS "Subtidal would like to record this computer's screen" dialog appears, with the body text from `Info.plist`'s `NSScreenCaptureUsageDescription`. Click "Open System Settings", enable Subtidal under Privacy & Security → Screen Recording, relaunch.

2. **AC3.1 (system audio capture):** play audio from any other app (a YouTube video tab is the easy test). Within ~3 seconds, captions appear in the NSPanel.

3. **AC3.6 (denied permission):** in System Settings → Privacy & Security → Screen Recording, disable Subtidal. Relaunch. An `NSUserNotification` appears with the "needs Screen Recording" message; the app does not crash. The NSPanel may stay visible but empty.

4. **AC3.7 (RT-safety):** code-review the `Delegate::stream_didOutput` callback (`src/audio/impl_macos/stream.rs`). Confirm against the rules in the `// RT-SAFE: ...` header comment:
   - No allocation in the hot path
   - No `eprintln!`/`println!`/`log`/`tracing`
   - `try_lock` only
   - Returns promptly
   Document the audit in a follow-up commit if any findings warrant.

5. **AC7.1 (TCC stability):** run `scripts/bundle-mac.sh && open target/release/Subtidal.app` again. No re-prompt — the previous grant persists. Repeat once more to confirm stability across multiple rebuilds.

6. **Clean shutdown (AC8.2):** Cmd-Q (when focused) or Ctrl-C from the launching terminal terminates cleanly. After exit:
   ```bash
   pgrep -f subtidal     # should return nothing
   lsof -p $(pgrep -f Subtidal) 2>/dev/null    # no orphan SCK file descriptors
   ```

7. **Force-close orphan-stream check (AC8.3):** relaunch the app and start a System Output capture. With audio actively flowing, force-kill via Activity Monitor *or* `kill -9 $(pgrep -f Subtidal)`. Then immediately:
   ```bash
   sleep 1
   pgrep -f Subtidal     # no surviving process
   lsof | grep -iE 'subtidal|ScreenCaptureKit|replayd' | grep -v grep     # no orphan SCK descriptors held by replayd or anyone else on Subtidal's behalf
   ```
   SCK runs out-of-process (`replayd`), so the test is: after Subtidal is gone, `replayd` should not be holding capture sessions that belonged to us. If the second `lsof` line shows lingering Subtidal-attributable handles, treat it as an AC8.3 failure.

Any failure: surface to the user with logs (`/tmp/subtidal-phase4.log`) before proceeding to Phase 5.

**Commit:** none (verification only).
<!-- END_TASK_6 -->
