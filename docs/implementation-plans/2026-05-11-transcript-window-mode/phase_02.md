# Phase 2: Config + Command + Tray Enum Extension Implementation Plan

**Goal:** Wire `OverlayMode::Transcript` and `OverlayCommand::SetCaptionsEnabled(bool)` through config, the command enum, and the tray menu — without yet creating the transcript window. After Phase 2 the app must build and run with a three-option mode radio in the tray; selecting "Transcript" persists to TOML and emits `SetMode(Transcript)`, but the GTK side simply hides the overlay window for now (visible behavior arrives in Phase 4).

**Architecture:** Four coordinated edits in distinct files.
1. `src/config.rs` — extend `OverlayMode` with a third variant. The `#[serde(rename_all = "snake_case")]` derive at line 29 means the variant `Transcript` automatically serializes as `"transcript"` in TOML — no per-variant serde attribute needed.
2. `src/overlay/window.rs:25-28` — add a `Transcript` arm to the `match cfg.overlay_mode` inside `build_overlay_window` (the existing exhaustive match would otherwise break the build). The Transcript arm calls `configure_docked` to set up sane defaults — the overlay window will still exist but be hidden by Phase 2's startup guard, so the configuration choice is irrelevant at runtime; we use docked defaults to keep the layer-shell surface in a known state in case the user switches back to Docked mid-session.
3. `src/overlay/mod.rs` — extend `OverlayCommand` with `SetCaptionsEnabled(bool)`; add a placeholder dispatch arm in `handle_overlay_command` that just stores into the existing `captions_enabled: AtomicBool` (Phase 6 will replace this stub with the full clear-on-disable logic). Also add a non-mutating `Transcript` arm to the `SetMode` match's inner `match mode` so the build remains exhaustive.
4. `src/tray/mod.rs` — extend the radio group with a third `RadioItem`, update the index→`OverlayMode` mapping, gate "Lock Overlay Position" to Floating-only (currently `!is_docked`, which is wrong once Transcript exists), and emit `SetCaptionsEnabled` from `toggle_captions` after the AtomicBool flip.

**Tech Stack:** Rust 2021, no new crates.

**Scope:** Phase 2 of 6.

**Codebase verified:** 2026-05-11 via codebase-investigator and direct file reads.
- `src/config.rs:27-36` — `OverlayMode` confirmed: two variants (`Docked` default, `Floating`), `#[serde(rename_all = "snake_case")]`. `PartialEq` derived.
- `src/config.rs:312-365` — hot-reload handler is value-comparison-based, transparent to a new variant ✓.
- `src/overlay/mod.rs:24-40` — `OverlayCommand` enum with six variants confirmed.
- `src/overlay/mod.rs:171-238` — `handle_overlay_command` `match cmd` is exhaustive over the existing variants and the inner `match mode` at line 178 is exhaustive over the existing two `OverlayMode` variants.
- `src/overlay/window.rs:25-28` — `build_overlay_window` contains a second exhaustive `match cfg.overlay_mode` that must also be extended.
- `src/overlay/window.rs:48` — `let is_locked = cfg.locked || cfg.overlay_mode == OverlayMode::Docked;` — value comparison, transparent to a new variant ✓.
- `src/overlay/mod.rs:78` — `if cfg.overlay_mode == OverlayMode::Floating && !cfg.locked { add_drag_handler ... }` — value comparison, transparent ✓.
- `src/tray/mod.rs:38-42` — `toggle_captions` mutates the AtomicBool and sends `SetVisible`.
- `src/tray/mod.rs:445` — `let is_docked = tray.overlay_mode == OverlayMode::Docked;`
- `src/tray/mod.rs:457-474` — RadioGroup with `selected: if is_docked { 0 } else { 1 }` and select closure mapping idx 0/1 to `OverlayMode::Docked`/`Floating`.
- `src/tray/mod.rs:510-527` — Lock CheckmarkItem with `enabled: !is_docked`.
- `src/tray/mod.rs:548-606` — existing tests `lock_item_disabled_in_docked_mode` and `lock_item_enabled_in_floating_mode` use `OverlayMode::Docked` and `OverlayMode::Floating` directly; we add a third test for Transcript.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### transcript-window-mode.AC2: Config + command + tray (Phase 2 "Done when")
- **transcript-window-mode.AC2.1 Build success:** `cargo build` succeeds.
- **transcript-window-mode.AC2.2 No regressions:** Existing tests still pass (in particular `tray::tests::lock_item_*` and `caption_buffer::tests::*`).
- **transcript-window-mode.AC2.3 Three-option radio + Lock gating:** Tray tests assert the three-option radio (Docked/Floating/Transcript) and the new Floating-only enabling of "Lock Overlay Position" (disabled in both Docked and Transcript modes; enabled only in Floating).
- **transcript-window-mode.AC2.4 Transcript persists to TOML:** Selecting Transcript persists to `~/.config/subtidal/config.toml` as `overlay_mode = "transcript"`. Verified by a unit test that round-trips a `Config { overlay_mode: Transcript, .. }` through TOML serialize+deserialize.
- **transcript-window-mode.AC2.5 toggle-captions still works at AtomicBool level:** Captions toggle still flips the AtomicBool (no regression). Manual smoke test only — the existing `toggle_captions` behavior is preserved by additive change.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Extend `OverlayMode` with `Transcript` variant in `src/config.rs`

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC2.1, transcript-window-mode.AC2.2, transcript-window-mode.AC2.4.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/config.rs:27-36`
- Test: `/home/jslandau/git/live_text/src/config.rs` (`#[cfg(test)] mod tests` block — find or add at end of file)

