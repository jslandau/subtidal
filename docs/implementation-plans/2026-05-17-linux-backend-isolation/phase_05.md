# Phase 5: `main.rs` Split, `compile_error!` Guard, CI, CLAUDE.md Implementation Plan

**Goal:** Complete the refactor by (1) extracting Linux-specific startup helpers from `main.rs` into a new `src/main_linux.rs`, (2) refactoring `main.rs` to use the library crate (Phase 1) and carry the `compile_error!` macOS hard-fail, (3) adding the GitHub Actions workflow that runs `cargo check --lib --target x86_64-apple-darwin` on every push, and (4) documenting the architecture in `CLAUDE.md`.

**Architecture:** Four independent components combined in one phase because they all serve the same goal of "finalize the isolation and install verification". The lib/bin separation introduced in Phase 1 pays off here: the binary's `compile_error!` guard hard-fails `cargo build` on non-Linux targets while the CI's `cargo check --lib` checks library code only and stays green. `main_linux.rs` consolidates all `std::os::unix::*` and `libc::*` usage in one file. The `cuda_available` and `run_cuda_probe` functions move from `src/stt/mod.rs` (where Phase 3 cfg-gated them) into `main_linux.rs` per the design's design — they belong with startup logic, not with the STT trait.

**Tech Stack:** Rust 2021 edition, GitHub Actions, Markdown. No new dependencies.

**Scope:** Phase 5 of 5 (final).

**Codebase verified:** 2026-05-17 via direct read.
- `src/main.rs` is 440 lines. `mod` declarations at lines 1–6 (`mod audio; mod config; mod models; mod stt; mod overlay; mod tray;`). `use` block at lines 8–12 (`use arc_swap::ArcSwap; use clap::Parser; use config::Config; use std::sync::Arc; use std::sync::atomic::AtomicBool;`). Functions to move: `ensure_provider_libs_next_to_exe` at 97–153 (uses `std::os::unix::fs::symlink` at line 123); `reexec_with_absolute_argv0_if_needed` at 155–183 (uses `std::os::unix::process::CommandExt` at line 156); `cuda_status_message` at 415–421; `#[cfg(test)] mod tests` at 423–440. `fn main()` at 185–413. Inline `unsafe { libc::_exit(0) }` at line 410.
- Call sites of items that move: `stt::run_cuda_probe()` at line 191; `stt::cuda_available(&model_dir)` at line 283.
- `src/stt/mod.rs` after Phase 3: `cuda_available` at 237 and `run_cuda_probe` at 270 are cfg-gated; both must be DELETED in this phase (they move to `main_linux.rs`). The `#[cfg(target_os = "linux")] use ort::ep::ExecutionProvider as _;` and `... use ort::ep::CUDA;` imports at lines 7–8 are referenced only by `run_cuda_probe` and must also move (verified by grepping for the import names elsewhere in the file).
- `CLAUDE.md` currently has `Freshness: 2026-05-13` and lacks a `## Platform Isolation` section.
- External research findings (verified 2026-05-17 via internet-researcher): `actions/checkout@v6` is the current major as of 2026; `dtolnay/rust-toolchain@stable` with `targets:` input is the canonical way to install Rust + cross-target in CI; `Swatinem/rust-cache@v2` (v2.9.x) caches per-target build artifacts. `cargo check --lib` does NOT trip the binary's `compile_error!`.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### linux-backend-isolation.AC1: Linux binary behavior preserved (final regression)
- **linux-backend-isolation.AC1.1 Success:** `cargo build --release` on Linux exits 0; binary produced.
- **linux-backend-isolation.AC1.2 Success:** CUDA-availability stderr line matches pre-refactor baseline.
- **linux-backend-isolation.AC1.3 Success:** Manual smoke test passes: captions in overlay; tray shows engine/source; source-switch works; captions toggle works; floating drag works; transcript Save dialog produces non-empty `.json`.

