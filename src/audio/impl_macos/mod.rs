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

/// User-visible message posted to the NSPanel when SCK fails to start
/// (typically because TCC permission was denied). Satisfies AC3.6's
/// "in-panel message" branch.
const TCC_DENIED_PANEL_MESSAGE: &str =
    "Grant Screen Recording permission in System Settings → Privacy & Security, then relaunch.";

/// Public entry point — symmetric with `audio::impl_linux::start_audio_thread`.
/// Phase 4 tuple has 2 elements; Phase 5 widens to add a fallback-event
/// receiver and surfaces neutral `AudioSource` types.
///
/// `error_caption_tx` is the same caption channel the STT pipeline writes
/// into; on TCC denial the audio thread posts a one-shot caption to it so
/// the user sees an actionable message in the NSPanel rather than an empty
/// overlay (AC3.6).
pub fn start_audio_thread(
    audio_wake: Arc<AudioWake>,
    error_caption_tx: async_channel::Sender<String>,
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
                // Surface the failure in the NSPanel via the caption channel.
                // send_blocking is fine here — this is the non-RT path and the
                // overlay's caption-bridge thread will drain immediately.
                let _ = error_caption_tx.send_blocking(TCC_DENIED_PANEL_MESSAGE.to_string());
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
    // The delegate is bound here and held in scope until after stop_capture;
    // dropping it earlier causes SCK to drop every audio buffer with
    // "streamOutput NOT found" (SCK does not retain the output).
    let (stream, _delegate) = rt.block_on(async {
        let content = stream::shareable_content_current().await
            .context("SCShareableContent — is Screen Recording permission granted?")?;
        stream::build_stream(&content, Arc::clone(&ring_producer), Arc::clone(&audio_wake))
    })?;

    // Start capturing audio from the stream.
    rt.block_on(async {
        stream::start_capture(&stream).await
            .context("SCStream.startCapture — TCC denied?")
    })?;

    // Wait for shutdown. Phase 5 extends the match to dispatch SwitchSource.
    let _ = rx_cmd.recv();

    // Stop capturing (best-effort cleanup).
    rt.block_on(async {
        let _ = stream::stop_capture(&stream).await;
    });

    Ok(())
}
