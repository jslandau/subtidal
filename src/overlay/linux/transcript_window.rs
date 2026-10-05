//! Transcript GTK4 window: widget tree, autoscroll logic, and caption display.
//!
//! Handles the construction of a regular (non-layer-shell) ApplicationWindow containing
//! a ScrolledWindow + TextView for displaying timestamped speech fragments. Provides
//! `append_fragment_to_view` (called per caption) and `clear_view` (reset on session end).

use crate::overlay::transcript_follow::FollowState;
use crate::overlay::transcript_log::{AppendKind, Fragment};
use crate::overlay::transcript_presentation::{gtk_offset, MetadataKind, Presentation};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Button, HeaderBar, ScrolledWindow, TextBuffer, TextTag,
    TextTagTable, TextView, WrapMode,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Handles needed by the orchestration layer to drive the transcript window.
/// All fields are GTK objects holding internal `Rc` reference counts; this
/// struct is `Clone` for cheap propagation into closures.
#[derive(Clone)]
pub struct TranscriptWindowState {
    pub window: ApplicationWindow,
    pub buffer: TextBuffer,
    pub scrolled: ScrolledWindow,
    /// Tag applied to the timestamp prefix on each paragraph.
    pub timestamp_tag: TextTag,
    speaker_tag: TextTag,
    tick: Rc<RefCell<Option<gtk4::TickCallbackId>>>,
    pub text_view: TextView,
    presentation: Rc<RefCell<Presentation>>,
    follow: Rc<RefCell<FollowState>>,
    programmatic: Rc<Cell<bool>>,
    user_input: Rc<Cell<bool>>,
    input_serial: Rc<Cell<u64>>,
    input_settle_serial: Rc<Cell<u64>>,
    input_cleanup: Rc<RefCell<Option<glib::SourceId>>>,
    #[cfg(test)]
    cleanup_registrations: Rc<Cell<usize>>,
    #[cfg(test)]
    cleanup_current: Rc<Cell<usize>>,
    scroll_sequence: Rc<Cell<bool>>,
    input_baseline: Rc<Cell<Option<(f64, f64, f64)>>>,
    reading_anchor: Rc<
        Cell<
            Option<(
                crate::overlay::transcript_presentation::FragmentPosition,
                i32,
            )>,
        >,
    >,
    follow_button: gtk4::CheckButton,
    status_label: gtk4::Label,
    empty_label: gtk4::Label,
}

impl TranscriptWindowState {
    fn update_status(&self) {
        let follow = self.follow.borrow();
        self.status_label.set_text(follow.status());
    }

    fn capture_anchor(&self) {
        let rect = self.text_view.visible_rect();
        if let Some(iter) = self.text_view.iter_at_location(rect.x(), rect.y()) {
            let p = self.presentation.borrow();
            let byte = p
                .text
                .char_indices()
                .nth(iter.offset().max(0) as usize)
                .map(|(b, _)| b)
                .unwrap_or(p.text.len());
            if let Some(position) = p.position(byte) {
                self.reading_anchor.set(Some((
                    position,
                    rect.y() - self.text_view.iter_location(&iter).y(),
                )));
            }
        }
    }

