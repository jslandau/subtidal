# Line-Fill Captions Implementation Plan — Phase 2: Config and Wiring

**Goal:** Add configurable `expire_secs` to AppearanceConfig, wire `max_chars_per_line` and `expire_secs` through to CaptionBuffer, handle hot-reload updates.

**Architecture:** `expire_secs` becomes a config field with serde support and default of 8. The overlay setup passes `max_chars_per_line` (from `estimate_max_chars`) and `expire_secs` to CaptionBuffer. The `UpdateAppearance` handler updates the buffer's values on config change. Hot-reload automatically propagates changes since `expire_secs` is part of `AppearanceConfig` which is already compared for changes.

**Tech Stack:** Rust, serde, toml

**Scope:** 3 phases from original design (phase 2 of 3)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### line-fill-captions.AC3: Configurable expiry timer
- **line-fill-captions.AC3.1 Success:** expire_secs field exists in AppearanceConfig with default value of 8
- **line-fill-captions.AC3.2 Success:** Changing expire_secs in config.toml and saving updates the buffer via hot-reload
- **line-fill-captions.AC3.3 Failure:** Invalid expire_secs value (0 or negative) uses default

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Add expire_secs to AppearanceConfig

**Verifies:** line-fill-captions.AC3.1, line-fill-captions.AC3.3

**Files:**
- Modify: `src/config.rs:62-96` — Add `expire_secs` field to `AppearanceConfig` struct and Default impl

**Implementation:**

Add `expire_secs` field to `AppearanceConfig` struct at `src/config.rs:64-79`:

```rust
/// Seconds before an idle caption line expires and is removed.
#[serde(default = "default_expire_secs")]
pub expire_secs: u64,
```

Add the default function (near `default_width` at line 81):

```rust
fn default_expire_secs() -> u64 {
    8
}
```

Update the `Default` impl at lines 85-96 to include:

```rust
expire_secs: 8,
```

For AC3.3 (invalid values): since the field is `u64`, negative values are impossible at the type level. For zero, add a `fn effective_expire_secs(&self) -> u64` method on `AppearanceConfig` that returns `default_expire_secs()` when `self.expire_secs == 0`, otherwise returns `self.expire_secs`. Use this method everywhere the value is read.

**Verification:**
Run: `cargo build`
Expected: Compiles without errors

**Commit:** `feat(config): add expire_secs field to AppearanceConfig`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Config tests for expire_secs

**Verifies:** line-fill-captions.AC3.1, line-fill-captions.AC3.3

**Files:**
- Modify: `src/config.rs` — Add tests to existing `#[cfg(test)] mod tests` block (currently at lines 329-421)

**Testing:**

Tests must verify each AC listed above:

- **line-fill-captions.AC3.1:** Verify `AppearanceConfig::default().expire_secs == 8`. Also do a TOML roundtrip test: serialize a config with `expire_secs: 10`, deserialize it, assert the value survived. Follow the existing `config_roundtrip` test pattern (tempfile, toml::to_string_pretty, Config::load_from).
- **line-fill-captions.AC3.3:** Deserialize a TOML string with `expire_secs = 0`. Call `effective_expire_secs()`. Verify it returns the default (8), not 0. Also test that a TOML missing the `expire_secs` field entirely results in the default value of 8 (this is already covered by the existing `config_partial_toml_fills_defaults` test pattern but should be explicitly checked for the new field).

**Verification:**
Run: `cargo test --bin subtidal config`
Expected: All config tests pass including the new ones

**Commit:** `test(config): add expire_secs roundtrip and validation tests`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_3 -->
### Task 3: Wire CaptionBuffer creation and hot-reload update

**Verifies:** line-fill-captions.AC3.2

**Files:**
- Modify: `src/overlay/mod.rs:196` — Update CaptionBuffer::new() call to pass max_chars_per_line and expire_secs
- Modify: `src/overlay/mod.rs:520-526` — Update UpdateAppearance handler to update buffer's expire_secs and max_chars_per_line

**Implementation:**

**Step 1: Update CaptionBuffer creation (line 196)**

Currently:
```rust
let caption_buffer = Rc::new(RefCell::new(CaptionBuffer::new(cfg.appearance.max_lines as usize)));
```

Change to:
```rust
let max_chars = estimate_max_chars(cfg.appearance.width, cfg.appearance.font_size);
let caption_buffer = Rc::new(RefCell::new(CaptionBuffer::new(
    cfg.appearance.max_lines as usize,
    max_chars as usize,
    cfg.appearance.effective_expire_secs(),
)));
```

Note: `max_chars` is already computed at line 279 for the GTK label. You may need to move this computation earlier (before line 196) or compute it twice. The simplest approach: compute it at line 196 and reuse the variable at line 279.

**Step 2: Add update methods to CaptionBuffer**

Add to the CaptionBuffer impl block:

```rust
fn update_config(&mut self, max_chars_per_line: usize, expire_secs: u64) {
    self.max_chars_per_line = max_chars_per_line;
    self.expire_secs = expire_secs;
}
```

**Step 3: Update UpdateAppearance handler (lines 520-526)**

The handler currently doesn't have access to `caption_buffer`. The `caption_buffer` is an `Rc<RefCell<CaptionBuffer>>` created at line 196, but the command handler at lines 520-526 is inside `handle_overlay_command` function which doesn't receive the buffer.

Two approaches:
1. **Clone the Rc into the command polling closure** (lines 239-248) and pass it to `handle_overlay_command`
2. **Add a separate UpdateAppearance path** in the command polling closure before calling `handle_overlay_command`

The cleanest approach: add `caption_buffer: &Rc<RefCell<CaptionBuffer>>` parameter to `handle_overlay_command`, and pass the cloned Rc from the command polling closure at line 243. Then in the `UpdateAppearance` arm, add:

```rust
OverlayCommand::UpdateAppearance(appearance) => {
    apply_appearance(&appearance);
    let label = find_caption_label(window);
    let max_chars = estimate_max_chars(appearance.width, appearance.font_size);
    label.set_max_width_chars(max_chars);
    label.set_lines(appearance.max_lines as i32);
    window.set_width_request(appearance.width);
    // Update buffer config for hot-reload
    let mut buf = caption_buffer.borrow_mut();
    buf.update_config(max_chars as usize, appearance.effective_expire_secs());
}
```

This also requires updating the `handle_overlay_command` function signature and all call sites. Check the function signature (it's called only from the command polling closure at line 243).

**Verification:**
Run: `cargo build`
Expected: Compiles without errors

Run: `cargo test --bin subtidal`
Expected: All existing tests pass

**Commit:** `feat(overlay): wire expire_secs and max_chars_per_line through to CaptionBuffer`

<!-- END_TASK_3 -->
