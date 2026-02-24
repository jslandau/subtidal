# Test Requirements: Live Captions

## Automated Tests

### AC1: Audio is captured from the selected PipeWire source

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC1.5 | PipeWire unavailable at startup exits with clear error | integration | `tests/integration/audio_startup.rs` | Requires PipeWire to be stopped; run in isolated CI environment or with mock. Phase 3 Task 3 documents the manual verification (`systemctl --user stop pipewire`). Can be automated with a wrapper that checks exit code and stderr content. |

### AC2: Live captions are produced with acceptable latency

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC2.4 | Silence produces no spurious caption output | unit | `src/stt/moonshine.rs` (inline tests) | Feed silent PCM (all zeros or below VAD threshold) to `MoonshineEngine::process_chunk()`; assert `Ok(None)`. Parakeet variant requires model files — see integration test. |
| live-captions.AC2.4 | Inference thread suppresses empty/whitespace text | unit | `src/stt/mod.rs` (inline `tests` module) | Already defined in Phase 4 Task 1: `inference_thread_suppresses_whitespace_only_text`, `inference_thread_suppresses_none_responses`. Uses `MockEngine`. |

### AC3: Overlay displays captions correctly

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC3.7 | Caption text respects configured appearance | unit | `src/overlay/mod.rs` (inline tests) | Test that `apply_appearance()` generates correct CSS string containing the configured colors and font size. Does not require a running display. |

### AC4: System tray controls work correctly

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC4.5 | Lock overlay item greyed out in docked mode | unit | `src/tray/mod.rs` (inline tests) | Call `build_overlay_submenu()` with `overlay_mode = Docked`; assert the Lock checkmark item has `enabled: false`. |

### AC5: STT engine management

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC5.2 | Subsequent runs skip download when models present | unit | `src/models/mod.rs` (inline tests) | Create dummy files at expected model paths in a tempdir; assert `parakeet_models_present()` / `moonshine_models_present()` returns true. |
| live-captions.AC5.3 | CUDA unavailable triggers Moonshine fallback | unit | `src/stt/mod.rs` (inline tests) | Test `cuda_available()` returns bool without panic. Full fallback logic (engine selection in main.rs) can be tested by mocking the return value or testing the match arm directly. |

### AC6: Configuration persists and hot-reloads

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| live-captions.AC6.1 | Config round-trips correctly through save/load | unit | `src/config.rs` (inline `tests` module) | Already defined in Phase 1 Task 2: `config_roundtrip` test. Covers engine, overlay_mode, locked, audio_source, position fields. |
| live-captions.AC6.3 | Malformed config.toml returns defaults | unit | `src/config.rs` (inline `tests` module) | Already defined in Phase 1 Task 2: `config_malformed_toml_returns_error` and `config_missing_file_returns_default`. `Config::load()` returns defaults on parse failure. |

### Audio Resampler (supports AC2 pipeline)

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| (infra) | Resampler produces correct 160ms chunk size | unit | `src/audio/resampler.rs` (inline `tests` module) | Already defined in Phase 3 Task 1: `resampler_produces_correct_chunk_size`. |
| (infra) | Resampler accumulates partial input without output | unit | `src/audio/resampler.rs` (inline `tests` module) | Already defined: `resampler_accumulates_partial_input`. |
| (infra) | Resampler accumulates across multiple pushes | unit | `src/audio/resampler.rs` (inline `tests` module) | Already defined: `resampler_accumulates_across_multiple_pushes`. |

### Inference Thread (supports AC2 pipeline)

| AC | Description | Test Type | Test File | Notes |
|----|-------------|-----------|-----------|-------|
| (infra) | Inference thread forwards recognized text | unit | `src/stt/mod.rs` (inline `tests` module) | Already defined in Phase 4 Task 1: `inference_thread_forwards_recognized_text`. Uses `MockEngine`. |
| (infra) | Inference thread suppresses None responses | unit | `src/stt/mod.rs` (inline `tests` module) | Already defined: `inference_thread_suppresses_none_responses`. |

## Human Verification

