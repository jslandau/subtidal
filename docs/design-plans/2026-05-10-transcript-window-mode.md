# Transcript Window Mode Design

## Summary

This design adds a `Transcript` mode to Subtidal's existing pair of overlay modes (Docked and Floating). Rather than a heads-up display overlay pinned to the screen edge, Transcript mode presents a standard desktop window with a scrollable, selectable text view showing every utterance captured since the application started, grouped into timestamped paragraphs by silence gaps. The same Nemotron speech recognition pipeline and PipeWire audio capture that power the overlay continue running unchanged — the transcript window is purely an alternative rendering surface, not a second inference path.

The implementation is organized around a clean split between a GTK-free data module (`transcript_log.rs`, fully unit-tested) and a GTK-coupled view module (`transcript_window.rs`), following the same pattern already used for the caption buffer. Both the overlay and the transcript window are constructed at startup and kept alive; switching modes is a visibility toggle rather than a create/destroy cycle. Captions are always appended to the `TranscriptLog` regardless of which mode is active, so switching to Transcript mid-session reveals the full history. A "Save" button writes both a human-readable `.txt` (paragraph-coalesced) and a machine-readable `.json` (per-fragment with ISO-8601 timestamps) from a single file chooser interaction.

## Definition of Done

**Primary deliverable:** A third overlay mode — `Transcript` — selectable from the tray radio alongside Docked/Floating. When active, the layer-shell overlay is hidden and a standard scrollable GTK window is shown in its place.

**The transcript window must:**
- Display all utterances from session start as timestamped paragraphs (fragments coalesced by ~1.5s silence gap)
- Auto-scroll to bottom unless the user has scrolled up (chat-app pattern)
- Support text selection and copy via standard GTK keybindings
- Use the system GTK theme (no custom CSS)
- Provide a 'Save Transcript' button that opens a file chooser and writes both `<name>.txt` (paragraphs) and `<name>.json` (per-fragment array)
- Start empty on every launch (no session reload)

**Shared with overlay:** Same Nemotron STT pipeline and audio capture — no duplicate inference. Mutually exclusive with overlay (one window at a time).

**Explicitly out of scope:** Speaker diarization, session reload across runs, multi-language UI, applying overlay AppearanceConfig to the window.

## Glossary

