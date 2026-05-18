# Phase 3: In-Place Cfg-Gating for `audio/`, `tray/`, `stt/` Implementation Plan

**Goal:** Cfg-gate three platform-bound subsystems so they compile out of the `cargo check --lib --target x86_64-apple-darwin` graph without changing any Linux runtime behavior. `audio/` and `tray/` follow the design's gate-and-re-export shell pattern (full body moved to `impl_linux.rs` via `git mv`); `stt/` uses surgical in-place gating because the module mixes platform-neutral types (`SttEngine`, `AudioWake`, `PipelineConfig`) with Linux-only items (`mod nemotron`, ORT CUDA helpers, `spawn_stt_thread`).

**Architecture:** Two `git mv`-based shell-conversions (`audio/`, `tray/`) and one in-place gating pass (`stt/`). All public symbol re-exports preserve names and paths so `main.rs` and other consumers (Phase 5) continue to work unchanged. `git mv` preserves `git log --follow` and `git blame` history. Test items remain on the host's main thread of compilation; this phase does not introduce any new tests, only `#[cfg]` attributes.

**Tech Stack:** Rust 2021 edition. No new dependencies. Uses `#[cfg(target_os = "linux")]`.

**Scope:** Phase 3 of 5.

**Codebase verified:** 2026-05-17 via codebase-investigator. Verified line ranges and public surfaces:
- `src/audio/mod.rs` (411 lines): declares `pub mod resampler;` (line 3); public exports `AudioCommand` (lines 25–30), `AudioNode` (17–22), `FallbackEvent` (33–36), `NodeList` (39), `start_audio_thread` (64–105), `validate_audio_source` (391–411). External callers: `src/main.rs:263,274,278,394`, `src/tray/mod.rs:3`, `src/stt/mod.rs:16` (via `resampler` only).
- `src/audio/resampler.rs`: imports `anyhow`, `audioadapter_buffers`, `rubato` only — fully neutral.
- `src/tray/mod.rs` (759 lines): monolithic; public exports `TrayState` (15–34), `spawn_tray` (577–584). External callers: `src/main.rs:319,334,357`. Imports from `crate::audio`, `crate::config`, `crate::overlay` (all re-routed automatically through the lib).
- `src/stt/mod.rs` (340 lines): neutral items at lines 19–22 (`SttEngine`), 30–81 (`AudioWake`), 84–90 (`PipelineConfig`); Linux-only items at lines 3 (`pub mod nemotron`), 7–8 (ORT imports), 101–223 (`spawn_stt_thread`), 225–235 (`build_engine`), 237–268 (`cuda_available`), 270–297 (`run_cuda_probe`). `spawn_stt_thread` is transitively Linux-only via `build_engine` at line 162. Existing `#[cfg(test)] mod tests` at lines 299–340 — verified by reading: must be inspected before gating to confirm none of its tests reference Linux-only items.
- `src/stt/nemotron.rs`: uses `parakeet_rs` — Linux-only by transitive crate dependency (the crate is gated to Linux in Cargo.toml after Phase 1).
- `src/main.rs` declares `mod audio; mod stt; mod tray;` (lines 1, 4, 6) and uses qualified paths (`audio::*`, `stt::*`, `tray::*`). Stays unchanged this phase; Phase 5 refactors `main.rs` to use the library and adds the `compile_error!` guard.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### linux-backend-isolation.AC1: Linux binary behavior preserved (regression checks)
- **linux-backend-isolation.AC1.1 Success:** `cargo build --release` on Linux exits 0 and produces `target/release/subtidal`.
- **linux-backend-isolation.AC1.2 Success:** CUDA-availability stderr message unchanged from Phase 2 baseline.
- **linux-backend-isolation.AC1.3 Success (audio + tray + STT portions):** Captions appear in the overlay (STT pipeline functional), tray menu shows engine and source and source-switch + captions-toggle both function. (Overlay drag and transcript-mode Save dialog are validated in Phase 4 once overlay/ is gated.)

### linux-backend-isolation.AC2: macOS-target cargo check passes from Linux (audio/tray/stt portion)
- **linux-backend-isolation.AC2.1 Success (partial):** After this phase, `cargo check --lib --target x86_64-apple-darwin` reports zero errors attributable to `src/audio/`, `src/tray/`, or `src/stt/`. Remaining errors are in `src/overlay/` (gated by Phase 4). The full AC2.1 success state arrives at the end of Phase 4.
- **linux-backend-isolation.AC2.2 Success (cumulative with Phase 1):** `cargo tree --target x86_64-apple-darwin` continues to show no `pipewire`, `gtk4`, `gtk4-layer-shell`, or `ksni` entries (Phase 1 enforced; verified here as a regression check).

