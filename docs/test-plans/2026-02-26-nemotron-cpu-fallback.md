# Human Test Plan: Nemotron CPU Fallback

## Prerequisites
- Wayland compositor with wlr-layer-shell support running
- PipeWire audio server running
- Application built: `cargo build --release`
- All automated tests passing: `cargo test` (32 tests, 0 failures)
- Access to one machine with CUDA GPU and one without (or ability to mask CUDA)

## Phase 1: Moonshine Removal Verification

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Run `grep -ri moonshine --include='*.rs' --include='*.toml' src/ Cargo.toml` from the project root | Zero matches. No moonshine references in implementation code or manifest. |
| 1.2 | Run `cargo tree \| grep tokenizers` | No output. The `tokenizers` crate dependency is fully removed. |
| 1.3 | Open `~/.config/subtidal/config.toml`, set `engine = "moonshine"`, then run `./target/release/subtidal` | Application starts without crashing. Stderr shows a warning about unknown/invalid engine value. Engine defaults to Nemotron. |
| 1.4 | Run `./target/release/subtidal --engine moonshine` | Application exits with non-zero code. Stderr contains "Unknown engine" and lists valid options ("nemotron", "parakeet"). |

## Phase 2: CUDA Fallback (CPU Machine)

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | On a machine without CUDA (or with `CUDA_VISIBLE_DEVICES=""` set), run `./target/release/subtidal` | Application starts successfully. Stderr contains "CUDA not available, Nemotron will use CPU". |
| 2.2 | Play a known audio clip (e.g., a TTS-generated sentence: "The quick brown fox jumps over the lazy dog") through system audio | Overlay displays a reasonable transcription of the sentence. Words may appear with some delay but should be intelligible. |
| 2.3 | Observe CPU usage during transcription via `htop` or `top` | ORT inference threads are visible. CPU usage is elevated but stable (no runaway threads). |

## Phase 3: CUDA Fallback (GPU Machine)

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | On a CUDA-equipped machine, run `./target/release/subtidal` | Application starts. Stderr contains "CUDA available, Nemotron will use GPU acceleration". |
| 3.2 | Run `nvidia-smi` while the application is running | An `onnxruntime` or `subtidal` process appears using GPU memory (typically 500MB-1.5GB for the 600M param model). |
| 3.3 | Play the same known audio clip as step 2.2 | Overlay displays transcription with lower latency than CPU mode. Output quality is comparable to pre-change behavior. |

## Phase 4: Tray Menu Verification

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Launch the application, right-click the system tray icon | Context menu appears. No "STT Engine" submenu is present. |
| 4.2 | Verify remaining menu items | "Overlay" submenu (with Docked/Floating/Lock items), audio source selection, and "Quit" are present and functional. |
| 4.3 | Hover over the tray icon | Tooltip text does not contain "moonshine" or "Moonshine". |

## End-to-End: Full Session Without CUDA

1. Ensure no CUDA drivers are available (or set `CUDA_VISIBLE_DEVICES=""`).
2. Delete any existing config: `rm ~/.config/subtidal/config.toml`.
3. Run `./target/release/subtidal`. Confirm default config is created and engine defaults to Nemotron.
4. Verify stderr shows the CUDA-unavailable CPU fallback message.
5. Play 30 seconds of spoken audio through system output.
6. Confirm overlay displays live captions that update in real time.
7. Right-click tray icon, switch overlay mode from Docked to Floating. Confirm overlay repositions.
8. Right-click tray icon, select Quit. Confirm clean shutdown (no errors in stderr).
9. Re-launch the application. Confirm config persisted and overlay mode is still Floating.

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 - Moonshine code deleted, builds | `cargo build` + `cargo test` (32 pass) | -- |
| AC1.2 - Model functions removed | `models/mod.rs` tests (4 Nemotron tests pass) | -- |
| AC1.3 - No moonshine refs | `tests/no_moonshine_refs.rs` (2 tests) | 1.1, 1.2, 4.3 |
| AC2.1 - Config fallback | `config_unknown_engine_defaults_to_nemotron` | 1.3 |
| AC2.2 - CLI nemotron/parakeet | `cli_parse_engine_*` (4 tests) | -- |
| AC2.3 - CLI moonshine error | `tests/cli.rs` (2 tests) | 1.4 |
| AC3.1 - CPU fallback | `cuda_status_message_when_unavailable` | 2.1, 2.2 |
| AC3.2 - GPU path | `cuda_status_message_when_available` | 3.1, 3.2, 3.3 |
| AC4.1 - Engine submenu hidden | `menu_excludes_stt_engine_submenu` | 4.1, 4.2 |
