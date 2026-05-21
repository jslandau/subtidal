# macOS Port — Test Requirements

**Source design:** docs/design-plans/2026-05-18-macos-port.md
**Source plan:** docs/implementation-plans/2026-05-18-macos-port/
**Generated:** 2026-05-18

## Summary
- Automated: 13
- Human verification: 30
- Total ACs: 43

Total ACs by family: AC1 (6) + AC2 (7) + AC3 (7) + AC4 (5) + AC5 (8) + AC6 (3) + AC7 (2) + AC8 (3) + AC9 (3) + AC10 (3) = 43.

Every criterion maps to exactly one verification path. Operational walkthroughs all live in Phase 7 Task 3 (`docs/macos-port-ac-results.md`, transient, deleted before merge), with originating phases noted.

---

## Automated Tests

### Phase 2 — overlay/macos/panel.rs unit tests

- **AC2.1** — Panel constructed with `level=.floating`, `collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]`, `isFloatingPanel=true`, `styleMask` includes `.borderless`+`.nonactivatingPanel`.
  - Type: unit test (target-os-gated).
  - File: `src/overlay/macos/panel.rs` (`#[cfg(all(test, target_os = "macos"))] mod tests` — `panel_constructed_with_required_flags`).
  - Asserts: `inspect(&panel)` returns the exact flag bits enumerated in the AC. Uses the `pub fn inspect(&NSPanel) -> PanelConfig` helper mandated by the design's "verifiable via inspection helper" wording.

- **AC2.4** — `set_above_fullscreen` flips `panel.level` between `NSFloatingWindowLevel` and `NSStatusWindowLevel` without rebuild.
  - Type: unit test (target-os-gated).
  - File: `src/overlay/macos/panel.rs` (`above_fullscreen_toggle_changes_level`).
  - Asserts: same `Retained<NSPanel>` pointer before/after toggle; inspected level changes; reverses on toggle-off. Phase 6 Task 6 additionally re-verifies end-to-end via the tray UI (operational), but the no-rebuild contract is asserted here.

### Phase 3 — stt/nemotron.rs unit tests

- **AC4.2** — CPU fallback on `WebGpu` init failure.
  - Type: unit test (target-os-gated, `#[ignore]`-requires model files).
  - File: `src/stt/nemotron.rs` (`#[cfg(all(test, target_os = "macos"))] mod tests` — `cpu_fallback_on_simulated_webgpu_failure`).
  - Asserts: `build_macos_with(model_dir, true, |_| Err(...))` returns `Ok(Nemotron)`, exercising the CPU branch.

### Phase 4 — audio/impl_macos/normalize.rs unit tests

- **AC3.7 (input/output shape correctness portion)** — Format normalization at SCK callback boundary.
  - Type: unit test (target-os-gated). Phase 4 Task 5 flags this as conditionally skippable if `CMSampleBuffer` cannot be constructed programmatically; in that case AC3.7 falls back fully to human verification via code review per the explicit user-approved deviation (see Human Verification section).
  - File: `src/audio/impl_macos/normalize.rs` (`#[cfg(all(test, target_os = "macos"))] mod tests`).
  - Asserts: 48 kHz stereo f32 packed → `Some(&[f32])` of correct length; 44.1 kHz / mono / i16 inputs → `None`.

### Phase 5 — audio/impl_macos unit tests

- **AC3.2 (enumeration correctness portion)** — `list_sources()` shape.
  - Type: unit test (target-os-gated, `#[ignore]`-requires graphical session + permission).
  - File: `src/audio/impl_macos/mod.rs` (`list_sources_returns_system_output_plus_running_apps`).
  - Asserts: result contains SystemOutput; contains at least one `App { .. }` entry; len ≥ 2. The per-app capture *behavior* itself requires hardware (see Human Verification).

### Phase 6 — overlay/macos integration tests

- **AC1.4** — Mode switch does not rebuild the panel.
  - Type: integration / unit test (target-os-gated).
  - File: `src/overlay/macos/panel.rs` (`mode_switch_does_not_rebuild_panel`).
  - Asserts: `Retained::as_ptr(&panel)` is identical before and after `apply_geometry(..., Docked, ...)` and `apply_geometry(..., Floating, ...)`.

- **AC1.5** — Empty Transcript Save produces valid JSON.
  - Type: integration test (target-os-gated).
  - File: `src/overlay/macos/transcript_window.rs` (`empty_transcript_save_produces_valid_json`).
  - Asserts: `TranscriptLog::to_json()` on an empty log parses as valid JSON via `serde_json`.

### Phase 7 — CI matrix