    fn restore_anchor(&self) {
        if self.user_input.get() {
            return;
        }
        let Some((position, pixel)) = self.reading_anchor.get() else {
            return;
        };
        if !self.window.is_mapped() {
            return;
        }
        self.cancel_follow();
        let generation = self.follow.borrow().generation;
        let state = self.clone();
        let previous = Cell::new(None);
        let frames = Cell::new(0);
        let id = self.text_view.add_tick_callback(move |_, _| {
            if state.follow.borrow().generation != generation || !state.window.is_mapped() {
                state.tick.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let adj = state.scrolled.vadjustment();
            let geometry = (adj.upper(), adj.page_size(), state.text_view.width());
            frames.set(frames.get() + 1);
            if previous.get() != Some(geometry) && frames.get() < 8 {
                previous.set(Some(geometry));
                return glib::ControlFlow::Continue;
            }
            let target = {
                let p = state.presentation.borrow();
                let iter = state
                    .buffer
                    .iter_at_offset(gtk_offset(&p.text, p.resolve(position)) as i32);
                f64::from(state.text_view.iter_location(&iter).y() + pixel)
            };
            state.programmatic.set(true);
            adj.set_value(target);
            state.programmatic.set(false);
            state.tick.borrow_mut().take();
            glib::ControlFlow::Break
        });
        *self.tick.borrow_mut() = Some(id);
    }

    fn cancel_input_cleanup(&self) {
        let source = self.input_cleanup.borrow_mut().take();
        if let Some(source) = source {
            source.remove();
            #[cfg(test)]
            self.cleanup_current.set(self.cleanup_current.get() - 1);
        }
    }

    fn input_cleanup_finished(&self) {
        // Returning Break removes the executing source; do not remove it twice.
        let source = self.input_cleanup.borrow_mut().take();
        #[cfg(test)]
        if source.is_some() {
            self.cleanup_current.set(self.cleanup_current.get() - 1);
        }
        drop(source);
    }

    #[cfg(test)]
    pub(super) fn test_cleanup_counts(&self) -> (usize, usize) {
        (self.cleanup_registrations.get(), self.cleanup_current.get())
    }

    fn cancel_follow(&self) {
        let id = self.tick.borrow_mut().take();
        if let Some(id) = id {
            id.remove();
        }
        self.follow.borrow_mut().invalidate();
    }

    #[cfg(test)]
    pub(super) fn test_anchor(
        &self,
    ) -> Option<(
        crate::overlay::transcript_presentation::FragmentPosition,
        i32,
    )> {
        self.capture_anchor();
        self.reading_anchor.get()
    }
    #[cfg(test)]
    pub(super) fn test_anchor_pixel(
        &self,
        position: crate::overlay::transcript_presentation::FragmentPosition,
    ) -> i32 {
        let p = self.presentation.borrow();
        let iter = self
            .buffer
            .iter_at_offset(gtk_offset(&p.text, p.resolve(position)) as i32);
        self.text_view.visible_rect().y() - self.text_view.iter_location(&iter).y()
    }
    #[cfg(test)]
    pub(super) fn test_follow_enabled(&self, enabled: bool) {
        self.follow_button.set_active(enabled);
    }
    #[cfg(test)]
    pub(super) fn test_input_active(&self) -> bool {
        self.user_input.get()
    }
    #[cfg(test)]
    pub(super) fn test_pending(&self) -> bool {
        self.tick.borrow().is_some()
    }
    #[cfg(test)]
    pub(super) fn test_user_scroll(&self, value: f64) {
        self.cancel_follow();
        let adj = self.scrolled.vadjustment();
        self.programmatic.set(true);
        adj.set_value(value);
        self.programmatic.set(false);
        self.follow
            .borrow_mut()
            .observe_user(adj.value(), adj.upper(), adj.page_size());
        self.capture_anchor();
        self.update_status();
    }

    /// Coalesce appends; stale callbacks cannot override a newer user decision.
    fn schedule_follow(&self) {
        if self.user_input.get() {
            return;
        }
        if !self.window.is_mapped() {
            return;
        }
        let Some(generation) = self.follow.borrow_mut().request() else {
            return;
        };
        let state = self.clone();
        let old_id = self.tick.borrow_mut().take();
        if let Some(id) = old_id {
            id.remove();
        }
        let previous = Cell::new(None);
        let frames = Cell::new(0);
        let id = self.text_view.add_tick_callback(move |_, _| {
            if !state.follow.borrow().eligible(generation) || !state.window.is_mapped() {
                state.follow.borrow_mut().complete(generation);
                state.tick.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            let adj = state.scrolled.vadjustment();
            let extent = (adj.upper(), adj.page_size());
            frames.set(frames.get() + 1);
            if previous.get() != Some(extent) && frames.get() < 8 {
                previous.set(Some(extent));
                return glib::ControlFlow::Continue;
            }
            state.programmatic.set(true);
            adj.set_value((adj.upper() - adj.page_size()).max(adj.lower()));
            state.programmatic.set(false);
            state.follow.borrow_mut().complete(generation);
            state.update_status();
            state.tick.borrow_mut().take();
            glib::ControlFlow::Break
        });
        *self.tick.borrow_mut() = Some(id);
    }
}

/// Build the transcript window: ApplicationWindow with HeaderBar (Save button),
/// ScrolledWindow wrapping a TextView, and timestamped-text rendering.
///
/// The window is created invisible; Phase 4's mode-switch wiring will show it.
/// The Save button click handler is a stub (Phase 5 replaces it with FileDialog).
pub fn build_transcript_window(
    app: &Application,
    transcript_log: Rc<RefCell<crate::overlay::transcript_log::TranscriptLog>>,
    engine_name: String,
    session_start: chrono::DateTime<chrono::Local>,
) -> TranscriptWindowState {
    // 1. Tag table + dimmed timestamp tag.
    let tag_table = TextTagTable::new();
    let timestamp_tag = TextTag::builder().name("timestamp").build();
    tag_table.add(&timestamp_tag);
    let speaker_tag = TextTag::builder().name("speaker").weight(600).build();
    tag_table.add(&speaker_tag);

    // 2. Buffer using that tag table.
    let buffer = TextBuffer::new(Some(&tag_table));

    // 3. TextView (read-only, word-wrap, no cursor).
    let text_view = TextView::builder()
        .buffer(&buffer)
        .wrap_mode(WrapMode::WordChar)
        .editable(false)
        .cursor_visible(false)
        .top_margin(20)
        .bottom_margin(20)
        .left_margin(20)
        .right_margin(20)
        .css_classes(["transcript-text"])
        .build();

    let tag = timestamp_tag.clone();
    text_view.connect_map(move |view| {
        let mut color = view.color();
        color.set_alpha(0.65);
        tag.set_foreground_rgba(Some(&color));
    });

    // 4. ScrolledWindow wrapping the TextView.
    let scrolled = ScrolledWindow::builder()
        .child(&text_view)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();

    // 5. HeaderBar with Save button on the end side.
    let header_bar = HeaderBar::new();
    let save_button = Button::with_label("Save…");
    header_bar.pack_end(&save_button);

    let empty_label = gtk4::Label::new(Some("Waiting for speech"));
    empty_label.set_can_target(false);
    empty_label.add_css_class("dim-label");
    let content = gtk4::Overlay::new();
    content.set_child(Some(&scrolled));
    content.add_overlay(&empty_label);
    let status_label = gtk4::Label::new(Some("Following live"));
    status_label.set_margin_start(20);
    status_label.set_margin_end(20);
    status_label.set_margin_top(8);
    status_label.set_margin_bottom(8);
    status_label.set_xalign(0.0);
    status_label.add_css_class("dim-label");
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    body.append(&content);
    body.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
    body.append(&status_label);
    let css = gtk4::CssProvider::new();
    css.load_from_data(".transcript-text { font-family: sans-serif; font-size: 14pt; }");
    gtk4::style_context_add_provider_for_display(
        &text_view.display(),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // 6. ApplicationWindow.
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Subtidal Transcript")
        .default_width(700)
        .default_height(500)
        .child(&body)
        .width_request(360)
        .height_request(260)
        .build();
    window.set_titlebar(Some(&header_bar));
    window.set_visible(false); // mode-switch wiring in Phase 4 controls visibility

    // 7. Save button handler: FileDialog flow with dual-write.
    {
        let log = Rc::clone(&transcript_log);
        let engine = engine_name.clone();
        let start = session_start;
        let parent_window = window.clone();
        let default_name = format!(
            "subtidal-transcript-{}.txt",
            session_start.format("%Y-%m-%d-%H%M%S")
        );

        save_button.connect_clicked(move |_btn| {
            let log = Rc::clone(&log);
            let engine = engine.clone();
            let parent_window = parent_window.clone();
            let default_name = default_name.clone();

            glib::MainContext::default().spawn_local(async move {
                // Build the file dialog with title, default filename, and a .txt filter.
                let txt_filter = gtk4::FileFilter::new();
                txt_filter.set_name(Some("Plain text"));
                txt_filter.add_pattern("*.txt");
                txt_filter.add_mime_type("text/plain");
                let filters = gtk4::gio::ListStore::new::<gtk4::FileFilter>();
                filters.append(&txt_filter);

                let dialog = gtk4::FileDialog::builder()
                    .title("Save Transcript")
                    .initial_name(&default_name)
                    .modal(true)
                    .filters(&filters)
                    .build();

                let chosen_file = match dialog.save_future(Some(&parent_window)).await {
                    Ok(f) => f,
                    Err(e) => {
                        // User cancelled or backend error — both reported as glib::Error.
                        // Cancel is the common case; print to stderr at debug level only.
                        eprintln!("transcript: save dialog dismissed: {e}");
                        return;
                    }
                };

                let txt_path = match chosen_file.path() {
                    Some(p) => p,
                    None => {
                        show_alert(
                            &parent_window,
                            "Save failed",
                            "Selected location has no local filesystem path (e.g., a remote-only URI).",
                        );
                        return;
                    }
                };
                let json_path = derive_json_sibling(&txt_path);
                // Note: the .json sibling is silently overwritten if it exists; only the
                // user-chosen .txt path goes through the OS overwrite confirmation.

                // Build the .txt and .json bodies on the main thread (cheap; serde_json
                // on a few thousand fragments is sub-millisecond).
                let log_borrow = log.borrow();
                let txt_body = format_paragraphs_as_txt(&log_borrow.paragraphs());
                let json_value = log_borrow.to_json(&engine, start);
                // pretty-printed with 2-space indent per design plan
                let json_body = serde_json::to_string_pretty(&json_value)
                    .unwrap_or_else(|e| format!("{{ \"serialization_error\": \"{e}\" }}"));
                drop(log_borrow);

                let txt_result = std::fs::write(&txt_path, txt_body);
                let json_result = std::fs::write(&json_path, json_body);

                match (txt_result, json_result) {
                    (Ok(()), Ok(())) => {
                        // Success: no dialog, just stderr breadcrumb.
                        eprintln!(
                            "transcript: saved {} and {}",
                            txt_path.display(),
                            json_path.display()
                        );
                    }
                    (Err(e_txt), Ok(())) => {
                        show_alert(
                            &parent_window,
                            "Partial save",
                            &format!(
                                "JSON written successfully to:\n{}\n\nbut writing the TXT failed:\n{}\n\nReason: {}",
                                json_path.display(), txt_path.display(), e_txt
                            ),
                        );
                    }
                    (Ok(()), Err(e_json)) => {
                        show_alert(
                            &parent_window,
                            "Partial save",
                            &format!(
                                "TXT written successfully to:\n{}\n\nbut writing the JSON sibling failed:\n{}\n\nReason: {}",
                                txt_path.display(), json_path.display(), e_json
                            ),
                        );
                    }
                    (Err(e_txt), Err(e_json)) => {
                        show_alert(
                            &parent_window,
                            "Save failed",
                            &format!(
                                "Neither file could be written.\n\nTXT path: {}\nReason: {}\n\nJSON path: {}\nReason: {}",
                                txt_path.display(), e_txt, json_path.display(), e_json
                            ),
                        );
                    }
                }
            });
        });
    }

    let follow_button = gtk4::CheckButton::with_label("Autoscroll");
    follow_button.set_active(true);
    let jump_button = Button::with_label("Jump to latest");
    follow_button.set_tooltip_text(Some("Automatically follow new speech when at the bottom"));
    jump_button.set_tooltip_text(Some(
        "Scroll to the latest speech without changing Autoscroll",
    ));
    save_button.set_tooltip_text(Some("Save transcript as text and JSON"));
    header_bar.pack_start(&follow_button);
    header_bar.pack_start(&jump_button);
    header_bar.set_title_widget(Some(&gtk4::Label::new(Some("Transcript"))));
    let state = TranscriptWindowState {
        window,
        buffer,
        scrolled,
        timestamp_tag,
        speaker_tag,
        tick: Rc::new(RefCell::new(None)),
        text_view,
        presentation: Rc::new(RefCell::new(Presentation::default())),
        follow: Rc::new(RefCell::new(FollowState::default())),
        programmatic: Rc::new(Cell::new(false)),
        user_input: Rc::new(Cell::new(false)),
        input_serial: Rc::new(Cell::new(0)),
        input_settle_serial: Rc::new(Cell::new(0)),
        input_cleanup: Rc::new(RefCell::new(None)),
        #[cfg(test)]
        cleanup_registrations: Rc::new(Cell::new(0)),
        #[cfg(test)]
        cleanup_current: Rc::new(Cell::new(0)),
        scroll_sequence: Rc::new(Cell::new(false)),
        input_baseline: Rc::new(Cell::new(None)),
        reading_anchor: Rc::new(Cell::new(None)),
        follow_button,
        status_label,
        empty_label,
    };
    let empty = state.empty_label.clone();
    state
        .buffer
        .connect_changed(move |buffer| empty.set_visible(buffer.char_count() == 0));
    let scroll = gtk4::EventControllerScroll::new(
        gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::KINETIC,
    );
    scroll.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let s = state.clone();
    scroll.connect_scroll(move |_, _, _| {
        note_user_input(&s);
        glib::Propagation::Proceed
    });
    let s = state.clone();
    scroll.connect_scroll_begin(move |_| {
        s.scroll_sequence.set(true);
        begin_user_input(&s);
    });
    let s = state.clone();
    scroll.connect_scroll_end(move |_| {
        s.scroll_sequence.set(false);
        settle_user_input(&s);
    });
    let s = state.clone();
    scroll.connect_decelerate(move |_, _, _| {
        s.scroll_sequence.set(false);
        if !s.user_input.get() {
            begin_user_input(&s);
        }
        settle_user_input(&s);
    });
    state.scrolled.add_controller(scroll);
    let keys = gtk4::EventControllerKey::new();
    keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let s = state.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if matches!(
            key,
            gtk4::gdk::Key::Up
                | gtk4::gdk::Key::Down
                | gtk4::gdk::Key::Page_Up
                | gtk4::gdk::Key::Page_Down
                | gtk4::gdk::Key::Home
                | gtk4::gdk::Key::End
        ) {
            note_user_input(&s);
        }
        glib::Propagation::Proceed
    });
    state.scrolled.add_controller(keys);
    let click = gtk4::GestureClick::new();
    click.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let s = state.clone();
    click.connect_pressed(move |_, _, _, _| {
        begin_user_input(&s);
    });
    let s = state.clone();
    click.connect_released(move |_, _, _, _| settle_user_input(&s));
    let s = state.clone();
    click.connect_cancel(move |_, _| settle_user_input(&s));
    state.scrolled.add_controller(click);
    let s = state.clone();
    state
        .scrolled
        .vadjustment()
        .connect_value_changed(move |adj| {
            if !s.programmatic.get() && s.user_input.get() {
                if let Some((value, upper, page)) = s.input_baseline.get() {
                    if (upper, page) == (adj.upper(), adj.page_size())
                        && (value - adj.value()).abs() > 0.01
                    {
                        s.cancel_follow();
                        s.follow.borrow_mut().observe_user(adj.value(), upper, page);
                        s.input_baseline.set(Some((adj.value(), upper, page)));
                        s.capture_anchor();
                        s.update_status();
                    }
                }
            }
        });
    let s = state.clone();
    state.scrolled.vadjustment().connect_changed(move |adj| {
        if s.user_input.get() {
            s.input_baseline
                .set(Some((adj.value(), adj.upper(), adj.page_size())));
            return;
        }
        if s.programmatic.get() {
            return;
        }
        let following = s.follow.borrow().following();
        if following {
            s.schedule_follow();
        } else {
            s.restore_anchor();
        }
    });
    {
        let s = state.clone();
        state.follow_button.connect_toggled(move |button| {
            s.cancel_input_cleanup();
            if !button.is_active() {
                s.capture_anchor();
            }
            s.input_serial.set(s.input_serial.get().wrapping_add(1));
            s.user_input.set(false);
            s.scroll_sequence.set(false);
            s.input_baseline.set(None);
            s.cancel_follow();
            s.follow.borrow_mut().set_enabled(button.is_active());
            s.update_status();
            s.schedule_follow();
        });
    }
    {
        let s = state.clone();
        jump_button.connect_clicked(move |_| {
            s.cancel_input_cleanup();
            s.input_serial.set(s.input_serial.get().wrapping_add(1));
            s.user_input.set(false);
            s.scroll_sequence.set(false);
            s.input_baseline.set(None);
            s.cancel_follow();
            s.follow.borrow_mut().jump();
            s.programmatic.set(true);
            let adj = s.scrolled.vadjustment();
            adj.set_value((adj.upper() - adj.page_size()).max(adj.lower()));
            s.programmatic.set(false);
            s.update_status();
            s.schedule_follow();
        });
    }
    {
        let s = state.clone();
        state.window.connect_map(move |_| {
            let following = s.follow.borrow().following();
            if following {
                s.schedule_follow();
            } else {
                s.restore_anchor();
            }
        });
        let s = state.clone();
        state.window.connect_unmap(move |_| {
            s.cancel_input_cleanup();
            s.input_serial.set(s.input_serial.get().wrapping_add(1));
            s.user_input.set(false);
            s.scroll_sequence.set(false);
            s.input_baseline.set(None);
            s.cancel_follow();
        });
        let s = state.clone();
        state.window.connect_close_request(move |_| {
            s.cancel_input_cleanup();
            s.cancel_follow();
            s.window.set_visible(false);
            glib::Propagation::Stop
        });
    }
    state
}

fn begin_user_input(state: &TranscriptWindowState) -> u64 {
    state.cancel_input_cleanup();
    state.cancel_follow();
    state
        .input_serial
        .set(state.input_serial.get().wrapping_add(1));
    state.user_input.set(true);
    let adj = state.scrolled.vadjustment();
    state
        .input_baseline
        .set(Some((adj.value(), adj.upper(), adj.page_size())));
    state.input_serial.get()
}

fn finish_user_input(state: &TranscriptWindowState, serial: u64) {
    if state.input_serial.get() != serial {
        return;
    }
    state.user_input.set(false);
    state.input_baseline.set(None);
    state.capture_anchor();
    state.update_status();
    state.schedule_follow();
}

fn settle_user_input(state: &TranscriptWindowState) {
    state.cancel_input_cleanup();
    if !state.user_input.get() {
        return;
    }
    state
        .input_settle_serial
        .set(state.input_settle_serial.get().wrapping_add(1));
    let settle_serial = state.input_settle_serial.get();
    let serial = state.input_serial.get();
    let s = state.clone();
    let previous = Rc::new(Cell::new(s.scrolled.vadjustment().value()));
    let stable = Rc::new(Cell::new(0));
    let started = std::time::Instant::now();
    // GTK exposes deceleration start, not ScrolledWindow kinetic completion.
    // This bounded quiet-value heuristic still needs native momentum validation.
    let source = glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        if s.input_serial.get() != serial
            || s.input_settle_serial.get() != settle_serial
            || !s.user_input.get()
        {
            s.input_cleanup_finished();
            return glib::ControlFlow::Break;
        }
        let value = s.scrolled.vadjustment().value();
        if (previous.get() - value).abs() <= 0.01 {
            stable.set(stable.get() + 1);
        } else {
            stable.set(0);
            previous.set(value);
        }
        if stable.get() >= 3 || started.elapsed() >= std::time::Duration::from_secs(2) {
            s.input_cleanup_finished();
            if !s.scroll_sequence.get() {
                finish_user_input(&s, serial);
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    *state.input_cleanup.borrow_mut() = Some(source);
    #[cfg(test)]
    {
        state
            .cleanup_registrations
            .set(state.cleanup_registrations.get() + 1);
        state.cleanup_current.set(state.cleanup_current.get() + 1);
        assert_eq!(
            state.cleanup_current.get(),
            1,
            "at most one physical input cleanup source"
        );
    }
}

fn note_user_input(state: &TranscriptWindowState) {
    begin_user_input(state);
    if !state.scroll_sequence.get() {
        settle_user_input(state);
    }
}

/// Append a single fragment to the transcript view, with timestamp on NewParagraph.
///
/// Tracks autoscroll position: if the view was near the bottom before insertion,
/// schedules a scroll-to-bottom for after layout. If scrolled up, new text arrives
/// silently (the user can scroll back down when ready).
///
/// `speaker_names` provides display-name overrides for diarization labels.
/// Unmapped speaker IDs fall back to "Speaker {N+1}".
pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
    speaker_names: &std::collections::HashMap<u32, String>,
) {
    state.programmatic.set(true);
    insert_fragment(state, fragment, kind, speaker_names);
    state.programmatic.set(false);
    state.schedule_follow();
}

fn insert_fragment(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
    speaker_names: &std::collections::HashMap<u32, String>,
) {
    let mut presentation = state.presentation.borrow_mut();
    let start = presentation.text.len();
    let metadata_start = presentation.metadata.len();
    let character_start = state.buffer.char_count();
    presentation.append(fragment, kind, speaker_names);
    let suffix = presentation.text[start..].to_owned();
    let spans: Vec<_> = presentation.metadata[metadata_start..]
        .iter()
        .map(|span| {
            (
                span.kind,
                character_start + gtk_offset(&suffix, span.range.start - start) as i32,
                character_start + gtk_offset(&suffix, span.range.end - start) as i32,
            )
        })
        .collect();
    drop(presentation);
    state.buffer.insert(&mut state.buffer.end_iter(), &suffix);
    for (kind, start, end) in spans {
        let tag = match kind {
            MetadataKind::Timestamp => &state.timestamp_tag,
            MetadataKind::Speaker => &state.speaker_tag,
        };
        state.buffer.apply_tag(
            tag,
            &state.buffer.iter_at_offset(start),
            &state.buffer.iter_at_offset(end),
        );
    }
}

/// Re-render the entire transcript view from a `TranscriptLog`. Used after a
/// `SetSpeakerNames` update to refresh embedded labels in the view. Cheaper
/// than walking the buffer and patching individual paragraphs, and correct
/// regardless of which fragments are visible.
///
/// Re-renders by clearing the view and re-appending every fragment in
/// `log.fragments()`, deciding paragraph breaks via `TranscriptLog`'s own
/// gap+speaker rules. The autoscroll position is preserved across the rebuild
/// when the view was near the bottom before.
pub fn rebuild_view(
    state: &TranscriptWindowState,
    log: &crate::overlay::transcript_log::TranscriptLog,
    speaker_names: &std::collections::HashMap<u32, String>,
) {
    if state.window.is_mapped() {
        state.capture_anchor();
    }
    state.cancel_follow();
    let following = state.follow.borrow().following();
    let old = state.presentation.borrow();
    let position = |offset: i32| {
        let byte = old
            .text
            .char_indices()
            .nth(offset.max(0) as usize)
            .map(|(b, _)| b)
            .unwrap_or(old.text.len());
        old.position(byte)
    };
    let selection = state
        .buffer
        .selection_bounds()
        .and_then(|(a, b)| Some((position(a.offset())?, position(b.offset())?)));
    drop(old);
    state.programmatic.set(true);

    // Clear and re-render.
    let (mut s, mut e) = (state.buffer.start_iter(), state.buffer.end_iter());
    state.buffer.delete(&mut s, &mut e);

    let rebuilt = Presentation::from_log(log, speaker_names);
    let spans: Vec<_> = rebuilt
        .metadata
        .iter()
        .map(|span| {
            (
                span.kind,
                gtk_offset(&rebuilt.text, span.range.start) as i32,
                gtk_offset(&rebuilt.text, span.range.end) as i32,
            )
        })
        .collect();
    state
        .buffer
        .insert(&mut state.buffer.end_iter(), &rebuilt.text);
    for (kind, start, end) in spans {
        let tag = match kind {
            MetadataKind::Timestamp => &state.timestamp_tag,
            MetadataKind::Speaker => &state.speaker_tag,
        };
        state.buffer.apply_tag(
            tag,
            &state.buffer.iter_at_offset(start),
            &state.buffer.iter_at_offset(end),
        );
    }
    *state.presentation.borrow_mut() = rebuilt;
    let presentation = state.presentation.borrow();
    let resolve = |p| {
        state
            .buffer
            .iter_at_offset(gtk_offset(&presentation.text, presentation.resolve(p)) as i32)
    };
    let selection = selection.map(|(a, b)| (resolve(a), resolve(b)));
    drop(presentation);
    if let Some((a, b)) = selection {
        state.buffer.select_range(&a, &b);
    }
    state.programmatic.set(false);
    if following {
        state.schedule_follow();
    } else {
        state.restore_anchor();
    }
}

/// Clear all text from the transcript view.
pub fn clear_view(state: &TranscriptWindowState) {
    state.cancel_input_cleanup();
    state.cancel_follow();
    state.follow.borrow_mut().clear();
    state.reading_anchor.set(None);
    state.user_input.set(false);
    state
        .input_serial
        .set(state.input_serial.get().wrapping_add(1));
    state.scroll_sequence.set(false);
    state.input_baseline.set(None);
    *state.presentation.borrow_mut() = Presentation::default();
    state.programmatic.set(true);
    let (mut start, mut end) = (state.buffer.start_iter(), state.buffer.end_iter());
    state.buffer.delete(&mut start, &mut end);
    state.programmatic.set(false);
    state.update_status();
}

/// Given the user-chosen `.txt` path (or any path), return the sibling `.json`
/// path with the same stem. If the input has no extension or a non-`.txt`
/// extension, the `.json` is appended to the stem unchanged.
///
/// Examples:
/// - `/tmp/foo.txt` -> `/tmp/foo.json`
/// - `/tmp/foo`     -> `/tmp/foo.json`
/// - `/tmp/foo.bar` -> `/tmp/foo.json`  (extension replaced)
pub fn derive_json_sibling(path: &std::path::Path) -> std::path::PathBuf {
    let mut out = path.to_path_buf();
    out.set_extension("json");
    out
}

/// Format a slice of paragraphs as the `.txt` save body:
/// `[HH:MM:SS] <paragraph text>\n` per paragraph.
pub fn format_paragraphs_as_txt(
    paragraphs: &[crate::overlay::transcript_log::Paragraph],
) -> String {
    let mut out = String::new();
    for p in paragraphs {
        out.push_str(&p.timestamp.format("[%H:%M:%S] ").to_string());
        out.push_str(&p.text);
        out.push('\n');
    }
    out
}

/// Show an alert dialog with the given title and body text.
fn show_alert(parent: &ApplicationWindow, title: &str, body: &str) {
    let alert = gtk4::AlertDialog::builder()
        .modal(true)
        .message(title)
        .detail(body)
        .build();
    alert.show(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ac5_6_json_sibling_replaces_txt_extension() {
        let p = PathBuf::from("/tmp/transcript.txt");
        assert_eq!(
            derive_json_sibling(&p),
            PathBuf::from("/tmp/transcript.json")
        );
    }

    #[test]
    fn ac5_6_json_sibling_no_extension() {
        let p = PathBuf::from("/tmp/transcript");
        assert_eq!(
            derive_json_sibling(&p),
            PathBuf::from("/tmp/transcript.json")
        );
    }

    #[test]
    fn ac5_6_json_sibling_other_extension() {
        let p = PathBuf::from("/tmp/transcript.log");
        assert_eq!(
            derive_json_sibling(&p),
            PathBuf::from("/tmp/transcript.json")
        );
    }

    #[test]
    fn ac5_6_json_sibling_with_dots_in_stem() {
        // PathBuf::set_extension only replaces the LAST extension; "a.b.txt" -> "a.b.json".
        let p = PathBuf::from("/tmp/2026.05.11.txt");
        assert_eq!(
            derive_json_sibling(&p),
            PathBuf::from("/tmp/2026.05.11.json")
        );
    }

    #[test]
    fn ac5_2_format_paragraphs_as_txt_matches_design_example() {
        use crate::overlay::transcript_log::Paragraph;
        use chrono::{Local, TimeZone};

        let ts1 = Local.timestamp_opt(1_700_000_000, 0).unwrap();
        let ts2 = Local.timestamp_opt(1_700_000_010, 0).unwrap();
        let paragraphs = vec![
            Paragraph {
                timestamp: ts1,
                text: "Hello everyone, welcome to the call. Let me share my screen.".to_string(),
            },
            Paragraph {
                timestamp: ts2,
                text: "So as you can see here, this is the dashboard.".to_string(),
            },
        ];

        let out = format_paragraphs_as_txt(&paragraphs);
        let expected = format!(
            "[{}] Hello everyone, welcome to the call. Let me share my screen.\n[{}] So as you can see here, this is the dashboard.\n",
            ts1.format("%H:%M:%S"), ts2.format("%H:%M:%S")
        );

        assert_eq!(out, expected);
    }
}