### linux-backend-isolation.AC2: macOS-target cargo check passes from Linux
- **linux-backend-isolation.AC2.1 Success (final):** `cargo check --lib --target x86_64-apple-darwin` exits 0 with no errors and no new warnings.
- **linux-backend-isolation.AC2.2 Success:** `cargo tree --target x86_64-apple-darwin` shows no `pipewire`, `gtk4`, `gtk4-layer-shell`, or `ksni` entries.

### linux-backend-isolation.AC3: CI workflow runs and passes
- **linux-backend-isolation.AC3.1 Success:** `.github/workflows/macos-check.yml` exists, defines one job on `ubuntu-latest` running `cargo check --lib --target x86_64-apple-darwin`, and uses `dtolnay/rust-toolchain@stable` with the `x86_64-apple-darwin` target plus `Swatinem/rust-cache@v2`.
- **linux-backend-isolation.AC3.2 Success:** First push of the branch triggers the workflow and the check completes green within reasonable time (under 10 minutes for the cold run).

### linux-backend-isolation.AC4: CUDA features are Linux-conditional (final regression)
- **linux-backend-isolation.AC4.4 Failure-mode check:** Binary still reports CUDA available (if it did pre-refactor), confirming feature unification was not silently undone.

### linux-backend-isolation.AC5: Architectural intent documented
- **linux-backend-isolation.AC5.1 Success:** `CLAUDE.md` contains a new "## Platform Isolation" section naming: the cfg-gating convention, the verification mechanism (CI check), the recipe for adding a new platform, and the location of the `compile_error!` guard.
- **linux-backend-isolation.AC5.2 Success:** The CLAUDE.md "Freshness" date is updated to 2026-05-17.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Extract Linux startup helpers and CUDA probes into new `src/main_linux.rs`

**Type:** Functionality (file split; preserves Linux runtime behavior).

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.2 (CUDA stderr unchanged), linux-backend-isolation.AC4.4 (CUDA still detected).

**Files:**
- Create: `/home/jslandau/git/live_text/src/main_linux.rs` (NEW).
- Modify: `/home/jslandau/git/live_text/src/main.rs` (delete the moved helpers and the inline `libc::_exit` site; reroute call sites to `main_linux::*`). NOTE: The full `main.rs` rewrite (lib-use + `compile_error!`) is Task 2 below; this task only does the surgical extraction.
- Modify: `/home/jslandau/git/live_text/src/stt/mod.rs` (delete `cuda_available` and `run_cuda_probe`; delete the ORT imports they exclusively used).

**Implementation:**

**Step 1 — Capture verbatim bodies.** Read these source ranges and capture their function bodies word-for-word:
- From `src/main.rs`: `fn ensure_provider_libs_next_to_exe` (lines 97–153); `fn reexec_with_absolute_argv0_if_needed` (lines 155–183); `fn cuda_status_message` (lines 415–421); and the `#[cfg(test)] mod tests { ... }` block (lines 423–440).
- From `src/stt/mod.rs` (post-Phase-3): the cfg-gated `pub fn cuda_available` (lines 237–268) and `pub fn run_cuda_probe` (lines 270–297).
- The two ORT `use` imports at `src/stt/mod.rs:7-8` (the `use ort::ep::ExecutionProvider as _;` and `use ort::ep::CUDA;`).

**Step 2 — Compose `src/main_linux.rs`.** The file is a single Linux-only module; it does NOT need internal `#[cfg(target_os = "linux")]` gates because the `mod main_linux;` declaration in `main.rs` (added in Task 2) is itself gated.

```rust
//! Linux-only startup helpers for the Subtidal binary.
//!
//! All `std::os::unix::*` and `libc` usage lives in this file. The binary's
//! `src/main.rs` orchestrates startup by `use`-ing these helpers via
//! `#[cfg(target_os = "linux")] mod main_linux;`.

use ort::ep::ExecutionProvider as _;
use ort::ep::CUDA;
use std::path::Path;

// ===== Provider-library symlinking (verbatim from old src/main.rs:97-153) =====
pub fn ensure_provider_libs_next_to_exe() {
    // ... body unchanged ...
}

