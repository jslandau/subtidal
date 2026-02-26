use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use std::time::Duration;

/// Which STT engine to use for inference.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    #[serde(alias = "parakeet")]
    Nemotron,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceConfig {
    /// CSS color string for background, e.g. "rgba(0,0,0,0.7)".
    pub background_color: String,
    /// CSS color string for caption text, e.g. "#ffffff".
    pub text_color: String,
    /// Font size in points.
    pub font_size: f32,
    /// Maximum number of caption lines to display.
    pub max_lines: u32,
    /// Caption area width in pixels (0 = auto/natural size).
    #[serde(default = "default_width")]
    pub width: i32,
    /// Caption area height in pixels (0 = auto/natural size).
    #[serde(default)]
    pub height: i32,
}

fn default_width() -> i32 {
    600
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        AppearanceConfig {
            background_color: "rgba(0,0,0,0.7)".to_string(),
            text_color: "#ffffff".to_string(),
            font_size: 16.0,
            max_lines: 3,
            width: 600,
            height: 0,
        }
    }
}

/// Docked mode positioning along the anchored edge.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    /// Centered on the anchored edge (default).
    #[default]
    Center,
    /// Stretched to fill the full edge (original behavior).
    Stretch,
    /// Offset from the start of the edge (left/top) in pixels.
    Offset(i32),
}

/// Root configuration struct. Serialized to ~/.config/subtidal/config.toml.
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

    /// Position of the overlay along the docked edge.
    #[serde(default)]
    pub dock_position: DockPosition,

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
            dock_position: DockPosition::default(),
            appearance: AppearanceConfig::default(),
            config_file_path: None,
        }
    }
}

impl Config {
    /// Returns the path to the config file: ~/.config/subtidal/config.toml
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("subtidal")
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

/// Start watching config.toml for changes. When config changes on disk,
/// sends UpdateAppearance to the overlay and updates the tray state.
///
/// Returns the debouncer watcher (must be kept alive for the lifetime of the watch).
/// Drop the returned watcher to stop watching.
///
/// Note: Programmatic saves (e.g. from tray callbacks) will trigger the watcher,
/// causing a redundant but harmless reload cycle. The updates are idempotent,
/// so this is accepted as a trade-off for simplicity.
pub fn start_hot_reload(
    overlay_tx: std::sync::mpsc::Sender<crate::overlay::OverlayCommand>,
    tray_handle: ksni::Handle<crate::tray::TrayState>,
    tokio_handle: tokio::runtime::Handle,
) -> anyhow::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let config_path = Config::config_path();

    // Ensure the config directory exists (it should from startup, but guard here).
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Track previous config state so we only send commands when values actually change.
    // This prevents the drag feedback loop: drag_end saves position → hot-reload fires →
    // SetMode would re-apply margins and reinstall the drag handler mid-interaction.
    let initial_cfg = Config::load();
    let prev_appearance = std::sync::Mutex::new(initial_cfg.appearance.clone());
    let prev_mode = std::sync::Mutex::new(initial_cfg.overlay_mode);
    let prev_locked = std::sync::Mutex::new(initial_cfg.locked);

    // Debounce at 500ms: multiple rapid writes (e.g. from an editor) collapse into one event.
    let mut debouncer = new_debouncer(Duration::from_millis(500), move |result: DebounceEventResult| {
        match result {
            Ok(_events) => {
                // Config file changed: reload and apply.
                match Config::load_from(&Config::config_path()) {
                    Ok(new_cfg) => {
                        // Only send overlay commands when the relevant values actually changed.
                        // Position-only saves (from dragging) must not trigger any overlay
                        // commands, as CSS reloads and relayouts during a drag cause jitter.
                        if let Ok(mut prev) = prev_appearance.lock() {
                            if *prev != new_cfg.appearance {
                                let _ = overlay_tx.send(
                                    crate::overlay::OverlayCommand::UpdateAppearance(new_cfg.appearance.clone())
                                );
                                *prev = new_cfg.appearance.clone();
                            }
                        }
                        if let Ok(mut prev) = prev_mode.lock() {
                            if *prev != new_cfg.overlay_mode {
                                let _ = overlay_tx.send(
                                    crate::overlay::OverlayCommand::SetMode(new_cfg.overlay_mode.clone())
                                );
                                *prev = new_cfg.overlay_mode.clone();
                            }
                        }
                        if let Ok(mut prev) = prev_locked.lock() {
                            if *prev != new_cfg.locked {
                                let _ = overlay_tx.send(
                                    crate::overlay::OverlayCommand::SetLocked(new_cfg.locked)
                                );
                                *prev = new_cfg.locked;
                            }
                        }
                        // Update tray to reflect new config state.
                        let tray_handle = tray_handle.clone();
                        tokio_handle.block_on(async {
                            tray_handle.update(|tray: &mut crate::tray::TrayState| {
                                tray.active_engine = new_cfg.engine.clone();
                                tray.overlay_mode = new_cfg.overlay_mode.clone();
                                tray.locked = new_cfg.locked;
                            }).await;
                        });
                    }
                    Err(e) => {
                        // AC6.3: malformed TOML → warn and keep current state.
                        eprintln!("warn: config hot-reload failed (malformed TOML): {e}");
                        eprintln!("warn: keeping current overlay appearance");
                    }
                }
            }
            Err(e) => {
                eprintln!("warn: config file watch error: {e:?}");
            }
        }
    })?;

    // Watch the config file itself (NonRecursive = only the file).
    debouncer.watcher().watch(
        &config_path,
        notify::RecursiveMode::NonRecursive,
    )?;

    Ok(debouncer)
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
