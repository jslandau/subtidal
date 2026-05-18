# macOS Port — Phase 7: Polish + CI matrix + AC validation

**Goal:** Extend cross-target CI to cover both Apple Silicon and Intel macOS targets, walk every Acceptance Criterion against the running `.app` and record results, document discovered macOS-specific landmines for the user's auto-memory, and refresh `CLAUDE.md` so it accurately describes the cross-platform (Linux + macOS) codebase rather than "Linux currently, macOS planned."

**Architecture:** Three artifacts move: (1) `.github/workflows/macos-check.yml` gains an `aarch64-apple-darwin` matrix entry alongside the existing `x86_64-apple-darwin` check so any future Linux-coupling regression in either direction breaks CI; (2) a transient `docs/macos-port-ac-results.md` walkthrough checklist tracks live AC validation on the target Mac and is deleted before merge (it is purely a tracking artifact, not a deliverable); (3) `CLAUDE.md` is rewritten in place to describe a Linux+macOS codebase — file map gets a `_macos` column, Platform Isolation gains a macOS example, the "Recipe for adding a new platform" prose becomes a "Platform implementations: Linux and macOS" section, and the Freshness date is bumped. Landmines discovered during Phases 0–6 (Metal VRAM pooling, parakeet-rs WebGPU empirics, TCC re-prompt scenarios, anything else surfaced) are emitted as memory-note prompts so the user can ingest them into auto-memory the same way `project_ort_argv0_quirk.md` and `project_gpu_cuda_landmines.md` were ingested.

**Tech Stack:** GitHub Actions matrix strategy (`strategy.matrix.target`), `dtolnay/rust-toolchain@stable` with multi-target `targets:` input, `Swatinem/rust-cache@v2`, Markdown for the AC checklist and CLAUDE.md edits, the user's auto-memory ingestion path for landmine documentation.

**Scope:** Phase 7 of 8 (final phase).

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

### macos-port.AC9: CI matrix coverage
- **macos-port.AC9.1 Success:** `cargo check --lib --target x86_64-apple-darwin` passes on `ubuntu-latest` (existing check, no regression).
- **macos-port.AC9.2 Success:** `cargo check --lib --target aarch64-apple-darwin` passes on `ubuntu-latest` (new matrix entry).
- **macos-port.AC9.3 Failure:** Accidentally introducing Linux coupling into a notionally-neutral module (e.g., importing `pipewire::*` from `audio/mod.rs`) breaks both cross-target checks.

### macos-port.AC10: Documentation and self-containment
- **macos-port.AC10.1 Success:** `CLAUDE.md` is updated at end of Phase 7 to describe the codebase as cross-platform (Linux + macOS), not "Linux currently with macOS planned".
- **macos-port.AC10.2 Success:** This design document, read in isolation by a fresh Mac session with no prior conversation context, contains sufficient detail to execute every phase (verifiable by handing the doc to a fresh agent and observing that no clarifying questions about the design itself are needed — only codebase-state questions).
- **macos-port.AC10.3 Success:** Newly discovered macOS landmines (analogues of `project_ort_argv0_quirk.md` and `project_gpu_cuda_landmines.md`) are documented in a form suitable for the user's auto-memory.

### Cross-cutting verification (no new ACs introduced)
- All `macos-port.AC1.*` through `macos-port.AC8.*` criteria are re-verified end-to-end on the target Mac and the result recorded in the transient checklist (this is the operational gate before merge, not a new AC).

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Extend macos-check.yml matrix to cover both Apple targets

**Verifies:** macos-port.AC9.1, macos-port.AC9.2, macos-port.AC9.3

**Files:**
- Modify: `.github/workflows/macos-check.yml` (current contents are a single non-matrix job; rewrite to a matrix job)

**Implementation:**

The current workflow is a single-target job:

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

Replace with a matrix over both Apple Darwin targets so a Linux-coupling regression breaks at least one matrix leg regardless of which side it appears on. Keep `runs-on: ubuntu-latest` — these are cross-compile *check* jobs, not native builds, so they continue to run on the cheap Linux runner.

Final contents:

