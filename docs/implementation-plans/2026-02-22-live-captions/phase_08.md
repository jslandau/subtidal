# Live Captions Implementation Plan — Phase 8: Full Integration and Error Handling

**Goal:** Wire all subsystems into a complete, robust startup sequence. Add PipeWire reconnection with fallback, inference engine runtime switching, Moonshine tokenizer, graceful SIGTERM/SIGINT shutdown, and AC1.4 desktop fallback notification.

**Architecture:** `src/main.rs` becomes the integration point for all subsystems. PipeWire reconnect logic moves into the audio thread. Engine switching restarts the inference thread. Shutdown coordinates all threads via cancellation channels. `notify-rust` sends desktop toasts for AC1.4.

**Tech Stack:** All from previous phases + `tokenizers 0.20` (Moonshine tokenizer), `ctrlc 3` (signal handling).

**Scope:** Phase 8 of 8. Depends on all previous phases.

**Codebase verified:** 2026-02-22 — all Phase 1–7 subsystems in place.

---

## Acceptance Criteria Coverage

All ACs from Phases 1–7 are verified together in end-to-end integration. Phase 8 specifically adds:

### live-captions.AC1: Audio is captured from the selected PipeWire source
- **live-captions.AC1.4 Failure:** If the selected application node disappears, capture falls back to system output, the tray source selection updates to reflect the change, and a desktop toast notification identifies what was lost and what it fell back to

### live-captions.AC2: Live captions are produced with acceptable latency
- **live-captions.AC2.1 Success:** (end-to-end verification with Parakeet)
- **live-captions.AC2.2 Success:** (end-to-end verification with Moonshine + tokenizer)

### live-captions.AC4: System tray controls work correctly
- **live-captions.AC4.4 Success:** STT engine can be switched at runtime via tray menu without restart

---

**Additional Cargo.toml dependencies for Phase 8:**

```toml
tokenizers = "0.20"
ctrlc = "3"
```

---

<!-- START_SUBCOMPONENT_A (tasks 1-5) -->
<!-- START_TASK_1 -->
### Task 1: Add Moonshine tokenizer to MoonshineEngine

**Files:**
- Modify: `src/stt/moonshine.rs`

**Verifies:** live-captions.AC2.2 (produces actual text, not token IDs)

**Step 1: Add tokenizer field to MoonshineEngine**

Add to the `MoonshineEngine` struct:

```rust
tokenizer: tokenizers::Tokenizer,
```

**Step 2: Load tokenizer in MoonshineEngine::new()**

The `tokenizer.json` is already downloaded by Phase 2's `ensure_moonshine_models()` (included in `MOONSHINE_FILES`) and tracked by Phase 1's `moonshine_model_files()`. Load it in the constructor:

```rust
let tokenizer_path = model_dir.join("tokenizer.json");
let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
    .with_context(|| format!("loading Moonshine tokenizer from {}", tokenizer_path.display()))?;
```

**Step 3: Replace placeholder decoding in run_inference()**

Replace the debug token-ID line with:

```rust
// Decode token IDs to text.
let decoded = self.tokenizer
    .decode(&output_tokens.iter().map(|&t| t as u32).collect::<Vec<_>>(), true)
    .context("decoding Moonshine output tokens")?;
```

**Step 4: Test Moonshine with real text output**

```bash
cargo run -- --engine moonshine
# Speak: "Hello, this is a test."
# Expected: "[CAPTION] Hello, this is a test." (or similar)
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement engine runtime switching (AC4.4)

**Files:**
- Modify: `src/main.rs`
- Modify: `src/audio/mod.rs` (expose a chunk channel sender for reconnection)

**Verifies:** live-captions.AC4.4

**Step 1: Refactor inference thread management into a restartable function**

Add to `src/stt/mod.rs`:

```rust
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
```

**Step 2: Handle EngineCommand in main.rs**

Replace the engine-switch stub from Phase 6 with the full respawn. Phase 4 Task 4 already creates `chunk_tx: Arc<Mutex<SyncSender<Vec<f32>>>>` — the engine-switch handler gets a clone of the Arc and replaces the inner `SyncSender` under the lock.

```rust
// Engine-switch handler (runs on its own thread, waits for EngineCommand::Switch).
// chunk_tx is Arc<Mutex<SyncSender<Vec<f32>>>> from Phase 4 Task 4.
let caption_tx_for_switch = caption_tx.clone();
let chunk_tx_for_switch = Arc::clone(&chunk_tx); // Phase 4's Arc<Mutex<SyncSender>>

