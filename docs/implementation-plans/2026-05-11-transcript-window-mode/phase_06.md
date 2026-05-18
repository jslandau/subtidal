# Phase 6: Clear-on-Disable Wiring + End-to-End Verification Implementation Plan

**Goal:** Replace the Phase 2/4 stub `OverlayCommand::SetCaptionsEnabled(false)` handler with the full clear-on-disable behavior: clear `TranscriptLog`, the transcript window's `TextBuffer`, the overlay's `CaptionBuffer`, and the overlay's caption label. Add `CaptionBuffer::clear` (verified missing in Phase 1 codebase investigation). On `SetCaptionsEnabled(true)` simply flip the `AtomicBool` — no clearing needed because the disable edge already cleared.

**Architecture:** Two surgical edits.
1. `src/overlay/caption_buffer.rs` — add `pub fn clear(&mut self)` plus a unit test that exercises it. Confirmed in Phase 1 codebase investigation that the method does NOT currently exist. The implementation is trivial: clear the internal `Vec<CaptionLine>` (and any other state — investigation pending; we read the file in this phase to confirm the exact field name).
2. `src/overlay/mod.rs` — replace the Phase 4 `SetCaptionsEnabled` arm in `handle_overlay_command` with the four-surface clear logic. Rename the `_transcript_log` parameter (Phase 4 prefix) back to `transcript_log` since it is now actually used.

**Tech Stack:** No new dependencies.

**Scope:** Phase 6 of 6.

**Codebase verified:** 2026-05-11 (and re-verified at start of Phase 6 implementation).
- `src/overlay/caption_buffer.rs` public methods: `new`, `push`, `expire`, `display_text`, `update_config`. **`clear` does NOT exist** — must be added.
- The exact internal state of `CaptionBuffer` will be re-read at implementation time to ensure `clear()` resets all relevant fields (Phase 1 investigation noted `lines: Vec<CaptionLine>` and `last_tail: String` as the two fields that exist; we re-verify before writing the impl).
- `src/overlay/mod.rs` after Phase 4 has the `SetCaptionsEnabled(enabled)` stub at the location where Phase 4 placed it (between `SetCaption` and `Quit` arms).
- `find_caption_label(window)` (used elsewhere in `handle_overlay_command`) returns the overlay's caption `Label` — we reuse it for the `set_text("")` call.

---

## Acceptance Criteria Coverage

This phase implements:

### transcript-window-mode.AC6: Clear-on-disable wiring (Phase 6 "Done when")
- **transcript-window-mode.AC6.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC6.2 CaptionBuffer::clear unit test:** Unit test added for `CaptionBuffer::clear` (verifying that after `clear()`, `display_text()` returns the empty string and a subsequent `push` starts a fresh paragraph).
- **transcript-window-mode.AC6.3 Toggle off blanks both surfaces:** Manual end-to-end — speak in any mode, toggle captions off (tray), confirm both overlay and transcript window go blank.
- **transcript-window-mode.AC6.4 Toggle on starts fresh:** Manual end-to-end — toggle captions on, speak again, confirm transcript starts fresh (no prior content).
- **transcript-window-mode.AC6.5 Save after toggle-off cycle yields only post-re-enable fragments:** Manual end-to-end — Save after a toggle-off cycle produces a transcript with only post-re-enable fragments.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add `CaptionBuffer::clear` with unit test

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC6.1, transcript-window-mode.AC6.2.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/caption_buffer.rs` — add a new `pub fn clear(&mut self)` method on `impl CaptionBuffer`. Place it after `update_config` (around the existing line 202 area; re-read for current location).
- Modify: `/home/jslandau/git/live_text/src/overlay/caption_buffer.rs` — add an `ac6_2_clear_*` test in the existing `#[cfg(test)] mod tests` block (which currently spans roughly lines 209-559).

**Implementation:**

