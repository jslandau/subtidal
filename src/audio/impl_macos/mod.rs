//! macOS audio capture (ScreenCaptureKit). Phase 4 ships SystemOutput-only
//! capture; per-app capture and source switching land in Phase 5.

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use ringbuf::HeapRb;
use ringbuf::traits::Split;

use crate::stt::AudioWake;

mod stream;     // SCStream + delegate (Task 3)
mod normalize;  // CMSampleBuffer → 48kHz stereo f32 (Task 3 + 5)

/// Commands sent to the audio thread. Phase 4 ships only `Shutdown`; Phase 5
/// adds `SwitchSource(AudioSourceId)`.
pub enum AudioCommand {
    Shutdown,
}

/// Public entry point — symmetric with `audio::impl_linux::start_audio_thread`.
/// Phase 4 tuple has 2 elements; Phase 5 widens to add a fallback-event
/// receiver and surfaces neutral `AudioSource` types.
pub fn start_audio_thread(
    audio_wake: Arc<AudioWake>,
) -> Result<(SyncSender<AudioCommand>, ringbuf::HeapCons<f32>)> {
    // Same capacity as Linux: 48000 frames × 2 channels = 96_000 f32 elements.
    const RING_BUF_CAPACITY: usize = 96_000;
    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(RING_BUF_CAPACITY).split();
    let (tx_cmd, rx_cmd) = sync_channel::<AudioCommand>(8);

    // Wrap producer in Arc<Mutex<>> so the SCK delegate (running on SCK's
    // internal dispatch queue) and the worker thread can both reference it.
    // RT-SAFE: the delegate uses try_lock only — see stream::Delegate doc.
    let ring_producer = Arc::new(Mutex::new(ring_producer));

    let producer_for_thread = Arc::clone(&ring_producer);
    let wake_for_thread = Arc::clone(&audio_wake);
    std::thread::Builder::new()
        .name("screen-capture-audio".into())
        .spawn(move || {
            if let Err(e) = run_sck_capture(producer_for_thread, wake_for_thread, rx_cmd) {
                eprintln!("error: SCK capture exited: {e:#}");
            }
        })?;

    Ok((tx_cmd, ring_consumer))
}

fn run_sck_capture(
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    audio_wake: Arc<AudioWake>,
    rx_cmd: Receiver<AudioCommand>,
) -> Result<()> {
    // Build a single-threaded tokio runtime for SCK async APIs (completion handlers).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // Fetch shareable content (displays) — this may prompt for TCC permission.
    let stream = rt.block_on(async {
        let content = stream::shareable_content_current().await
            .context("SCShareableContent — is Screen Recording permission granted?")?;
        stream::build_stream(&content, Arc::clone(&ring_producer), Arc::clone(&audio_wake))
    })?;

    // Start capturing audio from the stream.
    rt.block_on(async {
        stream::start_capture(&stream).await
            .context("SCStream.startCapture — TCC denied?")
    })?;

    // Spin until shutdown command received.
    loop {
        match rx_cmd.recv() {
            Ok(AudioCommand::Shutdown) | Err(_) => break,
        }
    }

    // Stop capturing (best-effort cleanup).
    rt.block_on(async {
        let _ = stream::stop_capture(&stream).await;
    });

    Ok(())
}