- **AC9.1** — `cargo check --lib --target x86_64-apple-darwin` green on ubuntu-latest.
  - Type: cargo-check in CI (matrix leg).
  - File: `.github/workflows/macos-check.yml` (matrix entry `x86_64-apple-darwin`).
  - Asserts: cross-target check completes cleanly on every push/PR.

- **AC9.2** — `cargo check --lib --target aarch64-apple-darwin` green on ubuntu-latest.
  - Type: cargo-check in CI (new matrix leg).
  - File: `.github/workflows/macos-check.yml` (matrix entry `aarch64-apple-darwin`).
  - Asserts: Apple Silicon cross-target check completes cleanly.

- **AC9.3** — Linux-coupling regression breaks both legs.
  - Type: cargo-check in CI + one-shot deliberate-regression verification (Phase 7 Task 1 throwaway-branch test). The ongoing automated guarantee is provided by AC9.1/AC9.2 running on every push; the bidirectional sensitivity is exercised once by the deliberate regression then reverted.
  - File: `.github/workflows/macos-check.yml` (both matrix legs with `fail-fast: false`).
  - Asserts: a `use pipewire as _;` added unconditionally to `src/audio/mod.rs` causes both matrix legs to fail; reverting restores green.

### Cross-cutting (every phase)

- **AC10.2** — Design doc is self-contained / executable by a fresh Mac session.
  - Type: structural automated guarantee via planning-skill discipline. Each phase plan was generated with no prior conversation context and committed before execution; the writing-implementation-plans skill enforces this self-containment as a contract on the documents themselves.
  - File: `docs/design-plans/2026-05-18-macos-port.md` (the document under test) and `docs/implementation-plans/2026-05-18-macos-port/phase_0[0-7].md`.
  - Asserts: documents are sufficient for a fresh agent — verified continuously by the executor itself reading each phase without supplemental context.

---

## Human Verification

All operational walkthroughs are recorded in Phase 7 Task 3's transient checklist `docs/macos-port-ac-results.md` (deleted by Phase 7 Task 6 before merge). Originating-phase verification steps are also listed in each phase's Task 6 (hardware walkthrough).

### Phase 2 — main-thread runtime and clean shutdown

- **AC8.1** — Captions arrive on main thread via caption bridge; no AppKit thread-affinity panics.
  - Blocked: requires the real AppKit run loop (`NSApplication.run()`), GCD main queue, and Console.app to filter thread-affinity warnings. None of these are available in a `cargo test` host process — `MainThreadMarker` exists in tests but `NSApplication` is not running, so dispatch_async to main never drains.
  - Step: Phase 2 Task 6 + Phase 7 Task 3 (`AC8.1`). Launch `.app`, observe Phase-2 test harness captions appear in the NSPanel, filter Console.app on subtidal process for AppKit warnings.

- **AC8.2** — Cmd-Q clean shutdown: SCK stopped, STT exits ≤250ms, audio/tray exit, exit 0.
  - Blocked: requires real signal delivery + AppKit terminate flow + `pgrep`/`echo $?` observation on a running bundle.
  - Step: Phase 2 Task 6 + Phase 7 Task 3 (`AC8.2`). Launch from terminal, Ctrl-C, check `echo $?` and `pgrep -f subtidal`.

### Phase 3 — STT engine on real hardware

- **AC4.1** — `NemotronEngine::new` selects WebGPU on Apple Silicon; log confirms.
  - Blocked: requires Apple Silicon Metal stack and parakeet-rs/ort runtime. Cannot be exercised on a Linux CI host.
  - Step: Phase 3 Task 5 + Phase 7 Task 3 (`AC4.1`). Inspect `/tmp/subtidal-phase3.log` for `info: Nemotron using execution provider: WebGPU`.

- **AC4.3** — Transcript on fixture WAV matches Linux baseline (token-sequence parity).
  - Blocked: requires running the model on real Metal hardware and on a real Linux+CUDA host, then diffing. The token-sequence comparison itself is human-judged (the design allows small whitespace differences).
  - Step: Phase 3 Task 5 + Phase 7 Task 3 (`AC4.3`).

- **AC4.4** — RTF on WebGPU path ≤ 1.0 on test machine.
  - Blocked: requires the Phase 0 spike on real Apple Silicon. RTF is hardware-specific and unmeasurable from CI.
  - Step: Phase 0 Task 4 + Phase 7 Task 3 (`AC4.4`). `cargo run --release --example macos_webgpu_smoke`; inspect printed `rtf`.

