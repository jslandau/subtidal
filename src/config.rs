use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which STT engine to use for inference.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Parakeet,
    Moonshine,
}

/// The PipeWire audio source to capture from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioSource {
    /// System-wide monitor sink (default output loopback).
    #[default]
    SystemOutput,
    /// A specific application's PipeWire node, identified by node ID.
    Application { node_id: u32, node_name: String },
}

/// Overlay display mode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayMode {
    /// Anchored to a screen edge via wlr-layer-shell.
    #[default]
    Docked,
    /// Freely positioned xdg_toplevel window.
    Floating,
}

/// Which screen edge the overlay is anchored to in docked mode.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenEdge {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

/// Position of the overlay window in floating mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

    /// Path to config file, set by load_from(). Used by save().
    #[serde(skip)]
    pub config_file_path: Option<PathBuf>,
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
            config_file_path: None,
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
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.config_file_path = Some(path.to_path_buf());
        Ok(cfg)
    }

    /// Persist the current config to disk. Creates parent directories if needed.
    /// If config_file_path is set, saves to that path; otherwise uses default config_path().
    pub fn save(&self) -> Result<()> {
        let path = if let Some(ref config_path) = self.config_file_path {
            config_path.clone()
        } else {
            Self::config_path()
        };
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
        let mut original = Config {
            engine: Engine::Moonshine,
            overlay_mode: OverlayMode::Floating,
            locked: false,
            ..Config::default()
        };
        original.config_file_path = Some(path.clone());
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
        fs::write(&path, "engine = \"moonshine\"\n").unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.engine, Engine::Moonshine);
        assert!(cfg.locked);
        assert_eq!(cfg.screen_edge, ScreenEdge::Bottom);
    }
}
