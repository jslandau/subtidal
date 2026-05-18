# Phase 1: Cargo.toml Restructuring Implementation Plan

**Goal:** Move Linux-only crates (`gtk4`, `gtk4-layer-shell`, `ksni`, `pipewire`, `libc`) and the `cuda` feature on `ort` / `parakeet-rs` behind `[target.'cfg(target_os = "linux")'.dependencies]`, leaving the Linux build unchanged while making the macOS-target dependency graph free of Linux crates.

**Architecture:** Two related changes to `Cargo.toml` plus the creation of a tiny `src/lib.rs`. First, Linux-only crates move out of `[dependencies]` into a new Linux target-conditional block. `ort` and `parakeet-rs` lose their unconditional `features = ["cuda"]` in `[dependencies]` and instead pick up `cuda` via a second target-conditional entry — Cargo's documented additive feature unification (under resolver v2, which the project uses by virtue of `edition = "2021"`) merges feature sets across the two entries while isolating the `cuda` feature to Linux-target compilations only. `glib` stays cross-platform per design (pure-Rust GObject bindings work on macOS; the Linux callers won't compile on macOS anyway).

Second, the crate gains a library target: a new `src/lib.rs` that simply re-declares the module tree (`pub mod audio; pub mod config; pub mod models; pub mod overlay; pub mod stt; pub mod tray;`) and a new `[lib]` section in `Cargo.toml`. This is the foundation that lets `cargo check --lib --target x86_64-apple-darwin` type-check the library while the binary's `compile_error!` macOS guard (added in Phase 5) keeps `cargo build` failing fast on macOS. The lib and bin share the crate name `subtidal` (Cargo distinguishes by target kind — the canonical Rust idiom used by ripgrep, cargo, etc.). At this phase the library is just a module-declaration shim; subsequent phases gate its contents.

**Tech Stack:** Cargo (resolver v2, edition 2021). No code changes.

**Scope:** Phase 1 of 5. This phase also introduces the `[lib]` target referenced by Phases 2–5 (CI runs `cargo check --lib --target x86_64-apple-darwin`).

**Codebase verified:** 2026-05-17 via codebase-investigator. Verified: current `[dependencies]` block at lines 12–73; `gtk4` at line 14 with `features = ["v4_10"]`; `gtk4-layer-shell` at 15; `glib` at 16 (keep); `ksni` at 19; `pipewire` at 22; `ort = { version = "2.0.0-rc.12", features = ["cuda"] }` at 29; `parakeet-rs = { version = "0.3.4", features = ["cuda"] }` at 30; `libc = "0.2"` at 67. No existing `[target.'cfg(...)']` block. `Cargo.lock` is checked in. Local rustup does NOT yet have `x86_64-apple-darwin` installed; Task 0 below adds it. External research confirmed (per Cargo Book "Platform-Specific Dependencies" and RFC 2957): the two-entry pattern is officially supported, version strings MUST appear in BOTH entries (short-form `{ features = [...] }` without version is a hard error), and resolver v2 (edition 2021 default) isolates the `cuda` feature to Linux-target compilations rather than bleeding it onto other targets.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### linux-backend-isolation.AC4: CUDA features are Linux-conditional
- **linux-backend-isolation.AC4.1 Success:** `Cargo.toml` lists `ort` and `parakeet-rs` in `[target.'cfg(target_os = "linux")'.dependencies]` with the `cuda` feature enabled, and either omits them from `[dependencies]` or lists them there without the `cuda` feature.
- **linux-backend-isolation.AC4.2 Success:** `cargo tree --target x86_64-apple-darwin -e features --package ort` shows `ort` resolved without the `cuda` feature.
- **linux-backend-isolation.AC4.3 Success:** `cargo tree --target x86_64-unknown-linux-gnu -e features --package ort` shows `ort` resolved with the `cuda` feature.
- **linux-backend-isolation.AC4.4 Failure:** If Linux loses CUDA (binary reports "CUDA unavailable" when it previously reported available), feature unification was misconfigured.

### linux-backend-isolation.AC1: Linux binary behavior preserved (regression-only scope this phase)
- **linux-backend-isolation.AC1.1 Success:** `cargo build --release` on Linux exits 0 and produces `target/release/subtidal`.
- **linux-backend-isolation.AC1.2 Success:** Launching `target/release/subtidal` emits a CUDA-availability status message on stderr matching the pre-refactor binary's message.