// ===== argv[0] reexec workaround (verbatim from old src/main.rs:155-183) =====
pub fn reexec_with_absolute_argv0_if_needed() {
    // ... body unchanged ...
}

// ===== CUDA availability probe (verbatim from old src/stt/mod.rs:237-268) =====
pub fn cuda_available(model_dir: &Path) -> bool {
    // ... body unchanged ...
}

// ===== CUDA probe entry point (verbatim from old src/stt/mod.rs:270-297) =====
pub fn run_cuda_probe() -> ! {
    // ... body unchanged ...
}

// ===== CUDA status message (verbatim from old src/main.rs:415-421) =====
pub fn cuda_status_message(cuda_available: bool) -> &'static str {
    // ... body unchanged ...
}

// ===== exit_without_atexit (NEW; extracts the inline libc::_exit at old src/main.rs:410) =====
/// Skip all atexit handlers (both Rust and C++). ORT's C++ atexit destructors
/// call cudaFreeHost after the CUDA driver has already shut down, causing
/// SIGABRT. `std::process::exit` still runs atexit handlers, so we bypass
/// them via `libc::_exit`.
pub fn exit_without_atexit(code: i32) -> ! {
    unsafe { libc::_exit(code) }
}

#[cfg(test)]
mod tests {
    // ... verbatim from old src/main.rs:423-440 ...
}
```

**Visibility:** Every moved function MUST be `pub` (currently `cuda_status_message` in `main.rs` is private — its tests are `mod tests` inside the same file, so it remained reachable; after the move the tests live in the same file as `cuda_status_message` so `pub` isn't strictly required for them, but mark it `pub` anyway for consistency and for `main.rs::main()` to import it).

**Step 3 — Delete the moved code.**

In `src/main.rs`:
- Delete `fn ensure_provider_libs_next_to_exe` (was lines 97–153).
- Delete `fn reexec_with_absolute_argv0_if_needed` (was lines 155–183).
- Delete `fn cuda_status_message` (was lines 415–421).
- Delete the `#[cfg(test)] mod tests { ... }` block (was lines 423–440).
- In `fn main()`: change the inline `unsafe { libc::_exit(0) }` (at line 410) to `main_linux::exit_without_atexit(0);`. Note: `main_linux::exit_without_atexit` has `-> !` so no `return` is needed; existing control flow is preserved.
- In `fn main()`: change `stt::run_cuda_probe()` (line 191) → `main_linux::run_cuda_probe()`.
- In `fn main()`: change `stt::cuda_available(&model_dir)` (line 283) → `main_linux::cuda_available(&model_dir)`.
- Add a `mod main_linux;` declaration after the existing `mod` block (line ~7). This declaration is NOT YET cfg-gated; Task 2 wraps it.

In `src/stt/mod.rs`:
- Delete `#[cfg(target_os = "linux")] pub fn cuda_available(model_dir: &std::path::Path) -> bool { ... }` (was lines 237–268 after Phase 3 added the cfg gate).
- Delete `#[cfg(target_os = "linux")] pub fn run_cuda_probe() -> ! { ... }` (was lines 270–297).
- Delete `#[cfg(target_os = "linux")] use ort::ep::ExecutionProvider as _;` (was line 7).
- Delete `#[cfg(target_os = "linux")] use ort::ep::CUDA;` (was line 8).
- Verify no other item in `src/stt/mod.rs` references `ort::ep::*`:
  ```bash
  grep -n 'ort::ep' src/stt/mod.rs
  ```
  Expected: zero matches after the deletion. If any remain (e.g., used by `build_engine`), STOP and re-examine — the move is incomplete and either those references move too or the imports are restored.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. Linux build still works.
