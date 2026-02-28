# Nemotron CPU Fallback Implementation Plan — Phase 3

**Goal:** Remove all Moonshine model management code from models/mod.rs.

**Architecture:** Delete Moonshine-specific functions, constants, and tests. The Nemotron model management code is preserved unchanged.

**Tech Stack:** Rust

**Scope:** 5 phases (this is phase 3). Covers design phase 2.

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements:

### nemotron-cpu-fallback.AC1: Moonshine code fully removed
- **nemotron-cpu-fallback.AC1.2 Success:** All Moonshine model management functions and constants removed from `models/mod.rs`

---

<!-- START_TASK_1 -->
### Task 1: Remove Moonshine model management from models/mod.rs

**Verifies:** nemotron-cpu-fallback.AC1.2

**Files:**
- Modify: `src/models/mod.rs` (remove functions, constants, tests)

**Implementation:**

Remove the following from `src/models/mod.rs`:

**Functions to delete:**
- Lines 23-27: `moonshine_model_dir()` function
- Lines 41-50: `moonshine_model_files()` function
- Lines 70-85: `moonshine_models_present_in()` and `moonshine_models_present()` functions
- Lines 98-105: `MOONSHINE_REPO` and `MOONSHINE_FILES` constants
- Lines 135-161: `ensure_moonshine_models()` async function

**Tests to delete:**
- Lines 199-203: `test_moonshine_model_dir_contains_models_dir`
- Lines 215-222: `test_moonshine_model_files_have_correct_names`
- Lines 231-235: `test_moonshine_models_present_nonexistent_returns_false`
- Lines 258-276: `test_moonshine_models_present_when_files_exist`

The remaining file should contain only: `models_dir()`, `nemotron_model_dir()`, `nemotron_model_files()`, `nemotron_models_present_in()`, `nemotron_models_present()`, `NEMOTRON_REPO`, `NEMOTRON_FILES`, `ensure_nemotron_models()`, `copy_model_file()`, and their Nemotron-only tests.

**Verification:**
Run: `cargo build 2>&1 | tail -3`
Expected: Build succeeds

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass (should be ~20 tests now, down from 26)

**Commit:**
```bash
git add src/models/mod.rs
git commit -m "chore: remove Moonshine model management from models/mod.rs

Delete moonshine_model_dir, moonshine_model_files, moonshine_models_present,
moonshine_models_present_in, ensure_moonshine_models, MOONSHINE_REPO,
MOONSHINE_FILES constants, and all Moonshine-specific tests."
```
<!-- END_TASK_1 -->
