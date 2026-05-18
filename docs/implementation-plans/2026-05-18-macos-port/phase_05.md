# macOS Port — Phase 5: Per-app capture + source switching

**Goal:** Enumerate running applications as audio sources; switch between sources live via `SCStream.updateContentFilter` (no flicker); auto-fall-back to SystemOutput when a captured app exits and post a desktop notification.

**Architecture:** Add a `config::AudioSource::App { bundle_id, label }` variant additive to the existing `Application { node_id, node_name }` (Linux). Surface a neutral `AudioSourceInfo { source, label }` and `FallbackEvent { previous_label, new_source }` from `audio/mod.rs`. The macOS `start_audio_thread` widens its return tuple to include `Receiver<FallbackEvent>`, takes an `initial_source` argument, and reacts to `AudioCommand::SwitchSource` by calling `SCStream.updateContentFilter`. The Phase 4 SCK delegate's `stream:didStopWithError:` body is replaced with the fallback flow: post `NSUserNotification`, send a `FallbackEvent`, and send `AudioCommand::SwitchSource(SystemOutput)` back into the worker. `main_macos` validates the persisted source against currently-running apps at startup and spawns a fallback-listener thread mirroring `src/main.rs:244`.

**Tech Stack:** `objc2-screen-capture-kit` (`SCRunningApplication.bundleIdentifier`, `SCContentFilter.initWithDisplay_includingApplications_exceptingWindows`, `SCStream.updateContentFilter`), `objc2-foundation` (`NSUserNotification` — or `objc2-user-notifications` if deprecated API is unusable).

**Scope:** Phase 5 of 8.

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

### macos-port.AC3: ScreenCaptureKit audio capture
- **macos-port.AC3.2 Success:** Selecting a specific running application as the audio source captures only that app's audio.
- **macos-port.AC3.3 Success:** Switching the audio source via the tray uses `SCStream.updateContentFilter` and does not interrupt the caption stream visibly (no panel flicker, no caption gap > 1 sample).
- **macos-port.AC3.4 Success:** When the captured app exits, an `NSUserNotification` is posted and the audio source falls back to System Output automatically.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add macOS App variant to config::AudioSource

**Files:**
- Modify: `src/config.rs:18-27` (extend the `AudioSource` enum additively)

**Implementation:**

```rust
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSource {
    /// System-wide monitor sink (default output loopback).
    #[default]
    SystemOutput,
    /// (Linux/PipeWire) A specific application's PipeWire node, identified by node ID.
    Application { node_id: u32, node_name: String },
    /// (macOS/ScreenCaptureKit) A specific application, identified by bundle ID.
    App { bundle_id: String, label: String },
}
```

`label` is the human-readable name at selection time (e.g., "Safari"), persisted so the tray menu has something sensible to show even when the app isn't currently running.

The `serde(tag = "type")` discriminator handles the TOML round-trip automatically (new variant writes `type = "app"`).

**Verification:**

```bash
cargo check --lib
cargo check --lib --target x86_64-apple-darwin
cargo test --lib
```
Existing `AudioSource` serde tests (if any) still pass; the variant is purely additive.

**Commit:** `config: add App { bundle_id, label } variant for macOS audio source`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Audio source neutral types + list_sources()

**Files:**
- Modify: `src/audio/mod.rs` — add `AudioSourceInfo` and `FallbackEvent` neutral types + macOS re-export
- Modify: `src/audio/impl_macos/mod.rs` — extend `AudioCommand`, add `list_sources()`

**Implementation:**

**`src/audio/mod.rs`:**

```rust
/// Neutral identifier surfaced to the tray menu. Phase 5 introduces this for
/// macOS; Linux's existing `AudioNode` continues alongside.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSourceInfo {
    pub source: crate::config::AudioSource,
    pub label: String,
}

/// Sent from the audio thread when a captured source disappears and the
/// thread has auto-switched to SystemOutput.
#[derive(Debug, Clone)]
pub struct FallbackEvent {
    pub previous_label: String,
    pub new_source: crate::config::AudioSource,
}

#[cfg(target_os = "macos")]
pub use impl_macos::{start_audio_thread, AudioCommand, list_sources};
```

(Linux's existing `FallbackEvent` at `impl_linux.rs:31` is unchanged — it lives in the Linux impl module. The neutral one above is used only by macOS for now. If a future refactor unifies them, that's separate work.)

**`src/audio/impl_macos/mod.rs`** — extend AudioCommand:

```rust
pub enum AudioCommand {
    Shutdown,
    /// Switch the SCStream's content filter live. Uses
    /// `SCStream.updateContentFilter` — no stop/restart.
    SwitchSource(crate::config::AudioSource),
}
```

