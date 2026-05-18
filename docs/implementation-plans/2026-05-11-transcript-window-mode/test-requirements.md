# Test Requirements: Transcript Window Mode

**Feature:** Transcript window mode (third OverlayMode alongside Docked/Floating)
**Design plan:** /home/jslandau/git/live_text/docs/design-plans/2026-05-10-transcript-window-mode.md
**Implementation plan:** /home/jslandau/git/live_text/docs/implementation-plans/2026-05-11-transcript-window-mode/

## Summary

This document maps each acceptance criterion to either:
- **Automated test**: file path + test name + test type (unit/integration/e2e)
- **Human verification**: justification (why automation is impractical) + verification approach

Coverage: every AC must map to one of the above. Acceptance is binary — partial coverage is not allowed.

A common justification reused for GTK-coupled cases is abbreviated **GWB** (GTK Widget Behavior): "GTK4 widget rendering and user input require a display server; the gtk4-rs test harness is fragile in CI environments and does not exercise compositor-level layer-shell behavior."

## Coverage Matrix

| AC ID | Title | Coverage | Location / Notes |
|-------|-------|----------|------------------|
| AC1.1 | Build success (Phase 1) | Automated | `cargo build` |
| AC1.2 | Paragraph coalescing on 1.5 s gap | Automated | `transcript_log::tests::ac1_2_paragraph_break_after_gap`, `ac1_2b_paragraph_continue_under_gap` |
| AC1.3 | Whitespace preservation | Automated | `transcript_log::tests::ac1_3_whitespace_preserved_in_paragraphs` |
| AC1.4 | `to_json` shape | Automated | `transcript_log::tests::ac1_4_to_json_shape`, `ac1_4b_to_json_timestamp_format_rfc3339` |
| AC1.5 | `paragraphs()` derivation | Automated | `transcript_log::tests::ac1_5_paragraphs_derivation_matches_txt` |
| AC1.6 | `clear()` empties fragments | Automated | `transcript_log::tests::ac1_6_clear_empties_fragments` |
| AC2.1 | Build success (Phase 2) | Automated | `cargo build` |
| AC2.2 | No regressions | Automated | `cargo test --lib` (caption_buffer, tray, transcript_log suites) |
| AC2.3 | Three-option radio + Lock gating | Automated | `tray::tests::ac2_3_lock_item_disabled_in_transcript_mode`, `ac2_3_radio_has_three_options_including_transcript`; existing `lock_item_disabled_in_docked_mode`, `lock_item_enabled_in_floating_mode` |
| AC2.4 | Transcript persists to TOML | Automated | `config::tests::ac2_4_overlay_mode_transcript_round_trips_through_toml` |
| AC2.5 | toggle-captions still works at AtomicBool level | Manual | Smoke test (additive change; no behavioral regression assertion automated) |
| AC3.1 | Build success (Phase 3) | Automated | `cargo build` (compiler-level type-check of widget tree, `RGBA::new`, `idle_add_local_once`, `insert_with_tags`) |
| AC3.2 | Window construction exercised | Manual | GWB |
| AC3.3 | Append produces visible timestamped lines | Manual | GWB |
| AC3.4 | Autoscroll pause and resume | Manual | GWB |
| AC3.5 | Selection and copy | Manual | GWB |
| AC4.1 | Build success (Phase 4) | Automated | `cargo build` |
| AC4.2 | No regressions | Automated | `cargo test --lib` |
| AC4.3 | Captions visible in Docked mode end-to-end | Manual | Requires PipeWire + STT inference + display server |
| AC4.4 | Mid-session switch reveals history | Manual | GWB + STT integration |
| AC4.5 | Switch back to overlay resumes | Manual | GWB |
| AC4.6 | Transcript retains history across mode switches | Automated (data-level) + Manual (e2e) | `transcript_log::tests::ac4_6_transcript_log_accumulates_across_simulated_mode_switches`; manual verifies GTK side |
| AC5.1 | Build success (Phase 5) | Automated | `cargo build` |
| AC5.2 | Both files written on success | Automated (formatter) + Manual (dialog) | `transcript_window::tests::ac5_2_format_paragraphs_as_txt_matches_design_example`; FileDialog interaction manual |
| AC5.3 | Default filename uses session timestamp | Manual | FileDialog default visible only with display server |
| AC5.4 | Malformed paths surface AlertDialog, not crash | Manual | GWB |
| AC5.5 | Partial-success reporting | Manual | Requires permissions manipulation + GWB |
| AC5.6 | `.json` sibling silently overwritten | Automated (path derivation) + Manual (overwrite UX) | `transcript_window::tests::ac5_6_json_sibling_*` (4 cases); overwrite-prompt absence verified manually |
| AC6.1 | Build success (Phase 6) | Automated | `cargo build` |
| AC6.2 | `CaptionBuffer::clear` unit test | Automated | `caption_buffer::tests::ac6_2_clear_resets_lines_and_last_tail`, `ac6_2_push_after_clear_starts_fresh_line` |
| AC6.3 | Toggle off blanks both surfaces | Manual | GWB + STT integration |
| AC6.4 | Toggle on starts fresh | Manual | GWB + STT integration |
| AC6.5 | Save after toggle-off cycle yields only post-re-enable fragments | Manual | End-to-end including FileDialog + filesystem inspection |

