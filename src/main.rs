mod audio;
mod config;
mod models;
mod stt;
mod overlay;
mod tray;

use clap::Parser;
use config::Config;
use ringbuf::traits::Consumer;
use std::sync::Arc;

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

fn main() {
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
    cfg.save().unwrap_or_else(|e| {
        eprintln!("warn: failed to save config: {e}");
    });

    println!("Config loaded: {:?}", Config::config_path());
    println!("Engine: {:?}", cfg.engine);
    println!("Audio source: {:?}", cfg.audio_source);
    println!("Model dir: {:?}", models::models_dir());

    // Phase 2: Ensure model files are present before starting
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to build tokio runtime: {e}");
            std::process::exit(1);
        });

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
    let (audio_cmd_tx, ring_consumer, node_list) =
        audio::start_audio_thread(cfg.audio_source.clone())
            .unwrap_or_else(|e| {
                eprintln!("error: failed to start audio capture: {e:#}");
                eprintln!("hint: is PipeWire running? (`systemctl --user status pipewire`)");
                std::process::exit(1);
            });

    // Validate the loaded audio source against available nodes; if invalid, fall back to SystemOutput.
    // This ensures that if a saved Application source's node_id disappears (e.g. app restarted),
    // we gracefully switch to the always-available SystemOutput instead of failing.
    let validated_source = {
        let nodes = node_list.lock().unwrap();
        audio::validate_audio_source(cfg.audio_source.clone(), &nodes)
    };
    if validated_source != cfg.audio_source {
        cfg.audio_source = validated_source.clone();
        // Notify audio thread of the fallback source if needed.
        let _ = audio_cmd_tx.send(audio::AudioCommand::SwitchSource(validated_source));
    }

    // Phase 4: Determine active engine (with CUDA fallback).
    let active_engine = cfg.engine.clone();
    let (active_engine, cuda_fallback_warning) = match active_engine {
        config::Engine::Parakeet => {
            if stt::cuda_available() {
                (config::Engine::Parakeet, None)
            } else {
                eprintln!("warn: CUDA not available, falling back to Moonshine (CPU)");
                (config::Engine::Moonshine, Some("CUDA unavailable — using Moonshine (CPU)"))
            }
        }
        config::Engine::Moonshine => (config::Engine::Moonshine, None),
    };

    // Create audio chunk channel (connects Phase 3 ring buffer drain to inference).
    // Wrap the SyncSender in Arc<Mutex<>> so Phase 8 engine switching can replace it
    // at runtime without restarting the bridge thread.
    let (chunk_tx_inner, chunk_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(32);
    let chunk_tx = std::sync::Arc::new(std::sync::Mutex::new(chunk_tx_inner));
    let (caption_tx, caption_rx) = std::sync::mpsc::sync_channel::<String>(64);

    // Spawn the audio→chunk bridge thread.
    // Drains the ring buffer, resamples, and sends 160ms chunks to the inference thread.
    // Locks chunk_tx on each send so Phase 8 can atomically swap the inner SyncSender.
    let mut ring_consumer_arc = ring_consumer;
    let chunk_tx_for_bridge = std::sync::Arc::clone(&chunk_tx);
    std::thread::spawn(move || {
        let mut resampler = audio::resampler::AudioResampler::new()
            .expect("creating resampler");
        let mut raw = vec![0f32; 4096];
        loop {
            let n = ring_consumer_arc.pop_slice(&mut raw);
            if n > 0 {
                match resampler.push_interleaved(&raw[..n]) {
                    Ok(chunks) => {
                        for chunk in chunks {
                            let tx = chunk_tx_for_bridge.lock().unwrap();
                            if tx.send(chunk).is_err() {
                                drop(tx); // release lock before sleep
                                std::thread::sleep(std::time::Duration::from_millis(10));
                                break; // engine switching — wait for new tx
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("warn: resampler error: {e}");
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });

    // Instantiate the active STT engine.
    let engine: Box<dyn stt::SttEngine> = match active_engine {
        config::Engine::Parakeet => {
            let model_dir = models::parakeet_model_dir();
            Box::new(
                stt::parakeet::ParakeetEngine::new(&model_dir)
                    .unwrap_or_else(|e| {
                        eprintln!("error: failed to load Parakeet model: {e:#}");
                        std::process::exit(1);
                    })
            )
        }
        config::Engine::Moonshine => {
            let model_dir = models::moonshine_model_dir();
            Box::new(
                stt::moonshine::MoonshineEngine::new(&model_dir)
                    .unwrap_or_else(|e| {
                        eprintln!("error: failed to load Moonshine model: {e:#}");
                        std::process::exit(1);
                    })
            )
        }
    };

    // Spawn the inference thread.
    let _inference_handle = stt::spawn_inference_thread(engine, chunk_rx, caption_tx);

    // Phase 6: Set up engine-switch channel.
    let (engine_switch_tx, engine_switch_rx) = std::sync::mpsc::sync_channel::<tray::EngineCommand>(4);

    // Wire engine-switch receiver (restarts inference thread on switch — Phase 8 completes this).
    {
        let audio_tx_for_engine = audio_cmd_tx.clone();
        std::thread::spawn(move || {
            for cmd in engine_switch_rx.iter() {
                match cmd {
                    tray::EngineCommand::Switch(new_engine) => {
                        eprintln!("info: engine switch to {new_engine:?} — respawn inference (Phase 8)");
                        // Full respawn logic wired in Phase 8.
                        let _ = audio_tx_for_engine; // used in Phase 8
                    }
                }
            }
        });
    }

    // Phase 5: Set up channels for caption and command delivery.
    // We use std::sync::mpsc because glib::channel is not available in glib 0.19.
    // The glib main loop will poll these channels via timeout_add.
    let (caption_tx_to_gtk, caption_rx_from_inference) = std::sync::mpsc::channel::<String>();
    let (cmd_tx_to_gtk, cmd_rx) = std::sync::mpsc::channel::<overlay::OverlayCommand>();

    // Bridge: forward inference thread captions directly.
    let caption_rx_from_inference_out = caption_rx; // from Phase 4 spawn_inference_thread
    std::thread::spawn(move || {
        for caption in caption_rx_from_inference_out.iter() {
            if caption_tx_to_gtk.send(caption).is_err() {
                break;
            }
        }
    });

    // Shared captions-enabled flag (also used by tray in Phase 6).
    let captions_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // Spawn the system tray (Phase 6).
    let tray_state = tray::TrayState {
        captions_enabled: Arc::clone(&captions_enabled),
        active_source: cfg.audio_source.clone(),
        overlay_mode: cfg.overlay_mode.clone(),
        locked: cfg.locked,
        active_engine: active_engine.clone(),
        cuda_warning: cuda_fallback_warning,
        overlay_tx: cmd_tx_to_gtk.clone(),
        audio_tx: audio_cmd_tx.clone(),
        engine_tx: engine_switch_tx,
        node_list: Arc::clone(&node_list),
    };

    // Use the already-built tokio runtime (from Phase 2 model download).
    let tray_handle = tray::spawn_tray(tray_state, &runtime);

    // Tray handle is stored so Phase 8 can call handle.update() when state changes.
    let _ = tray_handle; // used in Phase 8

    // Phase 7: Start config hot-reload watcher.
    // _config_watcher must stay in scope until process exit (drop = stop watching).
    // Typed as Option so the failure path compiles without a dummy Debouncer.
    let _config_watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> =
        match config::start_hot_reload(cmd_tx_to_gtk.clone()) {
            Ok(watcher) => {
                eprintln!("info: config hot-reload active (watching config.toml)");
                Some(watcher)
            }
            Err(e) => {
                eprintln!("warn: config hot-reload unavailable: {e}");
                eprintln!("warn: config.toml changes will require a restart to take effect");
                None
            }
        };

    // Run GTK4 main loop (blocks until application exits).
    overlay::run_gtk_app(cfg, caption_rx_from_inference, cmd_rx, Arc::clone(&captions_enabled));
}
