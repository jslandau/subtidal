# Phase 4: In-Place Cfg-Gating for `overlay/` Implementation Plan

**Goal:** Move the entire GTK + layer-shell subtree (`window.rs`, `drag.rs`, `input_region.rs`, `transcript_window.rs`, plus the orchestration in the old `overlay/mod.rs`) into a new `src/overlay/linux/` subdirectory behind a single `#[cfg(target_os = "linux")]` gate. Neutral overlay items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, `transcript_log`) stay at `crate::overlay::*` so that `crate::config`, `crate::tray::impl_linux`, and `src/main.rs` continue to import them unchanged.

**Architecture:** Four `git mv`s move the GTK source files; a new `src/overlay/linux/mod.rs` (NEW) owns the orchestration code (`run_gtk_app` and `handle_overlay_command`) plus the public declarations of the four submodules; the surviving `src/overlay/mod.rs` becomes a thin shell that re-exports `run_gtk_app` from `linux` (when on Linux) while keeping `OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, and `transcript_log` in place. Cross-module `crate::overlay::transcript_log::*` references from the moved `transcript_window.rs` resolve correctly post-move because they use absolute paths.

**Tech Stack:** Rust 2021 edition. No new dependencies. Uses `#[cfg(target_os = "linux")]`.

**Scope:** Phase 4 of 5.

**Codebase verified:** 2026-05-17 via codebase-investigator. Verified:
- `src/overlay/mod.rs` (404 lines): submodule declarations at lines 1–9 (`mod caption_buffer; mod drag; mod transcript_log; mod transcript_window; mod window; pub mod input_region;`); GTK use-statements at 11–24; `OverlayCommand` enum at 28–50; `CaptionsEnabled` type alias at 53; `run_gtk_app` at 59–247; `handle_overlay_command` at 249–404.
- `src/overlay/window.rs` (273), `src/overlay/drag.rs` (128), `src/overlay/input_region.rs` (52), `src/overlay/transcript_window.rs` (376) — all GTK-bound.
- `src/overlay/caption_buffer.rs` (598): only `use std::time::Instant;` — fully neutral.
- `src/overlay/transcript_log.rs` (341): only `chrono` + `serde` + `std` — fully neutral.
- External `crate::overlay::*` callers: `src/main.rs:404` calls `overlay::run_gtk_app(cfg, caption_rx, cmd_rx, captions_enabled)`; `src/config.rs` references `crate::overlay::OverlayCommand::{UpdateAppearance, SetMode, SetLocked, SetAboveFullscreen}`; `src/tray/mod.rs:5` (becomes `tray/impl_linux.rs:5` after Phase 3) has `use crate::overlay::OverlayCommand;`. No external caller of `crate::overlay::input_region::*`.
- `transcript_window.rs` imports `crate::overlay::transcript_log::{AppendKind, Fragment, Paragraph, TranscriptLog}` (absolute paths; resolve correctly after move). No `super::` paths in any of the four soon-to-move files (verified — see Verification step in Task 1).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### linux-backend-isolation.AC1: Linux binary behavior preserved
- **linux-backend-isolation.AC1.1 Success:** `cargo build --release` on Linux exits 0; binary produced.
- **linux-backend-isolation.AC1.2 Success:** CUDA stderr message unchanged.
- **linux-backend-isolation.AC1.3 Success (complete):** All three overlay modes function — docked captions at configured edge; floating mode with working drag (no jitter); transcript mode with timestamped paragraphs accumulating in scrollable view and Save dialog producing a non-empty `.json` sidecar. Tray-driven live toggles (above-fullscreen, locked, mode-switch) all live-apply.

### linux-backend-isolation.AC2: macOS-target cargo check passes from Linux
- **linux-backend-isolation.AC2.1 Success (complete):** `cargo check --lib --target x86_64-apple-darwin` exits 0 with no errors. (Phases 1–3 reduced the error set; Phase 4 closes the remaining overlay-attributed errors.)
- **linux-backend-isolation.AC2.2 Success (cumulative):** `cargo tree --target x86_64-apple-darwin` shows no `pipewire`, `gtk4`, `gtk4-layer-shell`, or `ksni` entries.

