# Nemotron CPU Fallback Implementation Plan — Phase 2

**Goal:** Delete the orphaned moonshine.rs source file and remove the tokenizers dependency.

**Architecture:** After Phase 1 removed `pub mod moonshine;` from stt/mod.rs, the moonshine.rs file is no longer compiled. Delete it and remove the tokenizers crate (its only consumer).

**Tech Stack:** Rust, Cargo

**Scope:** 5 phases (this is phase 2). Covers design phase 1 (file deletion + dependency removal).

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements:

### nemotron-cpu-fallback.AC1: Moonshine code fully removed
- **nemotron-cpu-fallback.AC1.1 Success:** `stt/moonshine.rs` deleted, `tokenizers` crate removed, project builds

---

<!-- START_TASK_1 -->
### Task 1: Delete moonshine.rs and remove tokenizers dependency

**Verifies:** nemotron-cpu-fallback.AC1.1

**Files:**
- Delete: `src/stt/moonshine.rs`
- Modify: `Cargo.toml:59-60` (remove tokenizers dep + comment)

**Implementation:**

**1a.** Delete the file:
```bash
rm src/stt/moonshine.rs
```

**1b.** Remove lines 59-60 from `Cargo.toml`:
```toml
# Moonshine tokenizer
tokenizers = "0.20"
```

**Verification:**
Run: `cargo build 2>&1 | tail -3`
Expected: Build succeeds

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

**Commit:**
```bash
git add -u src/stt/moonshine.rs Cargo.toml
git commit -m "chore: delete moonshine.rs and remove tokenizers dependency

stt/moonshine.rs is no longer compiled (pub mod removed in previous commit).
tokenizers 0.20 was only used by the Moonshine engine."
```
<!-- END_TASK_1 -->
