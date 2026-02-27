# Nemotron CPU Fallback Implementation Plan — Phase 4

**Goal:** Hide the engine submenu in the tray menu since only one engine variant exists.

**Architecture:** Conditionally skip the "STT Engine" submenu in the tray `menu()` method when only one Engine variant exists. The `build_engine_submenu` function is preserved for future use but not called.

**Tech Stack:** Rust, ksni

**Scope:** 5 phases (this is phase 4). Covers design phase 5.

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements:

### nemotron-cpu-fallback.AC4: Tray menu updated
- **nemotron-cpu-fallback.AC4.1 Success:** Engine submenu hidden when only one engine variant exists

---

<!-- START_TASK_1 -->
### Task 1: Hide engine submenu from tray menu

**Verifies:** nemotron-cpu-fallback.AC4.1

**Files:**
- Modify: `src/tray/mod.rs:74-145` (menu() method)
- Modify: `src/tray/mod.rs:307-335` (suppress unused warning)

**Implementation:**

In the `menu()` method (lines 74-145), remove the STT Engine submenu block (lines 108-114):

```rust
            // --- STT Engine submenu ---
            SubMenu {
                label: "STT Engine".to_string(),
                submenu: build_engine_submenu(&self.active_engine),
                ..Default::default()
            }
            .into(),
```

Delete these lines entirely. The vec entries before and after (Overlay submenu and separator) remain.

Add `#[allow(dead_code)]` above `build_engine_submenu` (line 307) to suppress the unused function warning while preserving it for future engines:

```rust
#[allow(dead_code)]
fn build_engine_submenu(active: &Engine) -> Vec<MenuItem<TrayState>> {
```

Note: `EngineCommand` and `engine_tx` are still used by the engine switch thread infrastructure in main.rs, so no `dead_code` annotations are needed for those — only for `build_engine_submenu`.

**Verification:**
Run: `cargo build 2>&1 | tail -3`
Expected: Build succeeds (no warnings about unused build_engine_submenu)

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

**Commit:**
```bash
git add src/tray/mod.rs
git commit -m "refactor: hide engine submenu in tray with single engine variant

Remove STT Engine submenu from tray menu since only Nemotron exists.
Preserve build_engine_submenu function (dead_code allowed) for future
engine additions."
```
<!-- END_TASK_1 -->
