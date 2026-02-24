//! PipeWire audio capture: stream setup, node enumeration, runtime source switching.

#![allow(dead_code)]

pub mod resampler;

use anyhow::Context;
use anyhow::Result;
use pipewire as pw;
use pw::properties::properties;
use ringbuf::HeapRb;
use ringbuf::traits::{Producer, Split};
use std::sync::{Arc, Mutex};
use std::thread;

/// A discovered PipeWire audio node (sink or application stream).
#[derive(Debug, Clone)]
pub struct AudioNode {
    pub node_id: u32,
    pub name: String,
    pub description: String,
    /// true = system sink/monitor; false = application output stream
    pub is_monitor: bool,
}

/// Commands sent to the PipeWire thread for runtime control.
pub enum AudioCommand {
    /// Switch to a new audio source.
    SwitchSource(crate::config::AudioSource),
    /// Shut down the PipeWire thread.
    Shutdown,
}

/// Shared list of discovered audio nodes (updated by registry callbacks).
pub type NodeList = Arc<Mutex<Vec<AudioNode>>>;

/// Wrapper holding both the PipeWire stream and its associated listener.
/// Ensures both are dropped together when the stream is switched or disconnected,
/// preventing listener memory leaks.
struct CaptureStream<'a> {
    stream: pw::stream::StreamBox<'a>,
    _listener: Box<dyn std::any::Any>,
}

/// Ring buffer capacity: 1 second of 48kHz stereo f32 samples.
/// HeapRb<f32> counts f32 elements, not bytes — so 48000 frames × 2 channels = 96_000 elements.
const RING_BUF_CAPACITY: usize = 48_000 * 2;

/// Start the PipeWire audio capture thread.
///
/// Returns:
/// - `tx_cmd`: send AudioCommand to the PipeWire thread
/// - `rx_audio`: receive raw interleaved stereo 48kHz f32 samples (drained by inference thread)
/// - `node_list`: shared list of available audio nodes (updated by registry)
///
/// Exits the process if PipeWire is unavailable (AC1.5).
pub fn start_audio_thread(
    initial_source: crate::config::AudioSource,
) -> Result<(
    std::sync::mpsc::SyncSender<AudioCommand>,
    ringbuf::HeapCons<f32>,
    NodeList,
)> {
    // Initialize PipeWire library (must be called before any PW objects).
    pw::init();

    // Test PipeWire availability by attempting to create a MainLoop.
    // If this fails, PipeWire is unavailable (AC1.5).
    // The actual MainLoop is created on the PipeWire thread below.

    let (ring_producer, ring_consumer) = HeapRb::<f32>::new(RING_BUF_CAPACITY).split();
    let ring_producer = Arc::new(Mutex::new(ring_producer));

    let node_list: NodeList = Arc::new(Mutex::new(Vec::new()));
    let node_list_clone = Arc::clone(&node_list);

    let (tx_cmd, rx_cmd) = std::sync::mpsc::sync_channel::<AudioCommand>(8);

    let ring_producer_thread = Arc::clone(&ring_producer);

    thread::Builder::new()
        .name("pipewire-audio".to_string())
        .spawn(move || {
            if let Err(e) = run_pipewire_loop(
                initial_source,
                ring_producer_thread,
                node_list_clone,
                rx_cmd,
            ) {
                eprintln!("error: PipeWire audio thread exited: {e:#}");
                std::process::exit(1);
            }
        })
        .context("spawning PipeWire thread")?;

    Ok((tx_cmd, ring_consumer, node_list))
}

/// Enumerate available audio nodes from the shared node list.
/// Called by the tray to build the audio source submenu.
pub fn list_nodes(node_list: &NodeList) -> Vec<AudioNode> {
    node_list.lock().unwrap().clone()
}

