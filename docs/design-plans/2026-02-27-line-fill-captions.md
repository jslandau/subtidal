# Line-Fill Captions Design

## Summary

Subtidal currently accumulates speech-to-text fragments into a flat string buffer and displays them as a block of text, relying on GTK's word-wrap to break lines. This design replaces that model with an explicit line-fill buffer: the application tracks each display line as a first-class object, fills it word by word up to a character limit, and shifts older lines off the top when the display is full. The result is a more predictable caption layout where line boundaries are controlled by the application rather than inferred by the rendering layer.

The implementation is organized in three phases. Phase 1 rewrites `CaptionBuffer` in `src/overlay/mod.rs` to hold a `Vec<CaptionLine>`, implement word-filling and line-shifting logic, apply a conservative character-width estimate, and drain lines gradually during silence via a per-line expiry timestamp. Phase 2 adds an `expire_secs` field to `AppearanceConfig` and wires it through to the buffer, including hot-reload support. Phase 3 updates documentation. The existing deduplication logic for RNNT decoder overlap is preserved unchanged.

## Definition of Done
1. **Caption display uses a fill-and-shift line model** — text fills line 1, then line 2, etc. up to `max_lines`. When all lines are full and new text arrives, line 1 disappears and all content shifts up, freeing the bottom line.
2. **Idle timer clears the oldest line** — when no new caption text arrives for a configurable duration (default ~8s), the top line clears and remaining lines shift up. This repeats until all lines are cleared.
3. **Expiry timer is configurable** — a new config field controls how long a line persists before being cleared during silence.
4. **Slide-up animation is nice-to-have** — smooth transition when lines shift, but instant vanish is acceptable.

## Acceptance Criteria

### line-fill-captions.AC1: Fill-and-shift line model
- **line-fill-captions.AC1.1 Success:** Text fills line 1 left-to-right, word by word, up to max_chars_per_line
- **line-fill-captions.AC1.2 Success:** When line 1 is full, text continues on line 2 (up to max_lines)
- **line-fill-captions.AC1.3 Success:** When all lines are full and new text arrives, line 1 is removed, all lines shift up, new text fills the freed bottom line
- **line-fill-captions.AC1.4 Success:** Continuation fragments (no leading space) join the previous word on the same line without inserting a space
- **line-fill-captions.AC1.5 Edge:** When a continuation fragment would cause the combined word to overflow the current line, the partial word moves to the next line and joins there (words never split across lines)
- **line-fill-captions.AC1.6 Edge:** RNNT decoder overlap is deduplicated (same behavior as current)

### line-fill-captions.AC2: Idle timer clears oldest line
- **line-fill-captions.AC2.1 Success:** When no new text arrives for expire_secs, the oldest (top) line is removed and remaining lines shift up
- **line-fill-captions.AC2.2 Success:** Expiry continues once per second until all lines are cleared during silence
- **line-fill-captions.AC2.3 Success:** Active lines (receiving new text) do not expire — last_active resets on each push

### line-fill-captions.AC3: Configurable expiry timer
- **line-fill-captions.AC3.1 Success:** expire_secs field exists in AppearanceConfig with default value of 8
- **line-fill-captions.AC3.2 Success:** Changing expire_secs in config.toml and saving updates the buffer via hot-reload
- **line-fill-captions.AC3.3 Failure:** Invalid expire_secs value (0 or negative) uses default

### line-fill-captions.AC4: Conservative character width estimate
- **line-fill-captions.AC4.1 Success:** estimate_max_chars returns a value ~15% smaller than previous calculation, providing visual padding

## Glossary

- **CaptionBuffer**: The struct in `src/overlay/mod.rs` that accumulates incoming speech fragments and produces display text. This design replaces its internal model from flat fragment accumulation to a line-oriented buffer.
- **CaptionLine**: New struct representing a single display line — holds text content and a `last_active` timestamp for expiry.
- **Fragment**: A short string emitted by the STT engine for a recognized audio chunk. Leading space signals a word boundary; no leading space signals a continuation of the previous word.
- **Continuation fragment**: A fragment without leading space, indicating it is a suffix of the previous word (e.g., `"llo"` after `"He"`). Joined without inserting a space.
- **RNNT (Recurrent Neural Network Transducer)**: The model architecture used by Nemotron. RNNT decoders can emit overlapping output across chunk boundaries; deduplication logic removes this overlap.
- **Fill-and-shift**: The display model introduced here. Text fills the bottom line; when full, a new line is added; when all lines are full, the oldest line is removed and content shifts up.
- **Conservative multiplier (0.85x)**: Scaling factor applied to `estimate_max_chars` to make lines shorter than theoretical maximum, providing visual padding and accommodating proportional font width variation.

## Architecture

Replace the flat fragment-accumulation model in `CaptionBuffer` with a line-oriented buffer that explicitly manages line boundaries using character counting.

**Data model:** `CaptionBuffer` holds a `Vec<CaptionLine>` where each `CaptionLine` contains its text content and a `last_active` timestamp. The buffer tracks `max_chars_per_line` (derived from `estimate_max_chars`), `max_lines` (from config), and `expire_secs` (new configurable field, default 8).

