//! Transcript GTK4 window: widget tree, autoscroll logic, and caption display.
//!
//! Handles the construction of a regular (non-layer-shell) ApplicationWindow containing
//! a ScrolledWindow + TextView for displaying timestamped speech fragments. Provides
//! `append_fragment_to_view` (called per caption) and `clear_view` (reset on session end).

use crate::overlay::transcript_log::{AppendKind, Fragment};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Button, HeaderBar, ScrolledWindow, TextBuffer, TextTag,
    TextTagTable, TextView, WrapMode,
};
use std::cell::RefCell;
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
}

/// Threshold (in pixels) below the scroll bottom at which the view is
/// considered "tailing" and new appends should auto-scroll.
const AUTOSCROLL_THRESHOLD_PX: f64 = 16.0;

/// Foreground color for paragraph timestamp prefix. Approximates GNOME's
/// `dim_label_color` without using the deprecated StyleContext::lookup_color API.
const TIMESTAMP_RGBA: (f32, f32, f32, f32) = (0.6, 0.6, 0.6, 1.0);

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
    let timestamp_tag = TextTag::builder()
        .name("timestamp")
        .foreground_rgba(&gtk4::gdk::RGBA::new(
            TIMESTAMP_RGBA.0,
            TIMESTAMP_RGBA.1,
            TIMESTAMP_RGBA.2,
            TIMESTAMP_RGBA.3,
        ))
        .build();
    tag_table.add(&timestamp_tag);

    // 2. Buffer using that tag table.
    let buffer = TextBuffer::new(Some(&tag_table));

    // 3. TextView (read-only, word-wrap, no cursor).
    let text_view = TextView::builder()
        .buffer(&buffer)
        .wrap_mode(WrapMode::WordChar)
        .editable(false)
        .cursor_visible(false)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();

    // 4. ScrolledWindow wrapping the TextView.
    let scrolled = ScrolledWindow::builder()
        .child(&text_view)
        .vexpand(true)
        .hexpand(true)
        .build();

    // 5. HeaderBar with Save button on the end side.
    let header_bar = HeaderBar::new();
    let save_button = Button::with_label("Save…");
    header_bar.pack_end(&save_button);

    // 6. ApplicationWindow.
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Subtidal Transcript")
        .default_width(700)
        .default_height(500)
        .child(&scrolled)
        .build();
    window.set_titlebar(Some(&header_bar));
    window.set_visible(false); // mode-switch wiring in Phase 4 controls visibility

    // 7. Save button stub handler — Phase 5 replaces this with FileDialog.
    //    The closure captures `transcript_log`, `engine_name`, and
    //    `session_start` (prefixed `_` here so the unused-variable lint
    //    stays quiet until Phase 5 actually consumes them).
    {
        let _log = Rc::clone(&transcript_log);
        let _engine = engine_name.clone();
        let _start = session_start;
        save_button.connect_clicked(move |_btn| {
            // Reference the captures so the closure-level binding sees them
            // as "used" (the closure body itself doesn't use them yet).
            let _ = (&_log, &_engine, &_start);
            eprintln!("transcript: Save clicked (Phase 5 will wire FileDialog)");
        });
    }

    TranscriptWindowState {
        window,
        buffer,
        scrolled,
        timestamp_tag,
    }
}

/// Append a single fragment to the transcript view, with timestamp on NewParagraph.
///
/// Tracks autoscroll position: if the view was near the bottom before insertion,
/// schedules a scroll-to-bottom for after layout. If scrolled up, new text arrives
/// silently (the user can scroll back down when ready).
pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
) {
    // 1. Sample autoscroll position BEFORE inserting.
    let was_at_bottom = is_near_bottom(&state.scrolled);

    // 2. Insert paragraph break + timestamp prefix on NewParagraph.
    let mut end = state.buffer.end_iter();
    if matches!(kind, AppendKind::NewParagraph) {
        // If the buffer has any prior content, prepend a newline to start a new line.
        if state.buffer.char_count() > 0 {
            state.buffer.insert(&mut end, "\n");
        }
        let timestamp_text = fragment.timestamp.format("[%H:%M:%S] ").to_string();
        state
            .buffer
            .insert_with_tags(&mut end, &timestamp_text, &[&state.timestamp_tag]);
    }

    // 3. Insert the fragment text (whitespace verbatim — the leading-space
    //    word-boundary signal must be preserved per the RNNT engine contract).
    state.buffer.insert(&mut end, &fragment.text);

    // 4. If we were tailing, schedule a scroll-to-bottom for after layout.
    if was_at_bottom {
        let scrolled = state.scrolled.clone();
        glib::idle_add_local_once(move || {
            let adj = scrolled.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        });
    }
}

/// Clear all text from the transcript view.
pub fn clear_view(state: &TranscriptWindowState) {
    let (mut start, mut end) = (state.buffer.start_iter(), state.buffer.end_iter());
    state.buffer.delete(&mut start, &mut end);
}

/// Check if the scrolled window is near the bottom (within AUTOSCROLL_THRESHOLD_PX).
///
/// Returns true on a freshly-built buffer (autoscroll on by default — the "chat-app pattern").
fn is_near_bottom(scrolled: &ScrolledWindow) -> bool {
    let adj = scrolled.vadjustment();
    let value = adj.value();
    let upper = adj.upper();
    let page_size = adj.page_size();
    // "near" = within AUTOSCROLL_THRESHOLD_PX of the bottom edge,
    //          or the content is shorter than one page (always tail).
    let bottom = upper - page_size;
    value >= bottom - AUTOSCROLL_THRESHOLD_PX
}