---

<!-- START_TASK_1 -->
### Task 1: `git mv` the four GTK files under `src/overlay/linux/`, create `src/overlay/linux/mod.rs` for orchestration, and rewrite `src/overlay/mod.rs` as a neutral shell

**Type:** Functionality (large structural refactor; preserves Linux behavior).

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.2, linux-backend-isolation.AC1.3 (all three overlay modes), linux-backend-isolation.AC2.1 (full), linux-backend-isolation.AC2.2 (regression).

**Files:**
- Move (via `git mv`):
  - `src/overlay/window.rs` → `src/overlay/linux/window.rs`
  - `src/overlay/drag.rs` → `src/overlay/linux/drag.rs`
  - `src/overlay/input_region.rs` → `src/overlay/linux/input_region.rs`
  - `src/overlay/transcript_window.rs` → `src/overlay/linux/transcript_window.rs`
- Create: `/home/jslandau/git/live_text/src/overlay/linux/mod.rs` (NEW; orchestration + submodule declarations).
- Rewrite: `/home/jslandau/git/live_text/src/overlay/mod.rs` (becomes thin shell holding neutral items only).
- Unchanged: `src/overlay/caption_buffer.rs`, `src/overlay/transcript_log.rs`.

**Implementation:**

Step 1 — Create the target subdirectory:
```bash
cd /home/jslandau/git/live_text
mkdir -p src/overlay/linux
```

Step 2 — `git mv` the four GTK-bound files (preserves `git log --follow` and `git blame`):
```bash
git mv src/overlay/window.rs src/overlay/linux/window.rs
git mv src/overlay/drag.rs src/overlay/linux/drag.rs
git mv src/overlay/input_region.rs src/overlay/linux/input_region.rs
git mv src/overlay/transcript_window.rs src/overlay/linux/transcript_window.rs
```

Step 3 — Confirm there are no `super::` paths inside the moved files that need rewriting:
```bash
grep -n 'super::' src/overlay/linux/*.rs
```
Expected: zero matches. If any match appears, rewrite that `use super::X` line to `use crate::overlay::X` (or whatever absolute path matches) so the file is location-independent.

Step 4 — Create `src/overlay/linux/mod.rs`. The body has three sections: submodule declarations, GTK use-imports (copied verbatim from lines 11–24 of the OLD `overlay/mod.rs` minus the four sibling-module imports), and the two orchestration functions copied VERBATIM from the OLD `overlay/mod.rs` lines 59–247 (`run_gtk_app`) and 249–404 (`handle_overlay_command`).

The skeleton:

```rust
//! Linux GTK4 + layer-shell overlay implementation.
//!
//! Contains the `run_gtk_app` entry point, the `OverlayCommand` dispatch loop body,
//! and the per-window GTK construction submodules. This entire subtree is cfg-gated
//! to `target_os = "linux"`; neutral overlay items (`OverlayCommand`,
//! `caption_buffer`, `transcript_log`) live one level up in `crate::overlay`.

pub mod drag;
pub mod input_region;
pub mod transcript_window;
pub mod window;

// === BEGIN: imports copied from old src/overlay/mod.rs lines 11-24,
// minus the `use crate::overlay::{caption_buffer, drag, window, input_region, transcript_log, transcript_window}`
// patterns. Replace those four sibling-module imports with the local `use` statements below.
// ===
use gtk4::prelude::*;
use gtk4::glib;
use gtk4::{Application, ApplicationWindow};
use gtk4_layer_shell::{Edge, KeyboardMode, LayerShell, Layer};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{self, Config, OverlayMode};
use crate::overlay::{
    caption_buffer::CaptionBuffer,
    transcript_log::TranscriptLog,
    CaptionsEnabled, OverlayCommand,
};

use drag::add_drag_handler;
use input_region::{clear_input_region, set_empty_input_region};
use transcript_window::{
    append_fragment_to_view, build_transcript_window, clear_view, populate_view,
};
use window::{apply_appearance, build_overlay_window, configure_docked, find_caption_label};

// === BEGIN: pub fn run_gtk_app — VERBATIM copy of old overlay/mod.rs lines 59-247 ===
// (paste body unchanged)

// === BEGIN: fn handle_overlay_command — VERBATIM copy of old overlay/mod.rs lines 249-404 ===
// (paste body unchanged)
```

