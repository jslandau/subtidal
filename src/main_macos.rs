//! macOS startup entry point.
//! Phase 5 (revised) implementation: real STT pipeline fed by Core Audio Process Taps.

use objc2::MainThreadMarker;
use subtidal::audio::{self, AudioCommand};
use subtidal::config::{self, Config};
use subtidal::overlay::{self, CaptionsEnabled, OverlayCommand};
use subtidal::{models, stt};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Main entry point for macOS Subtidal.
/// Acquires main-thread proof, loads config, builds shared state, spawns workers,
/// and calls overlay::run_app to block in NSApplication.run() until shutdown.
pub fn main() {
    // 1. Acquire MainThreadMarker at the very top. main() always starts on the main
    // thread, but be explicit.
    let _mtm = MainThreadMarker::new()
        .expect("main_macos::main must run on the main thread");

    // 2. Load config (use the neutral Config::load() which defaults gracefully).
    let config = Config::load();

    // 3. Ensure model files are present before starting.
    let model_dir = models::nemotron_model_dir();
    if !models::nemotron_models_present() {
        println!("Downloading Nemotron model files (first run)...");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(async {
            models::ensure_nemotron_models().await
                .unwrap_or_else(|e| {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                });
        });
    }

    // 3b. Request notification authorization (best-effort, ignore result).
    // This surfaces the macOS notification permission prompt. If denied,
    // notifications silently fail but the audio watchdog and captions still work.
    audio::notify_request_authorization_best_effort();

    // 4. Create shared AudioWake primitive for STT thread coordination.
    let audio_wake = Arc::new(stt::AudioWake::new());

    // 5. Lock-free engine selection (single engine for now, but the infrastructure exists).
    let engine_choice = Arc::new(arc_swap::ArcSwap::new(Arc::new(config::Engine::Nemotron)));

    // 6. Construct shared state: captions_enabled flag.
    let captions_enabled: CaptionsEnabled = Arc::new(AtomicBool::new(true));

    // 7. Build async channels for captions and overlay commands.
    let (caption_tx, caption_rx) = async_channel::unbounded::<String>();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<OverlayCommand>();

    // 8. Start Core Audio Process Tap audio capture. This spawns the audio-tap-worker
    // thread which constructs the tap and pushes samples into the ring buffer.
    // First launch triggers the Audio Capture TCC permission prompt. The caption
    // channel is handed over so the audio thread can post an in-panel error
    // message if TCC is denied (AC3.6).
    let (audio_cmd_tx, ring_consumer, fallback_rx) =
        audio::start_audio_thread(config.audio_source.clone(), Arc::clone(&audio_wake), caption_tx.clone())
            .unwrap_or_else(|e| {
                eprintln!("error: failed to start audio capture: {e:#}");
                eprintln!("hint: grant Audio Capture permission to Subtidal.app in System Settings → Privacy & Security.");
                std::process::exit(1);
            });

    // 8b. Spawn a listener thread for fallback events. This thread drains the receiver
    // and logs each event for diagnostic purposes (and future tray-state updates).
    let _fallback_listener = std::thread::Builder::new()
        .name("audio-fallback-listener".into())
        .spawn(move || {
            while let Ok(event) = fallback_rx.recv() {
                eprintln!(
                    "info: audio source '{}' disappeared; switched to {:?}",
                    event.previous_label, event.new_source
                );
            }
        })
        .expect("spawn fallback listener thread");

    // 9. Construct the STT pipeline configuration and spawn the thread.
    let pipeline_cfg = stt::PipelineConfig {
        engine_choice: Arc::clone(&engine_choice),
        captions_enabled: Arc::clone(&captions_enabled),
        unload_after: Some(std::time::Duration::from_secs(8)), // default from Linux
        model_dir: model_dir.clone(),
        use_cuda: true, // Request WebGPU on macOS; CPU fallback is automatic.
    };
    let _stt_handle = stt::spawn_stt_thread(
        ring_consumer,
        Arc::clone(&audio_wake),
        caption_tx.clone(),
        pipeline_cfg,
    );

    // 10. Install Ctrl-C handler to post OverlayCommand::Quit and signal both
    // the STT pipeline and SCK audio thread to shut down.
    let cmd_tx_signal = cmd_tx.clone();
    let audio_tx_signal = audio_cmd_tx.clone();
    let wake_for_signal = Arc::clone(&audio_wake);
    ctrlc::set_handler(move || {
        wake_for_signal.shutdown();
        let _ = audio_tx_signal.send(AudioCommand::Shutdown);
        let _ = cmd_tx_signal.send_blocking(OverlayCommand::Quit);
    })
    .expect("install ctrlc handler");

    // Call overlay::run_app to build the panel and run NSApplication.run().
    // This blocks until Quit is posted by the ctrlc handler.
    overlay::run_app(config, caption_rx, cmd_rx, captions_enabled);

    // 13. After run_app returns, release the STT thread + SCK audio thread and clean up.
    audio_wake.shutdown();
    let _ = audio_cmd_tx.send(AudioCommand::Shutdown);
    drop(caption_tx);
    drop(cmd_tx);

    // 14. Exit cleanly with code 0 (no CUDA atexit cleanup on macOS).
    std::process::exit(0);
}