**Push logic:** When a caption fragment arrives, it is deduplicated against the tail of all lines (same overlap removal as today). Words are appended to the current (bottom) line until it reaches `max_chars_per_line`. When a word doesn't fit, either a new line is created (if under `max_lines`) or all lines shift up (oldest removed, new empty line appended). If a continuation fragment (no leading space) would cause the combined word to overflow the current line, the partial word is moved from the current line to the next line and joined with the continuation there — words are never split across lines.

**Expiry logic:** A 1-second timer checks the oldest line's `last_active` timestamp. If older than `expire_secs`, that line is removed and remaining lines shift up. Only one line is removed per tick, creating a gradual drain effect during silence.

**Display:** `display_text()` joins all lines with `\n`. The GTK label renders these as explicit line breaks.

**Character width estimation:** `estimate_max_chars` gains a conservative multiplier (0.85×) so lines are slightly shorter than the available width, providing visual padding and reducing reliance on GTK word wrap as a fallback.

**Config:** `expire_secs` is added to `AppearanceConfig` (default 8, serialized as `expire_secs` in TOML). Hot-reload updates the buffer's expiry value when config changes.

## Existing Patterns

The current `CaptionBuffer` in `src/overlay/mod.rs:13-121` uses fragment-level accumulation with timestamp-based expiry — this design preserves the same expiry concept but operates at line granularity instead of fragment granularity. The deduplication logic (`remove_overlap`, `last_tail`) is preserved and applied to the concatenation of all line text.

`estimate_max_chars` in `src/overlay/mod.rs:429-440` already calculates characters-per-line from width and font size. This design reuses that function with a conservative adjustment.

`AppearanceConfig` in `src/config.rs:62-79` already holds `max_lines`, `width`, and `font_size`. Adding `expire_secs` follows the same pattern.

The GTK label in `src/overlay/mod.rs:276-292` already uses `wrap(true)`, `WordChar` wrap mode, `.lines(max_lines)`, and `EllipsizeMode::End`. These are preserved as safety nets but should rarely activate since line width is managed by the buffer.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: CaptionBuffer Rewrite
**Goal:** Replace fragment-based CaptionBuffer with line-based buffer, including all push/shift/expiry logic.

**Components:**
- `CaptionLine` struct in `src/overlay/mod.rs` — line text + last_active timestamp
- `CaptionBuffer` rewrite in `src/overlay/mod.rs` — `Vec<CaptionLine>`, push with word-fill logic, line shifting, word-unsplit on continuation overflow, expiry by oldest line, display_text joining with `\n`
- `estimate_max_chars` adjustment in `src/overlay/mod.rs` — apply 0.85× conservative multiplier
- Unit tests for CaptionBuffer — line filling, shifting when full, word-unsplit behavior, expiry drain, display_text output

**Dependencies:** None

**Done when:** CaptionBuffer unit tests pass covering: single-line fill, multi-line fill, shift on overflow, continuation word unsplit, idle expiry drain, display_text format. `cargo test` passes. `cargo build` succeeds.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Config and Wiring
**Goal:** Add configurable `expire_secs`, wire `max_chars_per_line` and `expire_secs` through to CaptionBuffer, handle hot-reload updates.

**Components:**
- `expire_secs` field in `AppearanceConfig` in `src/config.rs` — default 8, serde support
- `CaptionBuffer::new()` signature update — accepts `max_chars_per_line`, `max_lines`, `expire_secs`
- Overlay setup in `src/overlay/mod.rs` — pass config values to CaptionBuffer on creation
- `UpdateAppearance` handler in `src/overlay/mod.rs` — update buffer's `max_chars_per_line` and `expire_secs` on config change
- Config tests in `src/config.rs` — roundtrip with `expire_secs`

**Dependencies:** Phase 1

**Done when:** `expire_secs` appears in config TOML, hot-reload updates the buffer, config tests pass, `cargo build` succeeds.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Documentation Update
**Goal:** Update CLAUDE.md and README.md to document the new caption behavior and `expire_secs` config field.

**Components:**
- `CLAUDE.md` — update caption fragment description in Key Contracts, note line-fill model
- `README.md` — update config example to show `expire_secs`

**Dependencies:** Phase 2

**Done when:** Documentation reflects line-fill behavior and configurable expiry. No stale references to fragment-based caption model.
<!-- END_PHASE_3 -->

## Additional Considerations

**Proportional font drift:** Character counting is approximate for proportional fonts. The 0.85× conservative multiplier provides margin. If users report lines that are too short, the multiplier can be tuned. GTK word wrap remains as a fallback for edge cases.

**Word-unsplit complexity:** Moving a partial word from the end of one line to the next requires tracking word boundaries on the current line. The simplest approach: when a continuation fragment causes overflow, find the last space in the current line's text, split there, and move the trailing partial to the new line. If there's no space (entire line is one long word), allow GTK wrap to handle it.