```yaml
name: macOS target check

on:
  push:
  pull_request:

jobs:
  check:
    name: cargo check (${{ matrix.target }})
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        target:
          - x86_64-apple-darwin
          - aarch64-apple-darwin
    steps:
      - uses: actions/checkout@v6

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}

      - run: cargo check --lib --target ${{ matrix.target }} --verbose
```

Notes on each change:
- `strategy.fail-fast: false` — if one target fails, still run the other so the user sees both signals on a single CI run.
- `matrix.target` listed with `x86_64` first to preserve the existing target's primacy (and so the cache hits the historical key shape first).
- `Swatinem/rust-cache@v2` gets an explicit `key:` of the target name so the two matrix legs do not stomp each other's cache.
- `dtolnay/rust-toolchain@stable` accepts `targets:` (plural) as a comma- or newline-separated list, but per-matrix-leg we pass a single target so each runner only installs what it needs.
- `name: cargo check (${{ matrix.target }})` makes the per-leg status check identifiable in PR status UIs.

**Verification (after pushing the change):**
- Open the resulting CI run on GitHub Actions.
- Confirm both legs (`cargo check (x86_64-apple-darwin)` and `cargo check (aarch64-apple-darwin)`) appear and both pass.
- Confirm `fail-fast: false` behavior: locally introduce a deliberate Linux-coupling bug (e.g., add `use pipewire as _;` to the top of `src/audio/mod.rs` outside any cfg gate), push to a throwaway branch, observe BOTH matrix legs fail, then revert the throwaway branch. This satisfies AC9.3 operationally; revert before merging anything from this phase.

**Commit:**
```
ci: cover both apple-darwin targets via matrix
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Verify the matrix in a real CI run (no code change)

**Verifies:** macos-port.AC9.1, macos-port.AC9.2

**Files:** none

**Implementation:**

After Task 1 lands on the branch, push the branch and wait for the GitHub Actions run to complete.

Required signals (capture these in the AC checklist created in Task 3):
- Both `cargo check (x86_64-apple-darwin)` and `cargo check (aarch64-apple-darwin)` checks complete green.
- Total CI wall time has not regressed grossly (a ~2× increase is expected since two legs now run; anything beyond that suggests a cache-key bug — revisit Task 1's cache `key:`).

If either leg fails, treat it as a real bug in this phase's earlier work or in some earlier phase. Common causes to investigate first:
1. A neutral module accidentally pulls in a Linux-only crate without a `#[cfg(target_os = "linux")]` gate.
2. A `build.rs` branch runs CUDA scanning despite `TARGET` being a Darwin triple (regression of the Phase 1 build.rs gate).
3. A `Cargo.toml` dep is in the unconditional `[dependencies]` table when it should be under `[target.'cfg(target_os = "linux")'.dependencies]` (or the macOS equivalent).

Fix in place and re-push. Do not paper over with a `continue-on-error: true` or a target-specific skip — the whole point of this matrix is to *catch* coupling regressions.

**Verification:** Both matrix legs green on at least one push of the final phase-7 commit set.

**Commit:** none (verification only).
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Create the transient AC walkthrough checklist and execute it

**Verifies:** all `macos-port.AC*` criteria operationally (this is the merge-gate verification pass)

**Files:**
- Create: `docs/macos-port-ac-results.md` (transient — Task 6 deletes it before merge)

**Implementation:**

Create the checklist file with one entry per AC case from the design document. Each entry has a checkbox, the literal AC text, and a "Notes" line for capturing what was observed.

Full contents (copy AC text literally from `docs/design-plans/2026-05-18-macos-port.md`):

