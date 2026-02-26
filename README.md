# Subtidal

Real-time speech-to-text overlay for Linux/Wayland. Captures system or per-application audio via PipeWire, runs local STT inference, and displays live captions in a translucent overlay.

All processing happens locally — no cloud services, no network requests (except the initial one-time model download from HuggingFace).

## Features

- **Two STT engines**: Nemotron (GPU, CUDA) for high accuracy, Moonshine (CPU, experimental)
- **Per-application audio capture** via PipeWire — caption any app, not just the mic
- **Overlay modes**: docked (edge-anchored, click-through) or floating (draggable, resizable via tray)
- **System tray** for toggling captions, switching audio source/engine, adjusting overlay size
- **Hot-reloadable config** at `~/.config/subtidal/config.toml`

## Requirements

- Linux with Wayland compositor supporting `wlr-layer-shell` (Sway, Hyprland, etc.)
- PipeWire
- CUDA (optional, for Nemotron engine)
- Rust toolchain

## Install

```bash
cargo install --path .
```

Models are downloaded automatically on first run from HuggingFace to `~/.local/share/subtidal/models/`.

## Usage

```bash
subtidal [--engine nemotron|moonshine] [--config path] [--reset-config]
```

The system tray icon provides controls for:
- Toggling captions on/off (left-click)
- Selecting audio source (system output or specific application)
- Switching between docked and floating overlay
- Adjusting overlay size
- Switching STT engine
- Opening the config file

## Configuration

Config lives at `~/.config/subtidal/config.toml` and is hot-reloaded on save.

```toml
engine = "nemotron"           # or "moonshine"
overlay_mode = "floating"     # or "docked"
locked = true                 # click-through when true

[appearance]
background_color = "rgba(0,0,0,0.7)"
text_color = "#ffffff"
font_size = 16.0
max_lines = 3
width = 600

[position]
x = 100
y = 100
```

## License

MIT