- **Nemotron / RNNT**: A 600 M-parameter Recurrent Neural Network Transducer speech-to-text model. Nemotron is the specific model used by Subtidal, run via ONNX Runtime (ORT). RNNT models emit partial results as audio streams in, producing "fragments" rather than one final transcript per utterance.
- **layer-shell**: A Wayland protocol extension (`wlr-layer-shell`) that lets applications pin windows to screen edges or the background layer, bypassing normal window management. The Docked and Floating overlay modes use this; the new Transcript window is a regular toplevel and does not.
- **`GtkApplicationWindow` / `GtkScrolledWindow` / `GtkTextView`**: GTK4 widget types. `GtkApplicationWindow` is a top-level window managed by a `gtk::Application`. `GtkScrolledWindow` adds scroll bars to an inner widget. `GtkTextView` is a multi-line text display widget with built-in selection, copy, and keyboard navigation — used here for the transcript body.
- **`GtkHeaderBar`**: A GTK4 widget that renders a title bar with integrated action buttons; used here to host the "Save…" button.
- **`GtkTextBuffer` / `GtkTextTag`**: The data model backing a `GtkTextView`. `GtkTextTag` lets regions of text carry formatting attributes (e.g., the dimmed timestamp color applied via `dim_label_color`).
- **`async_channel`**: A Rust async-compatible multi-producer, single-consumer channel (from the `async-channel` crate) used to ferry caption strings and `OverlayCommand` messages from background threads to the GTK main thread.
- **`ArcSwap`**: A lock-free `Arc` container from the `arc-swap` crate that allows one thread to atomically swap a shared value while other threads read it without blocking. Used here for engine-choice switching between the tray and the STT pipeline thread.
- **`Rc<RefCell<T>>`**: Rust's single-threaded interior mutability pattern. Since GTK4 closures all run on the same main thread, shared mutable state (like `TranscriptLog`) can be wrapped in `Rc<RefCell<T>>` rather than `Arc<Mutex<T>>`.
- **`OverlayCommand`**: An enum of messages sent from the tray and other non-GTK contexts to the GTK main thread's command consumer future, dispatching actions like mode changes, visibility toggles, and the new captions-enable/disable signal.
- **`OverlayMode`**: The enum (`Docked`, `Floating`, `Transcript`) stored in config and tracked at runtime; determines which window is visible and how caption routing behaves.
- **`RadioGroup` / `RadioItem`**: The ksni tray menu concept for mutually exclusive menu entries. The tray currently has two radio items for Docked/Floating; this design adds a third for Transcript.
- **ksni**: A Rust crate that implements the D-Bus `StatusNotifierItem` specification, providing a system tray icon with a context menu. It runs on a tokio thread separate from GTK.
- **`glib::MainContext::spawn_local`**: The GTK/GLib mechanism for scheduling async futures on the GTK main thread's event loop, used to run caption and command consumer futures without spawning separate threads.
- **`notify-debouncer-mini`**: A Rust crate wrapping the `notify` filesystem-watcher crate with coalescing debounce logic, used for config hot-reload.
- **`chrono`**: A Rust date/time library. Added in this design to attach ISO-8601 timestamps to transcript fragments and format default save filenames.
- **`serde_json`**: Rust's standard JSON serialization library, used to serialize `TranscriptLog` to the `.json` save format.
- **`AppendKind`**: A two-variant enum returned by `TranscriptLog::push` indicating whether a new caption fragment begins a fresh paragraph (silence gap exceeded ~1.5 s) or continues the current one; drives how `append_fragment_to_view` inserts text into the `GtkTextView`.
- **FCIS (Functional Core / Imperative Shell)**: An architecture pattern that separates pure business logic (no side effects, easily testable) from code that touches I/O or UI. Expressed here as the `transcript_log.rs` (pure, GTK-free) vs. `transcript_window.rs` (GTK-coupled) split.
- **vadjustment**: The `gtk::Adjustment` object tracking the vertical scroll position of a `GtkScrolledWindow`. The autoscroll logic samples this value before each append to decide whether the user is "near the bottom."
- **`idle_add_local_once`**: A GLib call that defers a closure to run on the next main-loop idle pass, used here so autoscrolling happens after GTK has laid out the newly appended text.
- **`push_at` (test seam)**: An alternate `TranscriptLog` entry point that accepts an explicit timestamp instead of `Local::now()`, making deterministic unit tests possible without mocking the system clock.

## Architecture

A third `OverlayMode` variant (`Transcript`) is added to the existing two (`Docked`, `Floating`). Both an overlay layer-shell window AND a regular toplevel transcript window are constructed at startup; the active mode controls visibility. Switching modes toggles `set_visible` on both — no destroy/recreate, no layer-shell mutation on a non-layer-shell window.

The single STT pipeline (Nemotron RNNT) and PipeWire audio capture are unchanged. Every received caption is unconditionally appended to a `TranscriptLog` (the durable session record) regardless of which mode is active. The overlay's existing `CaptionBuffer` is updated only when overlay-mode is active; the transcript window's `GtkTextView` is updated on every append (safely, even while hidden) so that mid-session mode switches reveal the full history.

The transcript window is a standard `GtkApplicationWindow` containing a `GtkHeaderBar` (with a "Save…" button) and a `GtkScrolledWindow` wrapping a `GtkTextView`. Selection, copy, and scrollback are GTK4 built-ins. Autoscroll-on-tail samples the vertical adjustment at append time: if the user is "near the bottom" (within 16px), scroll to the new content; otherwise leave the viewport alone.

