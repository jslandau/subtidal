# Line-Fill Captions Test Requirements

Maps each acceptance criterion from the line-fill-captions design to specific tests.

---

## Automated Tests

### line-fill-captions.AC1: Fill-and-shift line model

#### line-fill-captions.AC1.1 — Text fills line 1 left-to-right, word by word, up to max_chars_per_line

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Push several space-prefixed words (e.g., `" Hello"`, `" world"`, `" this"`) to a buffer with `max_chars_per_line=20, max_lines=3`. Assert `display_text()` contains all words on a single line with no `\n` separator.

#### line-fill-captions.AC1.2 — When line 1 is full, text continues on line 2

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Push enough words to exceed `max_chars_per_line` on line 1. Assert `display_text()` contains a `\n` separating two lines, with the overflow word appearing on line 2.

#### line-fill-captions.AC1.3 — When all lines are full, line 1 is removed and lines shift up

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Push enough words to fill all `max_lines`, then push additional words. Assert line count equals `max_lines`, the original first line content is gone, and the new word appears on the bottom line.

#### line-fill-captions.AC1.4 — Continuation fragments join without inserting a space

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Push `" Hel"` followed by `"lo"` (no leading space). Assert `display_text()` shows `"Hello"` as a single joined word on one line.

#### line-fill-captions.AC1.5 — Continuation overflow moves partial word to next line

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Fill a line nearly to capacity with a multi-word string, push a word-start that fits, then push a continuation fragment that would overflow. Assert the partial word moved to the next line and joined with the continuation there, with no word split across lines.

#### line-fill-captions.AC1.6 — RNNT decoder overlap is deduplicated

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Push text that shares a 4+ character overlap with the tail of existing buffer content. Assert the overlapping portion is not duplicated in `display_text()`. This verifies `remove_overlap` behavior is preserved.

---

### line-fill-captions.AC2: Idle timer clears oldest line

#### line-fill-captions.AC2.1 — Oldest line removed after expire_secs of silence

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Create a buffer with multiple lines. Manually set `last_active` on the oldest line to `Instant::now() - Duration::from_secs(expire_secs + 1)`. Call `expire()`. Assert the oldest line was removed and remaining lines shifted up.

#### line-fill-captions.AC2.2 — Expiry continues once per tick until all lines cleared

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Set up multiple lines all with expired timestamps. Call `expire()` repeatedly. Assert exactly one line is removed per call, and after enough calls all lines are cleared (empty `display_text()`).

#### line-fill-captions.AC2.3 — Active lines do not expire; last_active resets on push

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Set up two lines, expire only the oldest (set its `last_active` in the past). Push new text to the buffer. Assert `last_active` on the active (bottom) line is recent and does not expire on the next `expire()` call.

---

### line-fill-captions.AC3: Configurable expiry timer

#### line-fill-captions.AC3.1 — expire_secs field exists with default value of 8

- **Test type:** Unit
- **Test file:** `src/config.rs` (`#[cfg(test)] mod tests`)
- **Description:** Assert `AppearanceConfig::default().expire_secs == 8`. Also perform a TOML roundtrip: serialize a config with `expire_secs: 10`, deserialize, assert the value survived. Follow the existing `config_roundtrip` test pattern.

#### line-fill-captions.AC3.2 — Hot-reload updates the buffer when expire_secs changes

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Create a `CaptionBuffer`, call `update_config` with a new `expire_secs` value, then verify the buffer uses the new value for subsequent expiry checks (e.g., a line that would not have expired under old config now expires under the new shorter value, or vice versa).

#### line-fill-captions.AC3.3 — Invalid expire_secs (0) falls back to default

- **Test type:** Unit
- **Test file:** `src/config.rs` (`#[cfg(test)] mod tests`)
- **Description:** Deserialize a TOML string with `expire_secs = 0`. Call `effective_expire_secs()`. Assert it returns 8 (the default), not 0. Also verify that a TOML missing `expire_secs` entirely deserializes to the default value of 8.

---

### line-fill-captions.AC4: Conservative character width estimate

#### line-fill-captions.AC4.1 — estimate_max_chars returns ~15% smaller value

- **Test type:** Unit
- **Test file:** `src/overlay/mod.rs` (`#[cfg(test)] mod tests`)
- **Description:** Call `estimate_max_chars` with known inputs (e.g., `width_px=800, font_size_pt=24.0`). Compute the old formula result manually (53 for these inputs). Assert the new result is strictly less than the old value and approximately 85% of it (i.e., 45 for these inputs).

---

## Human Verification

### line-fill-captions.AC3.2 — Hot-reload end-to-end

The unit test above verifies that `update_config` changes the buffer's behavior. However, the full hot-reload path (file change detected by notify watcher, config re-parsed, `UpdateAppearance` command sent, handler calls `update_config`) crosses multiple threads and the GTK main loop. This path cannot be fully exercised in a unit test.

**Manual verification approach:** Run the application, edit `~/.config/subtidal/config.toml` to change `expire_secs` to a different value (e.g., 2), save the file, and observe that caption lines now expire faster during silence. Change it back to 8 and verify the original behavior resumes.

### line-fill-captions.AC1 (visual correctness)

Unit tests verify the logical content of `display_text()` but cannot confirm the visual rendering in the GTK overlay (font metrics, actual line wrapping as a fallback, overlay positioning).

**Manual verification approach:** Run the application with live audio. Observe that captions fill left-to-right on line 1, wrap to line 2 when full, and shift up when all lines are occupied. Verify no words are visually split across lines and that the 0.85x conservative multiplier provides adequate visual padding without making lines excessively short.

### Definition of Done item 4 — Slide-up animation (nice-to-have)

This is explicitly a nice-to-have and not covered by any acceptance criterion. If implemented, it can only be verified visually.

**Manual verification approach:** Run the application, fill all caption lines, then push new text. Observe whether lines shift up smoothly or instantly. Both behaviors are acceptable per the design.