(Full AC1 and AC2 are completed in later phases. This phase only guarantees Linux regression-safety after the Cargo.toml edit.)

This phase also introduces the `[lib]` target that AC2.1 ultimately verifies against (`cargo check --lib --target x86_64-apple-darwin`).

---

<!-- START_TASK_1 -->
### Task 1: Install `x86_64-apple-darwin` rustup target on the build host

**Type:** Infrastructure.

**Files:** none (host-level rustup state).

**Implementation:**

The macOS-target verification commands in Task 2 require the Rust standard library for `x86_64-apple-darwin` to be available locally. Install it via rustup (downloads a precompiled libstd; does not require Xcode, a macOS SDK, or any linker — `cargo check` and `cargo tree` work without a linker).

```bash
rustup target add x86_64-apple-darwin
```

**Verification:**

```bash
rustup target list --installed | grep x86_64-apple-darwin
```
Expected output: `x86_64-apple-darwin`

**Commit:** None. This is host-state, not repo-state.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Restructure `Cargo.toml` to gate Linux-only crates and the `cuda` feature on `target_os = "linux"`

**Type:** Infrastructure.

**Verifies:** linux-backend-isolation.AC4.1, linux-backend-isolation.AC4.2, linux-backend-isolation.AC4.3, linux-backend-isolation.AC4.4, linux-backend-isolation.AC1.1, linux-backend-isolation.AC1.2.

**Files:**
- Modify: `/home/jslandau/git/live_text/Cargo.toml` (entire `[dependencies]` block at lines 12–73; insert new target-conditional block before `[profile.release]` at line 75).

**Implementation:**

Apply the following edits to `Cargo.toml`. Do NOT edit `Cargo.lock` by hand — Cargo will regenerate it when you next run `cargo build`.

**Edit A — remove from `[dependencies]`:**

Delete the following lines from inside `[dependencies]` (keep the surrounding neutral entries in place):

- Line 13–15 (GUI block): the `# GUI` comment and the `gtk4` and `gtk4-layer-shell` lines. Keep `glib = "0.19"` (line 16) in `[dependencies]` — design intentionally leaves `glib` cross-platform.
- Line 18–19 (System tray block): the `# System tray` comment and the `ksni = "0.3"` line.
- Line 21–22 (the `# Audio` comment line stays, but) delete `pipewire = "0.9"` from line 22. Keep `rubato`, `ringbuf`, `bytemuck`, `audioadapter-buffers` (lines 23–26) in `[dependencies]` — all neutral.
- Line 66–67: the `# Process exit without atexit handlers (avoids CUDA cleanup race)` comment and the `libc = "0.2"` line.

**Edit B — remove `cuda` feature from `[dependencies]` entries for ort and parakeet-rs:**

- Line 29: change `ort = { version = "2.0.0-rc.12", features = ["cuda"] }` to `ort = "2.0.0-rc.12"`.
- Line 30: change `parakeet-rs = { version = "0.3.4", features = ["cuda"] }` to `parakeet-rs = "0.3.4"`.

**Edit C — insert new target-conditional block:**

After the final entry in `[dependencies]` (the `async-channel = "2"` line, currently line 73) and a blank line, before `[profile.release]` (currently line 75), insert exactly:

```toml
# Linux-only crates and CUDA feature additions.
# Resolver v2 (edition 2021 default) isolates the `cuda` feature on `ort` and
# `parakeet-rs` to Linux-target compilations only; on non-Linux targets these
# entries are skipped entirely and the base `[dependencies]` versions (without
# `cuda`) are used. Version strings MUST match the base entries — Cargo rejects
# target-conditional entries that omit version.
[target.'cfg(target_os = "linux")'.dependencies]
gtk4 = { version = "0.10", features = ["v4_10"] }
gtk4-layer-shell = "0.7"
ksni = "0.3"
pipewire = "0.9"
libc = "0.2"
ort = { version = "2.0.0-rc.12", features = ["cuda"] }
parakeet-rs = { version = "0.3.4", features = ["cuda"] }
```

