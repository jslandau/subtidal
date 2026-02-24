mod config;
mod models;

use anyhow::{Context, Result};
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

    // Phase 2: Ensure model files are present before starting
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building tokio runtime")?;

    let engine = cfg.engine.clone();
    runtime.block_on(async move {
        match engine {
            config::Engine::Parakeet => {
                if !models::parakeet_models_present() {
                    println!("Downloading Parakeet model files (first run)...");
                    models::ensure_parakeet_models().await
                        .unwrap_or_else(|e| {
                            eprintln!("error: failed to download Parakeet model: {e:#}");
                            eprintln!("hint: check network connectivity and disk space in ~/.local/share/live-captions/models/");
                            std::process::exit(1);
                        });
                    println!("Parakeet models ready.");
                } else {
                    println!("Parakeet models already present, skipping download.");
                }
            }
            config::Engine::Moonshine => {
                if !models::moonshine_models_present() {
                    println!("Downloading Moonshine model files (first run)...");
                    models::ensure_moonshine_models().await
                        .unwrap_or_else(|e| {
                            eprintln!("error: failed to download Moonshine model: {e:#}");
                            eprintln!("hint: check network connectivity and disk space in ~/.local/share/live-captions/models/");
                            std::process::exit(1);
                        });
                    println!("Moonshine models ready.");
                } else {
                    println!("Moonshine models already present, skipping download.");
                }
            }
        }
    });

    // --- Remaining subsystem stubs (filled in subsequent phases) ---
    // Phase 3: PipeWire audio capture
    // Phase 4: STT inference thread
    // Phase 5: GTK4 overlay window
    // Phase 6: ksni system tray
    // Phase 7: config hot-reload
    // Phase 8: full integration

    Ok(())
}
