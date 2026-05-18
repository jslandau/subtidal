# Phase 1: Pure-Data Foundation Implementation Plan

**Goal:** Build `TranscriptLog`, `Fragment`, `Paragraph`, and `AppendKind` types with paragraph-coalescing logic, fully unit-tested with no GTK dependency.

**Architecture:** Pure-data Rust module placed at `src/overlay/transcript_log.rs`, declared from `src/overlay/mod.rs`. Mirrors the GTK-free pattern of `src/overlay/caption_buffer.rs`. Uses `chrono::DateTime<chrono::Local>` for timestamps and `serde_json::Value` for JSON serialization. A `push_at(text, ts)` test seam allows deterministic unit tests without mocking the system clock.

**Tech Stack:** Rust 2021 edition, `chrono = "0.4"` (with `serde` feature), `serde_json = "1"`. No GTK. No async.

**Scope:** Phase 1 of 6.

**Codebase verified:** 2026-05-11 via codebase-investigator. Verified: `chrono` and `serde_json` are NOT in `Cargo.toml` (must be added); `src/overlay/mod.rs` declares `mod caption_buffer; mod drag; mod window; pub mod input_region;` at lines 3–7; existing test naming convention is `acN_M_*` (e.g., `ac1_1_fill_single_line`), NOT `tN_*` as the design plan stated — we use the existing project convention. Test style: `#[cfg(test)] mod tests` at end of file, fixed inputs, `assert_eq!` with descriptive messages, direct timestamp manipulation for time-dependent cases.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### transcript-window-mode.AC1: Pure-data foundation (Phase 1 "Done when")
- **transcript-window-mode.AC1.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC1.2 Paragraph coalescing on 1.5 s gap:** Tests cover paragraph coalescing on the 1.5 s silence gap (fragments arriving ≤1.5 s apart belong to the same paragraph; >1.5 s apart start a new paragraph).
- **transcript-window-mode.AC1.3 Whitespace preservation:** Tests cover whitespace preservation (leading-space continuation per the RNNT word-boundary contract).
- **transcript-window-mode.AC1.4 to_json shape:** Tests cover `to_json` shape including session metadata (`session_start`, `engine`, `fragments` array of `{timestamp, text}`).
- **transcript-window-mode.AC1.5 paragraphs() derivation:** Tests cover `paragraphs()` derivation matching the `.txt` save output (each paragraph timestamped at the first fragment of its run).
- **transcript-window-mode.AC1.6 clear() empties fragments:** `clear()` empties fragments (post-`clear()`, `fragments()` returns empty slice; subsequent `push()` returns `NewParagraph`).

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add `chrono` and `serde_json` dependencies to `Cargo.toml`

**Type:** Infrastructure.

**Files:**
- Modify: `/home/jslandau/git/live_text/Cargo.toml`

**Implementation:**

In `Cargo.toml`, locate the `[dependencies]` section. After the existing `serde = { version = "1", features = ["derive"] }` line (currently line 44), add:

```toml
serde_json = "1"
chrono = { version = "0.4", default-features = false, features = ["clock", "serde", "std"] }
```

**Notes on chrono features:**
- `clock` is required to call `chrono::Local::now()`.
- `serde` is required so `DateTime<Local>` derives `Serialize`/`Deserialize` (RFC-3339 / ISO-8601 output).
- `std` is the default; `default-features = false` lets us opt out of the `oldtime`/`wasmbind` defaults that this CLI binary doesn't need. Keep additions minimal.

**Verification:**

Run: `cargo build --release`
Expected: Compiles cleanly. (No code uses these crates yet, but `cargo build` resolves and compiles them.)

Run: `cargo metadata --format-version 1 | grep -o '"name":"chrono"' | head -1`
Expected: `"name":"chrono"` printed, confirming chrono is in the dependency graph.

**Commit:**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add chrono and serde_json for transcript log"
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Create `src/overlay/transcript_log.rs` and declare it from `mod.rs`

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC1.1, transcript-window-mode.AC1.2, transcript-window-mode.AC1.3, transcript-window-mode.AC1.4, transcript-window-mode.AC1.5, transcript-window-mode.AC1.6.

**Files:**
- Create: `/home/jslandau/git/live_text/src/overlay/transcript_log.rs`
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:3-7` (add `mod transcript_log;` to the existing module declaration block)
- Test: `/home/jslandau/git/live_text/src/overlay/transcript_log.rs` (`#[cfg(test)] mod tests` at end of file — same pattern as `caption_buffer.rs`)

**Implementation:**

Create the file with the following structure. **Generate the code yourself based on the contracts below** — do not copy verbatim. The contract from the design plan is:

```rust
pub struct Fragment {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub text: String,
}

pub enum AppendKind {
    NewParagraph,
    ContinueParagraph,
}

pub struct Paragraph {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub text: String,
}

pub struct TranscriptLog { /* internal */ }

impl TranscriptLog {
    pub fn new(paragraph_gap: std::time::Duration) -> Self;
    pub fn push(&mut self, text: String) -> AppendKind;
    pub fn push_at(&mut self, text: String, ts: chrono::DateTime<chrono::Local>) -> AppendKind;
    pub fn fragments(&self) -> &[Fragment];
    pub fn paragraphs(&self) -> Vec<Paragraph>;
    pub fn to_json(&self, engine_name: &str, session_start: chrono::DateTime<chrono::Local>) -> serde_json::Value;
    pub fn clear(&mut self);
}
```

**Detailed semantics — read carefully:**

1. `Fragment` derives `Debug, Clone, Serialize` (from `serde::Serialize`). Does NOT derive `Deserialize` — we never read these back into Rust; `.json` is for downstream tooling only.
2. `AppendKind` is `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`.
3. `Paragraph` derives `Debug, Clone, PartialEq, Eq` (used by tests and the `.txt` save formatter).
4. `TranscriptLog` internal state:
   - `fragments: Vec<Fragment>`
   - `paragraph_gap: std::time::Duration` (the silence threshold that demarcates paragraphs; the design says ~1.5 s, the constructor accepts it as a parameter for testability).
5. `push(text)` is a thin wrapper that calls `push_at(text, chrono::Local::now())`.
6. `push_at(text, ts)`:
   - Determines `AppendKind` BEFORE pushing: if `fragments` is empty, returns `NewParagraph`. Otherwise, computes `gap = ts.signed_duration_since(last.timestamp)` and returns `NewParagraph` if `gap.to_std().unwrap_or_default() > paragraph_gap`, else `ContinueParagraph`. Use `chrono::Duration::to_std()` because `signed_duration_since` returns a signed `chrono::Duration`. If the gap is negative (clock went backwards — unusual but possible), treat it as `ContinueParagraph` (use `unwrap_or(Duration::ZERO)`).
   - Pushes `Fragment { timestamp: ts, text }` onto the vec.
   - Returns the previously-determined `AppendKind`.
7. `fragments()` returns `&self.fragments`.
8. `paragraphs()` walks `self.fragments`, splitting on the same gap rule used by `push_at`. For each paragraph: timestamp = first fragment's timestamp; text = concatenation of every fragment's text in order (no separator, no trim — leading whitespace on continuation fragments is the RNNT word-boundary signal and must be preserved verbatim).
9. `to_json(engine_name, session_start)` produces the exact shape from the design plan:
   ```json
   {
     "session_start": "2026-05-10T14:31:58.123-07:00",
     "engine": "nemotron",
     "fragments": [
       {"timestamp": "2026-05-10T14:32:01.456-07:00", "text": "Hello everyone,"}
     ]
   }
   ```
   Use `serde_json::json!({ ... })` for the outer object and rely on chrono's serde impl for timestamps. The `"fragments"` value is `serde_json::to_value(&self.fragments)?` — but since `to_json` returns `serde_json::Value` (not `Result`), call `.unwrap()` on a serialization that cannot fail (Vec of structs with simple field types is infallible).
10. `clear()`: `self.fragments.clear();`. After `clear()`, the next `push()` returns `NewParagraph` because the vec is empty.

**Behavioral asymmetry note (deliberate):** `TranscriptLog::push` accepts ALL fragments verbatim — including whitespace-only or empty strings. This differs from `CaptionBuffer::push` (in `caption_buffer.rs`), which short-circuits on `text.trim().is_empty()`. The asymmetry is intentional: `CaptionBuffer` is a *display buffer* that filters perceptually-irrelevant tokens to keep the overlay readable, while `TranscriptLog` is an *append-only durable history* that must preserve every signal the engine emitted (including the leading-space word-boundary marker that, when stripped, would lose information). Downstream `.json` consumers may filter as they see fit; the log itself does not. Do NOT add a `trim`/`is_empty` guard to `TranscriptLog::push`.

**Module declaration in `src/overlay/mod.rs`:**

The existing block at lines 3–7 reads:
```rust
mod caption_buffer;
mod drag;
mod window;

pub mod input_region;
```

Edit it to add the new module — keep alphabetical order so additions don't churn diffs:
```rust
mod caption_buffer;
mod drag;
mod transcript_log;
mod window;

pub mod input_region;
```

Phase 1 only declares the module. Phase 2 wires `OverlayCommand::SetCaptionsEnabled`, Phase 3 introduces `transcript_window`, and Phase 4 actually constructs a `TranscriptLog` instance. Until then `transcript_log` is dead code — that's fine because `cargo build` does not warn about unused private modules unless functions inside them are unused, AND we only have `pub` items, AND those `pub` items are reachable transitively through `pub use` from `mod.rs`? **No** — they are not yet `pub use`'d. To suppress the dead-code warning during this isolated phase, add `#![allow(dead_code)]` at the top of the new file. Phase 4 will remove this attribute when the constructors are actually called.

