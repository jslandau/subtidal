# Nemotron CPU Fallback Implementation Plan — Phase 5

**Goal:** Update documentation to reflect single-engine architecture and verify zero remaining moonshine references.

**Architecture:** Update CLAUDE.md, README.md, and stt/mod.rs doc comments. Run case-insensitive grep to verify complete removal.

**Tech Stack:** Markdown, Rust doc comments

**Scope:** 5 phases (this is phase 5). Covers design phase 6.

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements:

### nemotron-cpu-fallback.AC1: Moonshine code fully removed
- **nemotron-cpu-fallback.AC1.3 Success:** Case-insensitive grep for `moonshine` across codebase returns zero results (excluding design docs and git history)

---

<!-- START_TASK_1 -->
### Task 1: Update CLAUDE.md

**Verifies:** nemotron-cpu-fallback.AC1.3 (partial)

**Files:**
- Modify: `CLAUDE.md`

**Implementation:**

Make these changes to `CLAUDE.md`:

**Purpose section (line ~9):** Change "Nemotron GPU or Moonshine CPU" to "Nemotron (GPU or CPU)".

**Architecture listing:** Remove line `stt/moonshine.rs  — Moonshine encoder-decoder engine (ort, CPU)`.

**Key Contracts — Models:** Change `~/.local/share/subtidal/models/{nemotron,moonshine}/` to `~/.local/share/subtidal/models/nemotron/`.

**Key Contracts — Nemotron engine:** Update to note CPU support: "600M param RNNT model using parakeet-rs::Nemotron. Uses CUDA when available, falls back to CPU. Internally buffers 160ms chunks and emits results on 560ms boundaries."

**Dependencies:** Remove the comment about tokenizers if present, or note its removal.

**Invariants:** Change "CUDA unavailability triggers automatic fallback from Nemotron to Moonshine" to "CUDA unavailability triggers automatic fallback to CPU execution (Nemotron runs on both GPU and CPU)."

**Build & Run:** Change `[--engine nemotron|moonshine]` to `[--engine nemotron|parakeet]`. Change "CUDA optional (for Nemotron)" to "CUDA optional (GPU acceleration for Nemotron)."

**Verification:**
Run: `grep -i moonshine CLAUDE.md`
Expected: No output (zero matches)

**Commit:** Do not commit yet — bundle with Task 3
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update stt/mod.rs doc comment and test comment

**Verifies:** nemotron-cpu-fallback.AC1.3 (partial)

**Files:**
- Modify: `src/stt/mod.rs:31` (doc comment)
- Modify: `src/stt/mod.rs:145` (test comment)

**Implementation:**

**Line 31:** Change:
```rust
/// - `engine`: boxed SttEngine (Parakeet or Moonshine)
```
to:
```rust
/// - `engine`: boxed SttEngine (Nemotron via parakeet-rs)
```

**Line 145:** Change:
```rust
    /// AC5.3: CUDA unavailable triggers Moonshine fallback.
```
to:
```rust
    /// AC5.3: CUDA unavailable triggers CPU fallback.
```

**Verification:**
Run: `grep -i moonshine src/stt/mod.rs`
Expected: No output

**Commit:** Do not commit yet — bundle with Task 3
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update README.md and verify zero moonshine references

**Verifies:** nemotron-cpu-fallback.AC1.3

**Files:**
- Modify: `README.md` (3 lines with moonshine references)

**Implementation:**

**Line 9:** Change "Two STT engines: Nemotron (GPU, CUDA) for high accuracy, Moonshine (CPU, experimental)" to "**STT engine**: Nemotron (GPU via CUDA, or CPU fallback) for real-time speech recognition".

**Line 33:** Change `subtidal [--engine nemotron|moonshine]` to `subtidal [--engine nemotron|parakeet]`.

**Line 49:** Change `engine = "nemotron"           # or "moonshine"` to `engine = "nemotron"           # or "parakeet" (alias)`.

**Verification:**
Run: `grep -ri moonshine --include='*.rs' --include='*.toml' --include='*.md' . | grep -v design-plans | grep -v implementation-plans | grep -v test-plans | grep -v '.git'`
Expected: No output (zero matches outside plan docs)

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

**Commit:**
```bash
git add CLAUDE.md src/stt/mod.rs README.md
git commit -m "docs: update documentation for single-engine (Nemotron) architecture

Remove all Moonshine references from CLAUDE.md, README.md, and
stt/mod.rs doc comments. Update CLI docs to show nemotron|parakeet.
Update invariants to reflect CPU fallback instead of engine switching."
```
<!-- END_TASK_3 -->