std::thread::spawn(move || {
    for cmd in engine_switch_rx.iter() {
        match cmd {
            tray::EngineCommand::Switch(new_engine_choice) => {
                eprintln!("info: switching STT engine to {new_engine_choice:?}");

                let new_engine: Box<dyn stt::SttEngine> = match new_engine_choice {
                    config::Engine::Parakeet => {
                        match stt::parakeet::ParakeetEngine::new(&models::parakeet_model_dir()) {
                            Ok(e) => Box::new(e),
                            Err(e) => {
                                eprintln!("error: failed to load Parakeet: {e:#}");
                                continue;
                            }
                        }
                    }
                    config::Engine::Moonshine => {
                        match stt::moonshine::MoonshineEngine::new(&models::moonshine_model_dir()) {
                            Ok(e) => Box::new(e),
                            Err(e) => {
                                eprintln!("error: failed to load Moonshine: {e:#}");
                                continue;
                            }
                        }
                    }
                };

                // Spawn new inference thread and get its new SyncSender.
                let (new_chunk_tx, _handle) = stt::restart_inference_thread(
                    new_engine,
                    caption_tx_for_switch.clone(),
                );

                // Atomically replace the inner SyncSender.
                // The audio bridge thread will send to the new inference thread on next chunk.
                *chunk_tx_for_switch.lock().unwrap() = new_chunk_tx;

                eprintln!("info: engine switch complete — audio bridge now targeting new engine");
            }
        }
    }
});
```

**How the swap works:** Phase 4's audio bridge thread calls `chunk_tx_for_bridge.lock().unwrap().send(chunk)` on every chunk. When the engine-switch handler replaces `*chunk_tx_for_switch.lock()`, the very next chunk the bridge thread sends will go to the new inference engine. No restart of the audio pipeline is needed.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: PipeWire node disappear fallback and reconnect (AC1.4)

**Files:**
- Modify: `src/audio/mod.rs`

**Verifies:** live-captions.AC1.4

**Step 1: Track current_source and add fallback channel to run_pipewire_loop**

Phase 3's `run_pipewire_loop` now has a `disappeared_node_ids: Arc<Mutex<Vec<u32>>>` that the `global_remove` callback populates. Phase 8 adds:
1. A `current_source` variable tracking the actively captured `AudioSource`
2. A `fallback_tx: mpsc::SyncSender<FallbackEvent>` passed into `run_pipewire_loop` for notifying main.rs

Add at the top of `src/audio/mod.rs`:

```rust
pub struct FallbackEvent {
    pub lost_name: String,
    pub lost_id: u32,
}
```

Update `run_pipewire_loop` signature:

```rust
fn run_pipewire_loop(
    initial_source: crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    node_list: NodeList,
    rx_cmd: std::sync::mpsc::Receiver<AudioCommand>,
    fallback_tx: std::sync::mpsc::SyncSender<FallbackEvent>, // new
) -> Result<()> {
    // ... existing setup ...
    let mut current_source = initial_source.clone(); // track for fallback check
    // ...
```

Update `start_audio_thread` return signature:

```rust
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
) -> Result<(
    std::sync::mpsc::SyncSender<AudioCommand>,
    ringbuf::HeapCons<f32>,
    NodeList,
    std::sync::mpsc::Receiver<FallbackEvent>,  // new
)> {
    // ...
    let (fallback_tx, fallback_rx) = std::sync::mpsc::sync_channel::<FallbackEvent>(4);
    // Pass fallback_tx into the thread spawn:
    thread::Builder::new()
        .name("pipewire-audio".to_string())
        .spawn(move || {
            if let Err(e) = run_pipewire_loop(
                initial_source,
                ring_producer_thread,
                node_list_clone,
                rx_cmd,
                fallback_tx,  // new
            ) {
                eprintln!("error: PipeWire audio thread exited: {e:#}");
                std::process::exit(1);
            }
        })
        .context("spawning PipeWire thread")?;

    Ok((tx_cmd, ring_consumer, node_list, fallback_rx))
}
```

**Step 2: Handle disappeared nodes in the drain loop**

In Phase 3's main loop, replace the Phase 3 placeholder drain comment with actual fallback logic:

```rust
// Drain disappeared nodes and check for fallback (AC1.4).
if let Ok(mut ids) = disappeared_node_ids.try_lock() {
    ids.retain(|&id| {
        // Remove from node list so tray doesn't show stale entries.
        node_list.lock().unwrap().retain(|n| n.node_id != id);

        // Check if this is our currently captured application node.
        if let crate::config::AudioSource::Application { node_id: active_id, ref node_name } =
            current_source.clone()
        {
            if active_id == id {
                eprintln!(
                    "warn: audio node {id} ({node_name}) disappeared — falling back to system output"
                );
                current_source = crate::config::AudioSource::SystemOutput;
                drop(_stream);
                match create_capture_stream(&core, &current_source, Arc::clone(&ring_producer)) {
                    Ok(s) => {
                        _stream = s;
                    }
                    Err(e) => {
                        eprintln!("error: failed to reconnect to system output: {e:#}");
                    }
                }
                let _ = fallback_tx.send(FallbackEvent {
                    lost_name: node_name.clone(),
                    lost_id: active_id,
                });
            }
        }
        false // remove from ids list
    });
}
```

Also update the `SwitchSource` handler in the loop to keep `current_source` up to date:

```rust
Ok(AudioCommand::SwitchSource(new_source)) => {
    current_source = new_source.clone(); // track new source for fallback check
    drop(_stream);
    // ... existing reconnect logic ...
}
```

**Step 3: Update main.rs to receive fallback_rx**

In `main.rs`, update the `start_audio_thread` call:

```rust
let (audio_cmd_tx, ring_consumer, node_list, fallback_rx) =
    audio::start_audio_thread(cfg.audio_source.clone())
        .unwrap_or_else(|e| {
            eprintln!("error: failed to start audio capture: {e:#}");
            std::process::exit(1);
        });
