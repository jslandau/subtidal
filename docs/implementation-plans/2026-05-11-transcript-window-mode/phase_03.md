# Phase 3: Transcript GTK Window Implementation Plan

**Goal:** Build the transcript window in isolation: widget tree (`ApplicationWindow` + `HeaderBar` + `ScrolledWindow` + `TextView`), autoscroll-on-tail logic, dimmed-timestamp `TextTag`, `append_fragment_to_view`, and `clear_view`. The Save button in the header bar is wired to a stub click handler that prints to stderr — Phase 5 replaces the stub with the actual `FileDialog` flow.

**Architecture:** New module `src/overlay/transcript_window.rs`, declared from `src/overlay/mod.rs`. The module exports a `TranscriptWindowState` struct (holds the `ApplicationWindow`, `TextBuffer`, and `ScrolledWindow` for downstream code to manipulate) and three free functions: `build_transcript_window` (constructor), `append_fragment_to_view` (called once per caption), and `clear_view`. No layer-shell — this is a regular `gtk4::ApplicationWindow`, so no `init_layer_shell()` call.

**Tech Stack:** `gtk4 = "0.10"` (with `v4_10` feature, already present), `glib = "0.19"` re-exported via `gtk4::glib`, `chrono = "0.4"` (added in Phase 1).

**Scope:** Phase 3 of 6.

**Codebase verified:** 2026-05-11.
- `src/overlay/window.rs:6` — import style is `use gtk4::{Application, ApplicationWindow, Label};`. Phase 3 mirrors this style: `use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, HeaderBar, ScrolledWindow, TextBuffer, TextTag, TextView, WrapMode};`.
- `src/overlay/mod.rs:18` — `use gtk4::glib;` re-export pattern; we use `gtk4::glib::idle_add_local_once` to avoid pinning a specific glib crate version.
- `Cargo.toml:14` — `gtk4 = { version = "0.10", features = ["v4_10"] }` confirms `FileDialog`/`AlertDialog` are usable in Phase 5.
- `Cargo.toml:16` — `glib = "0.19"`. `glib::idle_add_local_once` is available in 0.19+ (confirmed via gtk4-rs 0.10 source dependency tree). Use the `gtk4::glib` re-export.
- Internet research confirmed: `gtk::StyleContext::lookup_color` is deprecated in GTK 4.10+; recommended replacement is to construct an `RGBA` directly. We will use a hard-coded RGBA approximating the GNOME `dim_label_color` (rgba 0.6, 0.6, 0.6, 1.0) so the transcript window matches GNOME's typical "secondary label" appearance without invoking deprecated APIs.

---

## Acceptance Criteria Coverage

This phase implements:

### transcript-window-mode.AC3: Transcript GTK window (Phase 3 "Done when")
- **transcript-window-mode.AC3.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC3.2 Window construction is exercised:** A test invokes `build_transcript_window(...)` (no display required if the test stays headless on the type level — see "Testing" caveat below) OR — if GTK testing infrastructure is impractical — a manual smoke test confirms the window appears with header bar, scrollable text view, and Save button.
- **transcript-window-mode.AC3.3 Append produces visible timestamped lines:** Manual smoke test (or stub-driver test): pushing synthetic Fragments via a timer appends visible timestamped lines.
- **transcript-window-mode.AC3.4 Autoscroll pause and resume:** Manual smoke test: scrolling up pauses autoscroll; scrolling back to bottom resumes it.
- **transcript-window-mode.AC3.5 Selection and copy:** Manual smoke test: Ctrl+A then Ctrl+C copies the full transcript to the clipboard (built-in GTK behavior since `editable=false, cursor_visible=false` does not disable selection).

The design plan acknowledges Phase 3 has "no automated test for the GTK widget itself" — we honor that constraint while still using the compiler to catch type errors in the constructor and append paths.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Create `src/overlay/transcript_window.rs` with widget tree and append/clear/autoscroll logic

**Type:** Functionality (GTK-coupled; widget behavior verified manually per design constraint).

**Verifies:** transcript-window-mode.AC3.1, transcript-window-mode.AC3.2, transcript-window-mode.AC3.3, transcript-window-mode.AC3.4, transcript-window-mode.AC3.5.