Add `list_sources`:

```rust
/// Enumerate audio sources visible in the tray menu.
/// Returns SystemOutput plus one entry per running app with a non-nil bundle ID.
pub fn list_sources() -> Result<Vec<crate::audio::AudioSourceInfo>> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let content = rt.block_on(async { stream::shareable_content_current().await })?;
    let mut out = vec![crate::audio::AudioSourceInfo {
        source: crate::config::AudioSource::SystemOutput,
        label: "System Output".to_string(),
    }];
    unsafe {
        let apps = content.applications();
        for i in 0..apps.count() {
            let app = apps.objectAtIndex(i);
            let bundle_ns = app.bundleIdentifier();
            let name_ns = app.applicationName();
            let bundle = bundle_ns.to_string();
            let name = name_ns.to_string();
            if bundle.is_empty() { continue; }
            out.push(crate::audio::AudioSourceInfo {
                source: crate::config::AudioSource::App {
                    bundle_id: bundle,
                    label: name.clone(),
                },
                label: name,
            });
        }
    }
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    Ok(out)
}
```

(Adapt the exact objc2 method spellings to docs.rs/objc2-screen-capture-kit/0.3.)

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
cargo check --lib
```

**Commit:** `macos: AudioSourceInfo + FallbackEvent neutral types + list_sources()`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Per-app SCContentFilter + live source switching

**Verifies:** macos-port.AC3.2, macos-port.AC3.3

**Files:**
- Modify: `src/audio/impl_macos/stream.rs` — add `build_filter(&AudioSource, &SCShareableContent) -> Result<Retained<SCContentFilter>>` and `update_content_filter(&SCStream, &SCContentFilter) -> Result<()>` (async wrapper)
- Modify: `src/audio/impl_macos/mod.rs` — extend `run_sck_capture` worker loop to handle `AudioCommand::SwitchSource`

**Implementation:**

**`stream::build_filter`:**

```text
pub fn build_filter(
    source: &AudioSource,
    content: &SCShareableContent,
) -> Result<Retained<SCContentFilter>> {
    let display = content.displays().first().context("no displays available")?;
    match source {
        AudioSource::SystemOutput | AudioSource::Application { .. } => {
            // Application{node_id} is a Linux variant; defensively map to SystemOutput
            // on macOS so cross-platform configs round-trip without panic.
            Ok(SCContentFilter::alloc()
                .initWithDisplay_excludingApplications_exceptingWindows(
                    display, &NSArray::new(), &NSArray::new(),
                ))
        }
        AudioSource::App { bundle_id, .. } => {
            let apps = unsafe { content.applications() };
            let target = (0..apps.count())
                .map(|i| unsafe { apps.objectAtIndex(i) })
                .find(|a| unsafe { a.bundleIdentifier().to_string() } == *bundle_id);
            match target {
                Some(app) => {
                    let only = NSArray::from_vec(vec![app]);
                    Ok(SCContentFilter::alloc()
                        .initWithDisplay_includingApplications_exceptingWindows(
                            display, &only, &NSArray::new(),
                        ))
                }
                None => anyhow::bail!("application with bundle_id {bundle_id} not running"),
            }
        }
    }
}
```

**`stream::update_content_filter`** — wraps `SCStream.updateContentFilter(_:completionHandler:)` via `block2` + `tokio::sync::oneshot`:

```text
pub async fn update_content_filter(
    stream: &SCStream,
    filter: &SCContentFilter,
) -> Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<Retained<NSError>>>();
    let tx_cell = std::cell::RefCell::new(Some(tx));
    let handler = RcBlock::new(move |err: *mut NSError| {
        if let Some(tx) = tx_cell.borrow_mut().take() {
            let err_owned = unsafe { err.as_ref().map(|e| Retained::retain(e).unwrap_or_else(|| panic!())) };
            let _ = tx.send(err_owned);
        }
    });
    unsafe { stream.updateContentFilter_completionHandler(filter, Some(&handler)) };
    match rx.await? {
        None => Ok(()),
        Some(err) => Err(anyhow::anyhow!("updateContentFilter: {}", err.localizedDescription())),
    }
}
```

(Exact `RcBlock` / `Retained` / completion-handler signature adapts per objc2 0.6 + block2 0.6 idioms.)

**`run_sck_capture` worker loop** — handle the new command:

```text
loop {
    match rx_cmd.recv() {
        Ok(AudioCommand::Shutdown) | Err(_) => break,
        Ok(AudioCommand::SwitchSource(new_source)) => {
            let content = match rt.block_on(async { stream::shareable_content_current().await }) {
                Ok(c) => c,
                Err(e) => { eprintln!("warn: shareable_content fetch failed: {e}"); continue; }
            };
            let filter = match stream::build_filter(&new_source, &content) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("warn: cannot switch to requested source: {e}; falling back to SystemOutput");
                    let _ = fallback_tx.send(FallbackEvent {
                        previous_label: source_label(&new_source),
                        new_source: AudioSource::SystemOutput,
                    });
                    stream::build_filter(&AudioSource::SystemOutput, &content)
                        .expect("SystemOutput filter must succeed")
                }
            };
            rt.block_on(async {
                if let Err(e) = stream::update_content_filter(&stream, &filter).await {
                    eprintln!("warn: updateContentFilter failed: {e}");
                }
            });
            // Update the delegate's notion of current source so a later
            // stream:didStopWithError: can label the fallback notification correctly.
            delegate.set_current_source(new_source);
        }
    }
}
```

`source_label(&AudioSource)` is a tiny match-helper returning a display string.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```