**Pre-step:** Read `src/overlay/caption_buffer.rs` to confirm the exact internal field set. Phase 1 codebase investigation noted `lines: Vec<CaptionLine>` and `last_tail: String`. Other private fields (e.g., `max_lines`, `max_chars_per_line`, `expire_secs`) are configuration and must NOT be reset by `clear()` — only the runtime caption state.

If the actual field set differs, adapt the implementation accordingly: the rule is "reset everything that holds caption text or per-line metadata; preserve everything that holds configuration."

Add the following method to `impl CaptionBuffer`:

```rust
/// Reset all caption state, leaving configuration (max_lines, max_chars_per_line,
/// expire_secs) untouched. After `clear()`:
/// - `display_text()` returns the empty string.
/// - The next `push()` begins a fresh first line.
/// - The RNNT word-boundary deduplication state is reset.
pub fn clear(&mut self) {
    self.lines.clear();
    self.last_tail.clear();
}
```

If reading the file reveals additional caption-state fields (e.g., a `current_line_index: usize`, a `silence_since: Option<Instant>`), add the appropriate reset to the body. If unsure about a field's classification (state vs. config), prefer to reset it — over-clearing is harmless because subsequent `push()` calls rebuild any needed transient state.

**Testing:**

Add to the existing `#[cfg(test)] mod tests` block at the end of `src/overlay/caption_buffer.rs`:

```rust
#[test]
fn ac6_2_clear_resets_lines_and_last_tail() {
    let mut buf = CaptionBuffer::new(3, 40, 8);
    buf.push("hello world".to_string());
    buf.push(" goodnight".to_string());
    assert!(!buf.display_text().is_empty(), "buffer should have content before clear");
    buf.clear();
    assert_eq!(buf.display_text(), "", "display_text should be empty after clear");
}

#[test]
fn ac6_2_push_after_clear_starts_fresh_line() {
    let mut buf = CaptionBuffer::new(3, 40, 8);
    buf.push("alpha bravo".to_string());
    buf.clear();
    buf.push("charlie".to_string());
    let out = buf.display_text();
    assert!(
        out.contains("charlie"),
        "post-clear push must appear in display_text; got: {out:?}"
    );
    assert!(
        !out.contains("alpha") && !out.contains("bravo"),
        "post-clear display_text must not contain pre-clear text; got: {out:?}"
    );
}
```

**Verification:**

Run: `cargo test --lib caption_buffer`
Expected: All pre-existing `caption_buffer` tests pass plus the two new `ac6_2_*` tests.

Run: `cargo build`
Expected: Compiles cleanly.

