//! Transcript NSWindow: NSScrollView + NSTextView with autoscroll and NSSavePanel export.
//!
//! Mirrors the Linux GTK4 transcript window implementation, providing a regular
//! window (not layer-shell overlay) with timestamped caption display and Save dialog.

use crate::overlay::transcript_log::TranscriptLog;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSSavePanel, NSScrollView, NSTextView,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSRange, NSString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Handles needed by the orchestration layer to drive the transcript window.
#[derive(Clone)]
pub struct TranscriptWindowState {
    pub window: Retained<NSWindow>,
    pub text_view: Retained<NSTextView>,
    pub log: Arc<Mutex<TranscriptLog>>,
}

/// Bundle returned by `build_transcript_window`: the state used to drive
/// the window plus the Retained<TranscriptActions> that owns the Save
/// button's action target. NSButton holds setTarget weakly, so the actions
/// object must outlive the window — keep this whole bundle alive.
pub struct TranscriptWindow {
    pub state: TranscriptWindowState,
    pub actions: Retained<TranscriptActions>,
}

/// Ivars for the save button action target.
pub struct TranscriptActionsIvars {
    window_state: RefCell<Option<TranscriptWindowState>>,
}

define_class!(
    /// Custom NSObject subclass for the Save button action and window delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalTranscriptActions"]
    #[ivars = TranscriptActionsIvars]
    pub struct TranscriptActions;

    impl TranscriptActions {
        /// Called when the Save button is clicked.
        #[unsafe(method(saveTranscript:))]
        fn save_transcript(&self, _sender: Option<&NSButton>) {
            if let Some(state) = self.ivars().window_state.borrow().as_ref() {
                let _mtm = MainThreadMarker::from(self);
                if let Err(e) = unsafe { save_transcript_impl(&state, _mtm) } {
                    eprintln!("warn: transcript save failed: {e}");
                }
            }
        }

        /// NSWindowDelegate: intercept the close button so it hides the window
        /// instead of destroying it, allowing re-entry into Transcript mode to
        /// bring it back via makeKeyAndOrderFront.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: Option<&NSWindow>) -> bool {
            if let Some(w) = sender {
                w.orderOut(None);
            }
            false
        }
    }
);

impl TranscriptActions {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let ivars = TranscriptActionsIvars {
            window_state: RefCell::new(None),
        };
        let allocated = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(allocated), init] }
    }

    fn set_window_state(&self, state: TranscriptWindowState) {
        *self.ivars().window_state.borrow_mut() = Some(state);
    }
}

/// Build the transcript window: NSWindow with NSScrollView + NSTextView and Save button.
pub fn build_transcript_window(
    mtm: MainThreadMarker,
    log: Arc<Mutex<TranscriptLog>>,
) -> TranscriptWindow {
    unsafe {
        // Window frame: 800x600 starting at (200, 200).
        let rect = CGRect::new(CGPoint::new(200.0, 200.0), CGSize::new(800.0, 600.0));

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable;

        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        );

        window.setTitle(&NSString::from_str("Subtidal — Transcript"));

        // Create NSScrollView with NSTextView. The scroll view fills the
        // container above the save-button strip; both stretch with the window.
        let scroll_rect = CGRect::new(
            CGPoint::new(0.0, 50.0),
            CGSize::new(rect.size.width, rect.size.height - 50.0),
        );
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), scroll_rect);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        // The NSTextView lives inside the scroll view's contentView. Configure
        // it for word-wrap that tracks the scroll view's width:
        //   - horizontallyResizable=false + widthTracksTextView=true makes the
        //     text container width follow the scroll content area, so lines
        //     reflow when the window is resized.
        //   - verticallyResizable=true allows the text view to grow downward
        //     as content is appended (the scroll view handles overflow).
        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            CGRect::new(CGPoint::new(0.0, 0.0), scroll_rect.size),
        );
        text_view.setEditable(false);
        text_view.setSelectable(true);
        text_view.setRichText(false);
        text_view.setHorizontallyResizable(false);
        text_view.setVerticallyResizable(true);
        text_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(container) = text_view.textContainer() {
            container.setWidthTracksTextView(true);
            container.setContainerSize(CGSize::new(scroll_rect.size.width, f64::MAX));
        }

        scroll.setDocumentView(Some(&text_view));

        // Save button — pinned to bottom-right.
        let button_rect = CGRect::new(
            CGPoint::new(rect.size.width - 120.0, 10.0),
            CGSize::new(100.0, 30.0),
        );
        let save_button = NSButton::initWithFrame(NSButton::alloc(mtm), button_rect);
        save_button.setTitle(&NSString::from_str("Save…"));
        save_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );

        // Create action target.
        let actions = TranscriptActions::new(mtm);

        // Create container view to hold scroll view and button.
        let container = NSView::initWithFrame(NSView::alloc(mtm), rect);
        container.addSubview(&scroll);
        container.addSubview(&save_button);

        // Set up button action and target. setTarget is weak — `actions` is
        // owned by the returned TranscriptWindow bundle, not by the button.
        use objc2::runtime::AnyObject;
        let target_obj: &AnyObject = actions.as_ref();
        save_button.setTarget(Some(target_obj));
        save_button.setAction(Some(sel!(saveTranscript:)));

        // Wire actions as window delegate so windowShouldClose: fires and
        // hides instead of destroying the window.
        let _: () = msg_send![&*window, setDelegate: target_obj];

        window.setContentView(Some(&container));

        let state = TranscriptWindowState {
            window,
            text_view,
            log,
        };
        actions.set_window_state(state.clone());

        state.window.setIsVisible(false);
        TranscriptWindow { state, actions }
    }
}