Save writes both `.txt` (paragraph-coalesced) and `.json` (per-fragment with ISO-8601 timestamps + session metadata) from one `gtk::FileDialog` interaction. Files are sibling artifacts sharing the user-chosen stem.

The captions-enabled toggle (tray) is extended: in addition to mutating the existing `AtomicBool`, the tray emits a new `OverlayCommand::SetCaptionsEnabled(bool)`. On the disable edge, the command handler clears `TranscriptLog`, the `GtkTextView` buffer, the overlay `CaptionBuffer`, and the overlay label. This guarantees a fresh start on every recognizer re-enable.

### Data flow

```
PipeWire RT callback → ring buffer → STT pipeline thread (Nemotron, ArcSwap)
  → caption_tx (async_channel::unbounded::<String>)
    → GTK main thread (caption consumer future):
        ├─ TranscriptLog.push(text)        [always]
        ├─ transcript_window append view   [always; safe while hidden]
        └─ if mode ∈ {Docked, Floating}:
             CaptionBuffer.push(text)
             overlay_label.set_text(...)
```

### Module layout

```
src/overlay/
├── mod.rs                ← edited: two-window orchestration, caption routing,
│                            SetCaptionsEnabled clear-on-disable
├── window.rs             ← unchanged
├── caption_buffer.rs     ← unchanged
├── drag.rs               ← unchanged
├── input_region.rs       ← unchanged
├── transcript_log.rs     ← NEW: pure data (Fragment, TranscriptLog,
│                            paragraph coalescer); GTK-free; unit-tested
└── transcript_window.rs  ← NEW: GTK4 window, TextView, autoscroll, save dialog
```

Edits to existing files:
- `src/config.rs` — add `OverlayMode::Transcript` variant (serde `"transcript"`).
- `src/tray/mod.rs` — third RadioItem; "Lock Overlay Position" enabled only when mode = Floating; toggle now emits `SetCaptionsEnabled`.
- `src/overlay/mod.rs` — adds `OverlayCommand::SetCaptionsEnabled(bool)`; routes commands to the right window based on current mode.
- `Cargo.toml` — adds `chrono = "0.4"` (with `serde` feature for JSON serialization).

### Contracts

`transcript_log.rs` (GTK-free, fully tested):

```rust
pub struct Fragment {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub text: String,
}

pub enum AppendKind {
    NewParagraph,        // gap exceeded → new line in display
    ContinueParagraph,   // append to current paragraph
}

pub struct Paragraph {
    pub timestamp: chrono::DateTime<chrono::Local>,
    pub text: String,
}

pub struct TranscriptLog { /* internal */ }

impl TranscriptLog {
    pub fn new(paragraph_gap: std::time::Duration) -> Self;
    pub fn push(&mut self, text: String) -> AppendKind;
    pub fn push_at(&mut self, text: String, ts: chrono::DateTime<chrono::Local>) -> AppendKind;  // test seam
    pub fn fragments(&self) -> &[Fragment];
    pub fn paragraphs(&self) -> Vec<Paragraph>;
    pub fn to_json(&self, engine_name: &str, session_start: chrono::DateTime<chrono::Local>) -> serde_json::Value;
    pub fn clear(&mut self);
}
```

`overlay/mod.rs` additions:

```rust
pub enum OverlayCommand {
    SetVisible(bool),
    SetMode(OverlayMode),
    SetLocked(bool),
    UpdateAppearance(AppearanceConfig),
    SetCaption(String),
    SetCaptionsEnabled(bool),          // NEW
    Quit,
}
```

`transcript_window.rs` (GTK-coupled):