### AC1: Audio is captured from the selected PipeWire source

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC1.1 | System output (monitor sink) captured by default on first launch | Requires a running PipeWire daemon and active audio playback to verify real audio flows through the pipeline. Cannot be unit-tested without a PipeWire mock that does not exist. | Launch the app with default config. Play audio through system output. Verify captions appear in the overlay. Phase 3 Task 3 includes a 5-second test consumer that prints chunk counts. |
| live-captions.AC1.2 | Selecting application node from tray switches capture | Requires a live PipeWire graph with an application producing audio, plus interaction with the ksni tray menu. | Play audio from a specific application (e.g., Firefox). Right-click tray, select that application under Audio Source. Verify captions now reflect that application's audio only. |
| live-captions.AC1.3 | Switching audio source does not require restart | Requires runtime PipeWire stream teardown/reconnect against a real daemon. | While captions are active, switch source via tray menu. Verify captions continue without restarting the application. No gap longer than 1 second. |
| live-captions.AC1.4 | Node disappear triggers fallback, tray update, and desktop notification | Requires a PipeWire application node to appear and then disappear (close the application). Desktop notification must be visually confirmed. | Start capturing from an application. Close that application. Verify: (1) desktop toast notification appears identifying lost source and fallback, (2) tray Audio Source radio resets to System Output, (3) captions continue from system output. Phase 8 Task 3 Step 6 documents this test. |

### AC2: Live captions are produced with acceptable latency

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC2.1 | Parakeet produces captions within 300ms of utterance end | Latency measurement requires real-time audio input with known utterance boundaries and a high-resolution timer correlated to display output. Depends on GPU hardware (CUDA). | Play a known speech recording through system audio. Use a stopwatch or instrumented logging (`Instant::now()` around `process_chunk`) to measure time from last audio chunk to caption string emission. Must be under 300ms. Phase 4 Task 4 Step 4 documents this. |
| live-captions.AC2.2 | Moonshine produces captions within 400ms of utterance end | Same latency measurement challenge. Depends on CPU performance and VAD silence detection delay (800ms silence window). | Same approach as AC2.1 but with `--engine moonshine`. Latency target is 400ms from utterance end. Phase 8 Task 1 Step 4 documents this. |
| live-captions.AC2.3 | Captions update continuously during sustained speech | Requires sustained real speech input and visual observation of continuous overlay updates without long gaps. | Play a 30+ second continuous speech recording. Observe the overlay. Captions should update at least every 1-2 seconds with no gaps longer than 3 seconds during active speech. |

### AC3: Overlay displays captions correctly

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC3.1 | Overlay appears above all other windows in docked mode | Requires a running Wayland compositor (KDE Plasma 6) with gtk4-layer-shell. Window stacking order is compositor-managed and not queryable programmatically. | Launch app with `overlay_mode = "docked"`. Open several maximized windows. Verify the caption overlay remains visible above all of them, anchored to the configured screen edge. |
| live-captions.AC3.2 | Overlay appears above all other windows in floating mode | Same compositor dependency as AC3.1. | Launch with `overlay_mode = "floating"`. Verify overlay floats above all windows. |
| live-captions.AC3.3 | Docked mode passes all pointer and keyboard events through | Click-through behavior depends on Wayland compositor honoring empty `wl_surface` input region. Cannot be verified without a compositor. | With docked overlay visible, click on content behind the overlay. Verify clicks reach the underlying window. Type on a text editor behind the overlay. Verify keystrokes reach it. |
| live-captions.AC3.4 | Floating mode locked state passes pointer events through | Same Wayland input region dependency. | Set `overlay_mode = "floating"`, `locked = true`. Click behind the overlay. Verify clicks pass through. |
| live-captions.AC3.5 | Floating mode unlocked: drag to reposition; position persists | Requires physical mouse interaction with the GTK4 window and a restart to verify persistence. | Set `overlay_mode = "floating"`, `locked = false`. Drag the overlay to a new position. Kill and restart the app. Verify it appears at the saved position. Check `config.toml` position values match. |
| live-captions.AC3.6 | Switching docked/floating at runtime works without restart | Requires tray interaction and visual confirmation of mode change on a live compositor. | While running in docked mode, switch to floating via tray. Verify window detaches from edge and becomes freely positioned. Switch back to docked. Verify it re-anchors. No restart needed. |
| live-captions.AC3.7 | Caption text respects configured appearance (visual) | While CSS generation is unit-testable, the visual rendering (actual colors, font size, transparency) depends on GTK4's CSS engine and the compositor. | Edit `config.toml`: set `background_color = "rgba(255,0,0,0.5)"`, `text_color = "#00ff00"`, `font_size = 24.0`. Restart (or wait for hot-reload). Visually confirm red semi-transparent background, green text, larger font. |
| live-captions.AC3.8 | Toggling captions off hides the overlay entirely | Requires tray left-click and visual confirmation that the window disappears from screen. | Left-click the tray icon. Verify the overlay window is no longer visible. Left-click again. Verify it reappears. |