/// Re-render the transcript log into the NSTextView. The caller is
/// responsible for having already pushed/relabelled fragments in the log;
/// this function only updates display. If the view was scrolled to the
/// bottom when we started, scroll back to the bottom after the update.
pub fn rebuild_view(
    state: &TranscriptWindowState,
    _mtm: MainThreadMarker,
    speaker_names: &HashMap<u32, String>,
) {
    let was_at_bottom = is_near_bottom(&state.text_view);

    let formatted = format_fragments(&state.log.lock().unwrap(), speaker_names);
    let ns = NSString::from_str(&formatted);
    state.text_view.setString(&ns);

    if was_at_bottom {
        let len = ns.length();
        state.text_view.scrollRangeToVisible(NSRange {
            location: len,
            length: 0,
        });
    }
}

/// Clear the displayed text. The caller is responsible for clearing the
/// underlying `TranscriptLog` separately (it is surface 1 of the
/// 4-surface clear; this is surface 2).
pub fn clear_view(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    let empty = NSString::from_str("");
    state.text_view.setString(&empty);
}

/// Show the transcript window (make key and order front).
pub fn order_front(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    state.window.makeKeyAndOrderFront(None);
}

/// Hide the transcript window.
pub fn order_out(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    state.window.orderOut(None);
}

