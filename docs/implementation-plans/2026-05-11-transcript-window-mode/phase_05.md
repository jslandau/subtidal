# Phase 5: Save Dialog + Dual-Write Implementation Plan

**Goal:** Replace the Phase 3 stub Save click handler with a real `gtk::FileDialog` flow that, on success, writes both `<chosen-stem>.txt` (paragraph view) and `<chosen-stem>.json` (per-fragment with session metadata) using `TranscriptLog::paragraphs()` and `TranscriptLog::to_json(...)` from Phase 1. On either-side write failure, surface a `gtk::AlertDialog` with the error and the path of whichever file did succeed.

**Architecture:** All changes are inside `src/overlay/transcript_window.rs`. The Save button click handler is rewritten to spawn a local future via `glib::MainContext::default().spawn_local`, which `await`s `dialog.save_future(Some(&window))`. On success, we extract the chosen `gio::File`'s path, derive a `.json` sibling by replacing the extension on the path stem, and call `std::fs::write` for each. We collect the per-side `Result` and report any failure via `AlertDialog::show`. The `transcript_log`, `engine_name`, and `session_start` parameters that Phase 3 captured into the stub closure are now actually used.

**Tech Stack:** `gtk4 = "0.10"` with `v4_10` feature (`FileDialog`, `AlertDialog` available). `serde_json = "1"` (added Phase 1) for `to_string_pretty`. `gio` re-exported via `gtk4::gio`.

**Scope:** Phase 5 of 6.

**Codebase verified:** 2026-05-11.
- `Cargo.toml:14` — `gtk4 = { version = "0.10", features = ["v4_10"] }` ✓ FileDialog/AlertDialog usable.
- `src/overlay/transcript_window.rs` — was created in Phase 3 with stub handler at the `save_button.connect_clicked(...)` block.
- `src/overlay/transcript_log.rs::paragraphs()` returns `Vec<Paragraph>` (each with `timestamp` and `text`); `to_json(engine_name, session_start)` returns `serde_json::Value`. Both are GTK-free, callable from any thread (we call them from the GTK main thread via `borrow()`).
- Internet research confirmed: `gtk::FileDialog::save_future(parent: Option<&impl IsA<Window>>)` is the idiomatic async API in gtk4-rs 0.10. Returns `Pin<Box<dyn Future<Output = Result<gio::File, glib::Error>>>>`. The `gio::File::path()` method returns `Option<PathBuf>` (None for non-local URIs — we treat as an error case).

---

## Acceptance Criteria Coverage

This phase implements:

### transcript-window-mode.AC5: Save dialog + dual-write (Phase 5 "Done when")
- **transcript-window-mode.AC5.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC5.2 Both files written on success:** Manual test — in Transcript mode after some speech, click Save, choose a path, verify both `.txt` and `.json` files exist with expected content.
- **transcript-window-mode.AC5.3 Default filename uses session timestamp:** Default filename is `subtidal-transcript-<YYYY-MM-DD-HHMMSS>.txt` derived from `session_start`. Verified by inspecting the FileDialog default in the manual test.
- **transcript-window-mode.AC5.4 Malformed paths surface AlertDialog, not crash:** Manual test — verify malformed paths produce an alert dialog rather than a crash.
- **transcript-window-mode.AC5.5 Partial-success reporting:** If one side writes but the other fails, the AlertDialog reports the error AND the path of the file that did succeed (so the user can manually recover).
- **transcript-window-mode.AC5.6 .json sibling is silently overwritten:** `gtk::FileDialog` only prompts for overwrite on the user-chosen `.txt` path. The `.json` sibling is silently overwritten — documented in a code comment near the save handler.

Per design plan, Phase 5 has no automated tests for the GTK file dialog path (mocking it is impractical). We add ONE pure-data unit test for the path-derivation helper, since that logic is testable in isolation.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add a pure helper `derive_json_sibling(path: &Path) -> PathBuf` with unit test

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC5.1, transcript-window-mode.AC5.6.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/transcript_window.rs` (add helper near the bottom, before `#[cfg(test)] mod tests` block — create the test block if it does not exist; this phase introduces the first test in this file).

