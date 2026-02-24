mod audio;
mod config;
mod models;

use anyhow::{Context, Result};
use clap::Parser;
use config::Config;
use ringbuf::traits::Consumer;

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

    // Phase 3: Start audio capture
    let (audio_cmd_tx, mut ring_consumer, _node_list) =
        audio::start_audio_thread(cfg.audio_source.clone())
            .unwrap_or_else(|e| {
                eprintln!("error: failed to start audio capture: {e:#}");
                eprintln!("hint: is PipeWire running? (`systemctl --user status pipewire`)");
                std::process::exit(1);
            });

    // Test consumer: resample and print chunk count (temporary, replaced in Phase 4)
    let mut resampler = audio::resampler::AudioResampler::new()
        .expect("creating resampler");
    let mut chunk_count = 0u64;

    println!("Audio capture started. Listening for 5 seconds...");
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < 5 {
        // Drain ring buffer into a temporary vec.
        let mut raw = vec![0f32; 4096];
        let n = ring_consumer.pop_slice(&mut raw);
        if n > 0 {
            match resampler.push_interleaved(&raw[..n]) {
                Ok(chunks) => {
                    chunk_count += chunks.len() as u64;
                    if !chunks.is_empty() {
                        eprintln!("info: produced {} 160ms chunks (total: {})", chunks.len(), chunk_count);
                    }
                }
                Err(e) => eprintln!("warn: resampler error: {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    println!("5 second test complete. Total 160ms chunks produced: {chunk_count}");
    let _ = audio_cmd_tx.send(audio::AudioCommand::Shutdown);

    // --- Remaining subsystem stubs (filled in subsequent phases) ---
    // Phase 4: STT inference thread
    // Phase 5: GTK4 overlay window
    // Phase 6: ksni system tray
    // Phase 7: config hot-reload
    // Phase 8: full integration

    Ok(())
}
