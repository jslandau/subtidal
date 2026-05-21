# Changelog

All notable changes to Subtidal are documented in this file.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] — 2026-05-21

### Added
- **macOS 14.4+ Apple Silicon support.** Full feature parity with the Linux build:
  - Audio capture via Core Audio Process Taps (`AudioHardwareCreateProcessTap` +
    aggregate device + IOProc), gated by the standard **Audio Capture** TCC
    permission (not Screen Recording).
  - Per-application audio source enumeration via `kAudioProcessPropertyBundleID`,
    with multi-PID watchdog (browsers split audio across helper processes that
    share a bundle id) and 1 Hz POSIX `kill(pid, 0)` liveness check; auto-falls
    back to SystemMix with a `UNUserNotification` when the captured app exits.
  - WebGPU (Metal) Nemotron inference with automatic CPU fallback.
  - Overlay panel via NSPanel — Docked, Floating, and Transcript modes with
    drag persistence, rounded translucent background, configurable font/colors,
    and pixel-accurate line counting via NSStringDrawing.
  - System tray via NSStatusItem + NSMenu, with submenus for Mode, Engine,
    Audio Source (pretty names via `NSRunningApplication.localizedName`), Font
    (curated picks + every installed monospace font), Lines (1–5), Above
    Fullscreen, and Lock Position. Dynamic 5 s refresh of the audio source list.
  - Config + hot-reload at `~/Library/Application Support/Subtidal/config.toml`.
  - `.app` bundle script at `scripts/bundle-mac.sh` (ad-hoc codesigned).
- **`appearance.font_family` config field** (default `"monospace"`) selectable
  from the new tray Font submenu. Both `"system"` and any installed font family
  name are accepted; resolves identically on Linux and macOS in spirit (Linux
  GTK wiring of the field is deferred — Linux currently still uses GTK's
  default font).

### Changed
- README rewritten to describe the cross-platform (Linux + macOS) codebase.
- `CLAUDE.md` rewritten to drop "Linux currently, macOS planned" framing.
- Cargo dependency layout split into per-OS conditional blocks; Resolver v2
  keeps `cuda` (Linux) and `webgpu` (macOS) features from bleeding cross-target.

### Removed
- The earlier `compile_error!("Subtidal currently only supports Linux. macOS
  support is planned.")` guard in `src/main.rs`. The guard now fires only on
  OSes other than Linux and macOS.

## [0.2.2] and earlier

Pre-changelog. See `git log` for history.