### AC4: System tray controls work correctly

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC4.1 | Left-click tray icon toggles captions on/off | Requires D-Bus interaction with the ksni StatusNotifierItem and visual confirmation of overlay visibility change. | Left-click the tray icon. Verify overlay hides. Left-click again. Verify overlay shows. Phase 6 Task 2 Step 5 documents this. |
| live-captions.AC4.2 | Right-click menu reflects current state | Requires rendering the ksni menu on a live D-Bus session and visual inspection of radio/checkmark states. | Change engine to Moonshine via tray. Close and reopen menu. Verify Moonshine radio is selected. Repeat for audio source and overlay mode. |
| live-captions.AC4.3 | Audio source submenu shows available PipeWire sinks and streams | Requires a live PipeWire daemon with running applications. Node enumeration depends on real PipeWire registry events. | Start several audio-producing applications. Right-click tray, open Audio Source submenu. Verify System Output plus each application appears as a radio option. |
| live-captions.AC4.4 | STT engine switched at runtime without restart | Requires both model sets downloaded, CUDA for Parakeet, and live audio to verify captions continue after switch. | While captions are active with Parakeet, switch to Moonshine via tray. Verify captions continue (possibly with brief pause during model load). No application restart. Phase 8 Task 2 implements the `Arc<Mutex<SyncSender>>` swap. |

### AC5: STT engine management

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC5.1 | First run auto-downloads model files | Requires network access to HuggingFace and several hundred MB of disk space. Download time makes this impractical for CI. | Delete `~/.local/share/live-captions/models/`. Run the app. Verify download progress appears on stderr and model files appear in the expected directories. Phase 2 Task 2 Steps 3-4 document this. |
| live-captions.AC5.3 | CUDA unavailable falls back to Moonshine with tray tooltip warning (visual) | While `cuda_available()` is unit-testable, the full fallback path (automatic engine switch + tray tooltip display) requires a system without CUDA or with CUDA libraries removed. | On a machine without CUDA (or with `libcudart` renamed): launch with default config (engine = parakeet). Verify app starts with Moonshine, tray title shows CUDA warning. Phase 4 Task 4 Step 5 documents this. |
| live-captions.AC5.4 | Model download failure exits with clear error | Requires simulating network failure or corrupted download. Could be automated with network mocking but the hf-hub client has no test hook. | Disconnect network. Delete model files. Run the app. Verify it exits with error message pointing to the missing model and suggesting to check connectivity. Phase 2 Task 2 Step 5 documents this. |

### AC6: Configuration persists and hot-reloads

| AC | Description | Why Not Automated | Verification Approach |
|----|-------------|-------------------|----------------------|
| live-captions.AC6.1 | Audio source, engine, overlay mode, position persist across restarts | Persistence is partially covered by the config roundtrip unit test. Full verification requires tray interaction, app restart, and confirming restored state matches. | Via tray, change audio source, engine, and overlay mode. Kill and restart the app. Verify all three settings are restored. Check `config.toml` contains the expected values. Phase 7 Task 3 Step 5 documents this. |
| live-captions.AC6.2 | Config.toml appearance edit applies within 1 second | Requires a running app with `notify` file watcher and visual confirmation of overlay change timing. The 1-second latency requirement is a real-time constraint. | While app is running, edit `config.toml` to change `background_color`. Save. Time the visual change on the overlay. Must occur within 1 second. Phase 7 Task 3 Step 3 documents this. |

## Coverage Summary

| Category | Count |
|----------|-------|
| Automated (unit tests, defined in implementation plan) | 10 |
| Automated (unit tests, new recommended) | 3 |
| Human verification required | 22 |
| **Total AC verification points** | **35** |

All 28 acceptance criteria (AC1.1 through AC6.3) are mapped above. Some ACs appear in both automated and human verification sections where a unit test covers part of the criterion (e.g., AC3.7 CSS generation is unit-testable but visual rendering requires human eyes; AC6.3 parse failure is unit-tested but startup-with-defaults behavior requires integration confirmation).
