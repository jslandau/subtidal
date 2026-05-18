# Phase 4: Two-Window Orchestration Implementation Plan

**Goal:** In `src/overlay/mod.rs`, build BOTH the layer-shell overlay window AND the new transcript window at startup, then route captions and `OverlayCommand`s based on the current `OverlayMode`. After Phase 4 the user can switch between Docked/Floating/Transcript via the tray and see captions appear in the appropriate surface; the `TranscriptLog` accumulates every fragment from session start so a mid-session switch to Transcript reveals the full history.

**Architecture:** Both windows are constructed once in the `connect_activate` closure and kept alive for the process lifetime. `Rc<RefCell<TranscriptLog>>` and `Rc<Cell<OverlayMode>>` are added to the closure's captured state. The caption consumer future unconditionally pushes to `transcript_log` and calls `append_fragment_to_view`; it conditionally updates `CaptionBuffer` and the overlay label only when the active mode is Docked or Floating. The command consumer future routes `SetMode` to a new helper that toggles `set_visible` on both windows; `SetVisible` routes to whichever window is currently active; `SetLocked` and `UpdateAppearance` no-op when the mode is Transcript (since they only mean anything for the layer-shell overlay).

**Tech Stack:** No new external dependencies. Uses the `transcript_log` and `transcript_window` modules from Phases 1 and 3 plus the new `OverlayCommand::SetCaptionsEnabled` from Phase 2.

**Scope:** Phase 4 of 6.

**Codebase verified:** 2026-05-11.
- `src/main.rs:290` — `let engine_choice = Arc::new(ArcSwap::from_pointee(cfg.engine.clone()));` — `Engine` lives in `config.rs:10` and currently has only `Nemotron`. We compute the engine display name inside the activation closure by reading `cfg.engine` (no need to thread `engine_choice` through `run_gtk_app`).
- `src/main.rs:403` — `overlay::run_gtk_app(cfg, caption_rx, cmd_rx, Arc::clone(&captions_enabled));` — current four-arg signature. **Phase 4 keeps this signature identical**; both `engine_name` (derived from `cfg.engine`) and `session_start` (`Local::now()`) are computed inside `connect_activate` so `main.rs` does not need to change.
- `src/overlay/mod.rs:99-119` — caption consumer future structure verified (caption_rx loop, enabled gate, buf.borrow_mut().push, label.set_text).
- `src/overlay/mod.rs:140-156` — command consumer future structure verified.
- `src/overlay/mod.rs:88-92` — CaptionBuffer Rc<RefCell<>> wiring confirmed.
- `src/overlay/mod.rs:155-158` — Phase 2 added an initial-visibility guard for Transcript mode; Phase 4 expands it to include showing the transcript window.
- `gtk4::glib` re-export usage confirmed at `src/overlay/mod.rs:18`.

---

## Acceptance Criteria Coverage

This phase implements:

### transcript-window-mode.AC4: Two-window orchestration (Phase 4 "Done when")
- **transcript-window-mode.AC4.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC4.2 No regressions:** Existing overlay tests still pass.
- **transcript-window-mode.AC4.3 Captions visible in Docked mode end-to-end:** Manual smoke test — launch app in Docked mode, speak, see overlay update.
- **transcript-window-mode.AC4.4 Mid-session switch reveals history:** Manual smoke test — switch to Transcript via tray, see same content in transcript window (history preserved).
- **transcript-window-mode.AC4.5 Switch back to overlay resumes:** Manual smoke test — switch back to Docked or Floating, overlay resumes.
- **transcript-window-mode.AC4.6 Transcript window retains history across mode switches:** Manual smoke test — multiple round-trips do not lose transcript content.

The design plan classifies Phase 4 as primarily integration; per its "Done when" the verification is `cargo build` + manual end-to-end. We add one automated test for the unit-testable invariant: `TranscriptLog` accumulation during simulated mode switches.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Lift `transcript_log` and `current_mode` into the activation closure; build the transcript window alongside the overlay

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC4.1, transcript-window-mode.AC4.2.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:68-92` (activation closure setup)
- Modify: `/home/jslandau/git/live_text/src/overlay/transcript_log.rs:1` (remove the `#![allow(dead_code)]` added in Phase 1)
- Modify: `/home/jslandau/git/live_text/src/overlay/transcript_window.rs:1` (remove the `#![allow(dead_code)]` added in Phase 3)

**Implementation:**

Inside `app.connect_activate(move |app| { ... })`, after the existing `caption_buffer` construction (lines 88-92) and BEFORE the `caption_rx`/`cmd_rx` extraction at line 96, insert:

```rust
// Transcript log: accumulates every recognized fragment from session start.
// Wrapped in Rc<RefCell<>> so the caption consumer (mutating push) and the
// command consumer (clear-on-disable in Phase 6) can share it on the GTK
// main thread without locking.
let transcript_log = Rc::new(RefCell::new(
    crate::overlay::transcript_log::TranscriptLog::new(std::time::Duration::from_millis(1500))
));

// Engine display name and session start: computed once at activation, both
// passed into the transcript window for use by the Save dialog (Phase 5).
//
// The explicit match (instead of `Display`/`Debug`) is intentional — when a
// future engine variant is added to `crate::config::Engine`, the Rust
// compiler will fail this match exhaustively, forcing the developer to
// pick a stable display string for the new engine. Using `format!("{:?}", cfg.engine)`
// would silently produce e.g. "Whisper" without thinking about JSON-stability.
let engine_name = match cfg.engine {
    crate::config::Engine::Nemotron => "nemotron".to_string(),
};
let session_start = chrono::Local::now();

// Current overlay mode tracker — read by the caption and command consumers
// to route updates to the correct surface. Wrapped in Cell<OverlayMode>
// because OverlayMode is Copy-able? No — Cell requires Copy. Use Rc<RefCell<>>.
// Actually OverlayMode derives Clone but not Copy; use Cell only if we make
// it Copy. Simpler: use Rc<RefCell<OverlayMode>> and borrow it for read.
let current_mode: Rc<RefCell<OverlayMode>> = Rc::new(RefCell::new(cfg.overlay_mode.clone()));

// Construct the transcript window (always built, initially hidden by the
// builder). Phase 5 wires the Save button.
let transcript_state = crate::overlay::transcript_window::build_transcript_window(
    app,
    Rc::clone(&transcript_log),
    engine_name.clone(),
    session_start,
);
```

**Note on `OverlayMode` and `Cell`:** `OverlayMode` derives `Clone, PartialEq, Default, Serialize, Deserialize` but NOT `Copy` (an enum carrying no data could be `Copy`, but the project chose not to derive it). Two options:

1. Use `Rc<RefCell<OverlayMode>>` (chosen above) — slightly more verbose at call sites but no derive change.
2. Add `Copy` to the derive list at `src/config.rs:28` and use `Rc<Cell<OverlayMode>>`.

We choose option 1 to keep the Phase 4 patch surgical to `mod.rs`.

**Remove the dead-code suppression** added in Phases 1 and 3:
- Delete `#![allow(dead_code)]` from line 1 of `src/overlay/transcript_log.rs`.
- Delete `#![allow(dead_code)]` from line 1 of `src/overlay/transcript_window.rs`.

After Task 1, both modules are referenced from `mod.rs` so `cargo build` no longer warns.

**Initial visibility (replaces the Phase 2 stub guard at lines 155-158):**

The Phase 2 patch added:
```rust
if cfg.overlay_mode == OverlayMode::Transcript {
    window.set_visible(false);
} else {
    window.present();
}
```

Replace this entire `if/else` with the proper Phase 4 routing:
```rust
match cfg.overlay_mode {
    OverlayMode::Docked | OverlayMode::Floating => {
        transcript_state.window.set_visible(false);
        window.present();
    }
    OverlayMode::Transcript => {
        window.set_visible(false);
        transcript_state.window.present();
    }
}
```

**Testing:**

No new tests in Task 1. Build and full test suite verification at end of Task 3.