- **AC4.5** — Engine swap reads `ArcSwap` on next chunk boundary; no concurrent session construction.
  - Blocked: this AC is explicitly a code-review verification per the design and Phase 3 Task 3's preamble. The single-threaded invariant cannot be asserted by a positive test — only by reading `src/stt/mod.rs::spawn_stt_thread` and confirming exactly one thread constructs engines.
  - Step: Phase 3 Task 3 code-review audit (documented in commit body) + Phase 7 Task 3 (`AC4.5`).

### Phase 4 — ScreenCaptureKit + TCC

- **AC3.1** — System Output captures system audio; video produces real-time captions.
  - Blocked: requires ScreenCaptureKit (macOS-only framework), real audio playback from another app, and granted Screen Recording TCC permission.
  - Step: Phase 4 Task 6 + Phase 7 Task 3 (`AC3.1`). Play a YouTube video, observe captions.

- **AC3.5** — First-run TCC prompt with `NSScreenCaptureUsageDescription` text.
  - Blocked: TCC dialogs are rendered by macOS's `tccd`, not in-process. Cannot be triggered or asserted programmatically without UI automation against a real first-launch state (requires resetting TCC database).
  - Step: Phase 4 Task 6 + Phase 7 Task 3 (`AC3.5`). Visually confirm dialog text matches Info.plist string.

- **AC3.6** — Refusing TCC produces user-visible error, not silent crash.
  - Blocked: requires toggling System Settings → Privacy & Security → Screen Recording, relaunch, and observing `NSUserNotification` delivery (handled by `usernoted`).
  - Step: Phase 4 Task 6 + Phase 7 Task 3 (`AC3.6`).

- **AC3.7** — SCK callback is RT-safe (no allocation, no logging, try_lock only, copy-and-return).
  - Blocked: **Per user-approved deviation 2026-05-18, documented in `phase_04.md` preamble**, the design's "debug-build instrumentation that asserts no `Mutex::lock` calls" is impractical (std's `Mutex::lock` is not overridable from outside std without an interception shim or custom mutex type). Phase 4 explicitly ships a `// RT-SAFE: ...` header comment enumerating the rules + code-review verification, matching the existing Linux PipeWire-callback precedent. The normalize-shape unit test (above) covers the input/output correctness sub-criterion only; the RT-safety contract itself is review-only.
  - Step: Phase 4 Task 6 code review of `Delegate::stream_didOutput` against the header rules + Phase 7 Task 3 (`AC3.7`).

- **AC7.1** — TCC grant persists across `cargo build && scripts/bundle-mac.sh && open Subtidal.app` cycles.
  - Blocked: requires real TCC database, real ad-hoc codesign, and observable persistence across multiple rebuilds.
  - Step: Phase 4 Task 6 + Phase 7 Task 3 (`AC7.1`). Bundle, launch, grant once, rebuild twice, observe no re-prompt.

### Phase 5 — per-app capture and live switching

- **AC3.2** — Per-app source captures only that app's audio.
  - Blocked: requires real SCK with a running target app (Safari + YouTube tab) and the ability to A/B audio from another app.
  - Step: Phase 5 Task 6 + Phase 7 Task 3 (`AC3.2`).

- **AC3.3** — Source switch via `updateContentFilter`: no flicker, no caption gap > 1 sample.
  - Blocked: requires real SCK stream, perceptual judgement about "flicker", and live captioning to observe gap.
  - Step: Phase 5 Task 6 (config-edit trigger) + Phase 6 Task 6 (tray-driven trigger) + Phase 7 Task 3 (`AC3.3`).

- **AC3.4** — Captured app exits → NSUserNotification posted + fallback to System Output.
  - Blocked: requires real notification daemon + real SCK stream-stop delegate callback fired by app termination.
  - Step: Phase 5 Task 6 + Phase 7 Task 3 (`AC3.4`). Cmd-Q Safari while captured.

### Phase 6 — overlay modes, tray, hot-reload

- **AC1.1** — Docked positions NSPanel at top of `NSScreen.main.visibleFrame`, full width.
  - Blocked: requires real NSScreen and visible compositor.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC1.1`).

- **AC1.2** — Floating shows NSPanel at config position, draggable.
  - Blocked: requires real mouse drag and visible compositor; drag gesture is AppKit-internal.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC1.2`).

- **AC1.3** — Transcript scrollable NSTextView, timestamped paragraphs, autoscroll-when-at-bottom.
  - Blocked: requires real NSTextView with scroll state and human judgment of autoscroll behavior.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC1.3`).

- **AC1.6** — External display connect/disconnect re-positions Docked panel.
  - Blocked: requires physical display change to trigger `NSApplicationDidChangeScreenParametersNotification`.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC1.6`).