cargo build --release
```

```bash
# 2. cuda_status_message tests still pass.
# The tests now live inside the `main_linux` module rather than at the top level of `main.rs`,
# so prefer the qualified filter:
cargo test --bin subtidal main_linux::tests::cuda_status_message 2>&1 | tail -10
# Alternative (less specific but also works — cargo test treats the argument as a substring filter):
#   cargo test --bin subtidal cuda_status_message 2>&1 | tail -10
```
Expected: `test main_linux::tests::cuda_status_message_when_available ... ok` and `test main_linux::tests::cuda_status_message_when_unavailable ... ok` both pass.

```bash
# 3. Runtime CUDA stderr unchanged.
./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider' | head -3
```

```bash
# 4. End-to-end smoke (AC1.3 audio + STT portion).
./target/release/subtidal &
APP_PID=$!
sleep 10
# Verify captions appear; tray works.
kill $APP_PID
wait $APP_PID 2>/dev/null || true
```

**Commit:**

```bash
git add src/main_linux.rs src/main.rs src/stt/mod.rs
git commit -m "refactor: extract Linux startup helpers into main_linux module"
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Refactor `src/main.rs` to use the library crate and add the `compile_error!` macOS guard

**Type:** Functionality.

**Verifies:** linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.3 (Linux build and smoke unchanged); linux-backend-isolation.AC2.1 (full pass — the macOS-target lib check now exits 0 because the `compile_error!` only fires for the bin); foundational for linux-backend-isolation.AC3.2 (CI green).

**Files:**
- Modify: `/home/jslandau/git/live_text/src/main.rs` (replace `mod audio; mod config; ... mod tray;` with `use subtidal::{...}`; insert `compile_error!` guard; gate `mod main_linux;` and the `use main_linux::*;` imports on `target_os = "linux"`).

**Implementation:**

Top-of-file new structure (everything before `fn main()`):

```rust
//! Subtidal binary entry point.
//!
//! Library code lives in the `subtidal` crate (`src/lib.rs`); Linux-specific
//! startup helpers live in `main_linux` (gated to `target_os = "linux"`).

#[cfg(target_os = "linux")]
mod main_linux;

// Hard-fail the binary build on non-Linux targets with a clear message.
// `cargo check --lib --target ...` does NOT compile this binary and so does
// not trip this error — that is how the CI macOS-check stays green while
// `cargo build` on macOS fails fast with a single, clear message.
//
// Placement note: this MUST come AFTER `mod main_linux;` is declared but
// BEFORE any `use` or other logic, per Rust's mod-resolution order. Placing
// it before `mod` declarations causes confusing cascading errors.
#[cfg(not(target_os = "linux"))]
compile_error!("Subtidal currently only supports Linux. macOS support is planned.");

use arc_swap::ArcSwap;
use clap::Parser;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[cfg(target_os = "linux")]
use subtidal::{audio, config::{self, Config}, models, overlay, stt, tray};

#[cfg(target_os = "linux")]
use main_linux::{
    cuda_available, cuda_status_message, ensure_provider_libs_next_to_exe,
    exit_without_atexit, reexec_with_absolute_argv0_if_needed, run_cuda_probe,
};
```

**Removed lines from old `main.rs`:**

- Lines 1–6 (`mod audio; mod config; mod models; mod stt; mod overlay; mod tray;`) — these modules now come from the `subtidal` library (Phase 1 added `pub mod` declarations to `src/lib.rs`). Re-declaring them here would cause Cargo to compile each module twice (once for the lib, once for the bin) — a footgun.
- Old line 10 `use config::Config;` — replaced by `use subtidal::config::{self, Config};` above.

**Updates inside `fn main()`:**