```

**Step 4: Handle FallbackEvent in main.rs**

```rust
// Capture a Tokio Handle from the runtime before spawning the plain OS thread.
// tokio::runtime::Handle::current() panics in plain threads; we must pass the
// Handle in from a scope where the runtime is live.
let tokio_handle = runtime.handle().clone();
let tray_handle_for_fallback = tray_handle.clone();
let glib_cmd_tx_for_fallback = glib_cmd_tx.clone();
std::thread::spawn(move || {
    for event in fallback_rx.iter() {
        // Desktop notification (AC1.4).
        let _ = notify_rust::Notification::new()
            .summary("Live Captions: Audio Source Lost")
            .body(&format!(
                "'{}' (id:{}) disconnected — switched to System Output.",
                event.lost_name, event.lost_id
            ))
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();

        // Update tray to reflect fallback source.
        // Uses the captured Handle to run the async update on the Tokio runtime.
        tokio_handle.block_on(async {
            tray_handle_for_fallback.update(|tray: &mut tray::TrayState| {
                tray.active_source = crate::config::AudioSource::SystemOutput;
            }).await;
        });

        // Update config.
        let mut cfg = crate::config::Config::load();
        cfg.audio_source = crate::config::AudioSource::SystemOutput;
        let _ = cfg.save();
    }
});
```
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: SIGTERM/SIGINT graceful shutdown

**Files:**
- Modify: `src/main.rs`

**Step 1: Add ctrlc handler**

Before `run_gtk_app`, add:

```rust
// Graceful shutdown on Ctrl-C / SIGTERM.
let audio_tx_for_signal = audio_cmd_tx.clone();
let glib_cmd_tx_for_signal = glib_cmd_tx.clone();
ctrlc::set_handler(move || {
    eprintln!("info: received shutdown signal, stopping...");
    // Shut down the audio thread.
    let _ = audio_tx_for_signal.send(audio::AudioCommand::Shutdown);
    // Signal GTK4 to quit cleanly via the existing glib channel.
    // overlay::OverlayCommand::Quit calls app.quit() from the GTK main thread,
    // ensuring all Drop impls run and the GTK main loop exits normally.
    let _ = glib_cmd_tx_for_signal.send(overlay::OverlayCommand::Quit);
})
.expect("setting Ctrl-C handler");
```

**Step 2: Verify clean exit**

```bash
cargo run &
sleep 3
kill -SIGTERM $!
# Expected: "info: received shutdown signal, stopping..." then clean exit.
```
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: End-to-end integration test and final verification

**Files:**
- No new files — this task verifies all ACs pass together.

**Verifies:** All ACs from live-captions.AC1 through live-captions.AC6

**Step 1: Build release binary**

```bash
cargo build --release
./target/release/live-captions
```

Expected: App starts, system tray appears, overlay shows at bottom of screen.

**Step 2: End-to-end caption test**

Speak clearly into a microphone or play a speech audio file:

```bash
# Play a speech audio file through default output:
mpv --no-video some-speech.mp3
```

Expected: Captions appear in the overlay within 300ms (Parakeet) or 400ms (Moonshine) of utterance end.

**Step 3: Silence test (AC2.4)**

Stop speaking for 10 seconds. Expected: No new caption output.

**Step 4: Engine switch test (AC4.4)**

Via tray → STT Engine → Moonshine. Expected: Engine switches without restart, captions continue.

**Step 5: Source switch test (AC1.3)**

Via tray → Audio Source → [an application]. Expected: Capture switches, captions continue. No restart needed.

**Step 6: AC1.4 fallback test**

Start capturing an application, then close that application:

Expected: Desktop toast notification appears within 2 seconds. Tray source resets to "System Output". Captions continue from system output.

**Step 7: Config hot-reload test (AC6.2)**

Edit `~/.config/live-captions/config.toml`, change `font_size = 24.0`. Expected: Overlay font increases within 1 second.

**Step 8: Malformed config test (AC6.3)**

```bash
echo "invalid" > ~/.config/live-captions/config.toml
./target/release/live-captions
# Expected: Starts with defaults (warning logged), does NOT crash.
```

**Step 9: PipeWire unavailable test (AC1.5)**

```bash
systemctl --user stop pipewire
./target/release/live-captions
# Expected: "error: failed to start audio capture: ..." and clear exit message.
systemctl --user start pipewire
```

**Step 10: Final commit**

```bash
git add src/ Cargo.toml Cargo.lock
git commit -m "feat: full integration — error handling, engine switching, PipeWire fallback, shutdown"
```
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_A -->