## Detailed Mappings

### transcript-window-mode.AC1: Pure-data foundation (Phase 1)

#### AC1.1 — Build success
**Automated via `cargo build`.** Phase 1 adds `chrono` + `serde_json` deps and the `transcript_log` module; compilation success is the verification.

#### AC1.2 — Paragraph coalescing on 1.5 s gap
**Automated unit tests** in `src/overlay/transcript_log.rs`:
- `ac1_2_paragraph_break_after_gap` — push at t=0, t=1.0s, t=3.0s with `paragraph_gap = 1500ms`; assert returns `NewParagraph`, `ContinueParagraph`, `NewParagraph`.
- `ac1_2b_paragraph_continue_under_gap` — boundary: push at t=0 and t=1.5s exactly; assert second is `ContinueParagraph` (gap is not strictly greater than 1.5s).

#### AC1.3 — Whitespace preservation
**Automated unit test** `ac1_3_whitespace_preserved_in_paragraphs`: push `"Hello"`, `" world"`, `","` within the gap; assert `paragraphs()` produces a single paragraph with text exactly `"Hello world,"` (leading space on continuation preserved, no separator inserted).

#### AC1.4 — `to_json` shape
**Automated unit tests:**
- `ac1_4_to_json_shape` — push two fragments, call `to_json("nemotron", session_start)`, assert object has keys `session_start`, `engine`, `fragments`; `engine == "nemotron"`; `fragments` is array len 2; each element has `timestamp` and `text` strings.
- `ac1_4b_to_json_timestamp_format_rfc3339` — assert serialized timestamp parses with `chrono::DateTime::parse_from_rfc3339`.

#### AC1.5 — `paragraphs()` derivation matches `.txt`
**Automated unit test** `ac1_5_paragraphs_derivation_matches_txt`: push fragments mimicking the design plan example (two paragraphs across the 1.5s gap); assert `paragraphs()` returns two `Paragraph` entries with the right text bodies and timestamps anchored at each run's first fragment. Defensive companion `ac1_extra_paragraphs_empty_when_no_fragments` ensures fresh log returns empty `Vec`.

#### AC1.6 — `clear()` empties fragments
**Automated unit test** `ac1_6_clear_empties_fragments`: push 3 fragments, assert `len == 3`; call `clear()`; assert `is_empty()`; push one more; assert returns `NewParagraph` and `len == 1`.

---

### transcript-window-mode.AC2: Config + command + tray (Phase 2)

#### AC2.1 — Build success
**Automated via `cargo build`.** Tasks 1+2+3 must all land before the tree compiles (exhaustive match dependency).

#### AC2.2 — No regressions
**Automated via `cargo test --lib`.** Pre-existing test suites must continue passing: `caption_buffer::tests::*`, `tray::tests::lock_item_disabled_in_docked_mode`, `tray::tests::lock_item_enabled_in_floating_mode`, `tray::tests::menu_excludes_stt_engine_submenu`, plus Phase 1 `transcript_log::tests::*`.