---

<!-- START_TASK_1 -->
### Task 1: `git mv` audio body to `src/audio/impl_linux.rs`; replace `audio/mod.rs` with a gate-and-re-export shell

**Type:** Functionality.

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.3 (audio capture and source switching), linux-backend-isolation.AC2.1 (audio portion).

**Files:**
- Rename: `/home/jslandau/git/live_text/src/audio/mod.rs` → `/home/jslandau/git/live_text/src/audio/impl_linux.rs` (via `git mv`).
- Modify: the moved `src/audio/impl_linux.rs` (remove the `pub mod resampler;` line, since `resampler` is declared from the new shell `mod.rs`).
- Create: `/home/jslandau/git/live_text/src/audio/mod.rs` (NEW shell).
- Unchanged: `/home/jslandau/git/live_text/src/audio/resampler.rs`.

**Implementation:**

Step 1 — Rename via `git mv` to preserve history:

```bash
cd /home/jslandau/git/live_text
git mv src/audio/mod.rs src/audio/impl_linux.rs
```

Step 2 — Edit the renamed file `src/audio/impl_linux.rs`: remove the `pub mod resampler;` declaration at line 3. (It is moving to the new shell `mod.rs`. Leaving it here would cause a duplicate-module-declaration error.) Everything else in the file stays exactly as-is.

Step 3 — Create the new shell `src/audio/mod.rs` with this exact content:

```rust
//! Platform-isolated audio subsystem.
//!
//! Public types (`AudioCommand`, `AudioNode`, `FallbackEvent`, `NodeList`) and entry
//! points (`start_audio_thread`, `validate_audio_source`) are re-exported from the
//! Linux implementation in `impl_linux.rs`. To add a new platform, create a sibling
//! `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export the same
//! public surface here.