**Commit:** Combined at end of Phase 6.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Implement clear-on-disable in `handle_overlay_command::SetCaptionsEnabled`

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC6.1, transcript-window-mode.AC6.3, transcript-window-mode.AC6.4, transcript-window-mode.AC6.5.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs` — find the Phase 4 `SetCaptionsEnabled` arm and rewrite it; rename the `_transcript_log` parameter to `transcript_log`.

**Implementation:**

**(a) Rename the parameter.** In `handle_overlay_command`'s signature (set up in Phase 4), change `_transcript_log: &Rc<RefCell<...>>` to `transcript_log: &Rc<RefCell<...>>` (drop the underscore). After Phase 6 the parameter is genuinely used.

**(b) Replace the existing `SetCaptionsEnabled` arm.** The Phase 4 stub:
```rust
OverlayCommand::SetCaptionsEnabled(enabled) => {
    captions_enabled.store(enabled, Ordering::Relaxed);
}
```
becomes:
```rust
OverlayCommand::SetCaptionsEnabled(enabled) => {
    // Update the AtomicBool first — the caption consumer future reads this
    // and short-circuits when false. Setting it before clearing prevents any
    // in-flight caption from being appended back into a buffer we just cleared.
    captions_enabled.store(enabled, Ordering::Relaxed);

    if !enabled {
        // Clear all four caption surfaces:
        // 1. Durable transcript log.
        transcript_log.borrow_mut().clear();
        // 2. Transcript window's TextBuffer (visible even while hidden;
        //    must be cleared so a future mode switch shows nothing).
        crate::overlay::transcript_window::clear_view(transcript_state);
        // 3. Overlay caption buffer (line-fill state).
        caption_buffer.borrow_mut().clear();
        // 4. Overlay caption label (the visible text in the layer-shell window).
        let label = find_caption_label(window);
        label.set_text("");
    }
    // On (true): no clearing — the prior disable already cleared everything,
    // and there is no carryover state from a freshly re-enabled recognizer.
}
```

**(c) Comment update on the stale Phase 4 placeholder docstring** (if present in the file from Phase 4). Make sure no comment still says "Phase 4: still the AtomicBool-only stub" — that comment is now wrong.

**Race-safety reasoning:** Both the caption consumer future and the command consumer future run on the GTK main thread via `glib::MainContext::default().spawn_local`. They are serialized by the main loop's task scheduling. Therefore there is no actual race between "clear caption_buffer" and "caption consumer pushes a new caption" — the futures cannot interleave at a fragment boundary. Also, since the AtomicBool is set BEFORE the clears, even a hypothetical reordering (which can't happen on a single-threaded executor) would fail closed: caption pushes would short-circuit, leaving the cleared state untouched.

**Testing:**

The integration is verified by the manual end-to-end test below. The data-level unit test for `CaptionBuffer::clear` lives in Task 1; `TranscriptLog::clear` was already tested in Phase 1 (`ac1_6_clear_empties_fragments`).

**Verification:**

Run: `cargo build`
Expected: Compiles cleanly with no warnings (in particular, no "unused variable: transcript_log" warning since the parameter is now used).

Run: `cargo test --lib`
Expected: All Phase 1–5 tests still pass plus the new Phase 6 `ac6_2_*` tests in `caption_buffer.rs`.

**Manual end-to-end test (the design plan's full Definition of Done verification):**

```bash
cargo run --release
```

1. **Default startup, Docked mode.** Speak. Captions appear in the layer-shell overlay.
2. **Switch to Transcript via tray.** Overlay disappears; transcript window appears with the full session history as paragraphs with `[HH:MM:SS]` prefixes.
3. **Speak more.** New fragments append to the transcript window in real time. Autoscroll keeps the bottom in view.
4. **Toggle captions off via the tray** (left-click the tray icon, or use the Captions checkmark menu item).
5. **Verify transcript window goes blank.** No content visible.
6. **Switch to Docked mode** (overlay reappears). **Verify the overlay also has no caption text.**
7. **Toggle captions on** (left-click tray again).
8. **Speak.** New captions appear in the overlay.
9. **Switch to Transcript.** Verify only post-re-enable fragments are present — none of the pre-toggle-off content.
10. **Click Save…** Choose `/tmp/post-clear.txt`. Open both files; verify they contain ONLY the post-re-enable fragments.
11. **Quit** via tray. App exits cleanly.

If any step fails, return to the relevant phase and fix before claiming Phase 6 done.

**Commit:**

```bash
git add src/overlay/caption_buffer.rs src/overlay/mod.rs
git commit -m "feat(transcript): clear all caption surfaces on captions-disable edge