```markdown
# macOS Port — Acceptance Criteria Walkthrough Results

**Status:** transient — delete before merging Phase 7.

**Purpose:** Live record of running every `macos-port.AC*.*` criterion against the built `.app` on the target Apple Silicon Mac. A criterion is "passed" only if the observable behavior matches the AC text verbatim.

**Test rig:**
- Hardware: <fill in M-series chip + RAM>
- macOS: <fill in version, must be ≥ 14.4>
- Bundle ID: com.subtidal.app
- Build: `cargo build --release && scripts/bundle-mac.sh`
- Launch: `open target/release/Subtidal.app`

---

## AC1: Three overlay modes function on macOS
- [ ] AC1.1 — Docked positions NSPanel at top of NSScreen.main.visibleFrame, full width. Notes:
- [ ] AC1.2 — Floating shows NSPanel at config position, draggable. Notes:
- [ ] AC1.3 — Transcript shows scrollable NSTextView, timestamped paragraphs, autoscroll-when-at-bottom. Notes:
- [ ] AC1.4 — Mode switch instant, no restart, both windows constructed once. Notes:
- [ ] AC1.5 — Empty-state Transcript Save produces valid JSON. Notes:
- [ ] AC1.6 — External display connect/disconnect re-positions Docked. Notes:

## AC2: NSPanel renders correctly across Spaces and fullscreen
- [ ] AC2.1 — Panel flags inspected: level=.floating, collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary], isFloatingPanel=true, styleMask includes .borderless+.nonactivatingPanel. Notes:
- [ ] AC2.2 — Panel visible on every Space (Mission Control verified). Notes:
- [ ] AC2.3 — Panel visible above Safari/Chrome fullscreen. Notes:
- [ ] AC2.4 — Show Above Fullscreen toggle flips level between .floating and .statusBar within one OverlayCommand cycle, no rebuild. Notes:
- [ ] AC2.5 — Docked/Floating: ignoresMouseEvents=true; clicks pass through. Notes:
- [ ] AC2.6 — Transcript: ignoresMouseEvents=false; Save button works. Notes:
- [ ] AC2.7 — Panel does not collide with menu bar. Notes:

## AC3: ScreenCaptureKit audio capture
- [ ] AC3.1 — System Output captures all system audio; video produces real-time captions. Notes:
- [ ] AC3.2 — Per-app source captures only that app's audio. Notes:
- [ ] AC3.3 — Source switch via updateContentFilter: no panel flicker, no caption gap > 1 sample. Notes:
- [ ] AC3.4 — Captured app exits: NSUserNotification posted, fallback to System Output. Notes:
- [ ] AC3.5 — First-run surfaces Screen Recording prompt with NSScreenCaptureUsageDescription text. Notes:
- [ ] AC3.6 — Refusing Screen Recording produces a user-visible error, not silent crash. Notes:
- [ ] AC3.7 — SCK callback: no allocation, no logging, try_lock only (debug-build assertion holds across a 5-minute capture). Notes:

## AC4: STT engine on macOS (WebGPU primary, CPU fallback)
- [ ] AC4.1 — NemotronEngine::new selects ExecutionProvider::WebGpu on Apple Silicon; log line confirms. Notes:
- [ ] AC4.2 — Injected WebGpu init fault triggers CPU fallback; log line confirms. Notes:
- [ ] AC4.3 — Transcript on tests/fixtures/macos-webgpu-smoke.wav matches Linux baseline (identical token sequence). Notes:
- [ ] AC4.4 — RTF on WebGPU path ≤ 1.0 on test machine. Notes:
- [ ] AC4.5 — Engine swap reads ArcSwap on next chunk boundary; no concurrent session construction. Notes:

## AC5: Tray (NSStatusItem) controls
- [ ] AC5.1 — Tray icon isTemplate=true; renders in light + dark. Notes:
- [ ] AC5.2 — Captions On/Off toggles CaptionsEnabled; checkmark reflects state. Notes:
- [ ] AC5.3 — Mode submenu lists Docked/Floating/Transcript; active checkmarked; selection posts SetMode. Notes:
- [ ] AC5.4 — Audio Source submenu populated from list_sources(); System Output + per-app entries. Notes:
- [ ] AC5.5 — Show Above Fullscreen toggle posts SetAboveFullscreen live (no rebuild). Notes:
- [ ] AC5.6 — Lock Position only enabled in Floating mode; locks drag. Notes:
- [ ] AC5.7 — Cmd-Q triggers applicationWillTerminate; all worker threads shut down cleanly. Notes:
- [ ] AC5.8 — Tray items reflect disabled state when feature unavailable (Lock Position disabled in Docked/Transcript). Notes:

## AC6: Hot-reload config
- [ ] AC6.1 — Edit ~/Library/Application Support/Subtidal/config.toml; debounced reload within ~500ms. Notes:
- [ ] AC6.2 — Drag writes do not trigger config-reload feedback loop. Notes:
- [ ] AC6.3 — Malformed TOML warned to stderr, ignored; app continues with previous config. Notes:

## AC7: TCC permission stability
- [ ] AC7.1 — Grant persists across cargo build && scripts/bundle-mac.sh && open Subtidal.app cycles. Notes:
- [ ] AC7.2 — Changing CFBundleIdentifier or signing identity invalidates grant and re-prompts (expected). Notes:

## AC8: Main-thread caption delivery and shutdown
- [ ] AC8.1 — Captions arrive via caption bridge; no AppKit thread-affinity panics. Notes:
- [ ] AC8.2 — Cmd-Q clean shutdown: SCK stopped, STT exits ≤250ms, audio/tray exit, NSApplication.run() returns, exit 0. Notes:
- [ ] AC8.3 — Force-close via Activity Monitor leaves no orphan SCK streams (lsof verified). Notes:

## AC9: CI matrix coverage (covered by Task 1+2)
- [ ] AC9.1 — x86_64-apple-darwin leg green. Notes:
- [ ] AC9.2 — aarch64-apple-darwin leg green. Notes:
- [ ] AC9.3 — Deliberate Linux-coupling regression breaks both legs (Task 2 throwaway-branch verification). Notes:

## AC10: Documentation and self-containment (covered by Tasks 4+5)
- [ ] AC10.1 — CLAUDE.md describes cross-platform codebase, not "Linux currently". Notes:
- [ ] AC10.2 — Design doc executable by a fresh Mac session (no clarifying design questions needed — only codebase-state questions). Notes:
- [ ] AC10.3 — Newly discovered macOS landmines documented for auto-memory ingestion. Notes:

---

## Merge-gate decision

- [ ] Every box above is checked.
- [ ] Any failures triggered a fix in an earlier phase (not papered-over here).
- [ ] This file is being deleted by Task 6 in the same commit set that merges Phase 7.
```