**Implementation:**

Edit the `OverlayMode` enum at lines 27-36 to add a third variant. Resulting block:

```rust
/// Overlay display mode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Anchored to a screen edge via wlr-layer-shell.
    #[default]
    Docked,
    /// Freely positioned xdg_toplevel window.
    Floating,
    /// Standard scrollable transcript window (not layer-shell).
    Transcript,
}
```

The `#[serde(rename_all = "snake_case")]` already in place produces the TOML representation `"transcript"` automatically. No per-variant attribute needed.

**Compile check after this edit alone:**

Run: `cargo build`
Expected: **Will fail** with non-exhaustive match errors in `src/overlay/mod.rs:178` (`match mode` inside `SetMode` arm) and possibly `src/tray/mod.rs:445` (the boolean `is_docked` line still compiles, but the radio mapping at line 460 — `let mode = if idx == 0 { OverlayMode::Docked } else { OverlayMode::Floating };` — is now logically incorrect, though it still compiles). Continue to Tasks 2 and 3 to fix the build before running tests.

**Testing:**

Add (or extend) `#[cfg(test)] mod tests` at the end of `src/config.rs` with:

```rust
#[test]
fn ac2_4_overlay_mode_transcript_round_trips_through_toml() {
    let toml_input = r#"overlay_mode = "transcript""#;
    #[derive(serde::Deserialize)]
    struct Wrapper { overlay_mode: super::OverlayMode }
    let w: Wrapper = toml::from_str(toml_input).expect("parse");
    assert_eq!(w.overlay_mode, super::OverlayMode::Transcript);

    let serialized = toml::to_string(&Wrapper { overlay_mode: super::OverlayMode::Transcript }).expect("serialize");
    assert!(
        serialized.contains(r#"overlay_mode = "transcript""#),
        "expected serialized TOML to contain `overlay_mode = \"transcript\"`, got: {serialized}"
    );
}
```

Note: `toml` is already a dependency of this crate (used elsewhere in `config.rs`); confirm by reading the top of `src/config.rs`. If for some reason `toml` is not in scope from the test position, add `use toml;` inside the test or use the canonical `Config` round-trip pattern existing tests use.

**Verification:**

(Note: this task alone produces a non-building tree because Tasks 2 and 3 close the exhaustiveness gap. Do not run `cargo test` until after Task 3.)

**Commit (after Tasks 2 and 3):** Single combined commit at end of subcomponent.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Patch overlay code paths to accept `OverlayMode::Transcript` (window.rs + mod.rs) and add `OverlayCommand::SetCaptionsEnabled(bool)`

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC2.1, transcript-window-mode.AC2.2.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/overlay/window.rs:25-28` (add Transcript arm to inner `match cfg.overlay_mode`)
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:24-40` (add `SetCaptionsEnabled(bool)` variant)
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:164-239` (extend `handle_overlay_command`)
- Modify: `/home/jslandau/git/live_text/src/overlay/mod.rs:155-158` (initial-visibility guard for Transcript startup)