pub mod resampler;

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{
    start_audio_thread, validate_audio_source, AudioCommand, AudioNode, FallbackEvent,
    NodeList,
};
```

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. git-blame and log-follow preserved through the rename.
git log --follow --oneline src/audio/impl_linux.rs | head -3
```
Expected: shows commits predating this phase (the file's original history under `audio/mod.rs`).

```bash
# 2. Linux build still succeeds.
cargo build --release
```
Expected: builds without errors.

```bash
# 3. Runtime smoke test — audio capture + source switch.
./target/release/subtidal &
APP_PID=$!
sleep 5
# In another terminal or on the desktop, use the tray menu:
#   tray -> Audio Source -> switch to a different system or app source.
# Verify: captions continue appearing for the new source. No panic in stderr.
# Stop the binary.
kill $APP_PID
wait $APP_PID 2>/dev/null || true
```
Expected: no panic; captions track the switched source.

**If any AC1.3 sub-check regresses (panic, no captions, drag jitter, missing tray entry):** do NOT commit. Run `git restore --staged .` and `git restore .` (or `git reset --hard HEAD` to drop all working-tree changes) and re-investigate the move. The most likely culprit is the `pub mod resampler;` line left in `impl_linux.rs` (duplicate module declaration) or an incorrect shell `mod.rs`.

```bash
# 4. macOS-target lib check: audio crate compiles out (no pipewire imports leak in).
cargo check --lib --target x86_64-apple-darwin 2>&1 | grep -E 'pipewire|audio/(mod|impl_linux)\.rs' | head -5
```
Expected: zero or near-zero matches. (If any match references `audio/impl_linux.rs`, the `#[cfg(target_os = "linux")] mod impl_linux;` gate did not take effect — re-check the shell mod.rs.)

**Commit:**

```bash
git add src/audio/mod.rs src/audio/impl_linux.rs
git commit -m "refactor(audio): cfg-gate Linux impl behind audio/impl_linux.rs"
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `git mv` tray body to `src/tray/impl_linux.rs`; replace `tray/mod.rs` with a gate-and-re-export shell

**Type:** Functionality.

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.3 (tray menu functional: engine display, captions toggle, source switch, overlay-submenu toggles), linux-backend-isolation.AC2.1 (tray portion).

**Files:**
- Rename: `/home/jslandau/git/live_text/src/tray/mod.rs` → `/home/jslandau/git/live_text/src/tray/impl_linux.rs` (via `git mv`).
- Create: `/home/jslandau/git/live_text/src/tray/mod.rs` (NEW shell).
- Unchanged: the moved `impl_linux.rs` itself (no `mod X;` declarations exist in the current file body; imports through `crate::audio`, `crate::config`, `crate::overlay` all continue working via the lib's re-exports).

**Implementation:**

Step 1:

```bash
git mv src/tray/mod.rs src/tray/impl_linux.rs
```

Step 2 — Create `src/tray/mod.rs` with this exact content:

```rust
//! Platform-isolated system-tray subsystem.
//!
//! Public types (`TrayState`) and entry points (`spawn_tray`) are re-exported from
//! the Linux implementation in `impl_linux.rs`. To add a new platform, create a
//! sibling `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export
//! the same public surface here.

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{spawn_tray, TrayState};
```

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. git-blame and log-follow preserved.
git log --follow --oneline src/tray/impl_linux.rs | head -3
```

```bash
# 2. Linux build + runtime tray smoke (AC1.3 tray portion).
cargo build --release
./target/release/subtidal &
APP_PID=$!
sleep 5
# Use the tray menu — verify:
#   - Engine name shown
#   - Audio Source submenu lists current sources; clicking a different one switches.
#   - Captions toggle hides/shows captions.
#   - Overlay submenu mode-switch live-applies (Docked/Floating/Transcript).
kill $APP_PID
wait $APP_PID 2>/dev/null || true
```

**If any tray smoke regresses (menu missing, source switch broken, captions toggle broken, overlay mode switch broken):** do NOT commit. Run `git restore --staged .` and `git restore .` (or `git reset --hard HEAD`) and re-investigate. Most likely the shell `tray/mod.rs` is missing a `pub use` for a symbol that the new `impl_linux.rs` was implicitly re-exporting before.

```bash
# 3. macOS-target lib check: tray crate compiles out (ksni absent from graph).
cargo check --lib --target x86_64-apple-darwin 2>&1 | grep -E 'ksni|tray/(mod|impl_linux)\.rs' | head -5
```
Expected: zero matches.

**Commit:**

```bash
git add src/tray/mod.rs src/tray/impl_linux.rs
git commit -m "refactor(tray): cfg-gate Linux impl behind tray/impl_linux.rs"
```
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: In-place cfg-gate Linux-only items in `src/stt/mod.rs`; leave neutral items unguarded

**Type:** Functionality (surgical edit; no `git mv`).

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.2 (CUDA status unchanged), linux-backend-isolation.AC1.3 (captions appear → STT pipeline works), linux-backend-isolation.AC2.1 (stt portion).

**Files:**
- Modify: `/home/jslandau/git/live_text/src/stt/mod.rs` (add six `#[cfg(target_os = "linux")]` attributes; possibly gate test items).
- Unchanged: `/home/jslandau/git/live_text/src/stt/nemotron.rs` (only reachable through the gated `pub mod nemotron;` declaration).

**Implementation:**

The `stt/` module mixes neutral and Linux-only items in one file. Use surgical in-place gating instead of a `git mv` shell-split: add `#[cfg(target_os = "linux")]` immediately above each Linux-only item.

**Edit A — gate the module declaration at line 3.** Change:
```rust
pub mod nemotron;
```
to:
```rust
#[cfg(target_os = "linux")]
pub mod nemotron;
```

**Edit B — gate the ORT imports at lines 7–8.** Change:
```rust
use ort::ep::ExecutionProvider as _;
use ort::ep::CUDA;
```
to:
```rust
#[cfg(target_os = "linux")]
use ort::ep::ExecutionProvider as _;
#[cfg(target_os = "linux")]
use ort::ep::CUDA;
```

> **Note (intentional churn):** These two ORT import lines are gated here in Phase 3 but will be DELETED entirely in Phase 5 Task 1 because `run_cuda_probe` and `cuda_available` (their only users) migrate to `src/main_linux.rs`. Gating in this phase is still necessary: Phase 3's macOS lib-check needs `stt/` to compile cleanly on the macOS target, which requires that the `use ort::ep::*` lines be gated out. The two-step approach (gate now, move later) keeps each phase's intermediate state buildable and verifiable in isolation. Do not skip the gating in Phase 3 thinking "it just gets deleted later".

**Edit C — gate `spawn_stt_thread` at line 101.** Insert a single line above the existing `pub fn spawn_stt_thread(` declaration:
```rust
#[cfg(target_os = "linux")]
pub fn spawn_stt_thread(
    // ... body unchanged ...
) {
    // ...
}
```

**Edit D — gate `build_engine` at line 225.** Insert a single line above the existing `fn build_engine(` declaration:
```rust
#[cfg(target_os = "linux")]
fn build_engine(
    // ... body unchanged ...
) -> Result<Box<dyn SttEngine>> {
    // ...
}
```

**Edit E — gate `cuda_available` at line 237.** Insert a single line above the existing `pub fn cuda_available(` declaration:
```rust
#[cfg(target_os = "linux")]
pub fn cuda_available(model_dir: &std::path::Path) -> bool {
    // ... body unchanged ...
}
```

**Edit F — gate `run_cuda_probe` at line 270.** Insert a single line above the existing `pub fn run_cuda_probe()` declaration:
```rust
#[cfg(target_os = "linux")]
pub fn run_cuda_probe() -> ! {
    // ... body unchanged ...
}
```

**Items that stay unguarded (DO NOT add a cfg gate):**
- `pub trait SttEngine` (lines 19–22)
- `pub struct AudioWake` + `impl AudioWake` (lines 30–81)
- `pub struct PipelineConfig` (lines 84–90)

**Pre-flight: handle existing tests.** Read `src/stt/mod.rs:299-340` (the existing `#[cfg(test)] mod tests` block). For each test function, determine whether it references any of the gated items (`cuda_available`, `run_cuda_probe`, `build_engine`, `spawn_stt_thread`, `NemotronEngine`).
- **If a test ONLY references neutral items (`AudioWake`, `SttEngine`, `PipelineConfig`):** leave it unguarded.
- **If a test references ANY gated item:** prefix the test function with `#[cfg(target_os = "linux")]` so it disappears from the lib's macOS-target compilation. If the test module itself is wrapped (i.e., contains a `use super::*;`), the gate goes on the individual `#[test] fn` items.

In the current code, the tests block is expected to cover only `AudioWake` / pipeline ergonomics; verify by reading lines 299–340 before applying gates. If any test does reference a gated item, document the additional gate above the test function.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. Linux build + tests still pass.
cargo build --release
cargo test --lib stt::
```
Expected: build succeeds; tests pass.

```bash
# 2. Runtime smoke: captions appear (STT pipeline end-to-end) AND CUDA status unchanged.
./target/release/subtidal 2>&1 | head -20 | tee /tmp/stt-smoke.log
# Within 10 seconds, exercise audio (play a YouTube video or speak into the captured source).
# Expected within 5–10s of audio: captions appear in the overlay.
# Ctrl-C.
grep -iE 'cuda|provider' /tmp/stt-smoke.log | head -3
```
Expected: CUDA-availability line matches the Phase 1/2 baseline.

```bash
# 3. macOS-target lib check: stt does not leak ort/parakeet_rs/libc.
cargo check --lib --target x86_64-apple-darwin 2>&1 | grep -E '\bort\b|parakeet|libc::|stt/mod\.rs|stt/nemotron\.rs' | head -5
```
Expected: zero matches from `stt/` lines. Remaining errors should be only in `overlay/` (gated in Phase 4).

```bash
# 4. Cumulative AC2 regression check.
cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "FAIL" || echo "OK"
```
Expected: prints `OK`.

**Commit:**

```bash
git add src/stt/mod.rs
git commit -m "refactor(stt): cfg-gate Linux-only items (nemotron, ORT, cuda probes) in stt/mod.rs"
```
<!-- END_TASK_3 -->

---

## Out of scope for this phase

- `src/overlay/` is untouched (Phase 4).
- `src/main.rs` still has direct, unguarded uses of `audio::*`, `tray::*`, `stt::*` items; the binary build is fine on Linux because those items exist there. The bin's compile-on-macOS state is irrelevant until Phase 5 (CI runs `cargo check --lib`, not the bin).
- The CI workflow file (`.github/workflows/macos-check.yml`) is Phase 5.
- AC3, AC4 (mostly), AC5 are not addressed here.
- Full AC2 success arrives only after Phase 4.
