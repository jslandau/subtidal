# Line-Fill Captions Implementation Plan — Phase 3: Documentation Update

**Goal:** Update CLAUDE.md and README.md to document line-fill caption behavior and `expire_secs` config field.

**Architecture:** Documentation-only changes. No code modifications.

**Tech Stack:** Markdown

**Scope:** 3 phases from original design (phase 3 of 3)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This is an infrastructure/documentation phase.

**Verifies: None** — documentation does not have testable acceptance criteria.

---

<!-- START_TASK_1 -->
### Task 1: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md:5` — Update freshness date
- Modify: `CLAUDE.md:46` — Rewrite "Caption fragments" bullet in Key Contracts

**Step 1: Update freshness date**

Change line 5 from:
```
Freshness: 2026-02-27
```
to the current date when this task executes.

**Step 2: Rewrite Caption fragments bullet**

Current text at line 46:
```
- **Caption fragments**: Engine whitespace is preserved for word boundary detection; fragments are not trimmed/joined with spaces (fixes split words like "del ve" -> "delve").
```

Replace with:
```
- **Caption display**: Line-fill model — text fills lines word-by-word up to a character limit (0.85× estimated max chars), then shifts oldest line off when all lines are full. During silence, lines expire one at a time after `expire_secs` (default 8s). Engine whitespace signals word boundaries: leading space = new word, no space = continuation of previous word. RNNT overlap deduplication is preserved.
```

**Verification:**
Run: `cat CLAUDE.md | head -50`
Expected: Updated freshness date and rewritten caption bullet visible

**Commit:** `docs: update CLAUDE.md for line-fill caption model`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update README.md config example

**Files:**
- Modify: `README.md:53-58` — Add `expire_secs` to the `[appearance]` config example

**Step 1: Add expire_secs to config example**

Current `[appearance]` block (lines 53-58):
```toml
[appearance]
background_color = "rgba(0,0,0,0.7)"
text_color = "#ffffff"
font_size = 16.0
max_lines = 3
width = 600
```

Add `expire_secs` field:
```toml
[appearance]
background_color = "rgba(0,0,0,0.7)"
text_color = "#ffffff"
font_size = 16.0
max_lines = 3
width = 600
expire_secs = 8                # seconds before idle caption lines clear
```

**Verification:**
Run: `cargo build`
Expected: Still compiles (documentation only, no code changes)

**Commit:** `docs: add expire_secs to README config example`

<!-- END_TASK_2 -->