**Files:**
- Create: `/home/jslandau/git/live_text/src/overlay/transcript_window.rs`
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:3-7` (add `mod transcript_window;` to the module declaration block)

**Implementation:**

Generate the file from these specifications. Do NOT copy the design plan's contracts verbatim — use them as direction; produce idiomatic Rust appropriate to the gtk4-rs 0.10 API.

**Module-level imports:**
```rust
use crate::overlay::transcript_log::{AppendKind, Fragment, TranscriptLog};
use gtk4::glib;
use gtk4::pango::WrapMode;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Button, HeaderBar, ScrolledWindow,
    TextBuffer, TextTag, TextTagTable, TextView,
};
use std::cell::RefCell;
use std::rc::Rc;
```

(The `transcript_log` import is used by the `engine_name` and `session_start` parameters today only as a stored field for Phase 5; it doesn't yet need direct module access. But importing the types now keeps `build_transcript_window`'s signature self-contained.)

**Public types:**

```rust
/// Handles needed by the orchestration layer to drive the transcript window.
/// All fields are GTK objects holding internal `Rc` reference counts; this
/// struct is `Clone` for cheap propagation into closures.
#[derive(Clone)]
pub struct TranscriptWindowState {
    pub window: ApplicationWindow,
    pub buffer: TextBuffer,
    pub scrolled: ScrolledWindow,
    /// Tag applied to the timestamp prefix on each paragraph.
    pub timestamp_tag: TextTag,
}
```

**Constants:**

```rust
/// Threshold (in pixels) below the scroll bottom at which the view is
/// considered "tailing" and new appends should auto-scroll.
const AUTOSCROLL_THRESHOLD_PX: f64 = 16.0;

/// Foreground color for paragraph timestamp prefix. Approximates GNOME's
/// `dim_label_color` without using the deprecated StyleContext::lookup_color API.
const TIMESTAMP_RGBA: (f32, f32, f32, f32) = (0.6, 0.6, 0.6, 1.0);
```

**`build_transcript_window`:**

```rust
pub fn build_transcript_window(
    app: &Application,
    transcript_log: Rc<RefCell<TranscriptLog>>,
    engine_name: String,
    session_start: chrono::DateTime<chrono::Local>,
) -> TranscriptWindowState {
    // 1. Tag table + dimmed timestamp tag.
    let tag_table = TextTagTable::new();
    let timestamp_tag = TextTag::builder()
        .name("timestamp")
        .foreground_rgba(&gtk4::gdk::RGBA::new(
            TIMESTAMP_RGBA.0, TIMESTAMP_RGBA.1, TIMESTAMP_RGBA.2, TIMESTAMP_RGBA.3,
        ))
        .build();
    tag_table.add(&timestamp_tag);

    // 2. Buffer using that tag table.
    let buffer = TextBuffer::new(Some(&tag_table));

    // 3. TextView (read-only, word-wrap, no cursor).
    let text_view = TextView::builder()
        .buffer(&buffer)
        .wrap_mode(WrapMode::WordChar)
        .editable(false)
        .cursor_visible(false)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();

    // 4. ScrolledWindow wrapping the TextView.
    let scrolled = ScrolledWindow::builder()
        .child(&text_view)
        .vexpand(true)
        .hexpand(true)
        .build();

    // 5. HeaderBar with Save button on the end side.
    let header_bar = HeaderBar::new();
    let save_button = Button::with_label("Save…");
    header_bar.pack_end(&save_button);

    // 6. ApplicationWindow.
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Subtidal Transcript")
        .default_width(700)
        .default_height(500)
        .child(&scrolled)
        .build();
    window.set_titlebar(Some(&header_bar));
    window.set_visible(false); // mode-switch wiring in Phase 4 controls visibility

    // 7. Save button stub handler — Phase 5 replaces this with FileDialog.
    //    The closure captures `transcript_log`, `engine_name`, and
    //    `session_start` (prefixed `_` here so the unused-variable lint
    //    stays quiet until Phase 5 actually consumes them).
    {
        let _log = Rc::clone(&transcript_log);
        let _engine = engine_name.clone();
        let _start = session_start;
        save_button.connect_clicked(move |_btn| {
            // Reference the captures so the closure-level binding sees them
            // as "used" (the closure body itself doesn't use them yet).
            let _ = (&_log, &_engine, &_start);
            eprintln!("transcript: Save clicked (Phase 5 will wire FileDialog)");
        });
    }

    TranscriptWindowState { window, buffer, scrolled, timestamp_tag }
}
```

**Important gtk4-rs 0.10 detail on `RGBA`:** `gtk4::gdk::RGBA::new(r, g, b, a)` takes `f32` arguments (not `f64`) in 0.10. The constants above use `f32`. Verify by checking the local docs of the installed crate version: `cargo doc --open -p gtk4` is available in dev environments, but not required — if compilation fails, the error message will pinpoint the type mismatch.

**`append_fragment_to_view`:**

```rust
pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
) {
    // 1. Sample autoscroll position BEFORE inserting.
    let was_at_bottom = is_near_bottom(&state.scrolled);

    // 2. Insert paragraph break + timestamp prefix on NewParagraph.
    let mut end = state.buffer.end_iter();
    if matches!(kind, AppendKind::NewParagraph) {
        // If the buffer has any prior content, prepend a newline to start a new line.
        if state.buffer.char_count() > 0 {
            state.buffer.insert(&mut end, "\n");
        }
        let timestamp_text = fragment.timestamp.format("[%H:%M:%S] ").to_string();
        state.buffer.insert_with_tags(&mut end, &timestamp_text, &[&state.timestamp_tag]);
    }

    // 3. Insert the fragment text (whitespace verbatim — the leading-space
    //    word-boundary signal must be preserved per the RNNT engine contract).
    state.buffer.insert(&mut end, &fragment.text);

    // 4. If we were tailing, schedule a scroll-to-bottom for after layout.
    if was_at_bottom {
        let scrolled = state.scrolled.clone();
        glib::idle_add_local_once(move || {
            let adj = scrolled.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        });
    }
}
```

**`clear_view`:**

```rust
pub fn clear_view(state: &TranscriptWindowState) {
    let (mut start, mut end) = (state.buffer.start_iter(), state.buffer.end_iter());
    state.buffer.delete(&mut start, &mut end);
}
```

**`is_near_bottom` helper:**

```rust
fn is_near_bottom(scrolled: &ScrolledWindow) -> bool {
    let adj = scrolled.vadjustment();
    let value = adj.value();
    let upper = adj.upper();
    let page_size = adj.page_size();
    // "near" = within AUTOSCROLL_THRESHOLD_PX of the bottom edge,
    //          or the content is shorter than one page (always tail).
    let bottom = upper - page_size;
    value >= bottom - AUTOSCROLL_THRESHOLD_PX
}
```

**Note on `is_near_bottom` first-append edge case:** When the view is freshly constructed with no content, `upper == page_size` so `bottom == 0` and `value == 0`, so the predicate returns `true` — autoscroll is enabled by default until the user scrolls up. That matches the design's "chat-app pattern."

**Module declaration in `src/overlay/mod.rs`:**

After the Phase 1 edit, the block at lines 3–8 reads:
```rust
mod caption_buffer;
mod drag;
mod transcript_log;
mod window;