**Commit:** Combined at end of Phase 4.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update the caption consumer future to always push to `transcript_log` and `transcript_state`, and conditionally update the overlay

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC4.3, transcript-window-mode.AC4.4, transcript-window-mode.AC4.6.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:99-119` (caption consumer future block)

**Implementation:**

Replace the existing caption-consumer block (lines 99-119) with a version that captures the new state and routes per-mode. The current block:

```rust
{
    let buf = Rc::clone(&caption_buffer);
    let label = label.clone();
    let window = window.clone();
    let enabled = Arc::clone(&captions_enabled_clone);
    let dragging = Rc::clone(&is_dragging);
    glib::MainContext::default().spawn_local(async move {
        while let Ok(text) = caption_rx.recv().await {
            if !enabled.load(Ordering::Relaxed) {
                continue;
            }
            buf.borrow_mut().push(text);
            if !dragging.get() {
                label.set_text(&buf.borrow().display_text());
                window.set_visible(true);
            }
        }
    });
}
```

becomes:

```rust
{
    let buf = Rc::clone(&caption_buffer);
    let label = label.clone();
    let window = window.clone();
    let enabled = Arc::clone(&captions_enabled_clone);
    let dragging = Rc::clone(&is_dragging);
    let log = Rc::clone(&transcript_log);
    let mode = Rc::clone(&current_mode);
    let tstate = transcript_state.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok(text) = caption_rx.recv().await {
            if !enabled.load(Ordering::Relaxed) {
                continue;
            }

            // Always: append to the durable transcript log AND to the transcript
            // window's TextBuffer (safe even while the window is hidden — GTK
            // queues layout updates and they materialize on .present()).
            let kind = log.borrow_mut().push(text.clone());
            let fragment = log
                .borrow()
                .fragments()
                .last()
                .cloned()
                .expect("just pushed a fragment");
            crate::overlay::transcript_window::append_fragment_to_view(&tstate, &fragment, kind);

            // Overlay surfaces (caption_buffer + label) only update when the
            // overlay is the active mode.
            let m = mode.borrow().clone();
            if matches!(m, OverlayMode::Docked | OverlayMode::Floating) {
                buf.borrow_mut().push(text);
                if !dragging.get() {
                    label.set_text(&buf.borrow().display_text());
                    window.set_visible(true);
                }
            }
        }
    });
}
```

**Note on `text.clone()`:** the `text: String` arrives once from the channel; we need it for both the transcript log push and (conditionally) the caption buffer push. The clone is unavoidable without restructuring the channel to a `Rc<String>` — not worth the change.

**Note on the `expect("just pushed a fragment")`:** `TranscriptLog::push` always appends, so `fragments().last()` is `Some` immediately afterward. The `expect` documents this invariant; if it ever fires, the bug is in `transcript_log.rs`, not here.

**Note on the expire timer at lines 121-136:** Leave unchanged. The expire timer only mutates `caption_buffer` (the overlay's line-fill state). Transcript view does not expire — it's an append-only history. No edit needed.

**Testing:**

Add to `src/overlay/transcript_log.rs`'s `#[cfg(test)] mod tests` block:

```rust
/// transcript-window-mode.AC4.6: TranscriptLog accumulates across simulated
/// mode-switch boundaries (purely a data-level test — the GTK side is exercised
/// manually).
#[test]
fn ac4_6_transcript_log_accumulates_across_simulated_mode_switches() {
    let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
    let t0 = ts(1_700_000_000, 0);
    log.push_at("hello".to_string(), t0);
    log.push_at(" world".to_string(), t0 + chrono::Duration::milliseconds(200));
    // simulate user toggling mode (no effect on the log itself)
    log.push_at("again".to_string(), t0 + chrono::Duration::milliseconds(2000));
    assert_eq!(log.fragments().len(), 3);
    assert_eq!(log.paragraphs().len(), 2,
        "second paragraph starts after >1.5s gap");
}
```

**Commit:** Combined at end of Phase 4.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update command dispatch to route per-mode; update `handle_overlay_command` signature for `current_mode` + `transcript_state` access

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC4.1, transcript-window-mode.AC4.2, transcript-window-mode.AC4.3, transcript-window-mode.AC4.4, transcript-window-mode.AC4.5.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:138-156` (command consumer future captures)
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:164-239` (`handle_overlay_command` signature + dispatch arms)

**Implementation:**

**(a) Update the command consumer future (lines 138-156)** to capture `current_mode` and `transcript_state`, and pass them to `handle_overlay_command`:

```rust
{
    let window = window.clone();
    let config = Arc::clone(&config_clone);
    let dragging = Rc::clone(&is_dragging);
    let buf = Rc::clone(&caption_buffer);
    let captions_enabled = Arc::clone(&captions_enabled_clone);
    let mode = Rc::clone(&current_mode);
    let tstate = transcript_state.clone();
    let log = Rc::clone(&transcript_log);
    glib::MainContext::default().spawn_local(async move {
        while let Ok(cmd) = cmd_rx.recv().await {
            let bypass_drag = matches!(
                cmd,
                OverlayCommand::Quit
                    | OverlayCommand::SetVisible(_)
                    | OverlayCommand::SetCaptionsEnabled(_)
                    | OverlayCommand::SetMode(_)
            );
            if bypass_drag || !dragging.get() {
                handle_overlay_command(
                    &window, cmd, &config, &dragging, &buf,
                    &captions_enabled, &mode, &tstate, &log,
                );
            }
        }
    });
}
```

