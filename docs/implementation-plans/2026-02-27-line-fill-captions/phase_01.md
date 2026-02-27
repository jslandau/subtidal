# Line-Fill Captions Implementation Plan — Phase 1: CaptionBuffer Rewrite

**Goal:** Replace fragment-based CaptionBuffer with line-based buffer implementing fill-and-shift caption display.

**Architecture:** CaptionBuffer holds `Vec<CaptionLine>` where each line tracks text content and a `last_active` timestamp. Push logic fills lines word-by-word up to `max_chars_per_line`, shifts oldest line off when all lines are full. Expiry removes one line per tick during silence.

**Tech Stack:** Rust, std::time::Instant

**Scope:** 3 phases from original design (phase 1 of 3)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### line-fill-captions.AC1: Fill-and-shift line model
- **line-fill-captions.AC1.1 Success:** Text fills line 1 left-to-right, word by word, up to max_chars_per_line
- **line-fill-captions.AC1.2 Success:** When line 1 is full, text continues on line 2 (up to max_lines)
- **line-fill-captions.AC1.3 Success:** When all lines are full and new text arrives, line 1 is removed, all lines shift up, new text fills the freed bottom line
- **line-fill-captions.AC1.4 Success:** Continuation fragments (no leading space) join the previous word on the same line without inserting a space
- **line-fill-captions.AC1.5 Edge:** When a continuation fragment would cause the combined word to overflow the current line, the partial word moves to the next line and joins there (words never split across lines)
- **line-fill-captions.AC1.6 Edge:** RNNT decoder overlap is deduplicated (same behavior as current)

### line-fill-captions.AC2: Idle timer clears oldest line
- **line-fill-captions.AC2.1 Success:** When no new text arrives for expire_secs, the oldest (top) line is removed and remaining lines shift up
- **line-fill-captions.AC2.2 Success:** Expiry continues once per second until all lines are cleared during silence
- **line-fill-captions.AC2.3 Success:** Active lines (receiving new text) do not expire — last_active resets on each push

### line-fill-captions.AC4: Conservative character width estimate
- **line-fill-captions.AC4.1 Success:** estimate_max_chars returns a value ~15% smaller than previous calculation, providing visual padding

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: CaptionLine struct and CaptionBuffer rewrite

**Verifies:** line-fill-captions.AC1.1, line-fill-captions.AC1.2, line-fill-captions.AC1.3, line-fill-captions.AC1.4, line-fill-captions.AC1.5, line-fill-captions.AC1.6, line-fill-captions.AC2.1, line-fill-captions.AC2.2, line-fill-captions.AC2.3

**Files:**
- Modify: `src/overlay/mod.rs:13-121` — Replace `CaptionBuffer` struct and impl block entirely

**Implementation:**

Replace the entire `CaptionBuffer` struct (lines 13-121) with a new line-based model. Preserve `remove_overlap` exactly as-is (lines 72-92).

**CaptionLine struct:**

```rust
struct CaptionLine {
    text: String,
    last_active: Instant,
}
```

**CaptionBuffer struct (new fields):**

```rust
struct CaptionBuffer {
    lines: Vec<CaptionLine>,
    max_lines: usize,
    max_chars_per_line: usize,
    expire_secs: u64,
    /// Track the last few words to detect and skip repeated output from the RNNT decoder.
    last_tail: String,
}
```

**Constructor:**

```rust
fn new(max_lines: usize, max_chars_per_line: usize, expire_secs: u64) -> Self {
    CaptionBuffer {
        lines: Vec::new(),
        max_lines,
        max_chars_per_line,
        expire_secs,
        last_tail: String::new(),
    }
}
```

**Push logic (`fn push(&mut self, text: String)`):**

1. Return early if `text.trim().is_empty()`
2. Build `last_tail` from all lines joined: `self.all_text()` — helper that joins all line text
3. Deduplicate via existing `remove_overlap(&self.last_tail, text.trim())`
4. Return early if deduped is empty
5. Preserve leading space from original engine output (same logic as current lines 52-56)
6. Determine if this is a continuation fragment: `!fragment.starts_with(char::is_whitespace)` AND `self.lines` is not empty
7. If continuation fragment:
   - Get the current (last) line's text
   - If appending the fragment to the last word on the current line would NOT overflow `max_chars_per_line`: append directly to current line's text, update `last_active`
   - If it WOULD overflow: find the last space in the current line. If found, split the partial word off (everything after last space), remove that partial from the current line, then add a new line (shifting if needed) with the partial word + continuation fragment joined. If no space found (entire line is one word), just start a new line with the continuation.
8. If NOT a continuation fragment (starts with space or lines are empty):
   - Split fragment into words (by whitespace)
   - For each word: if the current line is empty, place the word directly (no space prefix, check `word.len() <= max_chars_per_line`). If the current line is non-empty and has room (`current_line.text.len() + 1 + word.len() <= max_chars_per_line`), append ` {word}` (space + word) to the current line. Otherwise, add a new line (shifting if at max_lines) and start the new line with the word (no space prefix).
9. After all words placed, update `last_active` on the bottom line to `Instant::now()`
10. Rebuild `last_tail` from `self.all_text()`, keeping last 60 chars

**Helper `fn all_text(&self) -> String`:** joins all line text with empty string (`""`). This is correct because each word within a line already carries its leading space from the engine output (e.g., line text is `"Hello world"` not `"Hello" + " " + "world"`). The first word on each line has no leading space.

