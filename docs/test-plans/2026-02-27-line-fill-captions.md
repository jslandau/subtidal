# Human Test Plan: Line-Fill Captions

## Prerequisites
- Subtidal built and running on a Wayland compositor with wlr-layer-shell support
- PipeWire audio server running
- `cargo test` passing (all automated tests green)
- Config file at `~/.config/subtidal/config.toml` accessible for editing

## Phase 1: Visual Line-Fill Behavior (AC1)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Launch `./target/release/subtidal`. Start speaking short phrases into an active audio source. | Captions appear in the overlay, filling left-to-right on line 1. |
| 2 | Continue speaking until line 1 is visually full. | Text wraps to line 2 without splitting a word across lines. |
| 3 | Keep speaking until all configured lines (default 3) are full. | When new text arrives, the topmost line disappears, remaining lines shift up, and new text appears on the bottom line. |
| 4 | Observe word boundaries during fast speech. | No words appear split across lines. Continuation fragments join correctly. |
| 5 | Verify the 0.85x conservative multiplier provides adequate visual padding. | Lines should not overflow or clip at the right edge. There should be visible padding on the right side, but lines should not appear excessively short (roughly 15% shorter than the overlay width). |

## Phase 2: Idle Expiry (AC2)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Speak enough to fill 2-3 caption lines, then stop speaking entirely. | After approximately 8 seconds of silence, the topmost line disappears. |
| 2 | Continue waiting in silence. | Lines continue to disappear one at a time until the overlay is empty. |
| 3 | Resume speaking after all lines have cleared. | New captions appear normally on line 1; the buffer has fully reset. |

## Phase 3: Configurable Expiry Hot-Reload (AC3)

| Step | Action | Expected |
|------|--------|----------|
| 1 | While the application is running, open `~/.config/subtidal/config.toml` in a text editor. | File opens without issue. |
| 2 | Change `expire_secs` under `[appearance]` from 8 to 2. Save the file. | No crash or error in the application. |
| 3 | Speak to fill caption lines, then stop speaking. | Lines now expire noticeably faster (approximately 2 seconds instead of 8). |
| 4 | Change `expire_secs` back to 8. Save the file. | Original slower expiry behavior resumes on the next silence period. |
| 5 | Set `expire_secs = 0` in the config and save. | Application uses the default value of 8 seconds (fallback behavior). |

## End-to-End: Continuous Conversation with Config Change

1. Launch Subtidal. Play a podcast or video with continuous speech.
2. Observe captions filling and shifting for at least 30 seconds. Verify smooth line transitions with no visual glitches, duplicated words, or split words.
3. While audio continues, edit `config.toml` to set `expire_secs = 3` and `width = 400`. Save.
4. Verify the overlay resizes and lines become shorter. Expiry should now be faster during any pauses.
5. Revert config changes. Verify original behavior resumes.

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 Fill line 1 | `ac1_1_fill_single_line` | Phase 1, Step 1 |
| AC1.2 Overflow to line 2 | `ac1_2_overflow_to_second_line` | Phase 1, Step 2 |
| AC1.3 Shift when full | `ac1_3_shift_when_all_lines_full` | Phase 1, Step 3 |
| AC1.4 Continuation join | `ac1_4_continuation_no_space` | Phase 1, Step 4 |
| AC1.5 Continuation overflow | `ac1_5_partial_word_overflow` + extended tests | Phase 1, Step 4 |
| AC1.6 Overlap dedup | `ac1_6_overlap_deduplication` | Phase 1, Step 4 |
| AC2.1 Oldest line expires | `ac2_1_oldest_line_expires` | Phase 2, Step 1 |
| AC2.2 Gradual drain | `ac2_2_expiry_gradual_drain` | Phase 2, Step 2 |
| AC2.3 Active lines safe | `ac2_3_active_lines_dont_expire` | Phase 2, Step 3 |
| AC3.1 Default expire_secs | `appearance_config_default_expire_secs` + roundtrip | — (config-only) |
| AC3.2 Hot-reload | `ac3_2_update_config_hot_reload` | Phase 3, Steps 1-4 |
| AC3.3 Invalid fallback | `appearance_config_zero_expire_secs_uses_default` | Phase 3, Step 5 |
| AC4.1 Conservative multiplier | `ac4_1_conservative_multiplier` | Phase 1, Step 5 |