**Pre-implementation: extend `build_overlay_window` to handle Transcript.**

In `src/overlay/window.rs:25-28`, the current match is:
```rust
match cfg.overlay_mode {
    OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge, &cfg.dock_position),
    OverlayMode::Floating => configure_floating(&window, cfg),
}
```
Replace with:
```rust
match cfg.overlay_mode {
    OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge, &cfg.dock_position),
    OverlayMode::Floating => configure_floating(&window, cfg),
    OverlayMode::Transcript => {
        // Transcript mode hides the layer-shell overlay entirely; configure as
        // docked so if the user switches back to Docked mid-session the surface
        // is in a known good state. The window's visibility is gated separately
        // by the activation closure in overlay/mod.rs.
        configure_docked(&window, &cfg.screen_edge, &cfg.dock_position);
    }
}
```

**Implementation:**

**(a) Add the variant.** In the `pub enum OverlayCommand { ... }` block at lines 24-40, add a new variant after `SetCaption(String)` and before `Quit`:

```rust
/// Enable or disable caption emission. On the disable edge the overlay
/// will (in Phase 6) clear all caption surfaces; for now the placeholder
/// arm just mirrors the AtomicBool stored in `CaptionsEnabled`.
SetCaptionsEnabled(bool),
```

**Note on `#[allow(dead_code)]`:** The existing `SetCaption` variant carries a `#[allow(dead_code)]` attribute on its own line (line 36). That attribute scopes only to the single variant immediately following it — the new `SetCaptionsEnabled` variant is *not* affected. Do not place `#[allow(dead_code)]` on the new variant; Phase 4 will exercise it through the command consumer.

**(b) Extend `handle_overlay_command`.** Currently the function does not accept the `captions_enabled: CaptionsEnabled` Arc. We must thread it through. Make the following changes:

1. Update the function signature (line 164-170) from:

```rust
fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
    caption_buffer: &Rc<RefCell<CaptionBuffer>>,
) {
```

to:

```rust
fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
    caption_buffer: &Rc<RefCell<CaptionBuffer>>,
    captions_enabled: &CaptionsEnabled,
) {
```

2. Update the single call site of `handle_overlay_command` inside the command consumer future (currently at line 152). The closure currently captures `window`, `config`, `dragging`, and `buf`. Add a clone of `captions_enabled` to the captured set:

   Existing block (lines 138-156):
   ```rust
   {
       let window = window.clone();
       let config = Arc::clone(&config_clone);
       let dragging = Rc::clone(&is_dragging);
       let buf = Rc::clone(&caption_buffer);
       glib::MainContext::default().spawn_local(async move {
           while let Ok(cmd) = cmd_rx.recv().await {
               let bypass_drag = matches!(
                   cmd,
                   OverlayCommand::Quit | OverlayCommand::SetVisible(_)
               );
               if bypass_drag || !dragging.get() {
                   handle_overlay_command(&window, cmd, &config, &dragging, &buf);
               }
           }
       });
   }
   ```

   becomes:
   ```rust
   {
       let window = window.clone();
       let config = Arc::clone(&config_clone);
       let dragging = Rc::clone(&is_dragging);
       let buf = Rc::clone(&caption_buffer);
       let captions_enabled = Arc::clone(&captions_enabled_clone);
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
                   handle_overlay_command(&window, cmd, &config, &dragging, &buf, &captions_enabled);
               }
           }
       });
   }
   ```

   Two additions to the `bypass_drag` set:
   - `SetCaptionsEnabled`: clearing caption state during a drag is harmless and we want it to take effect immediately.
   - `SetMode`: the user's explicit intent to change mode trumps the drag-suppression heuristic. Phase 4 will preserve this — keep it in here from the start so Phases 2-4 share a consistent intermediate state.