**Shift logic (called when adding a new line and `self.lines.len() >= self.max_lines`):** remove `self.lines[0]`, then push new `CaptionLine` at the end.

**Expire logic (`fn expire(&mut self) -> bool`):**

New line-granularity expiry:
1. If `self.lines` is empty, return false
2. Check oldest line (`self.lines[0]`): if `last_active` is older than `expire_secs` from now, remove it
3. Only remove ONE line per call (gradual drain)
4. Rebuild `last_tail` after removal
5. Return true if a line was removed

**Display text (`fn display_text(&self) -> String`):**

```rust
fn display_text(&self) -> String {
    self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
}
```

**`remove_overlap` — preserve unchanged** from current lines 72-92.

**Verification:**
Run: `cargo build`
Expected: Compiles without errors

**Commit:** `feat(overlay): rewrite CaptionBuffer with line-fill model`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: CaptionBuffer unit tests

**Verifies:** line-fill-captions.AC1.1, line-fill-captions.AC1.2, line-fill-captions.AC1.3, line-fill-captions.AC1.4, line-fill-captions.AC1.5, line-fill-captions.AC1.6, line-fill-captions.AC2.1, line-fill-captions.AC2.2, line-fill-captions.AC2.3

**Files:**
- Modify: `src/overlay/mod.rs` — Add `#[cfg(test)] mod tests` block (or extend existing one which currently has 2 CSS tests at the bottom of the file)

**Testing:**

Add tests inside the existing `#[cfg(test)] mod tests` block in `src/overlay/mod.rs`. Use `use super::*;` to access private `CaptionBuffer` and `CaptionLine`.

Tests must verify each AC listed above:

- **line-fill-captions.AC1.1:** Push words with leading spaces (e.g., `" Hello"`, `" world"`, `" this"`) to a buffer with `max_chars_per_line=20, max_lines=3`. Verify `display_text()` shows all words on line 1 joined with spaces, no `\n` present.
- **line-fill-captions.AC1.2:** Push enough words to overflow line 1. Verify `display_text()` contains `\n` separating two lines, with the overflow word on line 2.
- **line-fill-captions.AC1.3:** Push enough words to fill all `max_lines`, then push more. Verify the first line's content has been removed, lines shifted up, and new word is on the bottom line. Total line count equals `max_lines`.
- **line-fill-captions.AC1.4:** Push `" Hel"` then `"lo"` (no leading space = continuation). Verify they join as `"Hello"` on the same line without an inserted space.
- **line-fill-captions.AC1.5:** Set up a line nearly full, push a word-start that fits, then push a continuation that would overflow. Verify the partial word moved to the next line and joined with the continuation there (not split across lines).
- **line-fill-captions.AC1.6:** Push text with overlapping prefix matching last_tail (4+ chars). Verify deduplication removes the overlap (same behavior as current `remove_overlap`).
- **line-fill-captions.AC2.1:** Create a buffer, push text, then manually set `last_active` on the oldest line to `Instant::now() - Duration::from_secs(expire_secs + 1)`. Call `expire()`. Verify oldest line was removed and remaining lines shifted up.
- **line-fill-captions.AC2.2:** Set up multiple lines all with expired timestamps. Call `expire()` multiple times. Verify one line removed per call until all are cleared.
- **line-fill-captions.AC2.3:** Set up two lines, expire only the oldest. Push new text. Verify `last_active` on the active line has been refreshed (not expired on next call).

Note: For expiry tests, you'll need to manipulate `last_active` directly on the `CaptionLine` since `Instant` can't be constructed from arbitrary times. Create lines manually in tests and insert them into the buffer's `lines` vec.

**Verification:**
Run: `cargo test --bin subtidal caption`
Expected: All new CaptionBuffer tests pass

Run: `cargo test --bin subtidal`
Expected: All existing tests still pass (including the 2 CSS tests in the same module)

**Commit:** `test(overlay): add CaptionBuffer line-fill unit tests`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_3 -->
### Task 3: Conservative multiplier for estimate_max_chars

**Verifies:** line-fill-captions.AC4.1

**Files:**
- Modify: `src/overlay/mod.rs:429-440` — Apply 0.85× multiplier to `estimate_max_chars`

**Implementation:**

In the existing `estimate_max_chars` function at line 429, apply a 0.85× conservative multiplier to the final result. Change the last line from:

```rust
(usable_width / avg_char_width).floor() as i32
```

to:

```rust
(usable_width / avg_char_width * 0.85).floor() as i32
```

This makes lines ~15% shorter than theoretical max, providing visual padding for proportional font width variation.

**Testing:**

Add a test to the existing `#[cfg(test)] mod tests` block:

- **line-fill-captions.AC4.1:** Call `estimate_max_chars` with known inputs (e.g., `width_px=800, font_size_pt=24.0`). Compute the old formula result manually (`(800 - 24).max(100) as f32 / (24.0 * 0.6)` = `776.0 / 14.4` = 53). Verify the new result is approximately `53 * 0.85 = 45`. The key assertion: new result is strictly less than old formula result, and approximately 85% of it.

**Verification:**
Run: `cargo test --bin subtidal estimate`
Expected: Conservative multiplier test passes

**Commit:** `feat(overlay): apply 0.85x conservative multiplier to estimate_max_chars`

<!-- END_TASK_3 -->
