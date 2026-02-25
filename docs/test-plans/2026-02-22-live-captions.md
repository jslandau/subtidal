# Live Captions — Human Test Plan

## Prerequisites

- Linux system with Wayland session running
- PipeWire audio daemon running (`systemctl --user status pipewire`)
- Both model sets downloaded (Parakeet in `~/.local/share/live-captions/models/parakeet/`, Moonshine in `~/.local/share/live-captions/models/moonshine/`)
- Application built: `cargo build --release`
- Unit tests passing: `cargo test` (26 tests)

## Phase 1: Audio Capture (AC1)

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Launch `live-captions` with default config. Play a YouTube video or music through system speakers. | Captions appear in the overlay reflecting spoken words from the audio. |
| 1.2 | Open Firefox and play audio. Right-click tray icon, open Audio Source submenu, select Firefox. | Captions now reflect only Firefox audio. Other system sounds are ignored. |
| 1.3 | While capturing Firefox, switch Audio Source back to System Output via tray. | Captions resume from system output within 1 second. No restart occurred. |
| 1.4 | Set Audio Source to a specific application. Close that application. | Desktop notification appears identifying the lost source and fallback. Tray Audio Source resets to System Output. Captions continue from system output. |
| 1.5 | Stop PipeWire: `systemctl --user stop pipewire`. Launch `live-captions`. | Application exits with a clear error message on stderr mentioning PipeWire. Exit code is non-zero. Restart PipeWire afterward: `systemctl --user start pipewire`. |

## Phase 2: Latency and Caption Quality (AC2)

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | With Parakeet engine, play a known speech recording. Observe overlay timing. | Captions appear within approximately 300ms of each utterance ending. |
| 2.2 | Switch to Moonshine engine via tray. Play the same speech recording. | Captions appear (note: Moonshine uses placeholder inference, output will not be meaningful text). |
| 2.3 | Play a 30+ second continuous speech recording. | Captions update at least every 1-2 seconds. No gap longer than 3 seconds during active speech. |

## Phase 3: Overlay Display (AC3)

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Launch with `overlay_mode = "docked"` in config.toml. Open several maximized windows. | Caption overlay remains visible above all windows, anchored to screen edge. |
| 3.2 | Launch with `overlay_mode = "floating"`. | Overlay floats above all windows. |
| 3.3 | In docked mode, click on content behind the overlay. Type in a text editor behind it. | Clicks and keystrokes pass through to the underlying window. |
| 3.4 | Set `overlay_mode = "floating"`, `locked = true`. Click behind overlay. | Clicks pass through to the window behind the overlay. |
| 3.5 | Set `overlay_mode = "floating"`, `locked = false`. Drag the overlay to a new position. Kill the app (`pkill live-captions`). Restart it. | Overlay appears at the saved position. Open `~/.config/live-captions/config.toml` and verify `position` values match. |
| 3.6 | While running in docked mode, switch to floating via tray. Then switch back to docked. | Window detaches from edge when floating, re-anchors when docked. No restart needed. |
| 3.7 | Edit `config.toml`: set `background_color = "rgba(255,0,0,0.5)"`, `text_color = "#00ff00"`, `font_size = 24.0`. Save. | Overlay shows red semi-transparent background, green text, noticeably larger font. |
| 3.8 | Left-click the tray icon. | Overlay disappears. Left-click again: overlay reappears. |

## Phase 4: Tray Controls (AC4)

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Left-click tray icon twice. | Overlay toggles off then on. |
| 4.2 | Change engine to Moonshine via tray right-click menu. Close menu. Reopen it. | Moonshine radio is selected. Repeat for audio source and overlay mode. |
| 4.3 | Start several audio-producing applications. Right-click tray, open Audio Source submenu. | System Output plus each running application appears as a radio option. |
| 4.4 | While captions are active with Parakeet, switch to Moonshine via tray. | Captions continue after a brief pause for model loading. No application restart. |

## Phase 5: STT Engine Management (AC5)

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | Delete `~/.local/share/live-captions/models/`. Launch the app. | Download progress appears on stderr. Model files appear in expected directories. App starts after download. |
| 5.3 | On a machine without CUDA: launch with default config (engine = parakeet). | App starts with Moonshine engine. Warning logged about CUDA unavailability. |
| 5.4 | Disconnect network. Delete model files. Launch the app. | App exits with an error message about missing models. |

## Phase 6: Configuration Persistence (AC6)

| Step | Action | Expected |
|------|--------|----------|
| 6.1 | Via tray, change audio source, engine, and overlay mode. Kill and restart the app. | All three settings are restored. Verify values in `~/.config/live-captions/config.toml`. |
| 6.2 | While app is running, edit `config.toml` to change `background_color`. Save. | Overlay background changes within 1 second. |

## End-to-End: Full Session Lifecycle

1. Delete config file and model directory. Launch `live-captions`.
2. Verify models download and captions begin with default settings.
3. Play audio. Verify captions appear.
4. Switch audio source to a specific application via tray.
5. Switch engine via tray. Verify captions continue.
6. Switch overlay mode docked to floating. Drag overlay to new position.
7. Lock overlay via tray. Verify clicks pass through.
8. Edit `config.toml` appearance. Verify hot-reload within 1 second.
9. Kill and restart the app. Verify all settings restored.
10. Close the captured application. Verify fallback notification and continued captioning.

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | -- | 1.1 |
| AC1.2 | -- | 1.2 |
| AC1.3 | -- | 1.3 |
| AC1.4 | -- | 1.4 |
| AC1.5 | -- (manual-only) | 1.5 |
| AC2.1 | -- | 2.1 |
| AC2.2 | -- | 2.2 |
| AC2.3 | -- | 2.3 |
| AC2.4 | `moonshine::silence_rms_below_threshold`, `stt::inference_thread_suppresses_whitespace_only_text` | -- |
| AC3.1-AC3.6 | -- | 3.1-3.6 |
| AC3.7 | `overlay::build_css_contains_appearance_settings` | 3.7 (visual) |
| AC3.8 | -- | 3.8 |
| AC4.1-AC4.4 | -- | 4.1-4.4 |
| AC4.5 | `tray::lock_item_disabled_in_docked_mode` | -- |
| AC5.1 | -- | 5.1 |
| AC5.2 | `models::test_parakeet_models_present_when_files_exist` | -- |
| AC5.3 | `stt::cuda_available_detection_does_not_panic` | 5.3 (full fallback) |
| AC5.4 | -- | 5.4 |
| AC6.1 | `config::config_roundtrip` | 6.1 (restart) |
| AC6.2 | -- | 6.2 |
| AC6.3 | `config::config_malformed_toml_returns_error` | -- |
