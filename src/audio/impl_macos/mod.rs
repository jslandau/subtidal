//! macOS audio capture (ScreenCaptureKit). Phase 4 ships SystemOutput-only
//! capture; per-app capture and source switching land in Phase 5.

use anyhow::Result;
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
    _ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    _audio_wake: Arc<AudioWake>,
    _rx_cmd: Receiver<AudioCommand>,
) -> Result<()> {
    // Task 3 fills this in.
    anyhow::bail!("run_sck_capture not yet implemented (Phase 4 Task 3)")
}
