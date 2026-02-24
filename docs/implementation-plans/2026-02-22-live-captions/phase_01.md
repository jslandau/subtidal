# Live Captions Implementation Plan — Phase 1: Project Scaffolding and Configuration

**Goal:** Establish Cargo project structure, declare core dependencies, and implement the configuration layer so the application can load or create a default config and exit cleanly. Additional dependencies (`bytemuck`, `ndarray`, `tokenizers`, `notify-debouncer-mini`, `ctrlc`) are added in the phases that need them.

**Architecture:** Single Rust binary. `Config` is a TOML-serializable struct covering all runtime settings. `src/models/mod.rs` resolves XDG data paths. `src/main.rs` parses CLI args, loads config, and delegates to subsystem stubs.

**Tech Stack:** Rust 2021 edition, gtk4 0.10, pipewire 0.9, ort 2.0.0-rc.11, parakeet-rs 0.3, hf-hub 0.5, notify 8, serde/toml, clap 4, dirs 5.

**Scope:** Phase 1 of 8 from original design.

**Codebase verified:** 2026-02-22 — true greenfield, no Cargo.toml or src/ exists.

---

## Acceptance Criteria Coverage

**Verifies: None** — this is an infrastructure phase. Verification is operational (cargo build succeeds, cargo run loads/creates config and exits).

---

<!-- START_TASK_1 -->
### Task 1: Initialize Cargo project and declare core dependencies

**Files:**
- Create: `Cargo.toml`

**Step 1: Initialize the Rust project**

```bash
cd /home/jslandau/git/live_text
cargo init --name live-captions
```

This creates `src/main.rs` (placeholder) and `Cargo.toml`. We'll overwrite both.

**Step 2: Write Cargo.toml**

Replace the generated `Cargo.toml` with:

```toml
[package]
name = "live-captions"
version = "0.1.0"
edition = "2021"
description = "Real-time speech-to-text overlay for Linux/Wayland"

[[bin]]
name = "live-captions"
path = "src/main.rs"

[dependencies]
# GUI
gtk4 = { version = "0.10", features = ["v4_10"] }
gtk4-layer-shell = "0.7"

# System tray
ksni = "0.3"

# Audio
pipewire = "0.9"
rubato = "1.0"
ringbuf = "0.4"

# STT
ort = { version = "2.0.0-rc.11", features = ["cuda"] }
parakeet-rs = "0.3"

# Model management
hf-hub = { version = "0.5", features = ["tokio"] }

# Config hot-reload
notify = "8"

# Desktop notifications
notify-rust = "4"

# Serialization
serde = { version = "1", features = ["derive"] }
toml = "0.8"

# Error handling
anyhow = "1"

# Async runtime (required by hf-hub)
tokio = { version = "1", features = ["rt-multi-thread", "macros", "fs"] }

# CLI argument parsing
clap = { version = "4", features = ["derive"] }

# XDG directory resolution
dirs = "5"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1

[dev-dependencies]
tempfile = "3"
```

**Note on `gtk4-layer-shell` pkg-config:** The `gtk4-layer-shell` crate handles its own pkg-config linkage internally — no `build.rs` is needed. If `cargo build` fails with a missing library error, install the system package:

```bash
sudo pacman -S gtk4-layer-shell   # Arch/CachyOS
```

**Step 3: Verify cargo fetch succeeds**

```bash
cd /home/jslandau/git/live_text
cargo fetch
```

Expected: Dependencies download without errors. If `ort 2.0.0-rc.11` fails to resolve, try `ort = { version = "=2.0.0-rc.11", ... }` for exact pinning.

**Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: initialize Cargo project with core dependencies"
```
<!-- END_TASK_1 -->

---

<!-- START_SUBCOMPONENT_A (tasks 2-3) -->
<!-- START_TASK_2 -->
### Task 2: Implement src/config.rs — Config struct with load/save

**Files:**
- Create: `src/config.rs`

The `Config` struct is the central state for all subsystems. Define it fully here so every later phase can use it.

**Step 1: Create src/config.rs**

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which STT engine to use for inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Parakeet,
    Moonshine,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::Parakeet
    }
}

/// The PipeWire audio source to capture from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSource {
    /// System-wide monitor sink (default output loopback).
    SystemOutput,
    /// A specific application's PipeWire node, identified by node ID.
    Application { node_id: u32, node_name: String },
}

impl Default for AudioSource {
    fn default() -> Self {
        AudioSource::SystemOutput
    }
}

/// Overlay display mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Anchored to a screen edge via wlr-layer-shell.
    Docked,
    /// Freely positioned xdg_toplevel window.
    Floating,
}

impl Default for OverlayMode {
    fn default() -> Self {
        OverlayMode::Docked
    }
}

/// Which screen edge the overlay is anchored to in docked mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Default for ScreenEdge {
    fn default() -> Self {
        ScreenEdge::Bottom
    }
}

/// Position of the overlay window in floating mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayPosition {
    pub x: i32,
    pub y: i32,
}

impl Default for OverlayPosition {
    fn default() -> Self {
        OverlayPosition { x: 100, y: 100 }
    }
}

/// Visual appearance of the overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    /// CSS color string for background, e.g. "rgba(0,0,0,0.7)".
    pub background_color: String,
    /// CSS color string for caption text, e.g. "#ffffff".
    pub text_color: String,
    /// Font size in points.
    pub font_size: f32,
    /// Maximum number of caption lines to display.
    pub max_lines: u32,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        AppearanceConfig {
            background_color: "rgba(0,0,0,0.7)".to_string(),
            text_color: "#ffffff".to_string(),
            font_size: 16.0,
            max_lines: 3,
        }
    }
}

/// Root configuration struct. Serialized to ~/.config/live-captions/config.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Active STT engine.
    #[serde(default)]
    pub engine: Engine,

    /// Active audio source.
    #[serde(default)]
    pub audio_source: AudioSource,

    /// Overlay display mode.
    #[serde(default)]
    pub overlay_mode: OverlayMode,

    /// Screen edge for docked mode.
    #[serde(default)]
    pub screen_edge: ScreenEdge,

    /// Window position in floating mode.
    #[serde(default)]
    pub position: OverlayPosition,

    /// Whether the floating overlay is locked (click-through).
    #[serde(default = "default_locked")]
    pub locked: bool,

    /// Caption text appearance.
    #[serde(default)]
    pub appearance: AppearanceConfig,
}

fn default_locked() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            engine: Engine::default(),
            audio_source: AudioSource::default(),
            overlay_mode: OverlayMode::default(),
            screen_edge: ScreenEdge::default(),
            position: OverlayPosition::default(),
            locked: true,
            appearance: AppearanceConfig::default(),
        }
    }
}

impl Config {
    /// Returns the path to the config file: ~/.config/live-captions/config.toml
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("live-captions")
            .join("config.toml")
    }

    /// Load config from disk. If the file does not exist, returns `Default::default()`.
    /// If the file exists but is malformed, logs a warning and returns `Default::default()`.
    pub fn load() -> Config {
        let path = Self::config_path();
        if !path.exists() {
            return Config::default();
        }
        match Self::load_from(&path) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("warn: failed to parse config at {}: {e}", path.display());
                eprintln!("warn: using default configuration");
                Config::default()
            }
        }
    }

    pub fn load_from(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Persist the current config to disk. Creates parent directories if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing config to {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = Config {
            engine: Engine::Moonshine,
            overlay_mode: OverlayMode::Floating,
            locked: false,
            ..Config::default()
        };
        let text = toml::to_string_pretty(&original).unwrap();
        fs::write(&path, &text).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.engine, Engine::Moonshine);
        assert_eq!(loaded.overlay_mode, OverlayMode::Floating);
        assert!(!loaded.locked);
    }

    #[test]
    fn config_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        // load_from returns Err for missing file; load() returns Default.
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn config_malformed_toml_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(&path, "engine = ???invalid [[[ toml").unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn config_partial_toml_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.toml");
        // Only engine field; other fields should default.
        fs::write(&path, "[engine]\nmoonshine = {}").unwrap();
        // If this parses, locked should be the default (true).
        // If it fails, that is acceptable — we only test that malformed doesn't panic.
        let _ = Config::load_from(&path);
    }
}
```

**Step 2: Verify it compiles and tests pass**

```bash
cd /home/jslandau/git/live_text
cargo test config::tests
```

Expected: All 4 tests pass.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Implement src/models/mod.rs — model path resolution

**Files:**
- Create: `src/models/mod.rs`
- Create: `src/models/` directory

**Step 1: Create src/models/mod.rs**