**Resulting top-of-file structure (informational, do not paste verbatim — it merges with the existing untouched entries):**

```toml
[package]
name = "subtidal"
# ... unchanged ...

[[bin]]
# ... unchanged ...

[dependencies]
glib = "0.19"
# Audio
rubato = "1.0"
ringbuf = "0.4"
bytemuck = "1"
audioadapter-buffers = "2"
# STT
ort = "2.0.0-rc.12"
parakeet-rs = "0.3.4"
ndarray = "0.16"
# ... all other neutral entries unchanged: hf-hub, notify, notify-debouncer-mini,
# notify-rust, serde, serde_json, toml, chrono, anyhow, tokio, clap, dirs,
# ctrlc, arc-swap, async-channel ...

[target.'cfg(target_os = "linux")'.dependencies]
gtk4 = { version = "0.10", features = ["v4_10"] }
gtk4-layer-shell = "0.7"
ksni = "0.3"
pipewire = "0.9"
libc = "0.2"
ort = { version = "2.0.0-rc.12", features = ["cuda"] }
parakeet-rs = { version = "0.3.4", features = ["cuda"] }

[profile.release]
# ... unchanged ...

[dev-dependencies]
# ... unchanged ...
```

**Verification:**

Run each command and confirm the expected output. **If any check fails, do NOT commit — back out the Cargo.toml change and investigate.**

```bash
cd /home/jslandau/git/live_text

# 1. Linux build still works.
cargo build --release
```
Expected: builds without errors; `target/release/subtidal` exists. `Cargo.lock` may be updated (and must be re-committed if so).

```bash
# 2. Runtime CUDA-status regression check (AC1.2 / AC4.4 catch).
# Capture stderr lines that mention CUDA from the post-refactor binary.
./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider' || true
```
Expected: the line(s) currently emitted by `cuda_status_message()` (e.g., reporting CUDA availability or fallback to CPU). **The text must match what a pre-refactor build emitted.** If the post-refactor binary now says CUDA is unavailable when it was available before, feature unification was misconfigured — Edit B or C is wrong. Tear down the binary (Ctrl-C) once you've read the CUDA line; the GUI does not need to come up.

Optional pre-refactor baseline capture (do this BEFORE applying the Cargo.toml edit if you have not already):
```bash
git stash
cargo build --release 2>&1 >/dev/null
./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider' > /tmp/cuda_baseline.txt
# Ctrl-C the binary once you've seen the line.
git stash pop
```
Then after the edit, compare:
```bash
diff /tmp/cuda_baseline.txt <(./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider')
```
Expected: no diff.

```bash
# 3. macOS-target dependency tree excludes Linux crates (AC2.2 partial).
cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "FAIL: Linux crate still in macOS-target graph" || echo "OK: pipewire/gtk4/gtk4-layer-shell/ksni absent on macOS target"
```
Expected: prints `OK: pipewire/gtk4/gtk4-layer-shell/ksni absent on macOS target`. Note: `cargo tree` traverses the dependency graph only — it does not invoke `build.rs` or compile source files, so it succeeds at this phase even though `cargo check --target x86_64-apple-darwin` will still fail until Phases 2–5 land.

```bash
# 4. CUDA feature is OFF for macOS target (AC4.2).
cargo tree --target x86_64-apple-darwin -e features --package ort | head -3
```
Expected: an `ort vX.Y.Z` line WITHOUT a `(cuda)` annotation or any `cuda` feature listed.

```bash
# 5. CUDA feature is ON for Linux target (AC4.3).
cargo tree --target x86_64-unknown-linux-gnu -e features --package ort | head -3
```
Expected: an `ort vX.Y.Z` line that includes the `cuda` feature.

```bash
# 6. Same checks for parakeet-rs.
cargo tree --target x86_64-apple-darwin -e features --package parakeet-rs | head -3
cargo tree --target x86_64-unknown-linux-gnu -e features --package parakeet-rs | head -3
```
Expected: macOS target shows parakeet-rs WITHOUT `cuda`; Linux target shows it WITH `cuda`.

**Commit:**

```bash
cd /home/jslandau/git/live_text
git add Cargo.toml Cargo.lock
git commit -m "build: gate Linux-only crates and cuda feature on target_os=linux"
```
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add `[lib]` target and create `src/lib.rs`