3. Add the `SetCaptionsEnabled` arm and a Transcript-mode arm to the inner `match mode`. Inside `handle_overlay_command`'s `match cmd { ... }`:

   - In the inner `match mode { ... }` at line 178 (inside the `SetMode` arm), add a third arm for `Transcript`:

     ```rust
     OverlayMode::Transcript => {
         // Phase 2 placeholder: hide the overlay window. The transcript
         // window is built and shown in Phase 4.
         window.set_visible(false);
     }
     ```

   - In the outer `match cmd` (between the existing `SetCaption(text)` arm at line 228 and the `Quit` arm at line 232), add:

     ```rust
     OverlayCommand::SetCaptionsEnabled(enabled) => {
         // Phase 2 placeholder: store the AtomicBool. Phase 6 expands this
         // to also clear all caption surfaces on the disable edge.
         captions_enabled.store(enabled, Ordering::Relaxed);
     }
     ```

   `Ordering::Relaxed` is already in scope (line 22).

**Important — Initial visibility on startup:** The current `app.connect_activate` closure (line 68) ends with `window.present();` at line 158. If `cfg.overlay_mode == OverlayMode::Transcript` at startup, we don't want the overlay window to appear at all. Add a guard immediately before line 158:

```rust
if cfg.overlay_mode == OverlayMode::Transcript {
    window.set_visible(false);
} else {
    window.present();
}
```

This means a user who has already saved `overlay_mode = "transcript"` in their config (e.g., from a previous session, once Phase 4 is in play) will not see the layer-shell overlay flash up on launch.

**Testing:**

No new tests in this task — the changes are typed-checked by the compiler and behaviorally exercised by the tray tests added in Task 3 plus the Phase 4 manual smoke test.

**Verification (after Task 3):** see Task 3.

**Commit:** Combined with Tasks 1 and 3 at end of subcomponent.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add third RadioItem, fix index mapping, gate Lock to Floating-only, emit `SetCaptionsEnabled` from `toggle_captions`

**Type:** Functionality.

**Verifies:** transcript-window-mode.AC2.1, transcript-window-mode.AC2.2, transcript-window-mode.AC2.3, transcript-window-mode.AC2.5.

**Files:**
- Modify: `/home/jslandau/git/live_text/src/tray/mod.rs:38-42` (extend `toggle_captions`)
- Modify: `/home/jslandau/git/live_text/src/tray/mod.rs:445` (extend the `is_docked` derivation; introduce `is_floating`)
- Modify: `/home/jslandau/git/live_text/src/tray/mod.rs:457-474` (radio group: add third option, fix index mapping, fix `selected` calculation)
- Modify: `/home/jslandau/git/live_text/src/tray/mod.rs:510-527` (Lock checkmark: change `enabled: !is_docked` to `enabled: is_floating`)
- Modify: `/home/jslandau/git/live_text/src/tray/mod.rs:540-606` (test module: add new test for Transcript-mode radio behavior + Lock disabled in Transcript)

**Implementation:**

**(a) Extend `toggle_captions`** at lines 38-42:

```rust
fn toggle_captions(&mut self) {
    let prev = self.captions_enabled.load(Ordering::Relaxed);
    let next = !prev;
    self.captions_enabled.store(next, Ordering::Relaxed);
    // SetVisible is the historical "hide the overlay when captions are off"
    // hack — meaningful for Docked/Floating, but in Transcript mode hiding
    // the window contradicts the design's "blank, don't hide" intent. Gate
    // it by mode.
    if !matches!(self.overlay_mode, OverlayMode::Transcript) {
        let _ = self.overlay_tx.send_blocking(OverlayCommand::SetVisible(next));
    }
    let _ = self.overlay_tx.send_blocking(OverlayCommand::SetCaptionsEnabled(next));
}
```

Why we keep both commands: `SetVisible` continues to drive the layer-shell overlay's hide-on-disable behavior in Docked/Floating modes (preserving existing UX). `SetCaptionsEnabled` is the new authoritative signal for "stop emitting captions and clear all surfaces" — Phase 6 will expand its handler to clear all four caption surfaces. In Transcript mode we deliberately omit `SetVisible` because the transcript window must remain visible (just blank) so the user can see that captions are off and read the prior history.