**Implementation:**

The save handler chooses the `.txt` path via FileDialog and must derive a `.json` path that is a sibling sharing the file stem. Pulling this out into a pure function keeps the GTK closure short and makes the path logic testable.

Add to `src/overlay/transcript_window.rs`:

```rust
/// Given the user-chosen `.txt` path (or any path), return the sibling `.json`
/// path with the same stem. If the input has no extension or a non-`.txt`
/// extension, the `.json` is appended to the stem unchanged.
///
/// Examples:
/// - `/tmp/foo.txt` -> `/tmp/foo.json`
/// - `/tmp/foo`     -> `/tmp/foo.json`
/// - `/tmp/foo.bar` -> `/tmp/foo.json`  (extension replaced)
pub fn derive_json_sibling(path: &std::path::Path) -> std::path::PathBuf {
    let mut out = path.to_path_buf();
    out.set_extension("json");
    out
}
```

**Testing:**

Add (or create) `#[cfg(test)] mod tests { ... }` at the end of `src/overlay/transcript_window.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ac5_6_json_sibling_replaces_txt_extension() {
        let p = PathBuf::from("/tmp/transcript.txt");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_no_extension() {
        let p = PathBuf::from("/tmp/transcript");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_other_extension() {
        let p = PathBuf::from("/tmp/transcript.log");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_with_dots_in_stem() {
        // PathBuf::set_extension only replaces the LAST extension; "a.b.txt" -> "a.b.json".
        let p = PathBuf::from("/tmp/2026.05.11.txt");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/2026.05.11.json"));
    }
}
```

These tests guard the path-derivation invariant against future regressions and satisfy AC5.6.

**Verification:**

Run: `cargo test --lib transcript_window`
Expected: All four `ac5_6_*` tests pass.

**Commit:** Combined at end of Phase 5.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Replace the stub Save handler with the real FileDialog → dual-write → AlertDialog flow

**Type:** Functionality (GTK-coupled; verified manually).

**Verifies:** transcript-window-mode.AC5.1, transcript-window-mode.AC5.2, transcript-window-mode.AC5.3, transcript-window-mode.AC5.4, transcript-window-mode.AC5.5.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/transcript_window.rs` — replace the stub handler in `build_transcript_window`; add `format_paragraphs_as_txt` helper.
- (No changes to other files.)

**Implementation:**

**(a) Add a paragraph formatter** — pure helper near the bottom of `transcript_window.rs`:

```rust
/// Format a slice of paragraphs as the `.txt` save body:
/// `[HH:MM:SS] <paragraph text>\n` per paragraph.
pub fn format_paragraphs_as_txt(paragraphs: &[crate::overlay::transcript_log::Paragraph]) -> String {
    let mut out = String::new();
    for p in paragraphs {
        out.push_str(&p.timestamp.format("[%H:%M:%S] ").to_string());
        out.push_str(&p.text);
        out.push('\n');
    }
    out
}
```

Add a unit test for this in the same `#[cfg(test)] mod tests` block:

```rust
#[test]
fn ac5_2_format_paragraphs_as_txt_matches_design_example() {
    use crate::overlay::transcript_log::Paragraph;
    use chrono::{Local, TimeZone};
    let ts1 = Local.timestamp_opt(1_700_000_000, 0).unwrap();
    let ts2 = Local.timestamp_opt(1_700_000_010, 0).unwrap();
    let paragraphs = vec![
        Paragraph { timestamp: ts1, text: "Hello everyone, welcome to the call. Let me share my screen.".to_string() },
        Paragraph { timestamp: ts2, text: "So as you can see here, this is the dashboard.".to_string() },
    ];
    let out = format_paragraphs_as_txt(&paragraphs);
    let expected = format!(
        "[{}] Hello everyone, welcome to the call. Let me share my screen.\n[{}] So as you can see here, this is the dashboard.\n",
        ts1.format("%H:%M:%S"), ts2.format("%H:%M:%S")
    );
    assert_eq!(out, expected);
}
```