**Type:** Infrastructure.

**Verifies:** Foundation for `linux-backend-isolation.AC2.1` (the macOS-target check verified in Phase 5 runs `cargo check --lib`, which requires this lib target).

**Files:**
- Create: `/home/jslandau/git/live_text/src/lib.rs`
- Modify: `/home/jslandau/git/live_text/Cargo.toml` (insert `[lib]` section between `[package]` and `[[bin]]`).

**Implementation:**

The macOS-target CI check (Phase 5) runs `cargo check --lib --target x86_64-apple-darwin` rather than the default `cargo check`, so the binary's `compile_error!` macOS guard (added in Phase 5) doesn't fire for the check. The library target lets the library code stand on its own.

**Edit A — create `src/lib.rs`:**

```rust
//! Subtidal library crate.
//!
//! The binary at `src/main.rs` is a thin orchestrator; all subsystem code lives here.
//! Linux-bound subsystems are cfg-gated in later refactor phases (`audio/`, `tray/`,
//! `stt/nemotron`, `overlay/linux/`). Neutral items (`config`, `models`,
//! `overlay::caption_buffer`, `overlay::transcript_log`, `audio::resampler`,
//! `stt::SttEngine`/`AudioWake`/`PipelineConfig`) compile on all targets.

pub mod audio;
pub mod config;
pub mod models;
pub mod overlay;
pub mod stt;
pub mod tray;
```

**Edit B — add `[lib]` to `Cargo.toml`:**

Insert this block between the existing `[package]` block (ends at line 6 in the pre-edit file) and the existing `[[bin]]` block:

```toml
[lib]
name = "subtidal"
path = "src/lib.rs"
```

(Cargo allows the lib and bin to share the crate name; they're distinct targets by kind. This is the standard idiom — `ripgrep`, `cargo` itself, `wasmtime`, all do this.)

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 0. Defensive: confirm no [lib] section already existed before this task.
grep -n '^\[lib\]' Cargo.toml | wc -l
```
Expected: prints `1` (exactly the one section just inserted). If it prints `0`, Edit B did not land. If it prints `2` or more, an existing `[lib]` block was already present and the executor must reconcile — `git diff Cargo.toml` should show only the single new block.

```bash
# 1. Linux build still works (binary + library).
cargo build --release
```
Expected: builds without errors. **Important:** the binary in `src/main.rs` still uses `mod audio;` / `mod stt;` etc. — these declarations co-exist with the library's `pub mod`s; Rust compiles them as separate compilation units. No source changes to `src/main.rs` or any subsystem file are required at this phase.

```bash
# 2. Library check on macOS target (the regression-free portion of AC2.1).
cargo check --lib --target x86_64-apple-darwin 2>&1 | tail -20
```
Expected: at this phase, the lib check WILL still fail — `src/audio/mod.rs`, `src/tray/mod.rs`, `src/stt/mod.rs`, and `src/overlay/*` are not yet cfg-gated, so they reference `pipewire`, `gtk4`, `ksni`, `libc`, `ort`, etc. on the macOS target. That's expected and is fixed by Phases 3–4. What this check confirms is that the lib target itself was set up correctly (i.e., the error is "can't find crate `pipewire`", NOT a `[lib]`-configuration error).

```bash
# 3. Binary build still attempted on Linux only (verify binary target preserved).
cargo build --bin subtidal --release 2>&1 | tail -5
```
Expected: builds, same as Edit A in Task 2.

**Commit:**

```bash
git add src/lib.rs Cargo.toml Cargo.lock
git commit -m "build: add lib target so macOS-check.yml can check the library only"
```
<!-- END_TASK_3 -->

---

## Out of scope for this phase

- `build.rs` is untouched. `cargo check --target x86_64-apple-darwin` will still fail until Phase 2 gates the CUDA-provider scanning logic. This is expected.
- No `src/` files are modified.
- No CI workflow is added (Phase 5).
- AC1 is verified only as a Linux-regression check (AC1.1 + AC1.2 partial); full AC1.3 smoke test happens after Phase 4. AC2, AC3, AC5 are entirely later phases.
