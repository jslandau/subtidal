# Linux Backend Isolation Design

## Summary

The refactor introduces platform isolation into Subtidal without writing any macOS implementations. Rather than designing new abstractions, it exploits Subtidal's existing inter-thread seams — `OverlayCommand`, `AudioCommand`, and `SttEngine` — which are already platform-neutral. Each Linux-bound module (`audio/`, `overlay/`, `tray/`, `stt/`) becomes a thin gate-and-re-export shell; the Linux implementation either moves to a sibling `impl_linux.rs` file or into a new `linux/` subdirectory, all wrapped in `#[cfg(target_os = "linux")]`. Cargo's target-conditional dependency blocks excise `pipewire`, `gtk4`, `gtk4-layer-shell`, and `ksni` from non-Linux dependency graphs entirely, and a two-entry pattern adds the `cuda` feature to `ort` and `parakeet-rs` only on Linux.

The payoff is a single cheap verification mechanism: `cargo check --target x86_64-apple-darwin` run from an ordinary Linux host, enforced by a new GitHub Actions workflow. Any future commit that accidentally leaks a Linux symbol into a notionally-neutral module fails that check immediately. No macOS toolchain is required, no macOS implementations are delivered, and the Linux binary's runtime behavior is unchanged.

## Definition of Done

**Primary deliverable:** Refactor the Subtidal codebase so all Linux-specific code in the `audio/`, `overlay/`, `tray/`, and `stt/` modules (plus the CUDA setup in `main.rs` and `build.rs`, and the `cuda` cargo features) is cleanly isolated behind `#[cfg(target_os = "linux")]` gates, using whatever per-subsystem pattern (trait abstraction, module aliasing, or simple cfg blocks) fits best. No macOS implementations are written.

**Success criteria:**

1. `cargo build --release` on Linux produces a working `subtidal` binary with identical runtime behavior to the pre-refactor master (overlay, tray, audio, STT, CUDA detection all unchanged).
2. `cargo check --target x86_64-apple-darwin` passes on a Linux host with no macOS toolchain installed beyond `rustup target add x86_64-apple-darwin`. Pure modules (`config`, `caption_buffer`, `transcript_log`, `resampler`, `stt` trait) compile; all platform-bound code is cfg-gated out.
3. A new GitHub Actions workflow (`.github/workflows/macos-check.yml`) runs the above macOS-target check on every push.
4. The `cuda` cargo features on `ort` and `parakeet-rs` are conditional on `target_os = "linux"`.
5. Architectural intent of the refactor (which modules are platform-neutral, where the cfg-gate boundaries sit per subsystem, and what a future macOS port would need to implement) is documented in `CLAUDE.md`.

**Out of scope:**

- Writing any macOS implementations (CoreAudio, NSWindow, NSStatusBar, CoreML EP).
- General CI improvements beyond the single macOS-check job (no Linux build/test job, no lint, no formatter).
- Refactoring code that isn't platform-bound (no speculative cleanup of `overlay/mod.rs` dispatch logic, no STT trait redesign).
- Forward-looking trait abstractions that anticipate macOS needs (per 1a-minimal posture, traits exist only when they serve the current Linux structure).

## Acceptance Criteria

### linux-backend-isolation.AC1: Linux binary behavior preserved