**(b) Replace the stub Save handler.** In `build_transcript_window`, find the `save_button.connect_clicked(...)` block (added in Phase 3 as a stub printing to stderr). Replace it with:

```rust
{
    let log = Rc::clone(&transcript_log);
    let engine = engine_name.clone();
    let start = session_start;
    let parent_window = window.clone();
    let default_name = format!(
        "subtidal-transcript-{}.txt",
        session_start.format("%Y-%m-%d-%H%M%S")
    );

    save_button.connect_clicked(move |_btn| {
        let log = Rc::clone(&log);
        let engine = engine.clone();
        let parent_window = parent_window.clone();
        let default_name = default_name.clone();

        glib::MainContext::default().spawn_local(async move {
            // Build the file dialog with title, default filename, and a .txt filter.
            let txt_filter = gtk4::FileFilter::new();
            txt_filter.set_name(Some("Plain text"));
            txt_filter.add_pattern("*.txt");
            txt_filter.add_mime_type("text/plain");
            let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
            filters.append(&txt_filter);

            let dialog = gtk4::FileDialog::builder()
                .title("Save Transcript")
                .initial_name(&default_name)
                .modal(true)
                .filters(&filters)
                .build();

            let chosen_file = match dialog.save_future(Some(&parent_window)).await {
                Ok(f) => f,
                Err(e) => {
                    // User cancelled or backend error — both reported as glib::Error.
                    // Cancel is the common case; print to stderr at debug level only.
                    eprintln!("transcript: save dialog dismissed: {e}");
                    return;
                }
            };

            let txt_path = match chosen_file.path() {
                Some(p) => p,
                None => {
                    show_alert(&parent_window, "Save failed",
                        "Selected location has no local filesystem path (e.g., a remote-only URI).");
                    return;
                }
            };
            let json_path = derive_json_sibling(&txt_path);

            // Build the .txt and .json bodies on the main thread (cheap; serde_json
            // on a few thousand fragments is sub-millisecond).
            let log_borrow = log.borrow();
            let txt_body = format_paragraphs_as_txt(&log_borrow.paragraphs());
            let json_value = log_borrow.to_json(&engine, start);
            // pretty-printed with 2-space indent per design plan
            let json_body = serde_json::to_string_pretty(&json_value)
                .unwrap_or_else(|e| format!("{{ \"serialization_error\": \"{e}\" }}"));
            drop(log_borrow);

            let txt_result = std::fs::write(&txt_path, txt_body);
            let json_result = std::fs::write(&json_path, json_body);

            match (txt_result, json_result) {
                (Ok(()), Ok(())) => {
                    // Success: no dialog, just stderr breadcrumb.
                    eprintln!(
                        "transcript: saved {} and {}",
                        txt_path.display(), json_path.display()
                    );
                }
                (Err(e_txt), Ok(())) => {
                    show_alert(&parent_window, "Partial save",
                        &format!(
                            "JSON written successfully to:\n{}\n\nbut writing the TXT failed:\n{}\n\nReason: {}",
                            json_path.display(), txt_path.display(), e_txt
                        ));
                }
                (Ok(()), Err(e_json)) => {
                    show_alert(&parent_window, "Partial save",
                        &format!(
                            "TXT written successfully to:\n{}\n\nbut writing the JSON sibling failed:\n{}\n\nReason: {}",
                            txt_path.display(), json_path.display(), e_json
                        ));
                }
                (Err(e_txt), Err(e_json)) => {
                    show_alert(&parent_window, "Save failed",
                        &format!(
                            "Neither file could be written.\n\nTXT path: {}\nReason: {}\n\nJSON path: {}\nReason: {}",
                            txt_path.display(), e_txt, json_path.display(), e_json
                        ));
                }
            }
        });
    });
}
```

