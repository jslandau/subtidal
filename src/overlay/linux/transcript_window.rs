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
///
/// `speaker_names` provides display-name overrides for diarization labels.
/// Unmapped speaker IDs fall back to "Speaker {N+1}".
pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
    speaker_names: &std::collections::HashMap<u32, String>,
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

        // If diarization is active, prepend the speaker label.
        if let Some(speaker_id) = fragment.speaker_id {
            let display = speaker_names
                .get(&speaker_id)
                .cloned()
                .unwrap_or_else(|| format!("Speaker {}", speaker_id + 1));
            let label = format!("{display}: ");
            state.buffer.insert(&mut end, &label);
        }
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
    let was_at_bottom = is_near_bottom(&state.scrolled);

    // Clear and re-render.
    let (mut s, mut e) = (state.buffer.start_iter(), state.buffer.end_iter());
    state.buffer.delete(&mut s, &mut e);

    // Derive paragraph break locally using the same rule as TranscriptLog::push_at:
    // first fragment, gap > 1.5s, or speaker change.
    let paragraph_gap_secs = 1.5_f64;
    let mut prev: Option<&Fragment> = None;
    for frag in log.fragments() {
        let kind = match prev {
            None => AppendKind::NewParagraph,
            Some(p) => {
                let speaker_change = frag.speaker_id.is_some()
                    && p.speaker_id.is_some()
                    && frag.speaker_id != p.speaker_id;
                let gap = frag.timestamp.signed_duration_since(p.timestamp);
                let secs = gap.num_milliseconds() as f64 / 1000.0;
                if speaker_change || secs > paragraph_gap_secs {
                    AppendKind::NewParagraph
                } else {
                    AppendKind::ContinueParagraph
                }
            }
        };
        append_fragment_to_view(state, frag, kind, speaker_names);
        prev = Some(frag);
    }

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
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_no_extension() {
        let p = PathBuf::from("/tmp/transcript");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_other_extension() {
        let p = PathBuf::from("/tmp/transcript.log");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/transcript.json"));
    }

    #[test]
    fn ac5_6_json_sibling_with_dots_in_stem() {
        // PathBuf::set_extension only replaces the LAST extension; "a.b.txt" -> "a.b.json".
        let p = PathBuf::from("/tmp/2026.05.11.txt");
        assert_eq!(derive_json_sibling(&p), PathBuf::from("/tmp/2026.05.11.json"));
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