**Important — VERBATIM:** the executor must `Read` `src/overlay/mod.rs` BEFORE rewriting it (to capture `run_gtk_app` and `handle_overlay_command` bodies and the exact set of GTK imports), then paste those bodies into `src/overlay/linux/mod.rs` byte-for-byte. The `use` set in the skeleton above is the EXPECTED set; if the file's actual `use` declarations differ (e.g., extra crate referenced), copy the actual lines. Do NOT add or remove imports.

The executor must also verify, after copying, that:
- `pub fn run_gtk_app(...)` keeps its existing signature exactly (do not generalize, do not introduce traits).
- `handle_overlay_command` keeps its private visibility and its existing signature.
- No re-numbering or refactoring of internal logic is introduced. This task is a relocation, not a refactor.

Step 5 — Rewrite `src/overlay/mod.rs` (the surviving thin shell):

```rust
//! Platform-isolated overlay subsystem.
//!
//! Neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`,
//! `transcript_log`) live here. The Linux GTK + layer-shell implementation is in
//! `linux/`, gated behind `#[cfg(target_os = "linux")]`. To add a new platform,
//! create a sibling subdirectory (e.g. `macos/`), gate it analogously, and
//! re-export the platform-specific entry points from here.

pub mod caption_buffer;
pub mod transcript_log;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::run_gtk_app;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::config::{AppearanceConfig, OverlayMode};

#[derive(Debug, Clone)]
pub enum OverlayCommand {
    SetVisible(bool),
    SetMode(OverlayMode),
    SetLocked(bool),
    SetAboveFullscreen(bool),
    UpdateAppearance(AppearanceConfig),
    #[allow(dead_code)]
    SetCaption(String),
    SetCaptionsEnabled(bool),
    Quit,
}

pub type CaptionsEnabled = Arc<AtomicBool>;
```

**VERBATIM check for `OverlayCommand`:** copy the enum definition (variants, derives, attributes including `#[allow(dead_code)] SetCaption(String)`) byte-for-byte from the old `overlay/mod.rs` lines 28–50. Do not adjust the variant set, visibility (it stays `pub`), or derives (`Debug, Clone`).

**Items deliberately removed from the new shell `overlay/mod.rs`:**
- The submodule declarations for `drag`, `transcript_window`, `window`, and the `pub mod input_region;` line — these now live in `overlay/linux/mod.rs`.
- The GTK use-imports (lines 11–24 of the old file) — they move into `overlay/linux/mod.rs`.
- The `run_gtk_app` and `handle_overlay_command` function bodies — they move into `overlay/linux/mod.rs`.