In Task 1, the call sites were already changed to `main_linux::run_cuda_probe()`, `main_linux::cuda_available(...)`, and `main_linux::exit_without_atexit(0)`. Since the `use main_linux::{...}` block now imports these names into scope, the qualifications can be reduced to bare names (`run_cuda_probe()`, `cuda_available(...)`, `exit_without_atexit(0)`). Both qualified and bare forms work; pick bare for consistency with the rest of `main()`'s style.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. Linux build still works.
cargo build --release
```
Expected: no errors; binary produced.

```bash
# 2. cargo check --lib --target x86_64-apple-darwin passes cleanly (THE acceptance criterion).
cargo check --lib --target x86_64-apple-darwin
echo "Exit: $?"
```
Expected: `Exit: 0` and no `error:` lines. This is AC2.1 full success.

```bash
# 3. cargo build --target x86_64-apple-darwin produces ONE clear compile_error.
cargo build --bin subtidal --target x86_64-apple-darwin 2>&1 | tail -10
```
Expected: a single `error: Subtidal currently only supports Linux. macOS support is planned.` originating from `src/main.rs` near the `compile_error!` macro. (This is the EXPECTED failure mode for macOS bin builds; the workflow does NOT exercise this path.)

```bash
# 4. End-to-end smoke (AC1.3).
./target/release/subtidal &
APP_PID=$!
sleep 10
# Exercise audio, tray menu (engine, source, captions toggle, overlay mode submenu).
# Exercise floating drag, transcript Save dialog.
kill $APP_PID
wait $APP_PID 2>/dev/null || true
```
Expected: all smoke steps pass.

**Commit:**

```bash
git add src/main.rs
git commit -m "refactor(main): use library crate and compile_error! on non-Linux targets"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_3 -->
### Task 3: Add `.github/workflows/macos-check.yml`

**Type:** Infrastructure.

**Verifies:** linux-backend-isolation.AC3.1, linux-backend-isolation.AC3.2 (after the branch is pushed).

**Files:**
- Create: `/home/jslandau/git/live_text/.github/workflows/macos-check.yml` (NEW; first workflow in the repo).

**Implementation:**

Create the directory if it doesn't exist and write the file:

```bash
mkdir -p .github/workflows
```

`.github/workflows/macos-check.yml` content (exact):

```yaml
name: macOS target check

on:
  push:
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-apple-darwin

      - uses: Swatinem/rust-cache@v2

      - run: cargo check --lib --target x86_64-apple-darwin --verbose
```

**Rationale for the choices:**
- `actions/checkout@v6` is the current major as of January 2026.
- `dtolnay/rust-toolchain@stable` is the canonical way to install Rust in CI in 2026 (the `actions-rs/toolchain` action has been archived). The `targets:` input is documented and supported.
- `Swatinem/rust-cache@v2` is the de-facto standard cargo-build cache action; the cache key already includes the target triple, so this job won't pollute caches from other native-target jobs (none today, but future-proof).
- `cargo check --lib` (NOT bare `cargo check`) is critical — bare `cargo check` would attempt to compile the binary target, which trips the `compile_error!` from Task 2 and would fail the workflow. The `--lib` flag scopes the check to the library only, which is the exact contract AC2.1 asks for.
- `--verbose` makes failure logs useful (the whole point of this workflow is to catch future regressions; verbose output speeds diagnosis).
- The job needs only the `x86_64-apple-darwin` rustup target — no macOS SDK, no `osxcross`, no linker. `cargo check` skips the link step.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. YAML is syntactically valid.
python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/macos-check.yml"))'
```
Expected: no output (silent success). If yaml is not installed, install briefly: `pip install pyyaml` or rely on `yq`.

```bash
# 2. File exists at the expected path with the expected name.
ls -l .github/workflows/macos-check.yml
```

```bash
# 3. (After pushing the branch — manual.)
# git push -u origin linux-backend-isolation
# Then view the workflow run:
# gh run list --workflow=macos-check.yml --limit=3
# gh run view <run-id> --log
```
Expected: workflow triggers on the push; cold first run finishes within 10 minutes; subsequent cache-hit runs under 1 minute.

**Commit:**

```bash
git add .github/workflows/macos-check.yml
git commit -m "ci: add cargo check --target x86_64-apple-darwin workflow"
```
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update `CLAUDE.md` with `## Platform Isolation` section and refresh date

**Type:** Infrastructure (documentation).

**Verifies:** linux-backend-isolation.AC5.1, linux-backend-isolation.AC5.2.

**Files:**
- Modify: `/home/jslandau/git/live_text/CLAUDE.md`.

**Implementation:**

Two edits.

**Edit A — Update `Freshness:` line.** Change `Freshness: 2026-05-13` (currently near the top of the file, around line 5) to `Freshness: 2026-05-17`.