**Testing:**

Tests must verify each AC listed above. Place all tests in a `#[cfg(test)] mod tests { ... }` block at the end of `transcript_log.rs`. Use the project's existing naming convention: `acN_M_short_description` (e.g., `ac1_2_paragraph_break_after_gap`). Imports inside the test module:
```rust
use super::*;
use chrono::{Duration, TimeZone, Local};
```

Build a small helper at the top of the test module to construct timestamps deterministically:
```rust
fn ts(secs: i64, nanos: u32) -> chrono::DateTime<chrono::Local> {
    Local.timestamp_opt(secs, nanos).unwrap()
}
```

Required tests (one per AC case):

- `ac1_1_build_success` — N/A as a runtime test; AC1.1 is verified by `cargo build` itself. Skip; do NOT invent a tautological test.
- `ac1_2_paragraph_break_after_gap` — Construct a log with `paragraph_gap = Duration::from_millis(1500)`. Push three fragments at t=0, t=1.0s, t=3.0s. Assert: first push returns `NewParagraph`; second returns `ContinueParagraph` (gap = 1.0s ≤ 1.5s); third returns `NewParagraph` (gap = 2.0s > 1.5s).
- `ac1_2b_paragraph_continue_under_gap` — Boundary test. Push at t=0 and t=1.5s exactly. Assert second push returns `ContinueParagraph` (gap = 1.5s, not strictly greater than 1.5s).
- `ac1_3_whitespace_preserved_in_paragraphs` — Push three fragments: `"Hello"`, `" world"`, `","` within the gap. Assert `paragraphs()` returns one paragraph with text `"Hello world,"` (leading space on `" world"` preserved, no separator inserted between adjacent fragments).
- `ac1_4_to_json_shape` — Push two fragments at known timestamps. Call `to_json("nemotron", session_start)`. Assert the returned `serde_json::Value` is an object with keys `"session_start"`, `"engine"`, `"fragments"`. Assert `engine` == `"nemotron"`. Assert `fragments` is an array of length 2; assert each element has `"timestamp"` and `"text"` keys with the expected string values. Use `serde_json::json!` for expected, then compare with `assert_eq!`. (Comparing `serde_json::Value` is supported.)
- `ac1_4b_to_json_timestamp_format_rfc3339` — Push one fragment at a known instant. Serialize. Extract `fragments[0].timestamp` as a string and assert it parses with `chrono::DateTime::parse_from_rfc3339(..)` successfully (this confirms chrono's serde impl produces RFC-3339, which is the documented contract).
- `ac1_5_paragraphs_derivation_matches_txt` — Push fragments mimicking the `.txt` example in the design plan (two paragraphs with the 1.5s gap between them). Assert `paragraphs()` returns two `Paragraph` entries with the right text bodies. The first paragraph's `.timestamp` matches the first fragment's; the second's matches the first fragment of its own run.
- `ac1_6_clear_empties_fragments` — Push three fragments. Assert `fragments().len() == 3`. Call `clear()`. Assert `fragments().is_empty()`. Push one more fragment. Assert it returns `NewParagraph` (because the vec is empty again) and `fragments().len() == 1`.
- `ac1_extra_paragraphs_empty_when_no_fragments` — Defensive: a fresh `TranscriptLog` returns an empty `Vec` from `paragraphs()`.

**Verification:**

Run: `cargo build`
Expected: Compiles cleanly with no warnings.

Run: `cargo test --lib transcript_log`
Expected: All `transcript_log::tests::*` tests pass. Specifically all `ac1_*` tests above.

Run: `cargo test --lib` (full suite)
Expected: Pre-existing tests (caption_buffer, etc.) still pass.

**Commit:**

```bash
git add src/overlay/transcript_log.rs src/overlay/mod.rs
git commit -m "feat(transcript): add TranscriptLog pure-data module with tests"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 1 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib transcript_log` reports all `ac1_*` tests passing.
- `cargo test --lib` overall suite passes (no regressions in `caption_buffer` or other modules).
- `git log` shows two commits in this phase: one for the dep bump, one for the module.

## What Phase 1 Deliberately Does NOT Do

- Does not import `gtk4`, `glib`, `gio`, or any GTK-related crate.
- Does not construct any `TranscriptLog` instance from `overlay/mod.rs` — that wiring is Phase 4.
- Does not introduce `OverlayCommand::SetCaptionsEnabled` — that's Phase 2.
- Does not introduce `OverlayMode::Transcript` — that's Phase 2.
- Does not implement `transcript_window.rs` — that's Phase 3.

The `#![allow(dead_code)]` on `transcript_log.rs` is intentional and removed in Phase 4.
