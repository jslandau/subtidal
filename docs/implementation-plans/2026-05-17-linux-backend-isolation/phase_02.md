# Phase 2: build.rs Cross-Target Gate Implementation Plan

**Goal:** Make `build.rs` early-return on non-Linux compilation targets so that `cargo check --target x86_64-apple-darwin` from a Linux host no longer scans for CUDA provider `.so` files that don't exist on the macOS target.

**Architecture:** Single-file edit to `build.rs`. Wrap the body of `fn main()` in an early-return gated on the `TARGET` environment variable. Crucially this uses `env::var("TARGET")`, NOT `cfg!(target_os = "linux")` — the latter reflects the HOST OS (where the compiler runs), not the cross-compilation target, and would silently fail to skip the scan during `cargo check --target x86_64-apple-darwin` from a Linux host. The existing helper `scan_ort_cache` already uses `env::var("TARGET")` (line 58), so the pattern is consistent with existing code.

**Tech Stack:** Rust 2021 edition build script. No new dependencies.

**Scope:** Phase 2 of 5.

**Codebase verified:** 2026-05-17 via codebase-investigator and direct read. Verified: `build.rs` is 82 lines; `fn main()` is at lines 6–24; it reads `OUT_DIR` and emits two `cargo:rustc-env` directives (`ORT_PROVIDER_LIB_DIR` and `ORT_DIST_HASH`); helpers `find_ort_provider_dir` (lines 26–48), `dirs_for_build` (50–55), and `scan_ort_cache` (57–81) are all only reachable from inside `main()`. `scan_ort_cache` already reads `env::var("TARGET")` at line 58, confirming the project already follows the TARGET-not-cfg pattern.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### linux-backend-isolation.AC2: macOS-target cargo check passes from Linux (build.rs portion)
- **linux-backend-isolation.AC2.1 Success (partial):** After this phase, the `build.rs` execution under `cargo check --target x86_64-apple-darwin` exits cleanly with no errors and no warnings attributable to `build-script-build`. The overall `cargo check` will still fail downstream on `src/` files until Phases 3–5; that is expected and outside this phase's scope.

### linux-backend-isolation.AC1: Linux binary behavior preserved (regression-only scope this phase)
- **linux-backend-isolation.AC1.1 Success:** `cargo build --release` on Linux still exits 0 and produces `target/release/subtidal`.
- **linux-backend-isolation.AC1.2 Success:** The post-edit binary still emits the same CUDA-availability stderr line as the Phase 1 build.

(Full AC2 completes only after Phases 3–5 land. This phase only closes the `build.rs`-attributed failure.)

---

<!-- START_TASK_1 -->
### Task 1: Wrap `build.rs::main()` body in a `TARGET`-OS gate

**Type:** Infrastructure.

**Verifies:** linux-backend-isolation.AC2.1 (build.rs portion), linux-backend-isolation.AC1.1 (regression).

**Files:**
- Modify: `/home/jslandau/git/live_text/build.rs` (replace `fn main()` at lines 6–24).

**Implementation:**

Replace `fn main()` (currently lines 6–24) with the version below. Helper functions (`find_ort_provider_dir`, `dirs_for_build`, `scan_ort_cache` at lines 26–81) are left UNCHANGED — they remain compiled as part of the build script but are unreachable on non-Linux targets, which is fine. Do not move them, do not gate them with `#[cfg]`, do not delete them.