#### AC2.3 — Three-option radio + Lock gating
**Automated tests** in `src/tray/mod.rs`:
- `ac2_3_radio_has_three_options_including_transcript` — inspect first `MenuItem::RadioGroup` in `build_overlay_submenu(&tray)`; assert labels equal `["Docked", "Floating", "Transcript"]` in that order (so index 0/1/2 maps correctly).
- `ac2_3_lock_item_disabled_in_transcript_mode` — construct `TrayState { overlay_mode: Transcript, .. }`; assert overlay submenu non-empty and reflects Transcript mode.
- Existing `lock_item_disabled_in_docked_mode` and `lock_item_enabled_in_floating_mode` continue to assert the Floating-only Lock semantics.

#### AC2.4 — Transcript persists to TOML
**Automated unit test** `config::tests::ac2_4_overlay_mode_transcript_round_trips_through_toml`: deserialize `overlay_mode = "transcript"` and assert it parses to `OverlayMode::Transcript`; reserialize and assert the output contains `overlay_mode = "transcript"`.

#### AC2.5 — toggle-captions still works at AtomicBool level
**Manual smoke test.** Justification: the change is additive (`SetCaptionsEnabled` is sent in addition to `SetVisible`); no automated assertion targets the AtomicBool flip in isolation. Verified by left-clicking the tray icon during the Phase 2 smoke test and observing no visible regression.

---

### transcript-window-mode.AC3: Transcript GTK window (Phase 3)

#### AC3.1 — Build success
**Automated via `cargo build`.** The compiler verifies widget tree types, `gdk::RGBA::new` `f32` argument types, `insert_with_tags` signature, `idle_add_local_once` import path, `WrapMode::WordChar` enum.

#### AC3.2 — Window construction exercised
**Manual smoke test (deferred to Phase 4 end-to-end).** Justification: GWB. The module is `mod transcript_window` (private), and Phase 3 deliberately defers construction to Phase 4's orchestration. Verified by Phase 4 step 2 (switch to Transcript via tray; window appears with HeaderBar, ScrolledWindow, Save button).

#### AC3.3 — Append produces visible timestamped lines
**Manual.** Justification: GWB. Verified by Phase 4 manual step 3 (speak; new fragments append in real time with `[HH:MM:SS]` prefixes).

#### AC3.4 — Autoscroll pause and resume
**Manual.** Justification: GWB. Verified by Phase 4 manual steps 4–5 (scroll up pauses autoscroll; scroll back to bottom resumes it). The threshold logic (`AUTOSCROLL_THRESHOLD_PX = 16.0`) is implicit in the visible behavior.

#### AC3.5 — Selection and copy
**Manual.** Justification: GWB. Verified by Ctrl+A then Ctrl+C in the transcript window during Phase 4 smoke test, then paste into a separate editor and confirm full transcript text is on the clipboard. Built-in GTK behavior with `editable=false, cursor_visible=false`.

---

### transcript-window-mode.AC4: Two-window orchestration (Phase 4)

#### AC4.1 — Build success
**Automated via `cargo build`.**

#### AC4.2 — No regressions
**Automated via `cargo test --lib`.**

#### AC4.3 — Captions visible in Docked mode end-to-end
**Manual.** Justification: requires PipeWire audio capture + Nemotron STT inference + display server with wlr-layer-shell support — none of which are reproducible in a CI environment. Verified by Phase 4 manual step 1 (launch in Docked, speak, observe overlay).

#### AC4.4 — Mid-session switch reveals history
**Manual.** Justification: GWB + STT integration. Verified by Phase 4 manual step 2 (switch to Transcript; same captions visible as paragraphs).

#### AC4.5 — Switch back to overlay resumes
**Manual.** Justification: GWB. Verified by Phase 4 manual steps 6–8 (round-trip Docked → Transcript → Docked → Transcript → Floating; overlay resumes line-fill display).

