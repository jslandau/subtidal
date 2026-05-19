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
use ringbuf::traits::{Producer, Split};

/// Fixture-WAV harness: reads a 16kHz mono WAV file, upsamples to 48kHz mono
/// via rubato Fft resampler, duplicates to stereo, and feeds into the ring buffer
/// at real-time pacing. Phase 3 only; superseded by Phase 4's ScreenCaptureKit.
fn feed_fixture_wav(
    path: &str,
    ring_producer: &mut ringbuf::HeapProd<f32>,
    wake: &Arc<stt::AudioWake>,
) -> anyhow::Result<()> {
    use hound::WavReader;
    use rubato::{Fft, FixedSync, Resampler};
    use audioadapter_buffers::direct::SequentialSliceOfVecs;

    // Read the 16kHz mono fixture.
    let reader = WavReader::open(path)
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

    // Upsample 16kHz mono → 48kHz mono using rubato Fft resampler.
    // FixedSync::Input means input size is fixed, output varies.
    // Input chunk size: 1280 samples at 16kHz = 80ms
    // Output: 1280 * 3 = 3840 samples at 48kHz (same 80ms duration)
    let input_chunk_size = 1280;
    let mut resampler = Fft::<f32>::new(
        16_000, // input sample rate
        48_000, // output sample rate
        input_chunk_size,
        2, // sub-chunks
        1, // mono: 1 input channel
        FixedSync::Input,
    )?;

    let expected_output_frames = (input_chunk_size * 48_000) / 16_000;

    // Pre-allocate channel buffers for resampling.
    let mut input_vecs = vec![Vec::with_capacity(input_chunk_size)];
    let mut output_vecs = vec![vec![0.0f32; expected_output_frames]];

    // Process all samples in chunks.
    let mut mono_48k = Vec::new();
    for chunk in mono_samples.chunks(input_chunk_size) {
        input_vecs[0].clear();
        input_vecs[0].extend_from_slice(chunk);

        // If this is a partial final chunk, pad with zeros.
        if input_vecs[0].len() < input_chunk_size {
            input_vecs[0].resize(input_chunk_size, 0.0);
        }

        let input_adapter =
            SequentialSliceOfVecs::new(&input_vecs, 1, input_chunk_size)
                .map_err(|e| anyhow::anyhow!("creating input adapter: {e}"))?;

        let mut output_adapter = SequentialSliceOfVecs::new_mut(
            &mut output_vecs,
            1,
            expected_output_frames,
        )
        .map_err(|e| anyhow::anyhow!("creating output adapter: {e}"))?;

        let (_input_count, output_count) = resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, None)
            .map_err(|e| anyhow::anyhow!("resampling audio: {e}"))?;

        mono_48k.extend_from_slice(&output_vecs[0][..output_count]);
    }

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

    // Loop the fixture continuously so STT has steady audio after model warm-up.
    // The early loop iterations will drop samples while the model loads; that's expected.
    'outer: loop {
        for chunk in stereo_48k.chunks(chunk_size) {
            if wake.is_shutdown() {
                break 'outer;
            }
            let _ = ring_producer.push_slice(chunk);
            wake.notify();
            std::thread::sleep(sleep_duration);
        }
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
