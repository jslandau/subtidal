# Manual verification checklist — performance & security remediation (2026-09)

Automated coverage for the macOS/GPU-gated findings is compile-level plus
extracted-predicate unit tests; this environment has no macOS runtime, UI, or
GPU. Each item below is the compensating runtime validation for its finding.
Run every item before a release that includes the remediation. All items were
introduced by the 2026-09-21 remediation plan (finding IDs referenced).

## MC-1 — IOProc guard behavior on a real aggregate device (F3/AC.4)

**Setup:** macOS 14.4+, Audio Capture permission granted.

1. Start Subtidal, select an application audio source, confirm captions flow.
2. During capture, observe `Console.app` (or the Subtidal stderr stream) for
   IOProc-related warnings — there must be none: a healthy aggregate device
   always passes the `audio_buffer_f32` guards (non-null, aligned,
   divisible byte size).
3. Exercise the degenerate shapes if reproducible (e.g. a source that
   produces a zero-length buffer during pause): the app must continue
   running; the callback returns `noErr` early and no panic/abort occurs.

**Pass:** no crashes, no dropped-audio warnings beyond known silence gaps.

## MC-2 — CFString leak gone over a 10-minute session (F7/AC.7)

**Setup:** Xcode Instruments (Leaks template), macOS.

1. Launch Subtidal with the tray active (the 5 s audio-source refresh timer
   drives `AudioObjectGetPropertyData` + `CFString` wrapping per process).
2. Record 10 minutes with the Leaks instrument while switching audio sources
   at least twice and letting the timer run throughout.
3. Inspect the CFString allocations: before the fix each 5 s enumeration
   leaked one CFString per audio process; after the fix
   (`wrap_under_create_rule`) the CFString count must be flat.

**Pass:** no growing CFString allocations attributable to
`tap.rs::read_tap_uid` / `tap_processes.rs::process_bundle_id`.

## MC-3 — Transcript scrolling smoothness after 1 hour of captions (F8/AC.8)

**Setup:** macOS, Transcript mode, a long audio session (podcast/meeting).

1. Keep Subtidal captioning for ≥ 1 hour in Transcript mode (the incremental
   append path replaced the per-caption full-history `rebuild_view`).
2. Scroll the transcript while captions continue appending: scrolling must
   stay responsive; autoscroll re-attaches when returning to the bottom.
3. Compare CPU/GPU utilization at t=10 min vs t=60 min — the per-caption
   cost must be flat, not growing with transcript length.

**Pass:** no perceptible scroll judder; flat per-caption cost.

## MC-4 — Caption panel responsiveness during source enumeration (F9/AC.9)

**Setup:** macOS, Docked or Floating caption mode, several audio apps open.

1. While captions are flowing, watch the panel during the 5 s tray refresh:
   Core Audio enumeration now runs on a background thread; AppKit labeling
   and the submenu rebuild run on main.
2. Launch/quit an audio app and confirm the Audio Source submenu updates
   within ~5 s without the caption panel freezing or dropping frames.

**Pass:** no visible UI stall correlated with the 5 s refresh.

## MC-5 — CUDA probe skip on second launch (F10/AC.10)

**Setup:** Linux + CUDA GPU box.

1. Delete `~/.cache/subtidal/cuda-probe.json` (first run forces a probe).
2. Start Subtidal; note startup time and the `info: CUDA available` line —
   the subprocess probe runs (it re-executes the binary with
   `__SUBTIDAL_CUDA_PROBE=1`; `ps` during startup shows it).
3. Quit and start again: the probe subprocess must NOT run (the verdict is
   served from the cache). Observe via startup timing or
   `ps aux | grep SUBTIDAL_CUDA_PROBE` during launch.
4. Touch a model file (`touch` + size change) or break CUDA (unload the
   driver module if testable): the next launch re-probes.

**Pass:** second launch skips the subprocess; environment changes re-probe.

## MC-6 — Long-tail hardening spot checks (F13/F14/F15/F21)

1. **F13:** profile a long Nemotron session and confirm the inference tick does
   not show a fresh large `Vec` allocation on each 560 ms boundary.
2. **F14:** during a process-source session, recycle a watched PID if a safe
   test harness permits it; on the next slow re-enumeration (~10 s), the tap
   must fall back when the PID is no longer associated with the watched bundle.
3. **F15:** run `scripts/bundle-mac.sh`, then
   `scripts/check-macos-version.sh`; it must exit 0, and the bundled plist must
   contain no `NSMicrophoneUsageDescription` key.
4. **F21:** inspect the PipeWire thread while idle and confirm the longer
   iterate timeout reduces wakeups without delaying an audio-command response.

**Pass:** no per-tick allocation regression, PID reuse is detected, bundle
version checks pass, and idle wakeups/command responsiveness meet expectations.

## Notes

- `cargo check --lib --target aarch64-apple-darwin` runs in CI
  (`.github/workflows/macos-check.yml`, compile-only). It cannot run from
  this Linux workstation (`onig_sys` C cross-compile requires a macOS SDK)
  — a pre-existing environment limitation, not a regression.
