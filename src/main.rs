mod audio;
mod config;
mod models;
mod stt;
mod overlay;
mod tray;

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

fn find_provider_dir() -> Option<std::path::PathBuf> {
    // Check next to the binary (cargo run with symlinks in target/release/)
    if let Some(exe_dir) = std::env::current_exe().ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        if exe_dir.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(exe_dir);
        }
    }

    // Use the provider directory embedded at compile time by build.rs
    if let Some(dir) = option_env!("ORT_PROVIDER_LIB_DIR") {
        let p = std::path::PathBuf::from(dir);
        if p.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(p);
        }
    }

    // Fallback: scan ort cache at runtime
    find_ort_cache_dir()
}

/// Ensure the CUDA provider .so files live next to the installed binary.
///
/// ORT's provider discovery uses dladdr on its own static code, which resolves to
/// the binary's directory (not CWD, not LD_LIBRARY_PATH). We symlink the provider
/// .so files from the ORT build cache into the exe dir so ORT's GetRuntimePath
/// finds them regardless of how subtidal was launched (CLI, app launcher, systemd).
fn ensure_provider_libs_next_to_exe() {
    let exe_dir = match std::env::current_exe().ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        Some(d) => d,
        None => return,
    };

    if exe_dir.join("libonnxruntime_providers_cuda.so").exists() {
        return;
    }

    let source_dir = match find_provider_dir() {
        Some(d) => d,
        None => return,
    };

    if source_dir == exe_dir {
        return;
    }

    for name in ["libonnxruntime_providers_cuda.so", "libonnxruntime_providers_shared.so"] {
        let src = source_dir.join(name);
        let dst = exe_dir.join(name);
        if !src.exists() || dst.exists() { continue; }
        let _ = std::os::unix::fs::symlink(&src, &dst);
    }
}

/// Locate this binary's ORT provider dir inside `~/.cache/ort.pyke.io/dfbin/`.
///
/// Keys on the exact `dist.hash` that `build.rs` embedded into the binary —
/// not mtime — so a stale sibling build of a different ORT version can't be
/// picked up and cause ABI mismatch. If the build-time hash is unavailable
/// (e.g. built without ORT_PROVIDER_LIB_DIR resolution), returns None and lets
/// the other layers of `find_provider_dir` handle it.
fn find_ort_cache_dir() -> Option<std::path::PathBuf> {
    let expected_hash = option_env!("ORT_DIST_HASH")?;
    let cache_dir = dirs::cache_dir()?.join("ort.pyke.io/dfbin");
    if !cache_dir.is_dir() {
        return None;
    }
    for arch_entry in std::fs::read_dir(&cache_dir).ok()?.flatten() {
        let candidate = arch_entry.path().join(expected_hash);
        if candidate.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(candidate);
        }
    }
    None
}

/// ORT's `GetRuntimePath` locates provider .so files relative to argv[0]'s parent
/// directory. When launched by bare name from a shell (e.g. `subtidal`), argv[0]
/// has no directory component and the runtime path becomes empty, so the CUDA
/// provider library is not found and CUDA EP registration silently falls back to
/// CPU. Re-exec ourselves with the absolute path from `current_exe()` so argv[0]
/// always has a directory component matching the real binary location.
fn reexec_with_absolute_argv0_if_needed() {
    use std::os::unix::process::CommandExt as _;

    if std::env::var_os("__SUBTIDAL_REEXECED").is_some() {
        return;
    }
    // Probe subprocess is already spawned with an absolute path by cuda_available().
    if std::env::var_os("__SUBTIDAL_CUDA_PROBE").is_some() {
        return;
    }
    let argv0 = match std::env::args_os().next() {
        Some(a) => a,
        None => return,
    };
    if std::path::Path::new(&argv0).parent().map(|p| !p.as_os_str().is_empty()).unwrap_or(false) {
        return; // already has a directory component
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let rest: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let err = std::process::Command::new(&exe)
        .args(&rest)
        .env("__SUBTIDAL_REEXECED", "1")
        .arg0(&exe)
        .exec();
    eprintln!("warn: failed to re-exec with absolute path ({err}); continuing. GPU acceleration may be unavailable.");
}

fn main() {
    reexec_with_absolute_argv0_if_needed();
    ensure_provider_libs_next_to_exe();

    // If we're a CUDA probe subprocess, run the probe and exit immediately.
    if std::env::var_os("__SUBTIDAL_CUDA_PROBE").is_some() {
        stt::run_cuda_probe();
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
    let use_cuda = stt::cuda_available(&model_dir);
    eprintln!("{}", cuda_status_message(use_cuda));

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

    // Use libc::_exit to skip all atexit handlers (both Rust and C++).
    // ORT's C++ atexit destructors call cudaFreeHost after the CUDA driver has
    // already shut down, causing SIGABRT. std::process::exit still runs atexit
    // handlers; _exit does not.
    unsafe { libc::_exit(0) }
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
