//! Main-thread caption storage and rendering shared by the app and native scenarios.
use super::transcript_window::{self, TranscriptWindowState};
use crate::config::{Config, OverlayMode};
use crate::overlay::{
    caption_buffer::CaptionBuffer, transcript_log::TranscriptLog, CaptionEvent, CaptionsEnabled,
    OverlayCommand,
};
use objc2::MainThreadMarker;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

pub(super) struct Routing<'a> {
    pub transcript: &'a TranscriptWindowState,
    pub buffer: &'a Mutex<CaptionBuffer>,
    pub log: &'a Mutex<TranscriptLog>,
    pub config: &'a Mutex<Config>,
    pub epoch: &'a AtomicU64,
    pub enabled: &'a CaptionsEnabled,
}

impl Routing<'_> {
    /// The sole caption-event storage writer. Admission epoch is captured before dispatch.
    pub fn caption(
        &self,
        event: CaptionEvent,
        admitted_epoch: u64,
        mtm: MainThreadMarker,
        mut label: impl FnMut(&str, &Config),
    ) {
        if !self.enabled.load(Ordering::Relaxed)
            || admitted_epoch != self.epoch.load(Ordering::Relaxed)
        {
            return;
        }
        let cfg = self.config.lock().unwrap().clone();
        match event {
            CaptionEvent::Append {
                text,
                speaker_id,
                emit_sample,
            } => {
                let display = {
                    let mut buffer = self.buffer.lock().unwrap();
                    buffer.push_with_speaker_and_sample(text.clone(), speaker_id, emit_sample);
                    buffer.display_text()
                };
                let (fragment, kind) = {
                    let mut log = self.log.lock().unwrap();
                    let kind = log.push_with_speaker_and_sample(text, speaker_id, emit_sample);
                    (log.fragments().last().unwrap().clone(), kind)
                };
                label(&display, &cfg);
                transcript_window::append_fragment_to_view(
                    self.transcript,
                    &fragment,
                    kind,
                    &cfg.speaker_names,
                );
            }
            CaptionEvent::Relabel {
                from_sample,
                new_speaker_id,
            } => {
                let changed = self
                    .log
                    .lock()
                    .unwrap()
                    .relabel_since(from_sample, new_speaker_id);
                let (n, display) = {
                    let mut buffer = self.buffer.lock().unwrap();
                    let n = buffer.relabel_since(from_sample, new_speaker_id);
                    (n, buffer.display_text())
                };
                if n > 0 {
                    label(&display, &cfg);
                }
                if changed > 0 {
                    transcript_window::rebuild_view(self.transcript, mtm, &cfg.speaker_names);
                }
            }
        }
    }

    /// Returns unhandled commands to the app; no persistence or panel construction here.
    pub fn command(
        &self,
        command: OverlayCommand,
        mtm: MainThreadMarker,
        mut label: impl FnMut(&str, &Config),
        mut panel_visibility: impl FnMut(bool),
    ) -> Option<OverlayCommand> {
        match command {
            OverlayCommand::SetVisible(visible) => self.visibility(
                mtm,
                visible && self.enabled.load(Ordering::Relaxed),
                &mut panel_visibility,
            ),
            OverlayCommand::SetMode(mode) => {
                self.config.lock().unwrap().overlay_mode = mode;
                self.visibility(
                    mtm,
                    self.enabled.load(Ordering::Relaxed),
                    &mut panel_visibility,
                );
            }
            OverlayCommand::SetCaptionsEnabled(enabled) => {
                self.enabled.store(enabled, Ordering::Relaxed);
                if !enabled {
                    self.epoch.fetch_add(1, Ordering::Relaxed);
                    self.log.lock().unwrap().clear();
                    transcript_window::clear_view(self.transcript, mtm);
                    self.buffer.lock().unwrap().clear();
                    let cfg = self.config.lock().unwrap().clone();
                    label("", &cfg);
                }
                self.visibility(mtm, enabled, &mut panel_visibility);
            }
            OverlayCommand::SetSpeakerNames(names) => {
                let old = {
                    let mut cfg = self.config.lock().unwrap();
                    std::mem::replace(&mut cfg.speaker_names, names.clone())
                };
                let display = {
                    let mut buffer = self.buffer.lock().unwrap();
                    for line in &mut buffer.lines {
                        let Some(id) = line.speaker_id else { continue };
                        let old_label = old
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| format!("Speaker {}", id + 1));
                        let prefix = format!("{old_label}: ");
                        if let Some(body) = line.text.strip_prefix(&prefix) {
                            let new_label = names
                                .get(&id)
                                .cloned()
                                .unwrap_or_else(|| format!("Speaker {}", id + 1));
                            line.text = format!("{new_label}: {body}");
                        }
                    }
                    buffer.speaker_names = names.clone();
                    buffer.display_text()
                };
                let cfg = self.config.lock().unwrap().clone();
                label(&display, &cfg);
                transcript_window::rebuild_view(self.transcript, mtm, &names);
            }
            other => return Some(other),
        }
        None
    }

    fn visibility(&self, mtm: MainThreadMarker, visible: bool, panel: &mut impl FnMut(bool)) {
        let mode = self.config.lock().unwrap().overlay_mode.clone();
        let transcript = visible && matches!(mode, OverlayMode::Transcript);
        panel(visible && !transcript);
        if transcript {
            transcript_window::order_front(self.transcript, mtm);
        } else {
            transcript_window::order_out(self.transcript, mtm);
        }
    }
}