`SetMode` was added to the `bypass_drag` set in Phase 2 already (see Phase 2 Task 2). Verify it's still there in this rewrite — switching modes during a drag is allowed; the user's intent to change mode trumps the drag-suppression heuristic.

**(b) Update `handle_overlay_command` signature** (currently extended in Phase 2 to take `captions_enabled`):

```rust
fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
    caption_buffer: &Rc<RefCell<CaptionBuffer>>,
    captions_enabled: &CaptionsEnabled,
    current_mode: &Rc<RefCell<OverlayMode>>,
    transcript_state: &crate::overlay::transcript_window::TranscriptWindowState,
    transcript_log: &Rc<RefCell<crate::overlay::transcript_log::TranscriptLog>>,
) {
    // ... arms below
}
```

(`transcript_log` is captured here in preparation for Phase 6's clear-on-disable handler. Phase 4 itself doesn't dereference it; the unused-variable warning is suppressed by the upcoming Phase 6 use. To keep Phase 4 strictly clean, prefix with `_transcript_log:` until Phase 6 — or accept a benign unused warning. **Choose `_transcript_log:` parameter name now and rename in Phase 6.**)

Final Phase 4 signature with the `_` prefix:
```rust
fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
    caption_buffer: &Rc<RefCell<CaptionBuffer>>,
    captions_enabled: &CaptionsEnabled,
    current_mode: &Rc<RefCell<OverlayMode>>,
    transcript_state: &crate::overlay::transcript_window::TranscriptWindowState,
    _transcript_log: &Rc<RefCell<crate::overlay::transcript_log::TranscriptLog>>,
) {
```

**(c) Update each match arm.** The Phase 2 stubs and the existing arms must be replaced with mode-aware behavior.