**Edit B — Append a new `## Platform Isolation` section.** Insert this section between the existing `## Build & Run` block and the end of the file (i.e., as the last `##`-level section). The section must name: (1) the cfg-gating convention, (2) the verification mechanism (CI check), (3) the recipe for adding a new platform, and (4) the location of the `compile_error!` guard.

Content to append:

```markdown
## Platform Isolation

Subtidal's source tree is structured so that all Linux-specific code is gated behind `#[cfg(target_os = "linux")]`. The crate exposes both a `[lib]` (`src/lib.rs`) and a `[[bin]]` (`src/main.rs`); the binary additionally carries a `#[cfg(not(target_os = "linux"))] compile_error!` guard that hard-fails non-Linux binary builds with a clear "macOS support is planned" message.

**Cfg-gating boundaries.** Each platform-bound subsystem follows one of three patterns:

- **Shell-and-re-export** (`audio/`, `tray/`): `mod.rs` is a thin shell that declares `#[cfg(target_os = "linux")] mod impl_linux;` and re-exports the public surface. The Linux implementation body lives in `impl_linux.rs`.
- **Subtree-and-re-export** (`overlay/`): `mod.rs` keeps neutral items (`OverlayCommand`, `CaptionsEnabled`, `caption_buffer`, `transcript_log`) at the module root and gates a `linux/` subdirectory holding the GTK orchestration (`run_gtk_app`, `handle_overlay_command`) and per-window submodules (`window`, `drag`, `input_region`, `transcript_window`).
- **In-place gating** (`stt/`): the module mixes neutral types (`SttEngine` trait, `AudioWake`, `PipelineConfig`) with Linux-only items (`mod nemotron`, `spawn_stt_thread`, `build_engine`). Linux-only items carry `#[cfg(target_os = "linux")]` directly; neutral items are unguarded.

**Cargo dependencies.** Linux-only crates (`pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc`) live in `[target.'cfg(target_os = "linux")'.dependencies]`. The `cuda` feature on `ort` and `parakeet-rs` is Linux-conditional via additive feature unification: each crate appears once in `[dependencies]` (without `cuda`) and once in the Linux-conditional block (with `cuda`). Resolver v2 (edition 2021 default) keeps the `cuda` feature from bleeding onto non-Linux targets.

**Verification mechanism.** A GitHub Actions workflow at `.github/workflows/macos-check.yml` runs `cargo check --lib --target x86_64-apple-darwin` on `ubuntu-latest` for every push and pull request. Any future commit that accidentally introduces Linux coupling into a notionally-neutral module fails the check. The workflow uses `--lib` (not bare `cargo check`) so the binary's `compile_error!` guard does not fire.

**Build-script gate.** `build.rs` early-returns on non-Linux targets via `env::var("TARGET").unwrap_or_default().contains("linux")`. The `cfg!(target_os = "linux")` macro is intentionally NOT used here — it reflects the build host, not the cross-compilation target, and would silently fail to skip CUDA-provider scanning during `cargo check --target x86_64-apple-darwin` from a Linux host.

**`compile_error!` location.** `src/main.rs` contains the macOS hard-fail guard immediately after the `mod main_linux;` declaration and before any `use` or other logic. Placement before `mod` declarations causes confusing cascading errors from rustc's mod-resolution pass.

**Recipe for adding a new platform (e.g., macOS).**

1. Remove the line `compile_error!("Subtidal currently only supports Linux. macOS support is planned.");` from `src/main.rs` (or refine its cfg predicate to exclude the new platform).
2. For each cfg-gated subsystem, mirror the Linux structure with a sibling implementation:
   - `audio/`: add `src/audio/impl_macos.rs` and gate it from `src/audio/mod.rs` with `#[cfg(target_os = "macos")]`.
   - `tray/`: same shape — add `src/tray/impl_macos.rs`.
   - `overlay/`: add `src/overlay/macos/` subdirectory and gate `mod macos;` from `src/overlay/mod.rs`.
   - `stt/`: add a `mod coreml;` (or analogous) and gate the Linux-specific items behind their existing cfgs.
