mod audio;
mod config;
mod models;
mod stt;
mod overlay;
mod tray;

use clap::Parser;
use config::Config;
use ringbuf::traits::Consumer;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

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
}

fn main() {
    // If we're a CUDA probe subprocess, run the probe and exit immediately.
    if std::env::var_os("__SUBTIDAL_CUDA_PROBE").is_some() {
        stt::run_cuda_probe();
    }

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

    // Phase 3: Start audio capture
    let (audio_cmd_tx, ring_consumer, node_list, fallback_rx) =
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

    // Probe CUDA availability by attempting a full model load in a subprocess.
    // This catches segfaults from CUDA version mismatches during session creation.
    let model_dir = models::nemotron_model_dir();
    let use_cuda = stt::cuda_available(&model_dir);
    eprintln!("{}", cuda_status_message(use_cuda));

    // Create audio chunk channel (connects Phase 3 ring buffer drain to inference).
    // Wrap the SyncSender in Arc<Mutex<>> so Phase 8 engine switching can replace it
    // at runtime without restarting the bridge thread.
    let (chunk_tx_inner, chunk_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(32);
    let chunk_tx = std::sync::Arc::new(std::sync::Mutex::new(chunk_tx_inner));
    let (caption_tx, caption_rx) = std::sync::mpsc::sync_channel::<String>(64);

    // Create shutdown flag for audio bridge thread.
    let bridge_shutdown = Arc::new(AtomicBool::new(false));

    // Spawn the audio→chunk bridge thread.
    // Drains the ring buffer, resamples, and sends 160ms chunks to the inference thread.
    // Locks chunk_tx on each send so Phase 8 can atomically swap the inner SyncSender.
    let mut ring_consumer_arc = ring_consumer;
    let chunk_tx_for_bridge = std::sync::Arc::clone(&chunk_tx);
    let bridge_shutdown_for_thread = Arc::clone(&bridge_shutdown);
    std::thread::spawn(move || {
        let mut resampler = audio::resampler::AudioResampler::new()
            .expect("creating resampler");
        let mut raw = vec![0f32; 4096];
        loop {
            if bridge_shutdown_for_thread.load(Ordering::Relaxed) {
                break;
            }
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

    // Instantiate the STT engine.
    let engine: Box<dyn stt::SttEngine> = {
        Box::new(
            stt::nemotron::NemotronEngine::new(&model_dir, use_cuda)
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to load Nemotron model: {e:#}");
                    std::process::exit(1);
                })
        )
    };

    // Clone caption_tx for engine switching before spawning the inference thread.
    let caption_tx_for_switch = caption_tx.clone();

    // Spawn the inference thread.
    let _inference_handle = stt::spawn_inference_thread(engine, chunk_rx, caption_tx);

    // Phase 6: Set up engine-switch channel.
    let (engine_switch_tx, engine_switch_rx) = std::sync::mpsc::sync_channel::<tray::EngineCommand>(4);

    // Phase 8: Wire engine-switch receiver (restarts inference thread on switch).
    // chunk_tx is Arc<Mutex<SyncSender<Vec<f32>>>> from Phase 4 Task 4.
    // The audio bridge thread calls chunk_tx.lock().unwrap().send(chunk) on every chunk.
    // When we replace *chunk_tx.lock(), the very next chunk goes to the new inference engine.
    // We store old inference thread handles in a Vec to prevent JoinHandle leaks.
    let inference_handles: Arc<Mutex<Vec<std::thread::JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let chunk_tx_for_switch = std::sync::Arc::clone(&chunk_tx); // Phase 4's Arc<Mutex<SyncSender>>
        let inference_handles = Arc::clone(&inference_handles);

        std::thread::spawn(move || {
            for cmd in engine_switch_rx.iter() {
                match cmd {
                    tray::EngineCommand::Switch(new_engine_choice) => {
                        eprintln!("info: switching STT engine to {new_engine_choice:?}");

                        let new_engine: Box<dyn stt::SttEngine> = match new_engine_choice {
                            config::Engine::Nemotron => {
                                let dir = models::nemotron_model_dir();
                                let cuda = stt::cuda_available(&dir);
                                match stt::nemotron::NemotronEngine::new(&dir, cuda) {
                                    Ok(e) => Box::new(e),
                                    Err(e) => {
                                        eprintln!("error: failed to load Nemotron: {e:#}");
                                        continue;
                                    }
                                }
                            }
                        };

                        // Spawn new inference thread and get its new SyncSender.
                        let (new_chunk_tx, handle) = stt::restart_inference_thread(
                            new_engine,
                            caption_tx_for_switch.clone(),
                        );

                        // Store the old handle to prevent JoinHandle leak.
                        // The inference thread will exit when the old chunk_tx is dropped.
                        // Before pushing, retain only handles whose threads have finished.
                        let mut handles = inference_handles.lock().unwrap();
                        handles.retain(|h| !h.is_finished());
                        handles.push(handle);

                        // Atomically replace the inner SyncSender.
                        // The audio bridge thread will send to the new inference thread on next chunk.
                        *chunk_tx_for_switch.lock().unwrap() = new_chunk_tx;

                        eprintln!("info: engine switch complete — audio bridge now targeting new engine");
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
        active_engine: cfg.engine.clone(),
        overlay_tx: cmd_tx_to_gtk.clone(),
        audio_tx: audio_cmd_tx.clone(),
        engine_tx: engine_switch_tx,
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
        match config::start_hot_reload(cmd_tx_to_gtk.clone(), tray_handle.clone(), runtime.handle().clone()) {
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

    // Phase 8: Graceful shutdown on Ctrl-C / SIGTERM.
    let audio_tx_for_signal = audio_cmd_tx.clone();
    let glib_cmd_tx_for_signal = cmd_tx_to_gtk.clone();
    let bridge_shutdown_for_signal = Arc::clone(&bridge_shutdown);
    ctrlc::set_handler(move || {
        eprintln!("info: received shutdown signal, stopping...");
        // Signal audio bridge thread to stop.
        bridge_shutdown_for_signal.store(true, Ordering::Relaxed);
        // Shut down the audio thread.
        let _ = audio_tx_for_signal.send(audio::AudioCommand::Shutdown);
        // Signal GTK4 to quit cleanly via the existing glib channel.
        // overlay::OverlayCommand::Quit calls app.quit() from the GTK main thread,
        // ensuring all Drop impls run and the GTK main loop exits normally.
        let _ = glib_cmd_tx_for_signal.send(overlay::OverlayCommand::Quit);
    })
    .expect("setting Ctrl-C handler");

    // Run GTK4 main loop (blocks until application exits).
    overlay::run_gtk_app(cfg, caption_rx_from_inference, cmd_rx, Arc::clone(&captions_enabled));
}

/// Returns the appropriate CUDA status message based on availability.
/// AC3.1 and AC3.2: Testable CUDA status logging.
fn cuda_status_message(cuda_available: bool) -> &'static str {
    if cuda_available {
        "info: CUDA available, Nemotron will use GPU acceleration"
    } else {
        "info: CUDA not available, Nemotron will use CPU"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_status_message_when_available() {
        let msg = cuda_status_message(true);
        assert!(msg.contains("GPU acceleration"));
        assert!(msg.contains("CUDA available"));
    }

    #[test]
    fn cuda_status_message_when_unavailable() {
        let msg = cuda_status_message(false);
        assert!(msg.contains("CPU"));
        assert!(msg.contains("CUDA not available"));
    }
}