- CaptionBuffer::clear added with unit tests
- SetCaptionsEnabled(false) handler clears TranscriptLog, transcript view, caption buffer, and overlay label
- SetCaptionsEnabled(true) only flips the AtomicBool — disable edge already cleared"
```
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Refresh `/home/jslandau/git/live_text/CLAUDE.md` to reflect the new architecture

**Type:** Documentation.

**Verifies:** transcript-window-mode.AC6.1 (build still passes; this is a docs-only edit).

**Files:**
- Modify: `/home/jslandau/git/live_text/CLAUDE.md`

**Implementation:**

The project's top-level `CLAUDE.md` describes two overlay modes and lists the modules under `src/overlay/`. After Phase 6, it must reflect the new third mode and two new modules.

Make the following edits:

1. **Update the freshness date** (currently `Freshness: 2026-04-22`) to today's implementation completion date.

2. **Update the Architecture section** module listing for `src/overlay/`. The current block reads:
   ```
   overlay/mod.rs    — overlay orchestration, OverlayCommand dispatch, run_gtk_app public API
   overlay/window.rs — GTK4 layer-shell window construction (docked/floating), CSS, caption label
   overlay/drag.rs   — floating-mode drag gesture with compositor-quirk coordinate compensation
   overlay/caption_buffer.rs — pure text buffer: line-fill, overlap dedup, expiry (GTK-free, well-tested)
   overlay/input_region.rs — Wayland input region for click-through
   ```
   Add two lines (alphabetical or logical grouping):
   ```
   overlay/transcript_log.rs   — pure data: timestamped fragments, paragraph coalescing, .json serialization (GTK-free, well-tested)
   overlay/transcript_window.rs — GTK4 toplevel window for transcript mode: scrollable TextView, autoscroll, Save dialog
   ```

3. **Update the "Key Contracts" section** entry for OverlayMode-related routing. Add a new bullet under "Key Contracts" (or extend the existing Caption display bullet):
   ```
   - **Overlay modes**: Three modes — `Docked` and `Floating` use the gtk4-layer-shell overlay; `Transcript` uses a regular GTK toplevel window with append-only timestamped paragraphs. Both windows are constructed at startup and visibility-toggled by mode. Captions always append to `TranscriptLog` regardless of mode (mid-session switch reveals full history). On captions-disable edge, all four caption surfaces are cleared (TranscriptLog, transcript view, CaptionBuffer, overlay label).
   ```

4. **Update the "Invariants" section** if any pre-existing invariant referenced "two modes" — search for "Docked|Floating" and update if applicable.

**Verification:**

Run: `git diff CLAUDE.md`
Expected: shows freshness date update, two new module lines, and the new Overlay modes contract bullet. No deletions of existing content unrelated to the transcript work.

**Commit:**

```bash
git add CLAUDE.md
git commit -m "docs: refresh CLAUDE.md for transcript window mode

- Add transcript_log.rs and transcript_window.rs to module listing
- Document third OverlayMode (Transcript) contract
- Update freshness date"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 6 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib` passes all tests including `caption_buffer::tests::ac6_2_*`.
- Full 11-step manual end-to-end test passes.
- All commits across Phases 1–6 are present on the `transcript-window-mode` branch.

## Phase 6 Closes the Feature

After Phase 6, the design plan's "Definition of Done" is fully satisfied:

- ✓ Third overlay mode (`Transcript`) selectable from the tray radio (Phase 2).
- ✓ Layer-shell overlay hidden when Transcript is active; transcript window shown in its place (Phases 2 + 4).
- ✓ Timestamped paragraphs coalesced by ~1.5s silence gap (Phase 1).
- ✓ Auto-scroll-on-tail with chat-app pause-when-scrolled-up pattern (Phase 3).
- ✓ Text selection and copy via standard GTK keybindings (Phase 3 — built-in).
- ✓ System GTK theme; no custom CSS (Phase 3).
- ✓ Save Transcript button writing both `.txt` (paragraphs) and `.json` (per-fragment) (Phase 5).
- ✓ Empty on every launch — no session reload (Phases 1 + 4: TranscriptLog constructed fresh inside connect_activate).
- ✓ Same Nemotron STT pipeline and audio capture; no duplicate inference (architecture-level — no new STT thread anywhere in the changes).
- ✓ Mutually exclusive overlay vs. transcript window (Phase 4 routing).
- ✓ Clear-on-disable side effects (Phase 6).

The `transcript-window-mode` branch is ready for code review and merge.