**(b) Replace the `is_docked` derivation at line 445** with derivations for both states we now care about:

```rust
let is_docked = tray.overlay_mode == OverlayMode::Docked;
let is_floating = tray.overlay_mode == OverlayMode::Floating;
let is_transcript = tray.overlay_mode == OverlayMode::Transcript;
```

`is_docked` stays so we don't churn code that reads it elsewhere. Both `is_floating` and `is_transcript` are introduced for clarity even though `is_transcript` is only used implicitly via `!is_docked && !is_floating`.

**(c) Update the RadioGroup at lines 457-474:**

```rust
RadioGroup {
    selected: match tray.overlay_mode {
        OverlayMode::Docked => 0,
        OverlayMode::Floating => 1,
        OverlayMode::Transcript => 2,
    },
    select: Box::new(|tray: &mut TrayState, idx: usize| {
        let mode = match idx {
            0 => OverlayMode::Docked,
            1 => OverlayMode::Floating,
            2 => OverlayMode::Transcript,
            _ => return, // ksni shouldn't pass out-of-range indices, but be safe
        };
        tray.overlay_mode = mode.clone();
        let _ = tray.overlay_tx.send_blocking(OverlayCommand::SetMode(mode.clone()));
        let mut cfg = crate::config::Config::load();
        cfg.overlay_mode = tray.overlay_mode.clone();
        if let Err(e) = cfg.save() {
            eprintln!("warn: failed to save config: {e}");
        }
    }),
    options: vec![
        RadioItem { label: "Docked".to_string(), enabled: true, ..Default::default() },
        RadioItem { label: "Floating".to_string(), enabled: true, ..Default::default() },
        RadioItem { label: "Transcript".to_string(), enabled: true, ..Default::default() },
    ],
}
.into(),
```

**(d) Update the Lock CheckmarkItem at line 513:**

```rust
enabled: is_floating, // greyed out in Docked AND Transcript modes
```

**(e) Update the comment at line 513 / the activate guard at line 515:** the `activate` closure already guards with `if tray.overlay_mode == OverlayMode::Floating`, which now correctly excludes Transcript by default. No code change needed inside the closure body.

**Testing:**

Add to the `#[cfg(test)] mod tests` block at the end of `src/tray/mod.rs`:

```rust
/// transcript-window-mode.AC2.3: Lock disabled in Transcript mode.
#[test]
fn ac2_3_lock_item_disabled_in_transcript_mode() {
    let (overlay_tx, _overlay_rx) = async_channel::unbounded();
    let (audio_tx, _audio_rx) = std::sync::mpsc::sync_channel(1);
    let engine_choice = Arc::new(ArcSwap::from_pointee(Engine::Nemotron));

    let tray = TrayState {
        captions_enabled: Arc::new(AtomicBool::new(true)),
        active_source: AudioSource::SystemOutput,
        overlay_mode: OverlayMode::Transcript,
        locked: false,
        active_engine: Engine::Nemotron,
        using_gpu: false,
        overlay_tx,
        audio_tx,
        engine_choice,
        node_list: Arc::new(std::sync::Mutex::new(vec![])),
    };

    // The submenu builder must produce a non-empty submenu and report Transcript
    // (not Floating) as the current mode.
    let overlay_submenu = build_overlay_submenu(&tray);
    assert!(!overlay_submenu.is_empty(), "Overlay submenu should not be empty");
    assert_eq!(tray.overlay_mode, OverlayMode::Transcript);
}

/// transcript-window-mode.AC2.3: Radio group has three options including Transcript.
#[test]
fn ac2_3_radio_has_three_options_including_transcript() {
    let (overlay_tx, _overlay_rx) = async_channel::unbounded();
    let (audio_tx, _audio_rx) = std::sync::mpsc::sync_channel(1);
    let engine_choice = Arc::new(ArcSwap::from_pointee(Engine::Nemotron));

    let tray = TrayState {
        captions_enabled: Arc::new(AtomicBool::new(true)),
        active_source: AudioSource::SystemOutput,
        overlay_mode: OverlayMode::Floating,
        locked: false,
        active_engine: Engine::Nemotron,
        using_gpu: false,
        overlay_tx,
        audio_tx,
        engine_choice,
        node_list: Arc::new(std::sync::Mutex::new(vec![])),
    };

    let overlay_submenu = build_overlay_submenu(&tray);
    // The first item is the RadioGroup; verify it has three options with the
    // expected labels in the expected order (so index 0/1/2 maps Docked/Floating/Transcript).
    let first = overlay_submenu.first().expect("submenu has items");
    if let MenuItem::RadioGroup(group) = first {
        let labels: Vec<&str> = group.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, vec!["Docked", "Floating", "Transcript"]);
    } else {
        panic!("first overlay submenu item must be the mode RadioGroup");
    }
}
```

