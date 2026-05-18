# Test Requirements: linux-backend-isolation

## Overview

Maps every acceptance criterion (AC1.1 through AC5.3) to either an automated verification command or a documented human verification procedure. Every AC has exactly one entry. Failure-mode ACs (AC1.4, AC2.3, AC3.3, AC4.4, AC5.3) do not have their own positive verification — they act as acceptance gates that block the phase commit if any of their group's success ACs fails.

All commands assume working directory `/home/jslandau/git/live_text` unless otherwise noted.

---

## AC1: Linux binary behavior preserved

### AC1.1 (success): `cargo build --release` on Linux exits 0
- **Type:** automated
- **Command:**
  ```bash
  cargo build --release
  ```
- **Expected:** exit code 0; `target/release/subtidal` exists.
- **Verified in phase:** every phase's Verification block runs this as the first regression check. Final pass at Phase 5 Task 2.

### AC1.2 (success): CUDA stderr message matches pre-refactor baseline
- **Type:** automated (with one-time human baseline capture)
- **Setup (once, before Phase 1):**
  ```bash
  git stash
  cargo build --release 2>&1 >/dev/null
  ./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider' > /tmp/cuda_baseline.txt
  # Ctrl-C the binary after the CUDA line appears.
  git stash pop
  ```
- **Verification command (each phase):**
  ```bash
  diff /tmp/cuda_baseline.txt <(./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider')
  ```
- **Expected:** no diff. Same CUDA-availability or fallback line as pre-refactor.
- **Verified in phase:** Phases 1, 2, 3 (post-stt edit), and 5 Task 1.

### AC1.3 (success): Full UI smoke test passes
- **Type:** human
- **Justification for human verification:** Requires a live Wayland session, real PipeWire audio capture, GTK4 + layer-shell rendering, a system tray, and human eyes on overlay positioning/drag fluidity. None of this is reproducible in headless CI; mocking would not exercise the actual platform-bound code paths the refactor is supposed to preserve.
- **Procedure:**
  1. Launch the binary: `./target/release/subtidal &`
  2. Within 10 seconds, play a YouTube video (or speak into the captured source). **Verify:** captions appear in the overlay within ~5 seconds.
  3. Right-click the system tray icon. **Verify:** menu shows current engine and current audio source.
  4. Tray > Audio Source > pick a different source. **Verify:** captions continue and track the new source. No panic in stderr.
  5. Tray > Captions > toggle off, then on. **Verify:** captions hide, then resume.
  6. Tray > Overlay > Mode > Floating. **Verify:** overlay re-renders as floating. Drag it with left-click. **Verify:** no jitter, no snap-back, no relayout artifacts.
  7. Tray > Overlay > Locked. **Verify:** drag becomes inert.
  8. Tray > Overlay > Mode > Transcript. **Verify:** transcript window opens; timestamped paragraphs accumulate as audio plays.
  9. Click Save in the transcript window. **Verify:** a `.json` sidecar is written to the chosen location and is non-empty (`wc -c` > 0; opens as valid JSON).
  10. Tray > Overlay > Show Above Fullscreen. Open a fullscreen browser video. **Verify:** overlay renders on top of the fullscreen client.
- **Verified in phase:** Phase 3 (audio + tray + STT portions), Phase 4 (overlay drag, transcript Save, mode switch — full AC1.3), Phase 5 Task 2 (final regression).