- **AC2.2** — Panel visible on every Space (Mission Control verified).
  - Blocked: requires Spaces / Mission Control interaction; not scriptable.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC2.2`).

- **AC2.3** — Panel visible above Safari/Chrome fullscreen.
  - Blocked: requires real browser fullscreen + visual confirmation that panel renders above.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC2.3`).

- **AC2.5** — Caption modes have `ignoresMouseEvents=true`; clicks pass through.
  - Blocked: requires click on the panel and confirming the click reaches the window underneath.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC2.5`).

- **AC2.6** — Transcript mode `ignoresMouseEvents=false`; Save button works.
  - Blocked: requires user click on the Save button and observing NSSavePanel.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC2.6`).

- **AC2.7** — Panel does not collide with menu bar.
  - Blocked: visual verification of Docked geometry vs menu bar.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC2.7`).

- **AC5.1** — Tray icon `isTemplate=true`; renders in light + dark mode.
  - Blocked: visual confirmation across appearance modes; requires switching System Settings appearance.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.1`).

- **AC5.2** — Captions On/Off toggles state; checkmark reflects.
  - Blocked: requires interacting with live NSStatusItem menu.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.2`).

- **AC5.3** — Mode submenu lists three modes; active checkmarked; selection posts `SetMode`.
  - Blocked: live menu interaction.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.3`).

- **AC5.4** — Audio Source submenu populated dynamically via `list_sources()`.
  - Blocked: live menu interaction + running session with audio-producing apps. The neutral `list_sources()` shape is covered by the Phase 5 unit test; the submenu population path through the tray is operational.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.4`).

- **AC5.5** — Show Above Fullscreen toggle posts `SetAboveFullscreen` live (no rebuild).
  - Blocked: live tray interaction; the no-rebuild property at the panel layer is covered by AC2.4's unit test, but the tray-to-panel wiring requires the menu.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.5`).

- **AC5.6** — Lock Position only enabled in Floating mode.
  - Blocked: live menu state observation across mode switches.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.6`).

- **AC5.7** — Cmd-Q clean termination via `applicationWillTerminate`.
  - Blocked: requires AppKit terminate flow.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.7`).

- **AC5.8** — Tray items reflect disabled state when feature unavailable.
  - Blocked: live menu state observation.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC5.8`).

- **AC6.1** — Editing config.toml triggers debounced reload within ~500ms.
  - Blocked: requires real file-watcher + visible reload effect.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC6.1`).

- **AC6.2** — Drag writes do not trigger config-reload feedback loop.
  - Blocked: requires sustained drag interaction; the absence of a feedback loop is observed as absence of mode-reset / glitch.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC6.2`).

- **AC6.3** — Malformed TOML warned to stderr, ignored; app continues.
  - Blocked: requires running app + writing bad TOML + observing stderr without crash.
  - Step: Phase 6 Task 6 + Phase 7 Task 3 (`AC6.3`).

### Phase 7 — TCC re-prompt and orphan-stream verification

- **AC7.2 (Failure criterion)** — Changing `CFBundleIdentifier` or signing identity invalidates grant and re-prompts.
  - Blocked: requires deliberately mutating the bundle ID once and observing TCC re-prompt; this is a destructive one-shot verification.
  - Step: Phase 7 Task 3 (`AC7.2`). Explicitly called out in Phase 4 preamble as Phase 7 walkthrough territory.

- **AC8.3 (Edge criterion)** — Force-close via Activity Monitor leaves no orphan SCK streams.
  - Blocked: requires `kill -9`, then `lsof` inspection of `replayd` for lingering Subtidal-attributable handles.
  - Step: Phase 4 Task 6 (initial check) + Phase 7 Task 3 (`AC8.3`).

### Phase 7 — documentation

- **AC10.1** — `CLAUDE.md` describes cross-platform codebase, not "Linux currently".
  - Blocked: doc-content judgment; the freshness/grep checks in Phase 7 Task 5 are necessary but not sufficient (they confirm strings are present, not that the prose is accurate).
  - Step: Phase 7 Task 5 (edits) + Phase 7 Task 3 (`AC10.1` — final read-through).

- **AC10.3** — Newly discovered macOS landmines documented for auto-memory ingestion.
  - Blocked: requires the user to confirm each landmine block is real (observed during phases 0–6) and elects to ingest it. The author cannot self-certify this; the design explicitly routes this through user confirmation.
  - Step: Phase 7 Task 4 (emit blocks to user) + Phase 7 Task 3 (`AC10.3`).