**(c) Add the `show_alert` helper** near the bottom of the module:

```rust
fn show_alert(parent: &ApplicationWindow, title: &str, body: &str) {
    let alert = gtk4::AlertDialog::builder()
        .modal(true)
        .message(title)
        .detail(body)
        .build();
    alert.show(Some(parent));
}
```

`AlertDialog::show` is fire-and-forget (no future); the user dismisses the dialog manually. We do not need the chosen-button index, so we don't use `choose_future`.

**Important note on `.json` sibling overwrite (AC5.6):** `gtk::FileDialog::save_future` shows the OS native overwrite confirmation only for the chosen file (`.txt`). The sibling `.json` is silently overwritten by `std::fs::write`. Add a brief code comment near the `let json_path = derive_json_sibling(&txt_path);` line:

```rust
// Note: the .json sibling is silently overwritten if it exists; only the
// user-chosen .txt path goes through the OS overwrite confirmation.
```

**Comment on `serde_json::to_string_pretty` failure mode:** This call is technically infallible for a `Value` constructed from owned data, but we still handle the `Result` defensively to avoid `.unwrap()` panicking the GTK main thread in the (impossible-but-tracked) case of a custom serializer trait failure.

**Compiler error to watch for:** The `gtk4::gio::ListStore::new::<T>()` API in 0.10 takes a generic `T: IsA<Object>`. If the turbofish syntax causes a "cannot infer type" error, the safer form is `ListStore::with_type(gtk4::FileFilter::static_type())`. Try the turbofish first; fall back to `with_type` if the compiler complains.

**Testing:**

Beyond the unit tests added in Task 1 and the new `ac5_2_format_paragraphs_as_txt_matches_design_example` test, this task has no automated tests — the FileDialog and AlertDialog paths are manual.

**Verification:**

Run: `cargo build`
Expected: Compiles cleanly with no warnings.

Run: `cargo test --lib`
Expected: All tests pass (including the new `ac5_*` tests).

**Manual smoke test (requires display + writable filesystem):**

1. `cargo run --release`
2. Switch to Transcript mode via tray. Speak a few sentences (or wait while a video plays).
3. Click "Save…" in the transcript window header bar.
4. Verify the default filename matches `subtidal-transcript-YYYY-MM-DD-HHMMSS.txt` with the session-start timestamp.
5. Choose `/tmp/test-transcript.txt`. Click Save.
6. In another terminal:
   ```bash
   cat /tmp/test-transcript.txt
   cat /tmp/test-transcript.json
   ```
   Verify `.txt` has `[HH:MM:SS] <paragraph>` lines; `.json` has `{ "session_start": ..., "engine": "nemotron", "fragments": [...] }`.
7. Click Save again, choose the same path → OS overwrite prompt appears for `.txt`; choose Replace; both files are silently overwritten.
8. Click Save, choose a path under a non-writable directory (e.g., `/proc/test.txt`) → AlertDialog appears reporting the failure, not a crash.

**Commit:**

```bash
git add src/overlay/transcript_window.rs
git commit -m "feat(transcript): wire Save button to FileDialog + dual-write .txt/.json"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 5 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib` passes including all new `ac5_*` tests.
- Manual smoke test passes all eight steps above.

## What Phase 5 Deliberately Does NOT Do

- Does not autosave or implement a retry mechanism (per design "Additional Considerations").
- Does not show a confirmation dialog on success — only on failure.
- Does not allow the user to choose `.json` as the chosen file (the FileDialog filter is `*.txt` only, and the `.json` sibling is implicit). If they manually rename the chosen path's extension to `.json` in the file dialog, we still treat it as the `.txt` path and produce a sibling that overwrites the original.
- Does not implement the clear-on-disable behavior — Phase 6.
