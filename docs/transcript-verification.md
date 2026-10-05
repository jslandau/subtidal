# Transcript implementation verification

Last checked: 2026-10-05

This record separates executed checks from verification limitations. The operator cleared the queued-update race for current use on 2026-10-05 and requested closing the implementation tasks; this is not a claim that every automated or manual acceptance check passed.

## Cleared for current use — monitor queued-update scrolling

The operator reports that macOS trackpad scrolling works, including momentum and rubberbanding. The queued-update race is cleared as a blocker for now and should be watched during ordinary use: while reading history, incoming speech must not pull the viewport back to the latest text. Also watch transitions around momentum completion, scrollbar dragging, Autoscroll off, hide/show and clear. If an unexpected jump occurs, record the interaction and reopen the issue.

The macOS transcript scrollbar now uses persistent native styling with autohide disabled, avoiding repeated append-induced flashing/fading. Compilation and native panel assertions passed after this change.

## Executed on Apple Silicon macOS

- `cargo test --lib`: 99 passed, 7 existing model-dependent tests ignored at the latest stable checkpoint.
- `cargo check --lib --target aarch64-apple-darwin`: passed without warnings at that checkpoint.
- Main-thread native runner: wrapped-history settled-bottom assertion and resize/control smoke passed before expanding event scenarios.
- Regression injection: temporarily replaced `FollowState::following()` with `false`; the native runner failed in `native_follow_scroll_matrix` at “settled native view must follow bottom”. Restored `self.enabled && self.pinned` immediately and reran successfully. No injected mutation remains.
- Shared tests cover follow transitions/stale generations, UTF-8/GTK-character/UTF-16 conversion, metadata selection positions, paragraph split/merge positions, configured gap/clock-backwards behavior and preserved export content.

## Source-reviewed, not executed on Linux

Linux GTK scenarios, Xvfb workflow and dependencies were added. This host has no Linux Rust target or GTK runtime; the attempted Linux target check failed before compiling project code because the target was missing. Cached GTK signatures were reviewed, but this does not establish successful compilation or native behavior.

The Linux controller tests emit native controller signals and move the actual GTK adjustment. They are not evidence of compositor-delivered GDK wheel, keyboard or scrollbar events. Native CI has not been run from this session.

The new workflow was parsed as YAML and checked for whitespace. An independent dependency audit confirmed Noble GTK meets the 4.10 floor, pinned gtk4-layer-shell v1.0.2 Meson flags/pkg-config match the locked sys crate, and ORT dependency-output discovery matches cached ort-sys source. Actual package installation/linkage remains unverified until CI runs.

## Required manual check — not performed

`manual_panel_visual_check` remains outstanding on both platforms:

- Light/dark appearance at minimum/default sizes: spacing, timestamp contrast, speaker emphasis and wrapped text.
- Trackpad momentum, scrollbar dragging and keyboard page navigation while new captions arrive.
- Accessibility focus/names, selection/copy and resize while reading history.
- Screenshots and observations must be recorded by a native UI operator; automated geometry checks are not a substitute.

## Remaining acceptance status

Expanded native macOS execution has passed the follow matrix, non-scrolling native mouse/key interaction, stale-scroll cancellation, 100-append callback coalescing (one actual registration/execution), and actual NSTextStorage tail edit-range observation. The panel-render properties and production routing (one push in each mode, hidden history, relabel without append, names, four-surface clear, stale/fresh epochs) now pass in the integrated native run. Unicode body selection and the original saved-character screen position survive rename, relabel and paused resize; both paragraph and continuation native edit-range checks preserve attributed prefixes. The earlier top-line anchor assertion was replaced with this stronger content-location assertion because line starts legitimately change during reflow.

The full runner still fails at the wheel branch of `native_input_race`: generated events have not produced native viewport movement, including direct responder, NSWindow dispatch and NSApplication dispatch experiments. The last run reports “native layout condition timed out”; it is a failed run, not a pass. The strict movement assertion remains enabled. Native keyboard PageUp exercises the queued-update race successfully; it does not substitute for wheel or scrollbar delivery. These remain verification limitations, not blockers to current use per the operator's decision. Linux native execution, delivered-input coverage and the full manual check were not completed in this session. The implementation tasks are closed with those limitations recorded; the known synthetic-wheel runner failure remains unchanged and is not reported as a passing test.