```rust
pub struct TranscriptWindowState {
    pub window: gtk4::ApplicationWindow,
    pub buffer: gtk4::TextBuffer,
    pub scrolled: gtk4::ScrolledWindow,
}

pub fn build_transcript_window(
    app: &gtk4::Application,
    transcript_log: std::rc::Rc<std::cell::RefCell<TranscriptLog>>,
    engine_name: String,
    session_start: chrono::DateTime<chrono::Local>,
) -> TranscriptWindowState;

pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
);

pub fn clear_view(state: &TranscriptWindowState);
```

### Save format contracts

**`.txt` format** (paragraph view):
```
[HH:MM:SS] Hello everyone, welcome to the call. Let me share my screen.
[HH:MM:SS] So as you can see here, this is the dashboard.
```

**`.json` format** (per-fragment, what `TranscriptLog::to_json` emits):
```json
{
  "session_start": "2026-05-10T14:31:58.123-07:00",
  "engine": "nemotron",
  "fragments": [
    {"timestamp": "2026-05-10T14:32:01.456-07:00", "text": "Hello everyone,"},
    {"timestamp": "2026-05-10T14:32:01.892-07:00", "text": " welcome to the call."}
  ]
}
```

Leading-space whitespace in fragments is preserved verbatim — it carries the RNNT word-boundary signal documented in the engine contract.

## Existing Patterns

The design follows existing patterns identified by codebase investigation:

- **GTK-free pure-data module + GTK-coupled view module split** — mirrors the existing `overlay/caption_buffer.rs` (pure logic, unit-tested) and `overlay/window.rs` (GTK calls) division. The recent `9e62f0d` commit established this as the project's overlay submodule layout.
- **`OverlayCommand` enum + `async_channel::Sender<OverlayCommand>` from tray → GTK main thread** — the new `SetCaptionsEnabled` variant plugs into the same dispatch path used by `SetMode`, `SetLocked`, `UpdateAppearance` (`src/overlay/mod.rs:166-238`).
- **Single-consumer `async_channel::unbounded::<String>` for captions** — unchanged. The new consumer logic is layered inside the existing `caption_rx.recv().await` loop at `src/overlay/mod.rs:107-118`; no fan-out, no broadcast primitive needed.
- **`Rc<RefCell<T>>` for buffer state shared between GTK closures** — `TranscriptLog` follows the `CaptionBuffer` pattern at `src/overlay/mod.rs:88-92`.
- **Tray `RadioGroup` with index → enum mapping** — third `RadioItem` slots in at `src/tray/mod.rs:469-474`; index mapping at `src/tray/mod.rs:460` becomes `0→Docked, 1→Floating, 2→Transcript`.
- **Hot-reload via `notify-debouncer-mini`** — existing handler at `src/config.rs:312-365` already covers `overlay_mode` changes; no edit needed since the new enum variant is transparent to the watcher.
- **Test style** — `#[cfg(test)] mod tests` blocks inside source files, AC-numbered test names (`t1_*`, `t2_*` etc.), `assert_eq!` with descriptive messages, fixed example inputs (no proptest). Matches `src/overlay/caption_buffer.rs:209-559`.

No divergence from existing patterns. The `chrono` dependency is new but minimal (one feature: `serde`).

## Implementation Phases

### Phase 1: Pure-data foundation
**Goal:** `TranscriptLog` and `Fragment` data model with paragraph coalescing, fully tested without GTK.

**Components:**
- `Cargo.toml` — add `chrono = { version = "0.4", features = ["serde"] }`.
- `src/overlay/transcript_log.rs` — `Fragment`, `Paragraph`, `AppendKind`, `TranscriptLog` per contract above. `push_at` test seam.
- `src/overlay/mod.rs` — `mod transcript_log;` declaration only.

**Dependencies:** None (first phase).

**Done when:** `cargo build` succeeds; `cargo test transcript_log` passes. Tests cover: paragraph coalescing on 1.5s gap, whitespace preservation (leading-space continuation), `to_json` shape including session metadata, `paragraphs()` derivation matching `.txt` save output, `clear()` empties fragments.