3. Add a `[target.'cfg(target_os = "macos")'.dependencies]` block in `Cargo.toml` listing macOS-only crates (e.g., `core-foundation`, `cocoa`).
4. Move the Linux-specific main helpers analogously into `src/main_macos.rs` and add the corresponding `#[cfg(target_os = "macos")] mod main_macos;` declaration in `src/main.rs`.
5. Add the new target as an entry (or matrix value) in `.github/workflows/macos-check.yml` (which can be renamed to e.g. `cross-target-check.yml` once it serves multiple targets).
```

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. Freshness date updated.
grep '^Freshness:' CLAUDE.md
```
Expected: `Freshness: 2026-05-17`.

```bash
# 2. New section header present.
grep -A1 '^## Platform Isolation' CLAUDE.md | head -3
```
Expected: section header line followed by its first body line.

```bash
# 3. AC5.1 required elements all present (cfg-gating convention, verification mechanism, recipe, compile_error! location).
grep -cE 'cfg-gating|cfg\(target_os|macos-check\.yml|compile_error!|Recipe' CLAUDE.md
```
Expected: ≥ 4 matches (one for each required element).

**Commit:**

```bash
git add CLAUDE.md
git commit -m "docs: document Platform Isolation contract in CLAUDE.md"
```
<!-- END_TASK_4 -->

---

## Final integration check (after all four task commits in this phase)

Run this check to confirm every acceptance criterion now passes:

```bash
cd /home/jslandau/git/live_text

echo "=== AC1.1: Linux build ==="
cargo build --release && echo "AC1.1 PASS"

echo "=== AC1.2: CUDA stderr ==="
./target/release/subtidal 2>&1 | head -5 | grep -iE 'cuda|provider' | head -1
echo "AC1.2 — compare line above to pre-refactor baseline; PASS if identical"

echo "=== AC1.3: Manual smoke test ==="
echo "AC1.3 — exercise binary UI: docked captions, floating drag, transcript Save, tray menu. PASS if all five pass."

echo "=== AC2.1: cargo check --lib --target x86_64-apple-darwin ==="
cargo check --lib --target x86_64-apple-darwin && echo "AC2.1 PASS"

echo "=== AC2.2: cargo tree macOS target excludes Linux crates ==="
cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "AC2.2 FAIL" || echo "AC2.2 PASS"

echo "=== AC3.1: workflow file ==="
test -f .github/workflows/macos-check.yml && echo "AC3.1 PASS"

echo "=== AC3.2: workflow runs green after push (manual via gh run list) ==="

echo "=== AC4.1: Cargo.toml structure ==="
grep -q "^\[target\.'cfg(target_os = \"linux\")'\.dependencies\]" Cargo.toml && echo "AC4.1 PASS"

echo "=== AC4.2: ort no cuda on macOS target ==="
cargo tree --target x86_64-apple-darwin -e features --package ort | head -3 | grep -q cuda && echo "AC4.2 FAIL" || echo "AC4.2 PASS"

echo "=== AC4.3: ort cuda on Linux target ==="
cargo tree --target x86_64-unknown-linux-gnu -e features --package ort | head -3 | grep -q cuda && echo "AC4.3 PASS" || echo "AC4.3 FAIL"

echo "=== AC5.1: CLAUDE.md section ==="
grep -q '^## Platform Isolation' CLAUDE.md && echo "AC5.1 PASS"

echo "=== AC5.2: Freshness date ==="
grep -q '^Freshness: 2026-05-17' CLAUDE.md && echo "AC5.2 PASS"
```

Expected: all `PASS` lines, no `FAIL` lines, no missing checks. AC1.3 and AC3.2 require manual verification (UI smoke and GitHub Actions run).

## Out of scope (post-Phase-5)

- Adding `aarch64-apple-darwin` to the workflow (one-line addition; design notes it as a future enhancement).
- Actual macOS implementations (CoreAudio, NSWindow, NSStatusBar, CoreML EP) — design explicitly out of scope.
- Linux build/test CI job — design explicitly out of scope (no general CI improvements beyond the macOS-check workflow).