**Execution discipline:** Work through the file top-to-bottom on the target Mac. For each unchecked box, perform the verification step, observe the result, then either tick the box (and note what was observed) or leave it unticked and surface the failure to the user. Do not bulk-tick; each entry is one observation.

If any AC fails, treat it as a real bug in the originating phase (AC1.x → Phase 6, AC3.x → Phase 4/5, AC4.x → Phase 3, AC7.x → Phase 1, AC8.x → Phase 2, AC9.x → Phase 7 Task 1, AC10.x → Phase 7 Task 4/5). Fix in place. Re-run the affected entry. Do not loosen the AC.

**Verification:** Every checkbox in `docs/macos-port-ac-results.md` ticked, with a Notes line per entry, before proceeding to Task 4.

**Commit (intermediate, will be reverted by Task 6 deletion):**
```
docs: track macOS port AC walkthrough results (transient)
```
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Capture discovered macOS landmines for user auto-memory ingestion

**Verifies:** macos-port.AC10.3

**Files:** none on disk (output is a chat-ready prompt block to the user)

**Implementation:**

Phase 0–6 implementations will have surfaced macOS-specific quirks not predicted in the design — things like Metal VRAM pooling delays after `ort::Session` drop, parakeet-rs WebGPU empirical RTF vs. expectations, NSPanel `.canJoinAllSpaces` interactions with auxiliary displays, TCC re-prompt scenarios that surprised us, SCK `CMSampleBuffer` shape variants that needed normalization, `objc2`-related drop-order pitfalls, and so on.

Walk back through the per-phase commits and notes (especially any "Capture discovered macOS landmines in a memory note" instructions from Phase 6 Task 6 and similar) and assemble a single message to the user formatted exactly as auto-memory ingestion expects. The user will copy each block into their auto-memory the same way they ingested `project_ort_argv0_quirk.md` and `project_gpu_cuda_landmines.md`.

Format (emit one block per discovered landmine, with these exact field names so the user can paste them straight into the memory file format described in their auto-memory system prompt):

````
---
name: project_macos_<short-kebab-case-topic>
description: <one-line summary — used for relevance lookup in future sessions>
metadata:
  type: project
---

<Body — lead with the fact, then **Why:** (the root cause / where the user got bitten), then **How to apply:** (when/where this guidance triggers).>