#### AC4.6 — Transcript retains history across mode switches
**Mixed:**
- **Automated data-level test** `transcript_log::tests::ac4_6_transcript_log_accumulates_across_simulated_mode_switches`: push 3 fragments at t0, t0+200ms, t0+2000ms; assert `fragments().len() == 3` and `paragraphs().len() == 2`.
- **Manual end-to-end** verifies GTK rendering preserves the same content across multiple mode round-trips (Phase 4 manual steps 6–8).

---

### transcript-window-mode.AC5: Save dialog + dual-write (Phase 5)

#### AC5.1 — Build success
**Automated via `cargo build`.**

#### AC5.2 — Both files written on success
**Mixed:**
- **Automated formatter test** `transcript_window::tests::ac5_2_format_paragraphs_as_txt_matches_design_example`: assert `[HH:MM:SS] <text>\n` per paragraph matches the design example body.
- **Manual** for FileDialog interaction + post-write file inspection: Phase 5 smoke step 5–6 — choose `/tmp/test-transcript.txt`, verify both `.txt` and `.json` exist with expected structure (`{"session_start", "engine": "nemotron", "fragments": [...]}`).

#### AC5.3 — Default filename uses session timestamp
**Manual.** Justification: FileDialog default filename is only observable through GTK FileDialog UI. Verified by Phase 5 smoke step 4 (filename matches `subtidal-transcript-YYYY-MM-DD-HHMMSS.txt` formed from `session_start`).

#### AC5.4 — Malformed paths surface AlertDialog, not crash
**Manual.** Justification: GWB + filesystem permission setup. Verified by Phase 5 smoke step 8 (choose path under non-writable directory like `/proc/test.txt`; AlertDialog appears, no crash).

#### AC5.5 — Partial-success reporting
**Manual.** Justification: requires constructing a filesystem state where `.txt` succeeds but `.json` fails (or vice versa) — typically by manipulating directory permissions between writes, which is hard to fixture deterministically. Verified by inspecting the `(Err, Ok)` and `(Ok, Err)` branches' AlertDialog messages, which must include the path of whichever side did succeed for manual recovery.

#### AC5.6 — `.json` sibling silently overwritten
**Mixed:**
- **Automated path-derivation tests** in `src/overlay/transcript_window.rs`:
  - `ac5_6_json_sibling_replaces_txt_extension` — `/tmp/transcript.txt` → `/tmp/transcript.json`
  - `ac5_6_json_sibling_no_extension` — `/tmp/transcript` → `/tmp/transcript.json`
  - `ac5_6_json_sibling_other_extension` — `/tmp/transcript.log` → `/tmp/transcript.json`
  - `ac5_6_json_sibling_with_dots_in_stem` — `/tmp/2026.05.11.txt` → `/tmp/2026.05.11.json`
- **Manual** verification of overwrite UX (Phase 5 smoke step 7): re-saving to the same path prompts overwrite for `.txt` only; the `.json` sibling overwrites silently with no prompt.

---

### transcript-window-mode.AC6: Clear-on-disable wiring (Phase 6)

#### AC6.1 — Build success
**Automated via `cargo build`.** The `_transcript_log` parameter is renamed to `transcript_log` (now used) — compiler verifies no unused-variable warning.

#### AC6.2 — `CaptionBuffer::clear` unit test
**Automated unit tests** in `src/overlay/caption_buffer.rs`:
- `ac6_2_clear_resets_lines_and_last_tail` — push two captions; assert `display_text()` non-empty; call `clear()`; assert `display_text()` is `""`.
- `ac6_2_push_after_clear_starts_fresh_line` — push, clear, push again; assert post-clear text appears in `display_text()`; assert pre-clear text does not.

The corresponding `TranscriptLog::clear` is already covered by `ac1_6_clear_empties_fragments`.

#### AC6.3 — Toggle off blanks both surfaces
**Manual.** Justification: GWB + STT integration. Verified by Phase 6 manual end-to-end steps 4–6 (toggle captions off; transcript window goes blank; switch to Docked; overlay also blank).

#### AC6.4 — Toggle on starts fresh
**Manual.** Justification: GWB + STT integration. Verified by Phase 6 manual end-to-end steps 7–9 (toggle on; speak; switch to Transcript; only post-re-enable fragments visible).