### AC1.4 (failure gate): regression in AC1.1–AC1.3 blocks the phase
- **Type:** N/A (gate, not a positive verification)
- **Procedure:** If `cargo build --release` fails, the CUDA stderr line diverges from baseline, or any AC1.3 sub-step regresses (captions don't appear, tray missing, drag jitters, Save produces empty .json), do **not** commit. Run `git restore --staged . && git restore .` (or `git reset --hard HEAD` if no commit has landed yet) and re-investigate. The phase is incomplete until AC1.1, AC1.2, and AC1.3 all pass cleanly.

---

## AC2: macOS-target cargo check passes from Linux

### AC2.1 (success): `cargo check --lib --target x86_64-apple-darwin` exits 0
- **Type:** automated
- **Prerequisite:** `rustup target add x86_64-apple-darwin` (Phase 1 Task 1).
- **Command:**
  ```bash
  cargo check --lib --target x86_64-apple-darwin
  echo "Exit: $?"
  ```
- **Expected (final state, after Phase 5):** `Exit: 0` and no `error:` lines.
- **Intermediate states:**
  - Phase 1: still fails (no cfg-gating yet); only the `[lib]` target is in place.
  - Phase 2: still fails; only `build.rs` portion clean. Confirm with: `cargo check --target x86_64-apple-darwin 2>&1 | grep -E 'build-script-build|build\.rs'` returns zero error lines.
  - Phase 3: fails only on `src/overlay/`. Confirm: `cargo check --lib --target x86_64-apple-darwin 2>&1 | grep -E '\bort\b|parakeet|libc::|pipewire|ksni|stt/(mod|nemotron)\.rs|audio/(mod|impl_linux)\.rs|tray/(mod|impl_linux)\.rs'` returns zero matches.
  - Phase 4: full pass (exit 0).
- **Verified in phase:** Phase 4 Task 1 (first full pass); Phase 5 Task 2 (final, after `compile_error!` lands).

### AC2.2 (success): `cargo tree --target x86_64-apple-darwin` excludes Linux crates
- **Type:** automated
- **Command:**
  ```bash
  cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "FAIL" || echo "OK"
  ```
- **Expected:** prints `OK` (no Linux-crate entries in the macOS-target graph).
- **Verified in phase:** Phase 1 Task 2 (first pass); regression-checked in Phases 2, 3, 4, 5.

### AC2.3 (failure gate): unresolved-import or cannot-find-type errors block the phase
- **Type:** N/A (gate)
- **Procedure:** If `cargo check --lib --target x86_64-apple-darwin` reports any `unresolved import` or `cannot find type` error for a Linux-specific symbol after the phase's planned edits, a cfg-gate is missing. Investigate the offending file and add the appropriate `#[cfg(target_os = "linux")]` gate. Do not declare the phase done until AC2.1 passes at its expected intermediate state.

---

## AC3: CI workflow runs and passes

### AC3.1 (success): workflow file exists with correct shape
- **Type:** automated
- **Commands:**
  ```bash
  test -f .github/workflows/macos-check.yml && echo "exists"
  python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/macos-check.yml"))' && echo "yaml-valid"
  grep -q 'runs-on: ubuntu-latest' .github/workflows/macos-check.yml && echo "ubuntu"
  grep -q 'dtolnay/rust-toolchain@stable' .github/workflows/macos-check.yml && echo "toolchain-action"
  grep -q 'x86_64-apple-darwin' .github/workflows/macos-check.yml && echo "target"
  grep -q 'Swatinem/rust-cache@v2' .github/workflows/macos-check.yml && echo "cache"
  grep -q 'cargo check --lib --target x86_64-apple-darwin' .github/workflows/macos-check.yml && echo "check-cmd"
  ```
- **Expected:** all seven lines print (`exists`, `yaml-valid`, `ubuntu`, `toolchain-action`, `target`, `cache`, `check-cmd`).
- **Verified in phase:** Phase 5 Task 3.

### AC3.2 (success): workflow runs green on first push within 10 minutes
- **Type:** human (observation of GitHub Actions output)
- **Justification for human verification:** Requires pushing the branch to GitHub and observing the Actions UI / `gh run view` output. Cannot be performed locally; the workflow trigger and runner allocation are GitHub-side. The "under 10 minutes" wall-clock budget is an external CI scheduling fact, not a property checkable from the working tree.
- **Procedure:**
  1. Push the branch: `git push -u origin <branch-name>`.
  2. Within ~1 minute, run `gh run list --workflow=macos-check.yml --limit=3`. **Verify:** the latest run is `queued` or `in_progress` for the pushed commit.
  3. Wait for completion. Run `gh run view <run-id>`. **Verify:** conclusion is `success`.
  4. Record the run duration (visible in `gh run view`). **Verify:** under 10 minutes for the cold first run.
  5. (Optional) push a no-op commit to confirm `Swatinem/rust-cache@v2` hits — the second run should complete in under 1 minute.
- **Verified in phase:** Phase 5 Task 3 (post-push).

### AC3.3 (failure gate): malformed workflow or red first run blocks the phase
- **Type:** N/A (gate)
- **Procedure:** If `yaml.safe_load` raises an error, any of the AC3.1 grep checks miss, or the first push results in a red run (whether due to YAML parse error, action-version typo, or the `cargo check --lib` itself failing on the runner), the phase is not done. Fix the underlying issue and re-push; AC3.2 must be re-observed for the new run.

---

## AC4: CUDA features are Linux-conditional

### AC4.1 (success): Cargo.toml structure is correct
- **Type:** automated
- **Commands:**
  ```bash
  grep -q "^\[target\.'cfg(target_os = \"linux\")'\.dependencies\]" Cargo.toml && echo "target-block"
  grep -A20 "^\[target\.'cfg(target_os = \"linux\")'\.dependencies\]" Cargo.toml | grep -E '^ort = .*"cuda"' && echo "ort-cuda-in-linux-block"
  grep -A20 "^\[target\.'cfg(target_os = \"linux\")'\.dependencies\]" Cargo.toml | grep -E '^parakeet-rs = .*"cuda"' && echo "parakeet-cuda-in-linux-block"
  # And the base [dependencies] entries must NOT carry the cuda feature.
  awk '/^\[dependencies\]/,/^\[/' Cargo.toml | grep -E '^ort = ' | grep -v cuda && echo "ort-base-no-cuda"
  awk '/^\[dependencies\]/,/^\[/' Cargo.toml | grep -E '^parakeet-rs = ' | grep -v cuda && echo "parakeet-base-no-cuda"
  ```
- **Expected:** all five lines print.
- **Verified in phase:** Phase 1 Task 2.

### AC4.2 (success): `ort` has no `cuda` feature on macOS target
- **Type:** automated
- **Command:**
  ```bash
  cargo tree --target x86_64-apple-darwin -e features --package ort | head -3
  ```
- **Expected:** an `ort vX.Y.Z` line WITHOUT `(cuda)` annotation or `cuda` feature listed. Confirm with: `cargo tree --target x86_64-apple-darwin -e features --package ort | head -3 | grep -q cuda && echo FAIL || echo OK` prints `OK`.
- **Verified in phase:** Phase 1 Task 2; regression-checked in Phase 4 Task 1.

### AC4.3 (success): `ort` has the `cuda` feature on Linux target
- **Type:** automated
- **Command:**
  ```bash
  cargo tree --target x86_64-unknown-linux-gnu -e features --package ort | head -3
  ```
- **Expected:** an `ort vX.Y.Z` line that includes `cuda` (either as a `(cuda)` annotation or in a feature list). Confirm with: `cargo tree --target x86_64-unknown-linux-gnu -e features --package ort | head -3 | grep -q cuda && echo OK || echo FAIL` prints `OK`. Same check applies to `parakeet-rs` (substitute `--package parakeet-rs`).
- **Verified in phase:** Phase 1 Task 2; regression-checked in Phase 4 Task 1.

### AC4.4 (failure gate): Linux CUDA regression blocks the phase
- **Type:** N/A (gate; subsumed by AC1.2's baseline-diff check)
- **Procedure:** If the post-refactor binary's stderr reports "CUDA unavailable" when the pre-refactor baseline reported CUDA available, feature unification was misconfigured (typo in feature name, version mismatch between the two `ort`/`parakeet-rs` entries, or the target-conditional block was placed wrong). Roll back the Cargo.toml edit and re-investigate. AC1.2's `diff /tmp/cuda_baseline.txt ...` check is the catch.

---

## AC5: Architectural intent documented

### AC5.1 (success): CLAUDE.md has a Platform Isolation section
- **Type:** automated
- **Commands:**
  ```bash
  grep -q '^## Platform Isolation' CLAUDE.md && echo "section-exists"
  # The section must name all four required elements: cfg-gating convention,
  # verification mechanism (CI check), recipe for new platform, compile_error! location.
  awk '/^## Platform Isolation/,/^## /' CLAUDE.md | grep -qE 'cfg\(target_os' && echo "cfg-convention"
  awk '/^## Platform Isolation/,/^## /' CLAUDE.md | grep -qE 'macos-check\.yml|cargo check.*x86_64-apple-darwin' && echo "verification-mechanism"
  awk '/^## Platform Isolation/,/^## /' CLAUDE.md | grep -qiE 'recipe|adding a new platform|new platform' && echo "recipe"
  awk '/^## Platform Isolation/,/^## /' CLAUDE.md | grep -qE 'compile_error!' && echo "compile-error-location"
  ```
- **Expected:** all five lines print (`section-exists`, `cfg-convention`, `verification-mechanism`, `recipe`, `compile-error-location`).
- **Verified in phase:** Phase 5 Task 4.

### AC5.2 (success): CLAUDE.md Freshness date updated to today
- **Type:** automated
- **Command:**
  ```bash
  grep '^Freshness:' CLAUDE.md
  ```
- **Expected:** `Freshness: 2026-05-17` (the design plan's date; matches the "today" at the time the refactor lands).
- **Verified in phase:** Phase 5 Task 4.

### AC5.3 (failure gate): documentation missing or inaccurate blocks the phase
- **Type:** N/A (gate)
- **Procedure:** If any AC5.1 grep misses, the Freshness date is stale, or the section describes structures that don't match what was built (e.g., references `src/platform/` when the actual layout is `src/audio/impl_linux.rs`, `src/overlay/linux/`, etc.), the phase is not done. Re-read the section against the final source tree (`tree src -L 2`) and reconcile.

---

## Phase-to-AC matrix

| Phase | Primary ACs verified | Regression checks |
|-------|----------------------|-------------------|
| 1     | AC4.1, AC4.2, AC4.3, AC1.1, AC1.2 | AC2.2 (partial) |
| 2     | AC2.1 (build.rs portion) | AC1.1, AC1.2, AC2.2, AC4.* |
| 3     | AC1.3 (audio/tray/STT portions), AC2.1 (partial) | AC1.1, AC1.2, AC2.2, AC4.* |
| 4     | AC1.3 (overlay portion), AC2.1 (full) | AC1.1, AC1.2, AC2.2, AC4.* |
| 5     | AC3.1, AC3.2, AC5.1, AC5.2; AC2.1 (final, with `compile_error!`) | All previous |

The failure-mode ACs (AC1.4, AC2.3, AC3.3, AC4.4, AC5.3) are not in the matrix because they do not have their own positive verification — they apply continuously across all phases as commit-blocking gates.