```rust
fn main() {
    // `cfg!(target_os = "linux")` here would check the HOST OS, not the target.
    // Read the TARGET env var (set by Cargo for build scripts) to detect the
    // actual compilation target so `cargo check --target x86_64-apple-darwin`
    // from a Linux host correctly skips CUDA-provider scanning.
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("linux") {
        return;
    }

    // ort-sys downloads ORT binaries into the cargo build cache under OUT_DIR.
    // The copy-dylibs feature symlinks provider .so files into target/{profile}/,
    // but `cargo install` only copies the final binary. We find the actual .so
    // directory from the build cache and embed it so the binary can locate providers
    // at runtime regardless of install location.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let ort_lib_dir = find_ort_provider_dir(&out_dir);
    if let Some(dir) = ort_lib_dir {
        println!("cargo:rustc-env=ORT_PROVIDER_LIB_DIR={}", dir.display());
        // The final path component of ORT_PROVIDER_LIB_DIR is ort-sys's `dist.hash`
        // (content hash of the upstream prebuilt tarball for this target+features).
        // Embedding it lets the runtime cache scan match the *exact* distribution
        // this binary was linked against, rather than guessing by mtime.
        if let Some(hash) = dir.file_name().and_then(|s| s.to_str()) {
            println!("cargo:rustc-env=ORT_DIST_HASH={}", hash);
        }
    }
}
```

**Notes:**
- `target.contains("linux")` is the design's recommended matcher. It correctly captures `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, etc., and does NOT capture `*-apple-darwin`, `*-pc-windows-*`, or any other non-Linux triple in the canonical target-triple namespace.
- The doc comment on line 1–2 of `build.rs` ("Embeds the ort-sys provider library directory…") stays unchanged.
- The `use std::path::PathBuf;` line at line 4 stays unchanged.

**Verification:**

```bash
cd /home/jslandau/git/live_text

# 1. Linux build still works and emits the CUDA env vars.
cargo clean
cargo build --release
```
Expected: builds without errors. `target/release/subtidal` is produced.

```bash
# Confirm CUDA env vars made it into the binary by inspecting embedded strings.
strings target/release/subtidal | grep -E 'ort\.pyke\.io|libonnxruntime_providers_cuda|x86_64-unknown-linux-gnu' | head
```
Expected: at least one match against `ort.pyke.io` cache paths or the linux-gnu target name. (If empty, the build script did NOT run its body — Edit was applied incorrectly. Check the `target.contains("linux")` matcher.)

```bash
# 2. Runtime CUDA-status regression check (AC1.2).
./target/release/subtidal 2>&1 | head -20 | grep -iE 'cuda|provider' | head -3
```
Expected: same CUDA-availability or fallback message as after Phase 1. Ctrl-C the binary once you've read the line.

```bash
# 3. macOS-target build.rs no longer errors.
cargo clean
cargo check --target x86_64-apple-darwin 2>&1 | tee /tmp/macos-check.log | tail -50
```
Expected: the overall `cargo check` STILL FAILS — but the failures are now attributed to crates like `pipewire`, `gtk4`, etc. in `src/` (Phases 3–5 fix those), NOT to `build-script-build` or `build.rs`.

```bash
# Confirm no build.rs-attributed errors.
grep -E 'build-script-build|build\.rs' /tmp/macos-check.log
```
Expected: zero lines, OR only informational lines like `Compiling subtidal v0.2.2 (build script)` with no `error:` adjacent. If you see `error[E...]: ... build-script-build`, the Edit was wrong.

```bash
# 4. AC4 from Phase 1 still holds (defense in depth — confirm we didn't undo Phase 1).
cargo tree --target x86_64-apple-darwin 2>&1 | grep -E '\b(pipewire|gtk4|gtk4-layer-shell|ksni)\b' && echo "FAIL: Linux crate leaked back" || echo "OK"
```
Expected: prints `OK`.

**Commit:**

```bash
git add build.rs
git commit -m "build: gate CUDA-provider scanning on Linux TARGET env var"
```
<!-- END_TASK_1 -->

---

## Out of scope for this phase

- `src/` is untouched. `cargo check --target x86_64-apple-darwin` will still fail on `pipewire`, `gtk4`, `ksni`, etc. consumers in `src/` until Phases 3–4 gate them.
- No CI workflow (Phase 5).
- AC2 completes only after Phase 5; this phase only closes the build-script-attributed portion.