/// Main PipeWire event loop (runs on dedicated thread).
fn run_pipewire_loop(
    initial_source: crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    node_list: NodeList,
    rx_cmd: std::sync::mpsc::Receiver<AudioCommand>,
) -> Result<()> {
    let mainloop = pw::main_loop::MainLoopRc::new(None)
        .context("creating PipeWire MainLoop — is PipeWire running?")?;
    let context = pw::context::ContextRc::new(&mainloop, None)
        .context("creating PipeWire Context")?;
    let core = context.connect_rc(None)
        .context("connecting to PipeWire — is PipeWire running?")?;
    let registry = core.get_registry()
        .context("getting PipeWire Registry")?;

    // Collect disappeared node IDs from the registry global_remove callback.
    // Phase 8's NodeDisappeared handler reads this list in the command loop below.
    let disappeared_node_ids: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));

    // Listen for node additions/removals to populate node_list.
    let node_list_registry = Arc::clone(&node_list);
    let _registry_listener = registry
        .add_listener_local()
        .global(move |global| {
            // Filter for audio nodes: application streams and monitor sinks.
            // global.props contains node properties.
            if let Some(props) = &global.props {
                let media_class: &str = props.get("media.class").unwrap_or("");
                let node_name = props.get("node.name").unwrap_or("").to_string();
                let description = props.get("node.description")
                    .or(props.get("node.nick"))
                    .unwrap_or(&node_name)
                    .to_string();

                let is_monitor = media_class == "Audio/Source"
                    && node_name.ends_with(".monitor");
                let is_app_stream = media_class == "Stream/Output/Audio";

                if is_monitor || is_app_stream {
                    let node = AudioNode {
                        node_id: global.id,
                        name: node_name,
                        description,
                        is_monitor,
                    };
                    node_list_registry.lock().unwrap().push(node);
                }
            }
        })
        .global_remove({
            // Phase 8 wires this to AudioCommand::NodeDisappeared (AC1.4).
            // We use an Arc<Mutex<Vec<u32>>> to collect disappeared node IDs
            // so the registry closure (which runs during mainloop.iterate()) can
            // communicate with the command-processing loop below without a second channel.
            let disappeared_ids = Arc::clone(&disappeared_node_ids);
            move |id| {
                disappeared_ids.lock().unwrap().push(id);
            }
        })
        .register();

    // Create the capture stream for the initial source.
    let mut _capture = create_capture_stream(&core, &initial_source, Arc::clone(&ring_producer))?;

    // Poll for AudioCommands and run the PipeWire event loop.
    // PipeWire Loop::iterate() processes pending events non-blockingly.
    let loop_ref = mainloop.loop_();
    loop {
        let _ = loop_ref.iterate(std::time::Duration::from_millis(10));

        match rx_cmd.try_recv() {
            Ok(AudioCommand::Shutdown) => break,
            Ok(AudioCommand::SwitchSource(new_source)) => {
                // Drop the current capture (stream and listener) to disconnect it from PipeWire.
                drop(_capture);
                // Reconnect to the new source.
                match create_capture_stream(&core, &new_source, Arc::clone(&ring_producer)) {
                    Ok(c) => {
                        _capture = c;
                        eprintln!("info: audio source switched to {:?}", new_source);
                    }
                    Err(e) => {
                        eprintln!("warn: failed to switch audio source: {e:#}");
                        // Attempt fallback to system output.
                        match create_capture_stream(
                            &core,
                            &crate::config::AudioSource::SystemOutput,
                            Arc::clone(&ring_producer),
                        ) {
                            Ok(c) => {
                                _capture = c;
                                eprintln!("warn: fell back to system output capture");
                            }
                            Err(e2) => {
                                eprintln!("error: failed to reconnect audio: {e2:#}");
                                return Err(e2);
                            }
                        }
                    }
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }

        // Phase 8: drain nodes that disappeared during this iterate() call.
        // The registry global_remove callback appends disappeared node IDs to
        // `disappeared_node_ids`. Phase 8 adds AudioCommand::NodeDisappeared handling
        // below via the fallback_tx; for Phase 3, just drain to avoid unbounded growth.
        // (Phase 8 replaces this comment with actual fallback logic.)
        if let Ok(mut ids) = disappeared_node_ids.try_lock() {
            ids.retain(|&id| {
                // Phase 8: check if `id` is the currently captured node and fall back.
                // For Phase 3: remove known nodes from the list so the tray stays accurate.
                node_list.lock().unwrap().retain(|n| n.node_id != id);
                false // retain returns false = remove the entry from disappeared_node_ids
            });
        }
    }

    Ok(())
}

/// Create a PipeWire capture stream connected to the given AudioSource.
/// Returns a CaptureStream wrapper holding both the stream and its listener,
/// ensuring proper cleanup when switched or dropped.
fn create_capture_stream<'a>(
    core: &'a pw::core::CoreRc,
    source: &crate::config::AudioSource,
    ring_producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
) -> Result<CaptureStream<'a>> {
    use pw::spa::pod::Pod;
    use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};

    // Build stream properties.
    let target_node = match source {
        crate::config::AudioSource::SystemOutput => None,
        crate::config::AudioSource::Application { node_id, .. } => Some(node_id.to_string()),
    };

    let mut stream_props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Communication",
        *pw::keys::APP_NAME => "live-captions",
        *pw::keys::NODE_NAME => "live-captions-capture",
    };

    if let Some(target) = &target_node {
        stream_props.insert("target.object", target.as_str());
    } else {
        // System output monitor: connect to the default monitor sink.
        stream_props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }

    let stream = pw::stream::StreamBox::new(core, "live-captions-capture", stream_props)
        .context("creating PipeWire stream")?;

    // Build SPA format parameters: F32LE, 48kHz, stereo.
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(48_000);
    audio_info.set_channels(2);

    // Encode the SPA param as a POD.
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )
    .context("serializing SPA audio format pod")?
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).context("creating SPA Pod")?];

    // Register the process callback (real-time — no allocation, no blocking).
    let ring_producer_cb = Arc::clone(&ring_producer);
    let _listener = stream
        .add_local_listener_with_user_data(ring_producer_cb)
        .process(|stream, ring_producer| {
            if let Some(mut buf) = stream.dequeue_buffer() {
                let datas = buf.datas_mut();
                if let Some(data) = datas.first_mut() {
                    let chunk = data.chunk();
                    let offset = chunk.offset() as usize;
                    let size = chunk.size() as usize;
                    if let Some(bytes) = data.data() {
                        let float_bytes = &bytes[offset..offset + size];
                        // Convert bytes to f32 slice (F32LE, native endian on x86).
                        let samples = bytemuck::cast_slice::<u8, f32>(float_bytes);
                        // Push to ring buffer — never block in RT context.
                        if let Ok(mut prod) = ring_producer.try_lock() {
                            let _ = prod.push_slice(samples); // drop samples if ring full
                        }
                    }
                }
            }
        })
        .register()
        .context("registering PipeWire stream listener")?;

    // Connect the stream.
    stream.connect(
        pw::spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )
    .context("connecting PipeWire capture stream")?;

    // Return both stream and listener wrapped together to ensure proper cleanup.
    Ok(CaptureStream {
        stream,
        _listener: Box::new(_listener),
    })
}

/// Validate that a saved audio source is still available.
/// If an Application source references a node_id that no longer exists,
/// falls back to SystemOutput (which is always available).
///
/// Returns the validated source (either the input source if valid, or SystemOutput as fallback).
pub fn validate_audio_source(
    saved_source: crate::config::AudioSource,
    current_nodes: &[AudioNode],
) -> crate::config::AudioSource {
    match &saved_source {
        crate::config::AudioSource::SystemOutput => {
            // System output is always available.
            saved_source
        }
        crate::config::AudioSource::Application { node_id, .. } => {
            // Check if the saved node_id exists in current nodes.
            if current_nodes.iter().any(|n| n.node_id == *node_id) {
                saved_source
            } else {
                eprintln!("warn: saved audio source (node_id={}) no longer available", node_id);
                eprintln!("warn: falling back to system output");
                crate::config::AudioSource::SystemOutput
            }
        }
    }
}