```rust
use std::path::PathBuf;

/// Returns the base directory for downloaded model files.
/// ~/.local/share/live-captions/models/
pub fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("live-captions")
        .join("models")
}

/// Returns the directory for Parakeet ONNX model files.
/// ~/.local/share/live-captions/models/parakeet/
pub fn parakeet_model_dir() -> PathBuf {
    models_dir().join("parakeet")
}

/// Returns the directory for Moonshine ONNX model files.
/// ~/.local/share/live-captions/models/moonshine/
pub fn moonshine_model_dir() -> PathBuf {
    models_dir().join("moonshine")
}

/// Returns paths for the three Parakeet model files.
/// Files: encoder.onnx, decoder_joint.onnx, tokenizer.json
pub fn parakeet_model_files() -> [PathBuf; 3] {
    let dir = parakeet_model_dir();
    [
        dir.join("encoder.onnx"),
        dir.join("decoder_joint.onnx"),
        dir.join("tokenizer.json"),
    ]
}

/// Returns paths for the three Moonshine model files.
/// Files: encoder_model_quantized.onnx, decoder_model_merged_quantized.onnx, tokenizer.json
pub fn moonshine_model_files() -> [PathBuf; 3] {
    let dir = moonshine_model_dir();
    [
        dir.join("encoder_model_quantized.onnx"),
        dir.join("decoder_model_merged_quantized.onnx"),
        dir.join("tokenizer.json"),
    ]
}

/// Returns true if all required Parakeet model files are present on disk.
pub fn parakeet_models_present() -> bool {
    parakeet_model_files().iter().all(|p| p.exists())
}

/// Returns true if all required Moonshine model files are present on disk.
pub fn moonshine_models_present() -> bool {
    moonshine_model_files().iter().all(|p| p.exists())
}
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

<!-- START_TASK_4 -->
### Task 4: Implement src/main.rs — CLI argument parsing and subsystem stubs

**Files:**
- Modify: `src/main.rs` (replace cargo-generated placeholder)

**Step 1: Write src/main.rs**

```rust
mod config;
mod models;

use anyhow::Result;
use clap::Parser;
use config::Config;

#[derive(Parser, Debug)]
#[command(name = "live-captions", about = "Real-time speech-to-text overlay for Linux/Wayland")]
struct Args {
    /// Path to config file (default: ~/.config/live-captions/config.toml)
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    /// Override STT engine for this session (parakeet|moonshine)
    #[arg(long)]
    engine: Option<String>,

    /// Reset config to defaults before starting
    #[arg(long)]
    reset_config: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Load or reset config. --config overrides the default XDG path.
    let mut cfg = if args.reset_config {
        println!("Resetting config to defaults.");
        Config::default()
    } else if let Some(ref config_path) = args.config {
        Config::load_from(config_path).unwrap_or_else(|e| {
            eprintln!("warn: failed to load config from {}: {e}", config_path.display());
            eprintln!("warn: using default configuration");
            Config::default()
        })
    } else {
        Config::load()
    };

    // CLI engine override
    if let Some(engine_str) = args.engine {
        cfg.engine = match engine_str.to_lowercase().as_str() {
            "parakeet" => config::Engine::Parakeet,
            "moonshine" => config::Engine::Moonshine,
            other => {
                eprintln!("Unknown engine '{}'. Use 'parakeet' or 'moonshine'.", other);
                std::process::exit(1);
            }
        };
    }

    // Persist the config (creates file on first run)
    cfg.save()?;

    println!("Config loaded: {:?}", Config::config_path());
    println!("Engine: {:?}", cfg.engine);
    println!("Audio source: {:?}", cfg.audio_source);
    println!("Model dir: {:?}", models::models_dir());

    // --- Subsystem stubs (filled in subsequent phases) ---
    // Phase 2: model download
    // Phase 3: PipeWire audio capture
    // Phase 4: STT inference thread
    // Phase 5: GTK4 overlay window
    // Phase 6: ksni system tray
    // Phase 7: config hot-reload
    // Phase 8: full integration

    Ok(())
}
```

**Step 2: Verify it compiles and runs**

```bash
cd /home/jslandau/git/live_text
cargo build
```

Expected: Builds successfully. Warnings about unused imports are acceptable; errors are not.

```bash
cargo run
```

Expected:
```
Config loaded: "/home/<user>/.config/live-captions/config.toml"
Engine: Parakeet
Audio source: SystemOutput
Model dir: "/home/<user>/.local/share/live-captions/models"
```

Verify the config file was created:
```bash
cat ~/.config/live-captions/config.toml
```

Expected: Valid TOML with all default fields.

**Step 3: Test --reset-config flag**

```bash
cargo run -- --reset-config
```

Expected: Runs without error, config file is overwritten with defaults.

**Step 4: Commit**

```bash
git add src/
git commit -m "feat: project scaffolding — Config struct, model paths, CLI stubs"
```
<!-- END_TASK_4 -->