/// Format fragments as timestamped paragraphs with speaker labels.
///
/// Paragraph breaks mirror `TranscriptLog::push_at_with_speaker`: first
/// fragment, a gap greater than 1.5 seconds, or a speaker change when both
/// adjacent fragments have speaker IDs. Fragment text is preserved verbatim.
pub fn format_fragments(log: &TranscriptLog, speaker_names: &HashMap<u32, String>) -> String {
    let mut out = String::new();
    let fragments = log.fragments();
    for (idx, fragment) in fragments.iter().enumerate() {
        let new_paragraph = if idx == 0 {
            true
        } else {
            let prev = &fragments[idx - 1];
            let speaker_changed = fragment.speaker_id.is_some()
                && prev.speaker_id.is_some()
                && fragment.speaker_id != prev.speaker_id;
            let gap = fragment
                .timestamp
                .signed_duration_since(prev.timestamp)
                .to_std()
                .unwrap_or(std::time::Duration::ZERO);
            speaker_changed || gap > std::time::Duration::from_millis(1500)
        };

        if new_paragraph {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&fragment.timestamp.format("[%H:%M:%S] ").to_string());
            if let Some(id) = fragment.speaker_id {
                out.push_str(&speaker_label(speaker_names, id));
                out.push_str(": ");
            }
        }
        out.push_str(&fragment.text);
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn speaker_label(speaker_names: &HashMap<u32, String>, id: u32) -> String {
    speaker_names
        .get(&id)
        .cloned()
        .unwrap_or_else(|| format!("Speaker {}", id + 1))
}

/// Format paragraphs as timestamped lines. Kept for neutral legacy tests and
/// any non-speaker callers; live macOS transcript rendering uses fragments.
#[cfg(test)]
pub fn format_paragraphs(paragraphs: &[crate::overlay::transcript_log::Paragraph]) -> String {
    let mut out = String::new();
    for p in paragraphs {
        out.push_str(&p.timestamp.format("[%H:%M:%S] ").to_string());
        out.push_str(&p.text);
        out.push('\n');
    }
    out
}

/// True iff the user is within ~10pt of the bottom of the visible area
/// (so we should re-stick to the bottom after appending). When the user
/// has scrolled up to read history, this returns false and autoscroll
/// pauses until they come back down.
fn is_near_bottom(text_view: &NSTextView) -> bool {
    let bounds = text_view.bounds();
    let visible = text_view.visibleRect();
    let bottom_of_bounds = bounds.origin.y + bounds.size.height;
    let bottom_of_visible = visible.origin.y + visible.size.height;
    (bottom_of_bounds - bottom_of_visible).abs() < 10.0
}

/// Save the transcript log to a JSON file via NSSavePanel.
unsafe fn save_transcript_impl(
    state: &TranscriptWindowState,
    mtm: MainThreadMarker,
) -> anyhow::Result<()> {
    let panel = NSSavePanel::savePanel(mtm);

    let default_name = format!(
        "subtidal-transcript-{}.json",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    );

    panel.setNameFieldStringValue(&NSString::from_str(&default_name));

    // Run the save panel.
    let response = panel.runModal();
    if response != 1000 {
        // 1000 is NSModalResponseOK; user cancelled or other response.
        return Ok(());
    }

    // Get the chosen URL.
    if let Some(url) = panel.URL() {
        if let Some(path_str) = url.path() {
            let path = path_str.to_string();
            let json_value = state
                .log
                .lock()
                .unwrap()
                .to_json("nemotron", chrono::Local::now());
            let json_str = serde_json::to_string_pretty(&json_value)
                .unwrap_or_else(|e| format!(r#"{{"error": "{}"}}"#, e));
            std::fs::write(&path, json_str)
                .map_err(|e| anyhow::anyhow!("Failed to write transcript: {}", e))?;
            eprintln!("info: transcript saved to {}", path);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_transcript_save_produces_valid_json() {
        let log = Arc::new(Mutex::new(TranscriptLog::new(
            std::time::Duration::from_millis(1500),
        )));
        let json = log
            .lock()
            .unwrap()
            .to_json("nemotron", chrono::Local::now());
        serde_json::to_string(&json).expect("valid JSON serialization");
    }

    #[test]
    fn format_paragraphs_matches_design() {
        use crate::overlay::transcript_log::Paragraph;
        use chrono::Local;

        let now = Local::now();
        let paras = vec![Paragraph {
            timestamp: now,
            text: "Hello world".to_string(),
        }];

        let formatted = format_paragraphs(&paras);
        assert!(formatted.starts_with("["));
        assert!(formatted.contains("Hello world"));
        assert!(formatted.ends_with("\n"));
    }

    #[test]
    fn format_fragments_uses_default_speaker_labels() {
        let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
        log.push_with_speaker_and_sample(" hello".to_string(), Some(0), 100);

        let formatted = format_fragments(&log, &HashMap::new());
        assert!(formatted.contains("Speaker 1:  hello"));
    }

    #[test]
    fn format_fragments_uses_custom_speaker_names() {
        let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
        log.push_with_speaker_and_sample(" hello".to_string(), Some(1), 100);
        let names = HashMap::from([(1, "Bob".to_string())]);

        let formatted = format_fragments(&log, &names);
        assert!(formatted.contains("Bob:  hello"));
        assert!(!formatted.contains("Speaker 2:"));
    }

    #[test]
    fn speaker_change_forces_labeled_new_paragraph() {
        use chrono::TimeZone;
        let ts = chrono::Local.timestamp_opt(10, 0).unwrap();
        let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
        log.push_at_with_speaker(" hello".to_string(), Some(0), 100, ts);
        log.push_at_with_speaker(" there".to_string(), Some(1), 200, ts);

        let formatted = format_fragments(&log, &HashMap::new());
        assert!(formatted.contains("Speaker 1:  hello\n"));
        assert!(formatted.contains("Speaker 2:  there\n"));
    }

    #[test]
    fn relabeled_fragments_render_with_new_speaker() {
        let mut log = TranscriptLog::new(std::time::Duration::from_millis(1500));
        log.push_with_speaker_and_sample(" hello".to_string(), Some(0), 100);
        log.push_with_speaker_and_sample(" world".to_string(), Some(0), 200);
        assert_eq!(log.relabel_since(150, 1), 1);

        let formatted = format_fragments(&log, &HashMap::new());
        assert!(formatted.contains("Speaker 1:  hello\n"));
        assert!(formatted.contains("Speaker 2:  world\n"));
    }
}