### Phase 2: Config + command + tray enum extension
**Goal:** Wire `OverlayMode::Transcript` and `OverlayCommand::SetCaptionsEnabled` through config, command enum, and tray menu — without yet creating the transcript window.

**Components:**
- `src/config.rs` — `OverlayMode::Transcript` variant with serde rename; default unchanged. Hot-reload path verified to handle the new variant without code change.
- `src/overlay/mod.rs` — add `OverlayCommand::SetCaptionsEnabled(bool)` variant; placeholder handler that just updates the existing `AtomicBool`.
- `src/tray/mod.rs` — third `RadioItem { label: "Transcript", ... }`; update index mapping at `:460`; gate "Lock Overlay Position" `enabled` field to `matches!(overlay_mode, OverlayMode::Floating)`; toggle captions handler emits `SetCaptionsEnabled` after mutating `AtomicBool`.

**Dependencies:** Phase 1 (so `transcript_log` module exists for downstream phases, though not used here).

**Done when:** `cargo build` succeeds; existing tests still pass; tray tests updated to assert the three-option radio and the new Floating-only enabling of Lock. Manual smoke test: tray menu shows three radio options; selecting Transcript persists to config TOML; toggle-captions still works at the AtomicBool level.

### Phase 3: Transcript GTK window
**Goal:** Build the transcript window in isolation — widget tree, autoscroll, timestamp tag, append function. Wired to a stub log for manual testing.

**Components:**
- `src/overlay/transcript_window.rs` — `TranscriptWindowState`, `build_transcript_window`, `append_fragment_to_view`, `clear_view` per contract. `HeaderBar` + "Save…" `Button` (button click handler is a stub for Phase 5). `ScrolledWindow` + `TextView` (editable=false, cursor_visible=false, wrap_mode=WordChar, 12px margins). `TextTag` for dimmed timestamp prefix using `style_context().lookup_color("dim_label_color")`. Autoscroll: sample `vadjustment` before append, scroll via `idle_add_local_once` after append if was-at-bottom (within 16px).
- `src/overlay/mod.rs` — `mod transcript_window;` declaration.

**Dependencies:** Phase 1 (Fragment/AppendKind types).

**Done when:** `cargo build` succeeds. Manual smoke test (temporary `main.rs` hook or example binary): launch shows a window; pushing synthetic Fragments via a timer appends visible timestamped lines; scrolling up pauses autoscroll; scrolling back to bottom resumes it; Ctrl+A then Ctrl+C copies the full transcript to the clipboard. No automated test for the GTK widget itself.

### Phase 4: Two-window orchestration in `overlay/mod.rs`
**Goal:** Build both windows at startup, route captions and commands based on `current_mode`.

**Components:**
- `src/overlay/mod.rs` — Add `transcript_log: Rc<RefCell<TranscriptLog>>` and `current_mode: Rc<Cell<OverlayMode>>` to the activation closure. Build transcript window alongside overlay. Initial visibility from `cfg.overlay_mode`. Caption consumer future: unconditionally push to `transcript_log` + call `append_fragment_to_view`, conditionally update `CaptionBuffer` + overlay label. Command consumer future: `SetMode(Transcript)` hides overlay + shows transcript window; `SetMode(Docked|Floating)` hides transcript + shows overlay (with existing reconfigure logic); `SetVisible` routes to active window only; `SetLocked` / `UpdateAppearance` no-op when mode = Transcript.
- `src/main.rs` — Pass session-start timestamp + engine-name string into `run_gtk_app` (or compute from `Local::now()` and engine ArcSwap at activation time — design lean is the latter to keep `main.rs` change minimal).

**Dependencies:** Phases 1, 2, 3.

**Done when:** `cargo build` succeeds; existing overlay tests still pass. Manual end-to-end test: launch app in Docked mode, speak, see overlay update; switch to Transcript via tray, see same content in transcript window; switch back, overlay resumes; transcript window retains history across all switches.