#### AC6.5 — Save after toggle-off cycle yields only post-re-enable fragments
**Manual end-to-end including filesystem inspection.** Verified by Phase 6 step 10 (Save to `/tmp/post-clear.txt`; `cat` both files; confirm contents are post-re-enable only). The data-level invariant — that `clear()` empties the log — is automated under AC1.6 and AC6.2; the end-to-end wiring (`SetCaptionsEnabled(false)` actually calls all four clears in order) is the manual portion.

---

## Human Verification Checklist

Run during release validation. Each item references the AC it covers. Check off in order; failures block release.

### Phase 2 smoke (after Phase 2 merge)
- [ ] AC2.5 — Left-click tray icon while in Docked mode. Captions toggle off (overlay hides) and back on. No crash. No visible regression vs. pre-Phase-2 behavior.

### Phase 3 / 4 smoke (after Phase 4 merge — Phase 3 widgets are not exercised in isolation)
- [ ] AC3.2 / AC4.3 — Launch `cargo run --release`. Default Docked mode. Speak; layer-shell overlay shows captions.
- [ ] AC3.2 / AC4.4 — Tray → Overlay → Transcript. Layer-shell overlay disappears; transcript window appears with HeaderBar, scrollable text, and "Save…" button. Prior captions visible as `[HH:MM:SS]` paragraphs.
- [ ] AC3.3 — Speak more. New fragments append with timestamps in real time.
- [ ] AC3.4 — Scroll up in transcript window; speak; viewport stays put (autoscroll paused).
- [ ] AC3.4 — Scroll back to bottom; speak; autoscroll resumes.
- [ ] AC3.5 — Ctrl+A then Ctrl+C in transcript window; paste into a separate editor; full transcript text on clipboard.
- [ ] AC4.5 — Switch back to Docked. Overlay reappears with line-fill display.
- [ ] AC4.6 — Switch Docked → Transcript → Floating → Transcript multiple times. Transcript content preserved across all switches; no truncation, no duplication.
- [ ] AC4 (hot-reload) — Edit `~/.config/subtidal/config.toml` externally, change `overlay_mode = "docked"` ↔ `"transcript"`. Within ~250 ms the active surface switches. Round-trip works.

### Phase 5 smoke (after Phase 5 merge)
- [ ] AC5.3 — In Transcript mode, click Save…; verify default filename matches `subtidal-transcript-YYYY-MM-DD-HHMMSS.txt` with the session-start timestamp.
- [ ] AC5.2 — Save to `/tmp/test-transcript.txt`. Confirm `/tmp/test-transcript.txt` contains `[HH:MM:SS] <paragraph>` lines. Confirm `/tmp/test-transcript.json` contains `{"session_start": ..., "engine": "nemotron", "fragments": [...]}`.
- [ ] AC5.6 — Save again to the same path; OS overwrite prompt appears for `.txt`; choose Replace; both files overwrite without an additional prompt for `.json`.
- [ ] AC5.4 — Save to a non-writable path (e.g., `/proc/test.txt`). AlertDialog reports the failure; no crash.
- [ ] AC5.5 — Construct a partial-failure scenario (e.g., make `/tmp/onlytxt-writable/` writable but pre-create `/tmp/onlytxt-writable/foo.json` with no write permission). Save to `/tmp/onlytxt-writable/foo.txt`. AlertDialog reports JSON write failure AND includes the successful `.txt` path for recovery.

### Phase 6 end-to-end (full Definition of Done)
- [ ] AC6.3 (step 4–6) — Speak in any mode. Toggle captions off via tray. Transcript window goes blank. Switch to Docked; overlay also blank.
- [ ] AC6.4 (step 7–9) — Toggle captions on. Speak. Switch to Transcript. Only post-re-enable fragments visible — no pre-toggle-off content.
- [ ] AC6.5 (step 10) — Click Save…; choose `/tmp/post-clear.txt`. `cat` both files; confirm contents include only post-re-enable fragments.
- [ ] Quit via tray. App exits cleanly.