Links: [[project_ort_argv0_quirk]] [[project_gpu_cuda_landmines]] (use only when a real cross-reference exists)
````

Expected landmine families (emit whichever actually occurred — do not invent ones that did not):

1. **Metal VRAM release timing on `ort::Session` drop.** Likely surfaced in Phase 3 when iterating engine init. Body should describe the observed delay and the workaround (small sleep between teardown and recreation in test loops). Cross-reference `[[project_gpu_cuda_landmines]]` since it is the macOS analogue.
2. **ORT WebGPU concurrent-session race (microsoft/onnxruntime#27592).** Likely surfaced in Phase 0 or Phase 3 if multi-threaded init was ever accidentally attempted. Body should reiterate the single-stt-thread invariant.
3. **TCC re-prompt scenarios encountered.** Likely from Phase 1 + Phase 4. Body documents which actions did vs. did not invalidate the grant during dev (Info.plist edits, codesign re-runs, in-place binary replacement, bundle rename).
4. **SCK `CMSampleBuffer` format quirks.** Likely from Phase 4 — what shapes SCK actually delivered vs. what the design assumed (48 kHz vs. 44.1 kHz on certain hardware, channel-count surprises, format tag variants).
5. **NSPanel / `.canJoinAllSpaces` / `.fullScreenAuxiliary` interaction surprises.** Likely from Phase 2 + Phase 6 — any case where the panel did not appear on a Space it should have, or behaved differently above a fullscreen Space vs. above a regular Space.
6. **`objc2-core-media` unsafe-FFI gotchas.** Likely from Phase 4 — the lack of safe wrappers for `AudioBufferList` extraction and any specific patterns we converged on.
7. **Dylib resolution / `@rpath` development workflow.** Likely from Phase 1 — when `DYLD_LIBRARY_PATH=target/release` was needed and when it was not.

Skip any family that did not actually bite us. Do not fabricate landmines to fill the list.

**Verification:**
- Each emitted block contains a real, observed (during Phases 0–6) macOS quirk.
- No fabricated entries.
- Every block parses as a valid auto-memory file (frontmatter + body, `name` is `project_macos_*`, `type: project`).
- User confirms ingestion (or explicitly declines individual blocks).

**Commit:** none (this task produces chat output for user ingestion, not repo files).
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->
<!-- START_TASK_5 -->
### Task 5: Refresh CLAUDE.md to describe the cross-platform codebase

**Verifies:** macos-port.AC10.1

**Files:**
- Modify: `CLAUDE.md` (current freshness date: 2026-05-17; current Purpose says "Real-time speech-to-text overlay for Linux/Wayland.")

**Implementation:**

The current `CLAUDE.md` describes Subtidal as a Linux/Wayland-only project. Five edits make it describe the post-Phase-7 reality. Make each edit as a separate `Edit` invocation so the diff stays reviewable.

**Edit 1: Freshness date and one-line Purpose.**

Old:
```
# Subtidal

Real-time speech-to-text overlay for Linux/Wayland.

Freshness: 2026-05-17
```

New:
```
# Subtidal

Real-time speech-to-text overlay. Linux/Wayland (PipeWire + GTK4 layer-shell) and macOS 14.4+ Apple Silicon (ScreenCaptureKit + AppKit).

Freshness: 2026-05-18
```

**Edit 2: Purpose section.**

Old:
```
## Purpose

Captures system or per-application audio via PipeWire, runs local STT inference (Nemotron GPU or CPU), and displays live captions in a GTK4 layer-shell overlay with system tray controls.
```

New:
```
## Purpose

Captures system or per-application audio (PipeWire on Linux, ScreenCaptureKit on macOS), runs local STT inference (Nemotron — CUDA on Linux, WebGPU/Metal on macOS, CPU fallback on both), and displays live captions in a platform-native overlay (GTK4 layer-shell on Linux, NSPanel on macOS) with a system-tray (ksni on Linux, NSStatusItem on macOS).
```

**Edit 3: File map — extend with macOS columns.**

The current Architecture file map lists `*_linux` files. Expand it so each platform-bound subsystem shows both implementations. Locate the file-map fenced block and replace it with:

```
lib.rs                        — library crate root; re-exports modules for cross-target `cargo check --lib`
main.rs                       — bin entry point: CLI args + per-platform dispatch + `compile_error!` guard for unsupported OSes
main_linux.rs                 — Linux startup orchestration (CUDA probe/reexec helpers, thread wiring)
main_macos.rs                 — macOS startup orchestration (block_on tokio model download, MainThreadMarker acquire, NSApplication.run())
config.rs                     — TOML config with hot-reload; per-platform config path (XDG on Linux, ~/Library/Application Support on macOS)
models/mod.rs                 — HuggingFace model download; per-platform models dir
audio/mod.rs                  — neutral shell; re-exports impl_linux on Linux, impl_macos on macOS
audio/impl_linux.rs           — PipeWire capture thread, node enumeration, source switching
audio/impl_macos.rs           — ScreenCaptureKit capture, SCShareableContent enumeration, SCStream.updateContentFilter switching
audio/resampler.rs            — rubato 48kHz stereo -> 16kHz mono (platform-neutral)
stt/mod.rs                    — SttEngine trait + AudioWake (neutral) + per-platform spawn_stt_thread / build_engine / `mod nemotron`
stt/nemotron.rs               — Nemotron RNNT engine (ort + parakeet-rs); CUDA on Linux, WebGPU on macOS, CPU fallback on both
overlay/mod.rs                — neutral: OverlayCommand, CaptionsEnabled; re-exports overlay/linux or overlay/macos
overlay/caption_buffer.rs     — pure line-fill buffer (neutral)
overlay/transcript_log.rs     — pure timestamped fragments + JSON serialization (neutral)
overlay/linux/...             — GTK4 layer-shell window, drag, input region, transcript window
overlay/macos/...             — NSPanel construction, drag (isMovableByWindowBackground), NSWindow+NSScrollView transcript window
tray/mod.rs                   — neutral shell; re-exports impl_linux or impl_macos
tray/impl_linux.rs            — ksni StatusNotifierItem
tray/impl_macos.rs            — NSStatusItem + NSMenu
resources/macos/Info.plist    — bundle plist (CFBundleIdentifier=com.subtidal.app, TCC usage descriptions)
resources/macos/tray-icon-template.png — 22×22 monochrome template icon
scripts/bundle-mac.sh         — builds, wraps in .app, ad-hoc codesigns
```

Use a single `Edit` with the entire current file-map block as `old_string` and the block above as `new_string`. If the diff fails to match, read the file fresh and re-issue.

**Edit 4: Thread Model section — add macOS counterpart.**

Append a new sub-section immediately after the Linux thread model:

```
**macOS thread model (inverted because AppKit owns the main thread):**

1. **Main thread** — NSApplication.run(); MainThreadMarker held; SCK delegate callbacks and NSMenu actions land here.
2. **screen-capture-audio** — owns the SCStream; SCK's internal dispatch queue pushes f32 PCM into the ring buffer and calls `AudioWake::notify()`. Same RT-safety discipline as the PipeWire callback.
3. **stt-pipeline** — identical to Linux. Must stay single-threaded (ORT WebGPU concurrent-session race, microsoft/onnxruntime#27592).
4. **Tray** — NSMenu actions fire on the main thread (AppKit guarantee); no separate tray thread is needed beyond the NSStatusItem object that lives on Main.
5. **Caption bridge** — blocks on `Receiver<String>::recv()` and marshals each caption to the main thread via `dispatch::Queue::main().exec_async`. Direct analogue of GTK's `glib::MainContext::spawn_local`.
```

**Edit 5: Replace "Recipe for adding a new platform" with "Platform implementations: Linux and macOS".**

The Platform Isolation section currently ends with a numbered "Recipe for adding a new platform (e.g., macOS)" list. Replace that subsection with:

```
**Platform implementations: Linux and macOS.**

The cfg-gating patterns above are realized by two concrete platform implementations:

- **Linux:** `impl_linux.rs` / `linux/` subtrees under `audio/`, `overlay/`, `tray/`; `main_linux.rs`; `[target.'cfg(target_os = "linux")'.dependencies]` enables `pipewire`, `gtk4`, `gtk4-layer-shell`, `ksni`, `libc`, and the `cuda` feature on `ort` + `parakeet-rs`.
- **macOS:** `impl_macos.rs` / `macos/` subtrees under the same; `main_macos.rs`; `[target.'cfg(target_os = "macos")'.dependencies]` enables `objc2`, `objc2-foundation`, `objc2-app-kit`, `objc2-screen-capture-kit`, `objc2-core-media`, `objc2-core-foundation`, `dispatch`, and the `webgpu` feature on `ort` + `parakeet-rs`.

`build.rs` early-returns on non-Linux targets (CUDA scanning is Linux-only); macOS uses `@rpath` / `@loader_path` for dylib resolution and needs no equivalent. The `compile_error!` guard in `src/main.rs` fires only on OSes other than Linux and macOS.

To add a third platform (e.g., Windows):
1. Refine the `compile_error!` predicate to exclude the new OS.
2. Add `impl_<os>.rs` (or `<os>/` subtree) siblings under `audio/`, `overlay/`, `tray/`.
3. Add a `[target.'cfg(target_os = "<os>")'.dependencies]` block.
4. Add `src/main_<os>.rs` and the corresponding `#[cfg(target_os = "<os>")] mod main_<os>;`.
5. Extend the `.github/workflows/macos-check.yml` matrix (or rename it) to include the new target.
```

**Verification:**
```bash
# Read the resulting CLAUDE.md and confirm:
grep -c "macOS" CLAUDE.md   # > 0
grep "Freshness: 2026-05-18" CLAUDE.md   # exactly one match
grep -c "main_macos.rs" CLAUDE.md   # ≥ 1
grep -c "impl_macos" CLAUDE.md   # ≥ 1
# And cross-target check still passes:
cargo check --lib --target x86_64-apple-darwin
```

**Commit:**
```
docs: CLAUDE.md describes cross-platform codebase (Linux + macOS)
```
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Optional CHANGELOG entry; delete transient AC checklist; final commit

**Verifies:** none directly (cleanup) — gates the merge.

**Files:**
- Modify (only if present): `CHANGELOG.md` (verified by `ls CHANGELOG.md` at start of task)
- Delete: `docs/macos-port-ac-results.md` (created by Task 3)

**Implementation:**

**Step 1: CHANGELOG (only if the file already exists).**

```bash
ls CHANGELOG.md 2>/dev/null
```

If the command exits non-zero (file does not exist), skip Step 1 entirely. The design document explicitly conditions this on "if present"; do not create a new `CHANGELOG.md`.

If `CHANGELOG.md` exists, read it to determine the conventions in use (Keep a Changelog format vs. project-specific). Add an entry at the top of the most recent unreleased section (or under a new `## [Unreleased]` heading if the project uses one):

```
### Added
- macOS 14.4+ Apple Silicon support: ScreenCaptureKit audio capture, NSPanel/NSWindow overlay across all three modes (Docked / Floating / Transcript), NSStatusItem tray, WebGPU (Metal) Nemotron with CPU fallback, hot-reload config from `~/Library/Application Support/Subtidal/config.toml`.
```

Match the surrounding entries' tense and punctuation style. If `CHANGELOG.md` uses a different section name than `Added` (e.g., `### New features`), use the project's convention.

**Step 2: Delete the transient AC checklist.**

```bash
rm docs/macos-port-ac-results.md
```

This file existed only to gate the merge; it is not a deliverable. Verify:

```bash
test ! -e docs/macos-port-ac-results.md && echo "removed"
```

**Step 3: Final cross-target check.**

```bash
cargo check --lib --target x86_64-apple-darwin --verbose
```

Must complete without errors or warnings introduced by this phase.

**Step 4: Commit.**

If Step 1 ran:
```
docs: changelog entry for macOS support; remove transient AC checklist
```

If Step 1 was skipped:
```
docs: remove transient macOS port AC checklist
```

**Verification:**
- `docs/macos-port-ac-results.md` does not exist.
- `cargo check --lib --target x86_64-apple-darwin` is green on the final commit.
- Both CI matrix legs (Task 1) report green on this commit.
- Every box in the (now deleted) AC checklist was ticked before deletion (Task 3 produced no unchecked entries).

**Commit:** as above.
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_C -->