### Phase 5: Save dialog + dual-write
**Goal:** Wire the "Save…" button to `gtk::FileDialog` and dual-write `.txt` + `.json`.

**Components:**
- `src/overlay/transcript_window.rs` — Save button click handler: open `gtk::FileDialog` with default filename `subtidal-transcript-<YYYY-MM-DD-HHMMSS>.txt`, filter for `.txt`. On success, derive `.json` sibling path from the chosen stem; write `paragraphs()` to `.txt` and `to_json(...)` to `.json` (pretty-printed, 2-space indent). On either-side write failure, show `gtk::AlertDialog` with the error and path of whichever succeeded.

**Dependencies:** Phase 3 (window exists), Phase 4 (engine name + session_start are passed in).

**Done when:** `cargo build` succeeds. Manual test: in Transcript mode after some speech, click Save, choose a path, verify both `.txt` and `.json` files exist with expected content; verify malformed paths produce an alert dialog rather than a crash.

### Phase 6: Clear-on-disable wiring + end-to-end verification
**Goal:** Implement the recognizer-toggle clearing path and verify the full feature set.

**Components:**
- `src/overlay/mod.rs` — `OverlayCommand::SetCaptionsEnabled(false)` handler: `transcript_log.borrow_mut().clear()`, `clear_view(&transcript_window)`, `caption_buffer.borrow_mut().clear()`, `overlay_label.set_text("")`. Update `captions_enabled.store()` from inside the handler too (so the handler is authoritative). `SetCaptionsEnabled(true)`: only flips the AtomicBool — no clearing needed since the disable already cleared.
- `src/overlay/caption_buffer.rs` — add `pub fn clear(&mut self)` if not already present (verify; minor addition if missing).

**Dependencies:** Phases 1–5.

**Done when:** `cargo build` succeeds. Unit test added for `CaptionBuffer::clear` if newly added. Manual end-to-end: speak in any mode, toggle captions off (tray), confirm both overlay and transcript window go blank; toggle on, speak again, confirm transcript starts fresh (no prior content). Save after a toggle-off cycle produces a transcript with only post-re-enable fragments.

## Additional Considerations

**Error handling:**
- File save failures: surfaced via `gtk::AlertDialog`. No retry, no autosave fallback. Partial success (e.g., `.txt` written but `.json` failed) reports both the failure and the path of the file that succeeded so the user can manually recover.
- Path collisions on the `.json` sibling: `gtk::FileDialog` only prompts for overwrite on the user-chosen `.txt` path. The `.json` sibling is silently overwritten. Documented in code comment near the save handler.
- `gtk::FileDialog` requires GTK 4.10+. The project already pins `gtk4 = "0.10"` which corresponds to GTK 4.10+, so this is satisfied.

**Performance:**
- `GtkTextView` is documented to handle 10k+ lines without degradation. A multi-hour transcription session is well within this bound. No scrollback culling needed.
- `TranscriptLog::push` is O(1) (Vec push + last-fragment timestamp check). `paragraphs()` is O(n) and called only on save, not per append.
- `serde_json::to_string_pretty` on 10k fragments is well under a second on the main thread — no need to spawn a worker for save.

**Mode-switch race:** The caption consumer reads `current_mode` (an `Rc<Cell<OverlayMode>>`) on each caption. Since both the consumer and the command handler run on the GTK main thread via `glib::MainContext::spawn_local`, there's no actual race — they're serialized by the main loop.

**Future extensibility:**
- The per-fragment JSON format is a stable contract; downstream tooling (e.g., a future diarization pass that adds speaker labels) can extend `Fragment` with an optional `speaker` field without breaking existing consumers.
- The `paragraph_gap` is a constant (1.5s) in this design. If user feedback indicates the threshold should be tunable, it can be promoted to `AppearanceConfig` or a new `TranscriptConfig` block in a follow-up.