Hardware exercise in Task 6.

**Commit:** `macos: per-app SCContentFilter + live source switching via updateContentFilter`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Source-disappeared fallback + NSUserNotification

**Verifies:** macos-port.AC3.4

**Files:**
- Modify: `src/audio/impl_macos/stream.rs` — replace Phase 4's log-only `stream:didStopWithError:` body
- Create: `src/audio/impl_macos/notify.rs` — `post_user_notification(title, body)` helper
- Modify: `src/audio/impl_macos/mod.rs` — plumb `fallback_tx` and `audio_cmd_tx` into the delegate ivars

**Implementation:**

**`stream::Delegate` ivars** (added in Phase 4; Phase 5 extends):

```text
pub struct Delegate {
    producer: Arc<Mutex<HeapProd<f32>>>,
    wake: Arc<AudioWake>,
    fallback_tx: SyncSender<FallbackEvent>,
    audio_cmd_tx: SyncSender<AudioCommand>,
    current_source: Mutex<AudioSource>,
}

impl Delegate {
    pub fn set_current_source(&self, source: AudioSource) {
        *self.current_source.lock().unwrap() = source;
    }
}
```

**`stream_didStopWithError`** — replace the Phase 4 stub body. This handler runs on SCK's dispatch queue, NOT the RT audio callback queue, so blocking is acceptable:

```text
unsafe fn stream_didStopWithError(&self, _stream: &SCStream, error: &NSError) {
    let prev = self.current_source.lock().unwrap().clone();
    let prev_label = source_label(&prev);

    let _ = notify::post_user_notification(
        "Subtidal: audio source unavailable",
        &format!("'{prev_label}' stopped producing audio. Falling back to System Output."),
    );

    let _ = self.fallback_tx.send(FallbackEvent {
        previous_label: prev_label.clone(),
        new_source: AudioSource::SystemOutput,
    });
    let _ = self.audio_cmd_tx.send(AudioCommand::SwitchSource(AudioSource::SystemOutput));
}
```

**`notify::post_user_notification`:**

```text
pub fn post_user_notification(title: &str, body: &str) -> Result<()> {
    unsafe {
        let n = NSUserNotification::new();
        n.setTitle(Some(&NSString::from_str(title)));
        n.setInformativeText(Some(&NSString::from_str(body)));
        let center = NSUserNotificationCenter::defaultUserNotificationCenter();
        center.deliverNotification(&n);
    }
    Ok(())
}
```