#[cfg(feature = "native-transcript-tests")]
pub(super) fn run_native_routing_scenarios(mtm: MainThreadMarker) {
    use std::{
        cell::RefCell,
        sync::{atomic::AtomicBool, Arc},
        time::Duration,
    };
    let log = Arc::new(Mutex::new(TranscriptLog::new(Duration::from_millis(1500))));
    let window = transcript_window::build_transcript_window(mtm, log.clone());
    let config = Mutex::new(Config::default());
    let buffer = Mutex::new(CaptionBuffer::new(5, 120, 60));
    let epoch = AtomicU64::new(0);
    let enabled = Arc::new(AtomicBool::new(true));
    let route = Routing {
        transcript: &window.state,
        buffer: &buffer,
        log: &log,
        config: &config,
        epoch: &epoch,
        enabled: &enabled,
    };
    let label = RefCell::new(String::new());
    let set_label = |text: &str, _: &Config| {
        *label.borrow_mut() = text.into();
    };
    let command = |cmd| {
        assert!(route.command(cmd, mtm, set_label, |_| {}).is_none());
    };
    let append = |text: &str, sample| CaptionEvent::Append {
        text: text.into(),
        speaker_id: Some(0),
        emit_sample: sample,
    };
    for (i, mode) in [
        OverlayMode::Docked,
        OverlayMode::Floating,
        OverlayMode::Transcript,
    ]
    .into_iter()
    .enumerate()
    {
        command(OverlayCommand::SetMode(mode));
        route.caption(
            append(&format!(" fragment{i}"), i as u64 + 1),
            0,
            mtm,
            set_label,
        );
        assert_eq!(
            log.lock().unwrap().fragments().len(),
            i + 1,
            "one recorded fragment per append in every mode"
        );
    }
    command(OverlayCommand::SetVisible(false));
    assert!(!window.state.window.isVisible());
    route.caption(append(" hidden-tail", 10), 0, mtm, set_label);
    assert_eq!(log.lock().unwrap().fragments().len(), 4);
    assert!(window
        .state
        .text_view
        .string()
        .to_string()
        .contains("hidden-tail"));
    command(OverlayCommand::SetVisible(true));
    assert!(window
        .state
        .text_view
        .string()
        .to_string()
        .contains("hidden-tail"));
    route.caption(
        CaptionEvent::Relabel {
            from_sample: 0,
            new_speaker_id: 1,
        },
        0,
        mtm,
        set_label,
    );
    assert_eq!(log.lock().unwrap().fragments().len(), 4);
    assert!(log
        .lock()
        .unwrap()
        .fragments()
        .iter()
        .all(|f| f.speaker_id == Some(1)));
    assert!(window
        .state
        .text_view
        .string()
        .to_string()
        .contains("Speaker 2"));
    command(OverlayCommand::SetSpeakerNames(
        [(1, "Ada".into())].into_iter().collect(),
    ));
    assert!(window.state.text_view.string().to_string().contains("Ada"));
    assert!(label.borrow().contains("Ada"));
    let queued_epoch = epoch.load(Ordering::Relaxed);
    command(OverlayCommand::SetCaptionsEnabled(false));
    assert!(log.lock().unwrap().fragments().is_empty());
    assert!(window.state.text_view.string().to_string().is_empty());
    assert!(buffer.lock().unwrap().display_text().is_empty());
    assert!(label.borrow().is_empty());
    command(OverlayCommand::SetCaptionsEnabled(true));
    route.caption(append(" stale", 11), queued_epoch, mtm, set_label);
    assert!(log.lock().unwrap().fragments().is_empty());
    assert!(window.state.text_view.string().to_string().is_empty());
    route.caption(
        append(" fresh", 12),
        epoch.load(Ordering::Relaxed),
        mtm,
        set_label,
    );
    assert_eq!(log.lock().unwrap().fragments().len(), 1);
    command(OverlayCommand::SetVisible(false));
}