```rust
match cmd {
    OverlayCommand::SetVisible(v) => {
        // Route to whichever window is currently active.
        let m = current_mode.borrow().clone();
        match m {
            OverlayMode::Docked | OverlayMode::Floating => window.set_visible(v),
            OverlayMode::Transcript => transcript_state.window.set_visible(v),
        }
    }
    OverlayCommand::SetMode(mode) => {
        // Persist the new mode locally and in config.
        //
        // NOTE on dual-write (pre-existing pattern; do NOT "fix"): the tray's
        // RadioGroup `select` closure ALREADY writes `cfg.overlay_mode` to disk
        // via `cfg.save()` (see `src/tray/mod.rs:464-467`). This handler updates
        // the in-memory `Arc<Mutex<Config>>` shared with the GTK side. The two
        // stores eventually reconverge via the notify-debouncer-mini hot-reload
        // watcher at `src/config.rs:312-365`. This dual-write is intentional
        // and predates the transcript work; preserve it.
        *current_mode.borrow_mut() = mode.clone();
        let mut cfg = config.lock().unwrap();
        cfg.overlay_mode = mode.clone();
        match mode {
            OverlayMode::Docked => {
                transcript_state.window.set_visible(false);
                for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                    window.set_anchor(edge, false);
                }
                configure_docked(window, &cfg.screen_edge, &cfg.dock_position);
                input_region::set_empty_input_region(window);
                window.set_visible(true);
            }
            OverlayMode::Floating => {
                transcript_state.window.set_visible(false);
                for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                    window.set_anchor(edge, false);
                }
                window.set_anchor(Edge::Top, true);
                window.set_anchor(Edge::Left, true);
                window.set_margin(Edge::Left, cfg.position.x);
                window.set_margin(Edge::Top, cfg.position.y);
                window.set_keyboard_mode(if cfg.locked {
                    KeyboardMode::None
                } else {
                    KeyboardMode::OnDemand
                });
                if cfg.locked {
                    input_region::set_empty_input_region(window);
                } else {
                    input_region::clear_input_region(window);
                    add_drag_handler(window, is_dragging);
                }
                window.set_visible(true);
            }
            OverlayMode::Transcript => {
                window.set_visible(false);
                // present() raises the window above other toplevel windows
                // and is a no-op if it's already presented and visible.
                transcript_state.window.present();
            }
        }
    }
    OverlayCommand::SetLocked(locked) => {
        // No-op when in Transcript mode: lock controls only affect the
        // floating layer-shell overlay.
        if matches!(*current_mode.borrow(), OverlayMode::Transcript) {
            return;
        }
        if locked {
            input_region::set_empty_input_region(window);
            window.set_keyboard_mode(KeyboardMode::None);
        } else {
            input_region::clear_input_region(window);
            window.set_keyboard_mode(KeyboardMode::OnDemand);
            add_drag_handler(window, is_dragging);
        }
    }
    OverlayCommand::UpdateAppearance(appearance) => {
        // No-op when in Transcript mode: appearance config applies to the
        // overlay only (per design "explicitly out of scope").
        if matches!(*current_mode.borrow(), OverlayMode::Transcript) {
            return;
        }
        apply_appearance(&appearance);
        let label = find_caption_label(window);
        let max_chars = estimate_max_chars(
            appearance.width, appearance.font_size,
            appearance.effective_char_width_fraction(),
        );
        label.set_max_width_chars(max_chars);
        label.set_lines(appearance.max_lines as i32);
        window.set_width_request(appearance.width);
        let mut buf = caption_buffer.borrow_mut();
        buf.update_config(
            appearance.max_lines as usize, max_chars as usize,
            appearance.effective_expire_secs(),
        );
    }
    OverlayCommand::SetCaption(text) => {
        // SetCaption is a legacy command path (currently `#[allow(dead_code)]`)
        // for direct overlay-label updates. It is NOT part of transcript
        // routing — the transcript window is updated only by the caption
        // consumer future via `append_fragment_to_view`. Leave SetCaption's
        // behavior unchanged.
        let label = find_caption_label(window);
        label.set_text(&text);
    }
    OverlayCommand::SetCaptionsEnabled(enabled) => {
        // Phase 4: still the AtomicBool-only stub. Phase 6 expands this with
        // the clear-on-disable side effects across all four caption surfaces.
        captions_enabled.store(enabled, Ordering::Relaxed);
    }
    OverlayCommand::Quit => {
        if let Some(app) = window.application() {
            app.quit();
        }
    }
}
```

**Testing:**

The integration behavior is verified by manual smoke testing per the design plan. The data-level invariant (transcript log accumulates) was added in Task 2.

**Verification (full Phase 4):**

Run: `cargo build`
Expected: Compiles cleanly with no warnings.

Run: `cargo test --lib`
Expected: All Phase 1, 2, and the new `ac4_*` test in `transcript_log.rs` pass.

**Manual smoke test:**

```bash
cargo run --release
```
1. Default mode is Docked. Speak — caption appears at the bottom edge as before.
2. Open tray → Overlay → click Transcript. Layer-shell overlay disappears; transcript window appears with the same captions visible as paragraphs with `[HH:MM:SS]` prefixes.
3. Speak more — new fragments append in real time with autoscroll.
4. Scroll up in the transcript window. Speak more — autoscroll pauses (your viewport stays put).
5. Scroll back to the bottom. Speak — autoscroll resumes.
6. Switch back to Docked via tray. Layer-shell overlay reappears with the most recent captions in line-fill mode.
7. Switch to Transcript again. Full session history is still present — no truncation, no duplication.
8. Switch to Floating. Window appears at saved position; transcript window hidden.
9. **Hot-reload smoke (I2):** With the app running in Docked mode, edit `~/.config/subtidal/config.toml` in another editor and change `overlay_mode = "docked"` to `overlay_mode = "transcript"`. Save. Within the notify-debouncer window (~250 ms by default), verify the layer-shell overlay disappears and the transcript window appears with the same content. Edit back to `"docked"`; verify the round-trip works.

**Commit:**

```bash
git add src/overlay/mod.rs src/overlay/transcript_log.rs src/overlay/transcript_window.rs
git commit -m "feat(transcript): orchestrate two-window mode with transcript routing

- Build transcript window alongside overlay in connect_activate
- Caption consumer always pushes to TranscriptLog + transcript view; overlay updated only in Docked/Floating
- Command dispatch routes SetVisible to the active window; SetMode toggles visibility; SetLocked/UpdateAppearance no-op in Transcript mode
- Remove dead_code suppressions from transcript_log.rs and transcript_window.rs"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 4 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib` passes all tests including `ac4_6_transcript_log_accumulates_across_simulated_mode_switches`.
- Manual smoke test passes all eight steps above.

## What Phase 4 Deliberately Does NOT Do

- Does not wire the Save button to FileDialog — Phase 5.
- Does not implement the clear-on-disable side effects of `SetCaptionsEnabled` — Phase 6.
- Does not change `run_gtk_app`'s public signature in `src/overlay/mod.rs:49-54` — `engine_name` and `session_start` are computed inside the activation closure.
- Does not modify `src/main.rs` — all changes are localized to `src/overlay/`.
