//! SCStream construction + RT-safe audio callback (Phase 4 Task 3).
//!
//! Defines the Obj-C `Delegate` class conforming to `SCStreamOutput` and `SCStreamDelegate`,
//! implements the RT-safe audio callback, and provides async wrappers for SCK's
//! completion-handler APIs.

use anyhow::{Context, Result};
use block2::RcBlock;
use objc2::define_class;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{msg_send, AnyThread, DefinedClass};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSArray, NSError, NSObject};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamOutput, SCStreamOutputType, SCWindow,
};
use ringbuf::traits::Producer;
use ringbuf::HeapProd;
use std::sync::{Arc, Mutex};

use super::normalize;
use crate::stt::AudioWake;

pub struct DelegateIvars {
    producer: Arc<Mutex<HeapProd<f32>>>,
    wake: Arc<AudioWake>,
}

define_class!(
    /// SCStream delegate + output handler for system audio capture.
    ///
    /// `stream_didOutput` runs on ScreenCaptureKit's internal dispatch queue
    /// under RT-safety constraints (see header comment on the callback).
    #[unsafe(super(NSObject))]
    #[ivars = DelegateIvars]
    pub struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl SCStreamOutput for Delegate {
        // RT-SAFE: this callback runs on ScreenCaptureKit's internal dispatch queue.
        // MUST NOT:
        //   - allocate (no Vec::push beyond ring capacity, no String::new, no Box::new)
        //   - call eprintln!/println!/log/tracing
        //   - call Mutex::lock (use try_lock only)
        //   - call any blocking I/O
        // Target callback duration: <50us. Drop samples silently on contention.
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[allow(non_snake_case)]
        unsafe fn stream_didOutput(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            of_type: SCStreamOutputType,
        ) {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            let Some(pcm) = normalize::extract_pcm(sample_buffer) else {
                return;
            };
            let ivars = self.ivars();
            if let Ok(mut prod) = ivars.producer.try_lock() {
                let _ = prod.push_slice(pcm);
            }
            ivars.wake.notify();
        }
    }

    unsafe impl SCStreamDelegate for Delegate {
        // Non-RT path: logging permitted (matches Phase 2/3 precedent).
        #[unsafe(method(stream:didStopWithError:))]
        #[allow(non_snake_case)]
        unsafe fn stream_didStopWithError(&self, _stream: &SCStream, error: &NSError) {
            eprintln!("warn: SCStream stopped: {}", error.localizedDescription());
        }
    }
);

impl Delegate {
    pub fn new(producer: Arc<Mutex<HeapProd<f32>>>, wake: Arc<AudioWake>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { producer, wake });
        unsafe { msg_send![super(this), init] }
    }
}

/// Build an SCStream configured for system audio capture (SystemOutput only).
pub fn build_stream(
    content: &SCShareableContent,
    producer: Arc<Mutex<HeapProd<f32>>>,
    wake: Arc<AudioWake>,
) -> Result<Retained<SCStream>> {
    unsafe {
        let displays = content.displays();
        let display = displays.firstObject().context("no displays available")?;

        let empty_apps: Retained<NSArray<SCRunningApplication>> = NSArray::new();
        let empty_windows: Retained<NSArray<SCWindow>> = NSArray::new();

        let filter = SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty_apps,
            &empty_windows,
        );

        let config = SCStreamConfiguration::new();
        config.setCapturesAudio(true);
        config.setExcludesCurrentProcessAudio(true);
        config.setSampleRate(48_000);
        config.setChannelCount(2);

        let delegate = Delegate::new(producer, wake);
        let delegate_proto: &ProtocolObject<dyn SCStreamDelegate> =
            ProtocolObject::from_ref(&*delegate);

        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(delegate_proto),
        );

        let output_proto: &ProtocolObject<dyn SCStreamOutput> =
            ProtocolObject::from_ref(&*delegate);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                output_proto,
                SCStreamOutputType::Audio,
                None,
            )
            .map_err(|e| anyhow::anyhow!("addStreamOutput: {}", e.localizedDescription()))?;

        Ok(stream)
    }
}

/// Fetch the current shareable content (displays, windows, applications).
///
/// First invocation triggers the macOS TCC prompt for Screen Recording.
pub async fn shareable_content_current() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Retained<SCShareableContent>>>();
    let tx_cell = std::sync::Mutex::new(Some(tx));

    unsafe {
        let block = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let Some(tx) = tx_cell.lock().ok().and_then(|mut g| g.take()) else {
                    return;
                };
                let result = if !error.is_null() {
                    Err(anyhow::anyhow!(
                        "SCShareableContent: {}",
                        (*error).localizedDescription()
                    ))
                } else if !content.is_null() {
                    Ok(Retained::retain(content)
                        .expect("non-null SCShareableContent already retained by callback"))
                } else {
                    Err(anyhow::anyhow!("SCShareableContent: nil content and nil error"))
                };
                let _ = tx.send(result);
            },
        );

        SCShareableContent::getShareableContentWithCompletionHandler(&block);
    }

    rx.await.context("shareable_content_current channel closed")?
}

/// Start capturing audio.
pub async fn start_capture(stream: &SCStream) -> Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<()>>();
    let tx_cell = std::sync::Mutex::new(Some(tx));

    unsafe {
        let block = RcBlock::new(move |error: *mut NSError| {
            let Some(tx) = tx_cell.lock().ok().and_then(|mut g| g.take()) else {
                return;
            };
            let result = if !error.is_null() {
                Err(anyhow::anyhow!(
                    "startCapture: {}",
                    (*error).localizedDescription()
                ))
            } else {
                Ok(())
            };
            let _ = tx.send(result);
        });

        stream.startCaptureWithCompletionHandler(Some(&block));
    }

    rx.await.context("start_capture channel closed")?
}

/// Stop capturing audio. Best-effort; errors are surfaced to the caller.
pub async fn stop_capture(stream: &SCStream) -> Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<()>>();
    let tx_cell = std::sync::Mutex::new(Some(tx));

    unsafe {
        let block = RcBlock::new(move |error: *mut NSError| {
            let Some(tx) = tx_cell.lock().ok().and_then(|mut g| g.take()) else {
                return;
            };
            let result = if !error.is_null() {
                Err(anyhow::anyhow!(
                    "stopCapture: {}",
                    (*error).localizedDescription()
                ))
            } else {
                Ok(())
            };
            let _ = tx.send(result);
        });

        stream.stopCaptureWithCompletionHandler(Some(&block));
    }

    rx.await.context("stop_capture channel closed")?
}