- **AC1.1 (success):** `cargo build --release` on Linux exits 0 and produces `target/release/subtidal`.
- **AC1.2 (success):** Launching `target/release/subtidal` emits a CUDA-availability status message on stderr matching the pre-refactor binary's message (verified by capturing stderr from the post-refactor binary and comparing the relevant line to a pre-refactor recording).
- **AC1.3 (success):** Manual smoke test passes: captions appear in the overlay, tray menu shows engine and source, source-switch and captions-toggle from tray both function, overlay drag works in floating mode, transcript-mode Save dialog produces a non-empty .json sidecar.
- **AC1.4 (failure):** If any of the above smoke checks regress (e.g., captions don't appear, tray missing, drag jitters), the phase is not done.

### linux-backend-isolation.AC2: macOS-target cargo check passes from Linux

- **AC2.1 (success):** On a Linux host with `rustup target add x86_64-apple-darwin` installed, `cargo check --target x86_64-apple-darwin` exits 0 with no errors and no warnings beyond pre-existing ones.
- **AC2.2 (success):** `cargo tree --target x86_64-apple-darwin` shows no `pipewire`, `gtk4`, `gtk4-layer-shell`, or `ksni` dependency entries.
- **AC2.3 (failure):** If `cargo check --target x86_64-apple-darwin` reports any "unresolved import" or "cannot find type" error for a Linux-specific symbol, a cfg-gate is missing somewhere; the refactor is not complete.

### linux-backend-isolation.AC3: CI workflow runs and passes

- **AC3.1 (success):** `.github/workflows/macos-check.yml` exists, defines one job on `ubuntu-latest` running `cargo check --target x86_64-apple-darwin`, and uses `dtolnay/rust-toolchain@stable` with the `x86_64-apple-darwin` target plus `Swatinem/rust-cache@v2`.
- **AC3.2 (success):** First push of the branch triggers the workflow and the check completes green within reasonable time (under 10 minutes for the cold run).
- **AC3.3 (failure):** If the workflow file is present but malformed (YAML syntax error, action version typo) or the check fails on the first push, the phase is not done.

### linux-backend-isolation.AC4: CUDA features are Linux-conditional

- **AC4.1 (success):** `Cargo.toml` lists `ort` and `parakeet-rs` in `[target.'cfg(target_os = "linux")'.dependencies]` with the `cuda` feature enabled, and either omits them from `[dependencies]` or lists them there without the `cuda` feature.
- **AC4.2 (success):** `cargo tree --target x86_64-apple-darwin -e features --package ort` shows `ort` resolved without the `cuda` feature.
- **AC4.3 (success):** `cargo tree --target x86_64-unknown-linux-gnu -e features --package ort` shows `ort` resolved with the `cuda` feature.
- **AC4.4 (failure):** If Linux loses CUDA (binary reports "CUDA unavailable" when it previously reported available), feature unification was misconfigured.

### linux-backend-isolation.AC5: Architectural intent documented

- **AC5.1 (success):** `CLAUDE.md` contains a new "## Platform Isolation" section that names: the cfg-gating convention, the verification mechanism (the CI check), the recipe for adding a new platform (mirror the linux subtree), and the location of the `compile_error!` guard.
- **AC5.2 (success):** The CLAUDE.md "Freshness" date is updated to today's date.
- **AC5.3 (failure):** Documentation missing or describing structures different from what was actually built.

## Glossary

- **`#[cfg(target_os = "linux")]`**: A Rust compile-time conditional attribute that includes or excludes the annotated item based on the compilation target OS, not the OS the compiler itself runs on.
- **`cargo check --target x86_64-apple-darwin`**: Runs Rust's type-checking and borrow-checker passes for a macOS target triple without producing a binary; used here as a cross-compilation correctness probe from a Linux host.
- **`compile_error!`**: A Rust built-in macro that emits a user-defined compiler error message, used here to produce a clear "macOS not yet supported" failure if someone attempts `cargo build` on macOS.
- **`[target.'cfg(...)'.dependencies]`**: A Cargo.toml section that declares dependencies that are only resolved and compiled when the build target satisfies the given cfg predicate.
- **Feature unification**: Cargo's rule that when a crate appears in multiple dependency entries, all requested features are merged (additive); exploited here so `cuda` is added to `ort` on Linux without duplicating the base version declaration.
- **`env::var("TARGET")` vs `cfg!(target_os)`**: In `build.rs`, `cfg!` macros reflect the host OS (where the compiler runs), not the cross-compilation target; reading the `TARGET` environment variable is the correct way to detect the compilation target during build-script execution.
- **`git mv`**: The git command that renames or moves files while preserving commit history, allowing `git log --follow` and `git blame` to trace a file back through its pre-rename history.
- **`SttEngine` trait**: Subtidal's internal abstraction over speech-to-text backends (`process_chunk(&[f32]) -> Option<String>`); already platform-neutral, it doubles as a platform-separation boundary for the STT subsystem.
- **`OverlayCommand` / `AudioCommand`**: Enums used as channel message types between Subtidal's threads; their neutrality means cfg-gating only the implementations behind them is sufficient for isolation.
- **`dtolnay/rust-toolchain`**: A widely-used GitHub Actions action that installs a specified Rust toolchain version; referenced as the canonical way to pin Rust in CI.
- **`Swatinem/rust-cache`**: A GitHub Actions action that caches Cargo's build artifacts between CI runs, avoiding cold-compile times on every push.
- **`rustup target add`**: The Rustup command that downloads a pre-built standard library for a new target triple, enabling cross-compilation or cross-checking without a full cross-toolchain.
- **Layer-shell (wlr-layer-shell)**: A Wayland protocol extension that lets applications render surfaces pinned to screen edges or above/below other windows; used by Subtidal's docked and floating overlay modes. Not available on macOS, making `gtk4-layer-shell` Linux-only.

## Architecture

In-place cfg-gating per module. Each platform-bound module's `mod.rs` becomes a thin gate-and-re-export layer that selects a Linux implementation submodule via `#[cfg(target_os = "linux")]`. Pure modules (no Linux coupling) stay where they are with no changes. No trait abstractions are introduced; the existing thread-separation seams (`OverlayCommand`, `AudioCommand`, `SttEngine`) already serve as platform-separation seams.

Linux-only Cargo dependencies (`pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc`) move into a `[target.'cfg(target_os = "linux")'.dependencies]` block. The `cuda` feature on `ort` and `parakeet-rs` becomes Linux-conditional via Cargo's additive feature unification: each crate appears once in `[dependencies]` (setting base version/features) and once in the target-conditional block (adding the `cuda` feature only on Linux).

`build.rs` early-returns on non-Linux targets, gating its CUDA-provider scanning. Critically, this uses `env::var("TARGET")`, not `cfg!(target_os = "linux")` — the latter checks the build host, not the cross-compilation target, and would silently fail to skip CUDA scanning during `cargo check --target x86_64-apple-darwin` from a Linux host.

`main.rs` carries a top-of-file `#[cfg(not(target_os = "linux"))] compile_error!(...)` placed after `mod` declarations. Library code (cfg-gated modules) still passes `cargo check` for the macOS target; `cargo build` of the binary fails fast on macOS with a clear "macOS support is planned" message. Linux-specific startup helpers (CUDA discovery, argv[0] reexec, provider symlinking, atexit-bypass) move into a new `src/main_linux.rs` submodule cfg-gated and `use`d from `main.rs`.

A single GitHub Actions workflow (`.github/workflows/macos-check.yml`) runs `cargo check --target x86_64-apple-darwin` on `ubuntu-latest` for every push and pull request. This is the verification mechanism for the refactor's correctness: any future change that accidentally introduces Linux coupling into a notionally-neutral module fails the check.

**Final layout:**

```
src/
  main.rs                          # compile_error! guard; calls into main_linux
  main_linux.rs                    # NEW: cfg-gated; CUDA discovery, argv[0] reexec, etc.
  config.rs                        # unchanged (neutral)
  models/                          # unchanged (neutral)
  audio/
    mod.rs                         # gate + re-exports
    impl_linux.rs                  # NEW: body moved from current audio/mod.rs
    resampler.rs                   # unchanged (neutral)
  overlay/
    mod.rs                         # gate + neutral re-exports (OverlayCommand stays here)
    caption_buffer.rs              # unchanged (neutral)
    transcript_log.rs              # unchanged (neutral)
    linux/                         # NEW: entire GTK subtree behind one cfg-gate
      mod.rs
      window.rs
      drag.rs
      input_region.rs
      transcript_window.rs
  tray/
    mod.rs                         # gate + re-exports
    impl_linux.rs                  # NEW: body moved from current tray/mod.rs
  stt/
    mod.rs                         # SttEngine trait stays neutral
    nemotron.rs                    # cfg-gated `mod nemotron;` in stt/mod.rs
build.rs                           # early-return on non-Linux TARGET
Cargo.toml                         # Linux deps + cuda feature target-conditional
.github/workflows/macos-check.yml  # NEW: cargo check --target x86_64-apple-darwin
CLAUDE.md                          # documents the Platform Isolation contract
```

## Existing Patterns

Investigation found that Subtidal's existing module boundaries are well-suited to platform isolation. The `OverlayCommand` enum, `AudioCommand` enum, `SttEngine` trait, and channel-based thread communication (`async_channel::Sender`, `std::sync::mpsc::SyncSender`, ringbuf consumers) were built for thread separation but inadvertently also serve as platform-separation seams: their types are already platform-neutral, so cfg-gating the implementations behind them is sufficient.

This refactor follows the `src/sys/mod.rs` re-export pattern used by [mio](https://github.com/tokio-rs/mio/blob/master/src/sys/mod.rs) and the target-conditional dependency pattern used by [cpal](https://github.com/RustAudio/cpal/blob/master/Cargo.toml) — both canonical multi-platform Rust libraries. The in-place flavor (per-module rather than centralized `src/platform/`) is chosen over the strict mio/cpal pattern to minimize churn: existing files don't move except for the overlay GTK subtree, so git blame and IDE navigation remain stable.

No existing CI configuration exists in the repository. The new `.github/workflows/macos-check.yml` is the first workflow file and follows current 2026 community standards: `dtolnay/rust-toolchain` for toolchain setup, `Swatinem/rust-cache` for build caching.

## Implementation Phases

### Phase 1: Cargo.toml Restructuring

**Goal:** Move Linux-only crates and the `cuda` feature behind target-conditional dependencies.

**Components:**
- `Cargo.toml` — `pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc` moved into `[target.'cfg(target_os = "linux")'.dependencies]`. `ort` and `parakeet-rs` appear in both `[dependencies]` (without `cuda`) and `[target.'cfg(target_os = "linux")'.dependencies]` (with `cuda`), relying on Cargo's additive feature unification.

**Dependencies:** None (first phase).

**Done when:**
- `cargo build --release` on Linux still produces a working binary.
- Runtime CUDA-available status message matches pre-refactor behavior (verified by launching the binary and observing the stderr line emitted by `cuda_status_message`).
- `cargo tree --target x86_64-apple-darwin` produces no errors and lists no `pipewire`, `gtk4`, `gtk4-layer-shell`, or `ksni` entries.

### Phase 2: build.rs Cross-Target Gate

**Goal:** Make `build.rs` correctly skip CUDA-provider scanning when cross-checking non-Linux targets.

**Components:**
- `build.rs` — wrap existing CUDA-scanning logic in `if env::var("TARGET").unwrap_or_default().contains("linux") { ... }`; early-return on non-Linux.

**Dependencies:** Phase 1.

**Done when:**
- `cargo build --release` on Linux behavior unchanged (binary built; provider .so files scanned/linked as before).
- `cargo check --target x86_64-apple-darwin` does not error out in `build.rs` execution (it will still fail downstream until later phases gate the source code, but `build.rs` itself completes cleanly).

### Phase 3: In-Place Cfg-Gating for audio/, tray/, stt/

**Goal:** Cfg-gate the three modules whose public interfaces are already platform-neutral.

**Components:**
- `src/audio/mod.rs` — becomes a gate-and-re-export shell; current body moves to `src/audio/impl_linux.rs` via `git mv`.
- `src/tray/mod.rs` — same shape: shell + `src/tray/impl_linux.rs`.
- `src/stt/mod.rs` — `mod nemotron;` declaration becomes `#[cfg(target_os = "linux")] mod nemotron;` and re-exports of Nemotron-specific items get the same gate. The `SttEngine` trait, `AudioWake`, `PipelineConfig`, and other neutral items stay unguarded.

**Dependencies:** Phase 1, Phase 2.

**Done when:**
- `cargo build --release` on Linux produces an unchanged binary.
- Runtime smoke test: launch binary, verify captions appear (audio capture + STT working), verify tray menu functional (engine display, captions toggle, source switch).
- `cargo check --target x86_64-apple-darwin` on these three modules in isolation (verifiable by temporarily commenting out other Linux-bound modules in `main.rs`) does not report cfg-leak errors.

### Phase 4: In-Place Cfg-Gating for overlay/

**Goal:** Move the GTK subtree behind a single cfg-gate.

**Components:**
- `src/overlay/mod.rs` — keeps `OverlayCommand` enum and other neutral items; the `mod window; mod drag; mod input_region; mod transcript_window;` declarations move under a new `#[cfg(target_os = "linux")] mod linux;` with re-exports.
- `src/overlay/linux/mod.rs` (NEW) — declares `pub mod window; pub mod drag; pub mod input_region; pub mod transcript_window;`. Contains the orchestration logic currently in `overlay/mod.rs` (the `run_gtk_app` entry point and `OverlayCommand` dispatch loop body).
- `src/overlay/linux/window.rs`, `drag.rs`, `input_region.rs`, `transcript_window.rs` — moved from `src/overlay/` via `git mv` to preserve history; imports updated for new module paths.
- `src/overlay/caption_buffer.rs` and `src/overlay/transcript_log.rs` — unchanged.

**Dependencies:** Phase 3.

**Done when:**
- `cargo build --release` on Linux produces an unchanged binary.
- Runtime smoke test across all three overlay modes:
  - Docked mode: caption text appears at configured screen edge.
  - Floating mode: drag works without jitter; lock state behaves correctly.
  - Transcript mode: timestamped paragraphs accumulate in scrollable window; Save dialog functions.
  - Tray-driven toggles function: above-fullscreen, locked, mode-switch all live-apply.
- `git log --follow src/overlay/linux/window.rs` shows pre-refactor history of the file.

### Phase 5: main.rs Split, compile_error! Guard, CI, CLAUDE.md

**Goal:** Complete the refactor with the final orchestration changes, install the verification mechanism, and document the architecture.

**Components:**
- `src/main_linux.rs` (NEW) — contains `cuda_available`, `run_cuda_probe`, `reexec_with_absolute_argv0_if_needed`, `ensure_provider_libs_next_to_exe`, `cuda_status_message`, `exit_without_atexit`, and the inline `#[cfg(test)] mod tests` for `cuda_status_message`. All `use std::os::unix::*` and `libc` usage is contained here.
- `src/main.rs` — top-of-file `#[cfg(not(target_os = "linux"))] compile_error!("Subtidal currently only supports Linux. macOS support is planned.");` after `mod` declarations; adds `#[cfg(target_os = "linux")] mod main_linux;` and `#[cfg(target_os = "linux")] use main_linux::{...};`. Body of `main()` calls these as before.
- `.github/workflows/macos-check.yml` (NEW) — single job on `ubuntu-latest`: checkout, install Rust stable with `x86_64-apple-darwin` target, restore Swatinem cache, run `cargo check --target x86_64-apple-darwin --verbose`.
- `CLAUDE.md` — new "## Platform Isolation" section (~20 lines) describing the cfg-gating contract, the CI verification mechanism, and the recipe for adding a new platform.

**Dependencies:** Phase 4.

**Done when:**
- `cargo build --release` on Linux produces an unchanged binary; full runtime smoke test passes.
- `cargo check --target x86_64-apple-darwin` from a Linux host (with `rustup target add x86_64-apple-darwin`) passes with zero errors.
- The CI workflow runs green on first push of the branch.
- `CLAUDE.md` section explains where the cfg-gates live, what `cargo check --target x86_64-apple-darwin` verifies, and what a future macOS port would add.

## Additional Considerations

**Cargo feature unification verification.** The two-entry pattern for `ort` and `parakeet-rs` (base entry + target-conditional entry with `cuda`) is correct per Cargo's documented feature unification, but a typo in feature names or version mismatch would silently strip CUDA on Linux. Phase 1's "binary prints CUDA-available status" check is the catch.

**git mv discipline.** Phases 3 and 4 move existing files. Use `git mv` rather than delete-and-recreate to preserve `git blame` and `git log --follow` history. The implementation plan should specify this explicitly per file.

**compile_error! placement order.** Placing the `compile_error!` before `mod` declarations causes confusing cascading errors (rustc analyzes mod declarations first and may emit unrelated complaints). Place it after all `mod` declarations but before any non-mod logic.

**`glib` crate left cross-platform.** The Rust `glib` crate is pure-Rust GObject bindings and works on macOS in principle. It is used only by Linux overlay code today; gating it would be defensible but pointless — the cfg-gated callers won't compile on macOS anyway, so leaving `glib` cross-platform keeps Cargo.toml smaller without enabling any extra macOS surface.

**CI caching cost.** First push runs the macOS-target check cold-compiles all cross-platform crates (~3–8 minutes on a small project). Subsequent runs hit Swatinem cache and complete in under a minute. Acceptable.

**aarch64-apple-darwin as future addition.** The workflow targets `x86_64-apple-darwin` only. Apple Silicon (`aarch64-apple-darwin`) is the platform someone would actually develop on, but for cfg-gating verification the architecture doesn't matter — both targets resolve cfg attributes identically. Adding an `aarch64-apple-darwin` matrix entry is a one-line workflow change if desired later.