pub mod input_region;
```
Edit it to add `transcript_window` (alphabetical order):
```rust
mod caption_buffer;
mod drag;
mod transcript_log;
mod transcript_window;
mod window;

pub mod input_region;
```

**Dead-code suppression:** Like Phase 1, `transcript_window`'s public items will be unused until Phase 4. Add `#![allow(dead_code)]` at the top of `src/overlay/transcript_window.rs`. Phase 4 removes this attribute.

**Testing:**

Per the design plan, this phase has **no automated tests for the GTK widget itself** — GTK windows require a display server and the gtk4-rs test harness is fragile in CI. The verification surface for Phase 3 is:

1. **Compiler-level:** `cargo build` proves the widget tree, `TextTag` registration, `insert_with_tags` call signatures, `idle_add_local_once` import path, and `RGBA::new` argument types are all correct.
2. **Manual smoke test:** see Verification below.

If you find yourself wanting an automated test, the right place is Phase 4's orchestration verification, not Phase 3.

**Verification:**

Run: `cargo build`
Expected: Compiles cleanly with no warnings.

Run: `cargo test --lib`
Expected: All Phase 1 and Phase 2 tests still pass; no new tests in this phase.

**Manual smoke test:** Deferred to Phase 4. Phase 4 wires both windows through the real app, exercising `build_transcript_window`, `append_fragment_to_view`, and the autoscroll/selection paths end-to-end with real STT input. There is no Phase-3-isolated smoke test because the modules are private (`mod transcript_window`, not `pub mod`) and exposing them temporarily for a one-off `examples/` binary creates a workflow hazard (easy to forget to revert).

For Phase 3 verification, **`cargo build` cleanly is sufficient**. All widget-level behavior is verified in Phase 4's manual end-to-end test.

**Commit:**

```bash
git add src/overlay/transcript_window.rs src/overlay/mod.rs
git commit -m "feat(transcript): add transcript_window GTK module"
```
<!-- END_TASK_1 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 3 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib` reports all pre-existing tests passing (no regressions).
- The new module compiles in isolation; widget construction, autoscroll, and append paths are type-correct.

## What Phase 3 Deliberately Does NOT Do

- Does not construct any `TranscriptWindowState` from `overlay/mod.rs` — that is Phase 4.
- Does not implement the Save button click handler (only stubs it) — Phase 5.
- Does not respond to `OverlayCommand::SetMode(Transcript)` — Phase 4.
- Does not call `idle_add_local_once` from any thread other than the GTK main thread (because no thread other than GTK main calls `append_fragment_to_view` yet).
- Does not introduce automated GTK widget tests; the design plan explicitly disallows them for this phase.