If `NSUserNotification` is unusable on macOS 14.4+ (it was deprecated in macOS 11; some toolchains still support it, some don't), add `objc2-user-notifications = "0.3"` to Cargo.toml's macOS block and migrate this helper to `UNUserNotificationCenter` (requires a one-time `requestAuthorization` call at app start). Surface the choice in the commit body.

**Wiring in `start_audio_thread`** — pass the new channels into `Delegate::new`:

```text
let (fallback_tx, fallback_rx) = sync_channel::<FallbackEvent>(4);
let delegate = Delegate::new(
    Arc::clone(&ring_producer),
    Arc::clone(&audio_wake),
    fallback_tx.clone(),
    tx_cmd.clone(),                 // for self-issued SwitchSource on stream-stop
    initial_source.clone(),
);
// Return: (tx_cmd, ring_consumer, fallback_rx)
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```

Hardware test in Task 6.

**Commit:** `macos: source-disappeared fallback to SystemOutput + NSUserNotification`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Widen start_audio_thread tuple + wire fallback rx in main_macos.rs

**Files:**
- Modify: `src/audio/impl_macos/mod.rs` — `start_audio_thread` now takes `initial_source` and returns a 3-tuple
- Modify: `src/main_macos.rs` — accept the new tuple element + spawn a fallback-listener thread (mirrors Linux `src/main.rs:244`)

**Implementation:**

**`start_audio_thread` new signature:**

```text
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
    audio_wake: Arc<AudioWake>,
) -> Result<(
    SyncSender<AudioCommand>,
    ringbuf::HeapCons<f32>,
    Receiver<crate::audio::FallbackEvent>,
)>
```

The body uses `initial_source` when constructing the initial `SCContentFilter` (replaces the Phase 4 hard-coded SystemOutput).

**`main_macos.rs`** updated call site:

```text
let (audio_cmd_tx, ring_consumer, fallback_rx) = audio::start_audio_thread(
    cfg.audio_source.clone(),
    Arc::clone(&audio_wake),
).unwrap_or_else(|e| {
    eprintln!("error: failed to start audio capture: {e:#}");
    std::process::exit(1);
});

// Validate persisted source against currently-running apps; fall back if absent.
if let config::AudioSource::App { bundle_id, .. } = &cfg.audio_source {
    let sources = audio::list_sources().unwrap_or_default();
    let present = sources.iter().any(|s| matches!(
        &s.source, config::AudioSource::App { bundle_id: b, .. } if b == bundle_id
    ));
    if !present {
        eprintln!("info: saved app '{bundle_id}' is not running; falling back to SystemOutput");
        cfg.audio_source = config::AudioSource::SystemOutput;
        let _ = audio_cmd_tx.send(audio::AudioCommand::SwitchSource(config::AudioSource::SystemOutput));
    }
}

// Spawn fallback listener (mirrors src/main.rs:244 Linux pattern).
std::thread::Builder::new()
    .name("audio-fallback-listener".into())
    .spawn(move || {
        while let Ok(ev) = fallback_rx.recv() {
            eprintln!(
                "info: audio source '{}' disappeared; switched to {:?}",
                ev.previous_label, ev.new_source
            );
            // The NSUserNotification was already posted from the delegate.
            // Future hardening (Phase 6+): also update tray checkmark state.
        }
    })?;
```

Update the Ctrl-C handler — already sending `AudioCommand::Shutdown` from Phase 4; no change needed.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
cargo check --lib
```

**Commit:** `macos: widen start_audio_thread tuple + wire fallback listener in main_macos`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Unit test + hardware verification

**Verifies:** macos-port.AC3.2, macos-port.AC3.3, macos-port.AC3.4

**Files:**
- Modify: `src/audio/impl_macos/mod.rs` — add `#[cfg(all(test, target_os = "macos"))] mod tests`

**Implementation (unit test):**

```rust
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::config::AudioSource;

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
```

**Implementation (hardware walkthrough):**

On the target Apple Silicon Mac, with Screen Recording granted and Safari (with a YouTube tab) open:

1. **AC3.2 (per-app capture):** edit `~/Library/Application Support/Subtidal/config.toml`:
   ```toml
   [audio_source]
   type = "app"
   bundle_id = "com.apple.Safari"
   label = "Safari"
   ```
   Save → config hot-reload picks it up (Phase 6's tray will offer the menu choice; for Phase 5 the config edit is the trigger) → captions now reflect ONLY Safari audio. Verify by playing audio in another app and confirming it's NOT captured.

2. **AC3.3 (live switching without flicker):** edit the config back to `type = "system_output"` while audio is playing. The NSPanel should not flicker; captions continue with at most a sub-second gap.

3. **AC3.4 (source-disappeared fallback):** while capturing Safari, Cmd-Q Safari. An `NSUserNotification` should appear ("Subtidal: audio source unavailable") and capture switches to SystemOutput automatically. `/tmp/subtidal-phase5.log` shows:
   ```
   info: audio source 'Safari' disappeared; switched to SystemOutput
   ```

4. **Cross-target CI:**
   ```bash
   cargo check --lib --target x86_64-apple-darwin
   ```
   Green.

**Note on AC3.3 trigger source:** Phase 5 verifies AC3.3 via the config hot-reload path (the design's intended trigger is the tray menu, which lands in Phase 6). The underlying mechanism — `SCStream.updateContentFilter` — is the same; the trigger path differs. Phase 6 re-verifies via the tray.

**Commit:** `macos: list_sources unit test + Phase 5 hardware verification notes`
<!-- END_TASK_6 -->
