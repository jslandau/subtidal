mod audio;
mod config;
mod models;
mod stt;
mod overlay;
mod tray;
mod main_linux;

use arc_swap::ArcSwap;
use clap::Parser;
use config::Config;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Install the .desktop file and app icon to XDG standard locations on first run.
fn ensure_desktop_entry_installed() {
    static APP_ICON: &[u8] = include_bytes!("../assets/icons/hicolor/scalable/apps/subtidal.svg");
    static DESKTOP_ENTRY: &[u8] = include_bytes!("../assets/subtidal.desktop");

    let data_dir = match dirs::data_dir() {
        Some(d) => d,
        None => return,
    };

    let icon_path = data_dir.join("icons/hicolor/scalable/apps/subtidal.svg");
    let desktop_path = data_dir.join("applications/subtidal.desktop");

    let files: &[(&std::path::Path, &[u8])] = &[
        (&icon_path, APP_ICON),
        (&desktop_path, DESKTOP_ENTRY),
    ];

    let needs_install = files.iter().any(|(path, data)| {
        !path.exists() || path.metadata().map(|m| m.len() != data.len() as u64).unwrap_or(true)
    });

    if needs_install {
        for (path, data) in files {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(path, data) {
                eprintln!("warn: failed to write {}: {e}", path.display());
            }
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "subtidal", about = "Real-time speech-to-text overlay for Linux/Wayland")]
struct Args {
    /// Path to config file (default: ~/.config/subtidal/config.toml)
    #[arg(long)]
    config: Option<std::path::PathBuf>,

    /// Override STT engine for this session (nemotron|parakeet)
    #[arg(long)]
    engine: Option<String>,

    /// Reset config to defaults before starting
    #[arg(long)]
    reset_config: bool,

    /// Start with captions disabled (model still loads; toggle from tray). Useful for autostart.
    #[arg(long)]
    start_disabled: bool,
}

fn main() {
    main_linux::reexec_with_absolute_argv0_if_needed();
    main_linux::ensure_provider_libs_next_to_exe();

    // If we're a CUDA probe subprocess, run the probe and exit immediately.
    if std::env::var_os("__SUBTIDAL_CUDA_PROBE").is_some() {
        main_linux::run_cuda_probe();
    }

    let args = Args::parse();

    ensure_desktop_entry_installed();

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
        match Config::parse_engine(&engine_str) {
            Some(engine) => cfg.engine = engine,
            None => {
                eprintln!("Unknown engine '{}'. Valid engines: nemotron, parakeet.", engine_str);
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

    runtime.block_on(async {
        if !models::nemotron_models_present() {
            println!("Downloading Nemotron model files (first run)...");
            models::ensure_nemotron_models().await
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to download Nemotron model: {e:#}");
                    eprintln!("hint: check network connectivity and disk space in ~/.local/share/subtidal/models/");
                    std::process::exit(1);
                });
            println!("Nemotron models ready.");
        } else {
            println!("Nemotron models already present, skipping download.");
        }
    });

    // Shared wake primitive: PipeWire RT callback notifies, STT thread waits.
    let audio_wake = Arc::new(stt::AudioWake::new());

    // Start audio capture. The PipeWire callback will call `audio_wake.notify()`
    // after each push to the ring buffer.
    let (audio_cmd_tx, ring_consumer, node_list, fallback_rx) =
        audio::start_audio_thread(cfg.audio_source.clone(), Arc::clone(&audio_wake))
            .unwrap_or_else(|e| {
                eprintln!("error: failed to start audio capture: {e:#}");
                eprintln!("hint: is PipeWire running? (`systemctl --user status pipewire`)");
                std::process::exit(1);
            });

    // Validate saved audio source against discovered nodes; fall back to
    // SystemOutput if the saved Application node is no longer present.
    let validated_source = {
        let nodes = node_list.lock().unwrap();
        audio::validate_audio_source(cfg.audio_source.clone(), &nodes)
    };
    if validated_source != cfg.audio_source {
        cfg.audio_source = validated_source.clone();
        let _ = audio_cmd_tx.send(audio::AudioCommand::SwitchSource(validated_source));
    }

    // Probe CUDA in a subprocess so a CUDA-provider crash can't take the parent down.
    let model_dir = models::nemotron_model_dir();
    let use_cuda = main_linux::cuda_available(&model_dir);
    eprintln!("{}", main_linux::cuda_status_message(use_cuda));

    // Captions-enabled flag: read by STT thread to skip inference, by tray/overlay for UI.
    let captions_enabled = Arc::new(AtomicBool::new(!args.start_disabled));

    // Lock-free engine selection. Tray writes this; STT thread reads on each chunk.
    let engine_choice = Arc::new(ArcSwap::from_pointee(cfg.engine.clone()));

    let unload_after = if cfg.vram_unload_secs == 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(cfg.vram_unload_secs))
    };

    // Caption and command channels to the GTK main loop.
    // async-channel integrates with glib::MainContext::spawn_local so the GTK
    // side consumes them as futures rather than via 100ms polling.
    let (caption_tx, caption_rx) = async_channel::unbounded::<String>();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<overlay::OverlayCommand>();

    // Spawn the combined STT pipeline: ring consumer + resampler + engine + caption sender.
    let _stt_handle = stt::spawn_stt_thread(
        ring_consumer,
        Arc::clone(&audio_wake),
        caption_tx,
        stt::PipelineConfig {
            engine_choice: Arc::clone(&engine_choice),
            captions_enabled: Arc::clone(&captions_enabled),
            unload_after,
            model_dir: model_dir.clone(),
            use_cuda,
        },
    );

    // Spawn the system tray.
    let tray_state = tray::TrayState {
        captions_enabled: Arc::clone(&captions_enabled),
        active_source: cfg.audio_source.clone(),
        overlay_mode: cfg.overlay_mode.clone(),
        locked: cfg.locked,
        above_fullscreen: cfg.above_fullscreen,
        active_engine: cfg.engine.clone(),
        using_gpu: use_cuda,
        overlay_tx: cmd_tx.clone(),
        audio_tx: audio_cmd_tx.clone(),
        engine_choice: Arc::clone(&engine_choice),
        node_list: Arc::clone(&node_list),
    };

    // Use the already-built tokio runtime (from Phase 2 model download).
    let tray_handle = tray::spawn_tray(tray_state, &runtime);

    // Phase 8: Handle FallbackEvent from audio thread (AC1.4).
    // Capture a Tokio Handle from the runtime before spawning the plain OS thread.
    // tokio::runtime::Handle::current() panics in plain threads; we must pass the
    // Handle in from a scope where the runtime is live.
    let tokio_handle = runtime.handle().clone();
    let tray_handle_for_fallback = tray_handle.clone();
    std::thread::spawn(move || {
        for event in fallback_rx.iter() {
            // Desktop notification (AC1.4).
            let _ = notify_rust::Notification::new()
                .summary("Live Captions: Audio Source Lost")
                .body(&format!(
                    "'{}' (id:{}) disconnected — switched to System Output.",
                    event.lost_name, event.lost_id
                ))
                .timeout(notify_rust::Timeout::Milliseconds(5000))
                .show();

            // Update tray to reflect fallback source.
            // Uses the captured Handle to run the async update on the Tokio runtime.
            tokio_handle.block_on(async {
                tray_handle_for_fallback.update(|tray: &mut tray::TrayState| {
                    tray.active_source = crate::config::AudioSource::SystemOutput;
                }).await;
            });

            // Update config.
            let mut cfg = crate::config::Config::load();
            cfg.audio_source = crate::config::AudioSource::SystemOutput;
            let _ = cfg.save();
        }
    });

    // Phase 7: Start config hot-reload watcher.
    // _config_watcher must stay in scope until process exit (drop = stop watching).
    // Typed as Option so the failure path compiles without a dummy Debouncer.
    let _config_watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> =
        match config::start_hot_reload(cmd_tx.clone(), tray_handle.clone(), runtime.handle().clone()) {
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

    // Graceful shutdown on Ctrl-C / SIGTERM.
    let audio_tx_for_signal = audio_cmd_tx.clone();
    let glib_cmd_tx_for_signal = cmd_tx.clone();
    let wake_for_signal = Arc::clone(&audio_wake);
    ctrlc::set_handler(move || {
        eprintln!("info: received shutdown signal, stopping...");
        // Wake the STT thread and tell it to exit.
        wake_for_signal.shutdown();
        // Shut down the PipeWire capture thread.
        let _ = audio_tx_for_signal.send(audio::AudioCommand::Shutdown);
        // Tell GTK to quit cleanly from its main thread.
        let _ = glib_cmd_tx_for_signal.send_blocking(overlay::OverlayCommand::Quit);
    })
    .expect("setting Ctrl-C handler");

    if args.start_disabled {
        let _ = cmd_tx.send_blocking(overlay::OverlayCommand::SetVisible(false));
    }

    overlay::run_gtk_app(cfg, caption_rx, cmd_rx, Arc::clone(&captions_enabled));

    main_linux::exit_without_atexit(0)
}