**Items deliberately KEPT in the new shell `overlay/mod.rs`:**
- `pub mod caption_buffer;` (still there; the file did not move).
- `pub mod transcript_log;` (still there; the file did not move). NOTE: in the old `overlay/mod.rs`, these were `mod` not `pub mod`. After this phase they should be `pub mod` so the absolute path `crate::overlay::transcript_log::*` from the moved `transcript_window.rs` continues to resolve. If they were already `pub mod` skip this; if they were private, change to `pub mod`. (Verify by reading the OLD file before rewriting.)
- `OverlayCommand` enum and `CaptionsEnabled` type alias.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. git-blame preserved on all four moved files.
git log --follow --oneline src/overlay/linux/window.rs            | head -3
git log --follow --oneline src/overlay/linux/drag.rs              | head -3
git log --follow --oneline src/overlay/linux/input_region.rs      | head -3
git log --follow --oneline src/overlay/linux/transcript_window.rs | head -3
```
Expected: each prints commits predating this phase (pre-move history under `src/overlay/`).

```bash
# 2. Linux build succeeds.
cargo build --release
```
Expected: builds without errors. If `cannot find type` or `unresolved import` errors appear, they're almost certainly in `src/overlay/linux/mod.rs`'s use-imports — re-check that the moved files' public functions are imported into `linux/mod.rs` correctly.

```bash
# 3. Full overlay-mode smoke test (AC1.3 — this is the acceptance trio).
./target/release/subtidal &
APP_PID=$!
sleep 5

# 3a. Docked mode: caption text appears at the configured screen edge.
#     Generate some audio (play a YouTube video or speak into the captured source).
#     Verify captions appear at the configured edge.

# 3b. Switch to Floating mode via tray > Overlay > Mode > Floating.
#     - Captions still appear.
#     - Drag the overlay around with the left mouse button — verify no jitter, no
#       relayout artifacts, no snap-back.
#     - Tray > Overlay > Locked toggles drag on/off.

# 3c. Switch to Transcript mode via tray > Overlay > Mode > Transcript.
#     - Transcript window opens as a regular toplevel.
#     - Timestamped paragraphs accumulate in the scrollable view as audio plays.
#     - Click the Save button — verify a `.json` sidecar is written and is non-empty.

# 3d. Toggle above-fullscreen via tray > Overlay > Show Above Fullscreen.
#     - Open a fullscreen browser video; verify the overlay still renders on top.

# 3e. Switch overlay sources via tray > Audio Source (separate from overlay mode).
#     - Captions track the new source.

kill $APP_PID
wait $APP_PID 2>/dev/null || true
```
Expected: all five sub-tests pass. **If any regress, do NOT commit.** Run `git restore --staged .` and `git restore .` (or `git reset --hard HEAD` to drop all uncommitted changes; do not use `--hard HEAD~1` since this phase hasn't committed yet) and re-investigate. The most likely culprit is a wrong `use` path in `overlay/linux/mod.rs` (e.g., importing `crate::overlay::transcript_log::Paragraph` where the function actually needs `crate::overlay::transcript_log::TranscriptLog`), or a `pub mod` visibility mismatch on `caption_buffer`/`transcript_log` in the new shell `overlay/mod.rs` (they MUST be `pub mod`, not bare `mod`, so the moved `transcript_window.rs` can resolve `crate::overlay::transcript_log::*`).

```bash
# 4. macOS-target lib check passes cleanly (full AC2.1).
cargo check --lib --target x86_64-apple-darwin
echo "Exit: $?"
```
Expected: exit code 0 and no `error:` lines in the output.

```bash
# 5. AC2.2 regression check.
cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "FAIL" || echo "OK"
```
Expected: prints `OK`.

```bash
# 6. AC4 regression check (CUDA still feature-isolated).
cargo tree --target x86_64-apple-darwin -e features --package ort | head -3
cargo tree --target x86_64-unknown-linux-gnu -e features --package ort | head -3
```
Expected: first command shows `ort` without `cuda`; second shows `ort` with `cuda`.

**Commit:**

```bash
git add src/overlay/mod.rs src/overlay/linux/
git commit -m "refactor(overlay): move GTK subtree under overlay/linux/ behind cfg-gate"
```
<!-- END_TASK_1 -->

---

## Out of scope for this phase

- `src/main.rs` and `src/main_linux.rs` split — Phase 5.
- `compile_error!` macOS guard for the binary — Phase 5.
- `.github/workflows/macos-check.yml` — Phase 5.
- `CLAUDE.md` "Platform Isolation" section — Phase 5.
