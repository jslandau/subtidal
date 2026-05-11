# Manual Test Plan: Transcript Window Mode

**Feature:** Transcript window mode (third OverlayMode alongside Docked/Floating)
**Date:** 2026-05-11
**Binary:** `./target/release/subtidal`
**Prerequisites:** PipeWire running, Wayland compositor with wlr-layer-shell, microphone or system audio source available.

---

## Setup

- `cargo build --release`
- Kill any running subtidal instance: `pkill subtidal`
- Remove any stale config: `rm -f ~/.config/subtidal/config.toml`
- Launch: `./target/release/subtidal`

---

## Phase 2: Tray radio + persistence

- [ ] **AC2.5** — With app running in Docked mode, left-click the tray icon. Overlay hides. Left-click again. Overlay reappears. No crash, no visible regression.
- [ ] **AC2.3 (radio)** — Right-click tray icon → Overlay submenu. Confirm exactly three radio options appear in order: Docked, Floating, Transcript. Docked is selected by default.
- [ ] **AC2.3 (lock gating)** — In Docked mode, confirm Lock option is absent or disabled in tray. Switch to Floating; confirm Lock option appears. Switch to Transcript; confirm Lock option is absent or disabled again.
- [ ] **AC2.4 (persistence)** — Switch to Transcript via tray. Quit app (tray → Quit). Relaunch. Confirm `~/.config/subtidal/config.toml` contains `overlay_mode = "transcript"` and the app starts in Transcript mode (layer-shell overlay absent, transcript window visible).

---

## Phase 3 / 4: Two-window orchestration

- [ ] **AC4.3** — Launch fresh in Docked mode (delete config first). Speak aloud. Confirm layer-shell overlay shows live captions in line-fill style.
- [ ] **AC3.2 / AC4.4** — Tray → Overlay → Transcript. Confirm: layer-shell overlay disappears; a separate transcript window appears with a HeaderBar, a scrollable text area, and a "Save…" button. Prior captions from step above are visible as `[HH:MM:SS]` timestamped paragraphs.
- [ ] **AC3.3** — Speak more while in Transcript mode. Confirm new fragments append in real time with `[HH:MM:SS]` timestamp prefixes at paragraph boundaries.
- [ ] **AC3.4 (pause)** — Scroll upward in the transcript window. Speak. Confirm the viewport stays at the scrolled-up position (autoscroll is paused while not at bottom).
- [ ] **AC3.4 (resume)** — Scroll back to the very bottom of the transcript window. Speak. Confirm the view scrolls automatically to keep the latest text visible.
- [ ] **AC3.5** — Press Ctrl+A in the transcript window, then Ctrl+C. Paste into a text editor. Confirm the full transcript text (all paragraphs) is on the clipboard.
- [ ] **AC4.5** — Switch back to Docked via tray. Confirm layer-shell overlay reappears with live line-fill display.
- [ ] **AC4.6 (history across switches)** — Perform the sequence: Docked → Transcript → Floating → Transcript → Docked → Transcript. After each return to Transcript, confirm all accumulated captions are still present, with no truncation or duplication.
- [ ] **AC4 (hot-reload)** — While running in Docked mode, open `~/.config/subtidal/config.toml` in an editor, change `overlay_mode = "docked"` to `overlay_mode = "transcript"`, and save. Within ~250 ms confirm the layer-shell overlay disappears and the transcript window opens. Change back to `"docked"` and save; confirm the reverse. Both directions work.

---

## Phase 5: Save dialog and dual-write

- [ ] **AC5.3** — In Transcript mode, click "Save…". Confirm the FileDialog's default filename matches the pattern `subtidal-transcript-YYYY-MM-DD-HHMMSS.txt` where the timestamp matches the session start time (visible in the transcript's first `[HH:MM:SS]` entry, roughly).
- [ ] **AC5.2** — Save to `/tmp/test-transcript.txt`. After the dialog closes:
  - `cat /tmp/test-transcript.txt` — confirm lines are in the format `[HH:MM:SS] <paragraph text>`, one per paragraph.
  - `cat /tmp/test-transcript.json` — confirm a JSON object with keys `session_start` (RFC 3339 string), `engine` (`"nemotron"`), and `fragments` (array; each element has `timestamp` and `text` string fields).
- [ ] **AC5.6 (overwrite UX)** — Click "Save…" again and save to the same `/tmp/test-transcript.txt` path. Confirm the OS/GTK file chooser prompts to overwrite the `.txt` file. After confirming, both files are updated. Confirm no additional overwrite prompt appeared for the `.json` sibling.
- [ ] **AC5.4** — Click "Save…" and attempt to save to a non-writable path, e.g., `/proc/test.txt`. Confirm an AlertDialog appears reporting the write failure. Confirm the app does not crash and remains usable.
- [ ] **AC5.5 (partial failure)** — Construct a partial-failure scenario: `mkdir /tmp/onlytxt && chmod 755 /tmp/onlytxt && touch /tmp/onlytxt/foo.json && chmod 444 /tmp/onlytxt/foo.json`. Click "Save…" and save to `/tmp/onlytxt/foo.txt`. Confirm an AlertDialog reports the `.json` write failure AND includes the path `/tmp/onlytxt/foo.txt` (successful side) so manual recovery is possible. Clean up: `rm -rf /tmp/onlytxt`.

---

## Phase 6: Clear-on-disable wiring

- [ ] **AC6.3 (step 1)** — In any mode, speak so captions/transcript text is visible.
- [ ] **AC6.3 (step 2)** — Toggle captions off via tray (left-click tray icon or menu toggle). While in Transcript mode: confirm the transcript window text area goes blank immediately.
- [ ] **AC6.3 (step 3)** — Switch to Docked mode. Confirm the layer-shell overlay caption label is also blank.
- [ ] **AC6.4 (step 1)** — Toggle captions on again via tray.
- [ ] **AC6.4 (step 2)** — Speak new content.
- [ ] **AC6.4 (step 3)** — Switch to Transcript mode. Confirm only post-re-enable fragments are visible — no content from before the toggle-off.
- [ ] **AC6.5** — Click "Save…" and save to `/tmp/post-clear.txt`. Run `cat /tmp/post-clear.txt` and `cat /tmp/post-clear.json`. Confirm both files contain only the post-re-enable fragments; no pre-toggle-off content is present.
- [ ] **Quit** — Tray → Quit. Confirm app exits cleanly (no crash, no zombie process visible in `ps aux | grep subtidal`).