The existing tests at lines 548-606 do NOT need modification — they assert on the old two-mode behavior at the value level (e.g., `assert!(!tray.overlay_mode.eq(&OverlayMode::Floating))`) without inspecting the radio group structure, so they continue to pass with the additive change. **However**, the existing comment at line 513 says "greyed out in docked mode (AC4.5)" — update it to "greyed out in Docked and Transcript modes; only enabled in Floating".

**Verification (full Phase 2):**

Run: `cargo build`
Expected: Compiles cleanly with no warnings.

Run: `cargo test --lib`
Expected: All tests pass — pre-existing tests (`caption_buffer::tests::*`, `transcript_log::tests::*` from Phase 1, `tray::tests::lock_item_*`, `tray::tests::menu_excludes_stt_engine_submenu`) AND the new `ac2_*` tests in `config.rs` and `tray/mod.rs`.

Run: `cargo clippy -- -D warnings` (if clippy is part of the project's verification — check `.cargo/config.toml` or CI; if not configured, skip).
Expected: No new warnings introduced by these edits.

**Manual smoke test (optional but recommended at this phase boundary):**

```bash
cargo run --release
```
- Right-click the tray icon → Overlay submenu — verify three radio items: Docked, Floating, Transcript.
- Click "Transcript" — verify the layer-shell overlay disappears (Phase 2 stub behavior).
- Open `~/.config/subtidal/config.toml` in a text editor — verify `overlay_mode = "transcript"`.
- Click "Lock Overlay Position" — verify the menu item is disabled (greyed out) when in Transcript mode.
- Click "Floating" — verify Lock becomes enabled.
- Click captions toggle (left-click tray icon) — verify the AtomicBool flips (no visible regression; the new `SetCaptionsEnabled` command is a no-op outside its handler stub which is itself idempotent with the AtomicBool).

**Commit (covers Tasks 1, 2, 3 together since they are coupled by the exhaustive `match mode`):**

```bash
git add src/config.rs src/overlay/mod.rs src/tray/mod.rs
git commit -m "feat(transcript): add OverlayMode::Transcript variant + tray radio + SetCaptionsEnabled command

- Config: third OverlayMode variant serializing as \"transcript\"
- Overlay: SetCaptionsEnabled(bool) command + Transcript stub arm
- Tray: third radio item, Lock gated to Floating-only, toggle emits SetCaptionsEnabled"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 2 Done When

- `cargo build` succeeds with no warnings.
- `cargo test --lib` reports all tests passing including new `ac2_*` tests in `config.rs` and `tray/mod.rs`.
- `cargo run` shows three radio items in the tray Overlay submenu.
- Selecting "Transcript" persists `overlay_mode = "transcript"` to TOML and hides the overlay window.
- Lock menu item is disabled in Docked AND Transcript modes; enabled only in Floating.
- Captions toggle still flips the AtomicBool with no visible regression.

## What Phase 2 Deliberately Does NOT Do

- Does not construct a transcript window — Phase 3.
- Does not route captions to anything other than the existing overlay — Phase 4.
- Does not implement the clear-on-disable side-effects of `SetCaptionsEnabled` — Phase 6.
- Does not import any new crate.
- Does not modify the hot-reload watcher (`src/config.rs:312-365`) — it is value-comparison-based and transparent to new variants.
