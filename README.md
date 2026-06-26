# Subtidal

Real-time speech-to-text overlay for Linux (Wayland) and macOS. Captures system or per-application audio, runs local STT inference, and displays live captions in a translucent overlay.

All processing happens locally — no cloud services, no network requests (except the initial one-time model download from HuggingFace).

## Features

- **STT engine**: Nemotron RNNT — CUDA (Linux), WebGPU/Metal (macOS Apple Silicon), or CPU fallback on both.
- **Per-application audio capture**: PipeWire on Linux, Core Audio Process Taps on macOS. Caption any app, not just the mic. On macOS, helper-heavy apps are grouped under user-facing app names while preserving helper process capture.
- **Auto-fallback** when the captured app exits: switches back to system output, with a desktop notification.
- **Overlay modes**: Docked (edge-anchored, click-through) and Floating (draggable, lockable for click-through).
- **Transcript mode**: separate scrollable window with timestamped paragraphs and "Save as JSON".
- **System tray** for toggling captions, switching audio source / engine / font / line count, mode and lock toggles.
- **Hot-reloadable config** — edits to the TOML file land live without restart.

## Requirements

### Linux
- Wayland compositor supporting `wlr-layer-shell` (Sway, Hyprland, niri, etc.)
- PipeWire
- CUDA (optional; CPU fallback otherwise)
- Rust toolchain

### macOS
- macOS 14.4+ on Apple Silicon (M1 or newer). Intel Macs are not supported.
- Audio Capture permission granted via System Settings → Privacy & Security → Screen & System Audio Recording, grant Subtidal permission for System Audio Recording Only. Subtidal will prompt on first launch.
- WebGPU is automatic on Apple Silicon; CPU fallback otherwise.
- Rust toolchain

## Install

### Linux
```bash
cargo install --path .
```

Models download automatically on first run from HuggingFace to `~/.local/share/subtidal/models/`.

### macOS
```bash
scripts/bundle-mac.sh
open target/release/Subtidal.app
```

The bundle is ad-hoc codesigned so the TCC permission grant persists across launches. Models download to `~/Library/Application Support/Subtidal/models/` on first run.

## Usage

### Linux
```bash
subtidal [--engine nemotron] [--config path] [--reset-config]
```

### macOS
Launch `Subtidal.app`. Configuration lives in `~/Library/Application Support/Subtidal/config.toml` (TOML, hot-reloaded).

### Tray controls (both platforms)
- Captions on/off
- Mode: Docked / Floating / Transcript
- Audio source: System Output or a specific application. macOS shows user-facing app names and hides grouped helper/background processes.
- Engine: Nemotron
- Font: curated picks plus all installed monospace families (macOS)
- Lines: 1–5
- Show Above Fullscreen / Lock Position

## Configuration

Config path:
- Linux: `~/.config/subtidal/config.toml`
- macOS: `~/Library/Application Support/Subtidal/config.toml`

```toml
engine = "nemotron"
overlay_mode = "floating"     # "docked" | "floating" | "transcript"
locked = true                 # click-through when Floating
above_fullscreen = false      # render above compositor-fullscreened clients (Linux)

[appearance]
background_color = "rgba(0,0,0,0.7)"
text_color = "#ffffff"
font_family = "monospace"     # "monospace" | "system" | any installed family name
font_size = 16.0
max_lines = 3
width = 600
expire_secs = 8               # seconds before idle caption lines clear
char_width_fraction = 0.95    # fraction of line width to use (0.0-1.0)

[position]
x = 100
y = 100
```

## License

MIT
