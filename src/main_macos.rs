//! macOS startup entry point.
//! Phase 3 implementation: real STT pipeline with fixture-WAV harness.
//! Phases 4-5 wire in live audio capture via ScreenCaptureKit.

use objc2::MainThreadMarker;
use subtidal::config::{self, Config};
use subtidal::overlay::{self, OverlayCommand, CaptionsEnabled};
use subtidal::{models, stt};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use ringbuf::HeapRb;
use ringbuf::traits::Split;

/// Fixture-WAV harness: reads a 16kHz mono WAV file, upsamples to 48kHz mono
/// via rubato SincFixedIn, duplicates to stereo, and feeds into the ring buffer
/// at real-time pacing. Phase 3 only; superseded by Phase 4's ScreenCaptureKit.
fn feed_fixture_wav(
    path: &str,
    ring_producer: &mut ringbuf::HeapProd<f32>,
    wake: &Arc<stt::AudioWake>,
) -> anyhow::Result<()> {
    use hound::WavReader;
    use rubato::{Resampler, SincFixedIn};

    // Read the 16kHz mono fixture.
    let mut reader = WavReader::open(path)
        .map_err(|e| anyhow::anyhow!("failed to open fixture WAV: {e}"))?;
    let spec = reader.spec();

    if spec.sample_rate != 16000 {
        return Err(anyhow::anyhow!(
            "fixture WAV must be 16kHz (got {}Hz)",
            spec.sample_rate
        ));
    }

    if spec.channels != 1 {
        return Err(anyhow::anyhow!(
            "fixture WAV must be mono (got {} channels)",
            spec.channels
        ));
    }

    // Read all samples from the fixture.
    let mono_samples: Vec<f32> = reader
        .into_samples::<i16>()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|s| s as f32 / 32768.0)
        .collect();

    // Upsample 16kHz mono → 48kHz mono using rubato SincFixedIn.
    // SincFixedIn requires input in the form of Vec<Vec<f32>> (per-channel).
    let mut resampler = SincFixedIn::<f32>::new(
        3.0, // ratio: 48kHz / 16kHz = 3.0
        2.0, // oversampling factor for SincFixedIn
        rubato::SincInterpolationType::Linear,
        256, // chunk size (arbitrary; we process the entire fixture at once)
        1,   // mono: 1 input channel
    );

    // Wrap mono samples in a Vec<Vec<f32>> for rubato API.
    let input = vec![mono_samples];
    let (mono_48k, _) = resampler.process(&input, None)
        .map_err(|e| anyhow::anyhow!("resampling failed: {e}"))?;
    let mono_48k = mono_48k.into_iter().next().unwrap();

    // Duplicate mono to stereo (interleaved L, R, L, R, ...).
    let mut stereo_48k = Vec::with_capacity(mono_48k.len() * 2);
    for &sample in &mono_48k {
        stereo_48k.push(sample); // L
        stereo_48k.push(sample); // R
    }

    // Pace the pushes: 48kHz stereo = 96000 samples/sec = 96 interleaved samples per millisecond.
    // Feed 960 interleaved samples (10ms of 48kHz stereo) every 10ms.
    let chunk_size = 960; // 10ms of 48kHz stereo interleaved
    let sleep_duration = std::time::Duration::from_millis(10);

    for chunk in stereo_48k.chunks(chunk_size) {
        if wake.is_shutdown() {
            break;
        }
        let n = ring_producer.push_slice(chunk);
        if n < chunk.len() {
            eprintln!(
                "warn: ring buffer full, dropped {} samples",
                chunk.len() - n
            );
        }
        wake.notify();
        std::thread::sleep(sleep_duration);
    }

    Ok(())
}

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

    // 4. Create shared AudioWake primitive for STT thread coordination.
    let audio_wake = Arc::new(stt::AudioWake::new());

    // 5. Create ring buffer for audio samples (same capacity as Linux: 48kHz stereo, 1 sec).
    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(48_000 * 2).split();

    // 6. Lock-free engine selection (single engine for now, but the infrastructure exists).
    let engine_choice = Arc::new(arc_swap::ArcSwap::new(Arc::new(config::Engine::Nemotron)));

    // 7. Construct shared state: captions_enabled flag.
    let captions_enabled: CaptionsEnabled = Arc::new(AtomicBool::new(true));

    // 8. Build async channels for captions and overlay commands.
    let (caption_tx, caption_rx) = async_channel::unbounded::<String>();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<OverlayCommand>();

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

    // 10. Phase 3 only: spawn the fixture-WAV harness to feed the ring buffer.
    // Superseded by Phase 4's ScreenCaptureKit integration.
    {
        let mut ring_producer = ring_producer;
        let wake = Arc::clone(&audio_wake);
        std::thread::Builder::new()
            .name("phase3-wav-harness".into())
            .spawn(move || {
                feed_fixture_wav(
                    "tests/fixtures/macos-webgpu-smoke.wav",
                    &mut ring_producer,
                    &wake,
                )
                .unwrap_or_else(|e| eprintln!("warn: fixture harness exited: {e}"));
            })
            .expect("spawn phase3-wav-harness");
    }

    // 11. Install Ctrl-C handler to post OverlayCommand::Quit.
    let cmd_tx_signal = cmd_tx.clone();
    let wake_for_signal = Arc::clone(&audio_wake);
    ctrlc::set_handler(move || {
        wake_for_signal.shutdown();
        let _ = cmd_tx_signal.send_blocking(OverlayCommand::Quit);
    })
    .expect("install ctrlc handler");

    // 12. Call overlay::run_app to build the panel and run NSApplication.run().
    // This blocks until Quit is posted (from ctrlc or signal handler).
    overlay::run_app(config, caption_rx, cmd_rx, captions_enabled);

    // 13. After run_app returns, release the STT thread and clean up.
    audio_wake.shutdown();
    drop(caption_tx);
    drop(cmd_tx);

    // 14. Exit cleanly with code 0 (no CUDA atexit cleanup on macOS).
    std::process::exit(0);
}
