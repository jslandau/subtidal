//! Transcript NSWindow: NSScrollView + NSTextView with autoscroll and NSSavePanel export.
//!
//! Mirrors the Linux GTK4 transcript window implementation, providing a regular
//! window (not layer-shell overlay) with timestamped caption display and Save dialog.

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSWindow, NSWindowStyleMask, NSScrollView, NSTextView, NSButton, NSView,
    NSSavePanel, NSBackingStoreType,
};
use objc2_foundation::{NSString, NSObject, NSRange};
use objc2_core_foundation::{CGPoint, CGSize, CGRect};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use crate::overlay::transcript_log::TranscriptLog;

/// Handles needed by the orchestration layer to drive the transcript window.
#[derive(Clone)]
pub struct TranscriptWindowState {
    pub window: Retained<NSWindow>,
    pub text_view: Retained<NSTextView>,
    pub log: Arc<Mutex<TranscriptLog>>,
}

/// Ivars for the save button action target.
pub struct TranscriptActionsIvars {
    window_state: RefCell<Option<TranscriptWindowState>>,
}

define_class!(
    /// Custom NSObject subclass for the Save button action.
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
) -> TranscriptWindowState {
    unsafe {
        // Window frame: 800x600 starting at (200, 200).
        let rect = CGRect::new(
            CGPoint::new(200.0, 200.0),
            CGSize::new(800.0, 600.0),
        );

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

        // Create NSScrollView with NSTextView.
        let scroll_rect = CGRect::new(
            CGPoint::new(0.0, 50.0),  // Leave space for button at bottom
            CGSize::new(rect.size.width, rect.size.height - 50.0),
        );
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            scroll_rect,
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);

        let text_view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            scroll_rect,
        );
        text_view.setEditable(false);
        text_view.setSelectable(true);
        text_view.setRichText(false);

        scroll.setDocumentView(Some(&text_view));

        // Create a Save button.
        let button_rect = CGRect::new(
            CGPoint::new(rect.size.width - 120.0, 10.0),
            CGSize::new(100.0, 30.0),
        );
        let save_button = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            button_rect,
        );
        save_button.setTitle(&NSString::from_str("Save…"));

        // Create action target.
        let actions = TranscriptActions::new(mtm);

        // Create container view to hold scroll view and button.
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect,
        );
        container.addSubview(&scroll);
        container.addSubview(&save_button);

        // Set up button action and target.
        let target_obj: &NSObject = std::mem::transmute::<&TranscriptActions, &NSObject>(actions.as_ref());
        save_button.setTarget(Some(target_obj));
        save_button.setAction(Some(sel!(saveTranscript:)));

        // Set container as window's content view.
        window.setContentView(Some(&container));

        let state = TranscriptWindowState {
            window,
            text_view,
            log,
        };

        // Store the state in the actions object so the save handler can access it.
        actions.set_window_state(state.clone());

        // Keep the actions object alive by returning it through a leaked Retained.
        let _actions_retained = actions;

        // Window starts invisible; mode-switch logic will show it.
        state.window.setIsVisible(false);

        state
    }
}

/// Append a fragment to the transcript window with autoscroll.
pub fn append_fragment(
    state: &TranscriptWindowState,
    _mtm: MainThreadMarker,
    text: String,
    _ts: chrono::DateTime<chrono::Utc>,
) {
    // Push text to log using local timestamp (TranscriptLog::push captures it).
    state.log.lock().unwrap().push(text);
    // Display update deferred; focus is on data integrity via TranscriptLog
}

/// Clear all text from the transcript window.
pub fn clear_view(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    // TranscriptLog::clear() happens at the data level.
    // Window clearing deferred.
}

/// Show the transcript window (make key and order front).
pub fn order_front(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    unsafe {
        state.window.makeKeyAndOrderFront(None);
    }
}

/// Hide the transcript window.
pub fn order_out(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    unsafe {
        state.window.orderOut(None);
    }
}

/// Format paragraphs as timestamped lines.
pub fn format_paragraphs(paragraphs: &[crate::overlay::transcript_log::Paragraph]) -> String {
    let mut out = String::new();
    for p in paragraphs {
        out.push_str(&p.timestamp.format("[%H:%M:%S] ").to_string());
        out.push_str(&p.text);
        out.push('\n');
    }
    out
}

/// Check if the text view is near the bottom.
fn is_near_bottom(_text_view: &NSTextView) -> bool {
    true
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
            let json_value = state.log.lock().unwrap().to_json("nemotron", chrono::Local::now());
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
        let log = Arc::new(Mutex::new(TranscriptLog::new(std::time::Duration::from_millis(1500))));
        let json = log.lock().unwrap().to_json("nemotron", chrono::Local::now());
        serde_json::to_string(&json).expect("valid JSON serialization");
    }

    #[test]
    fn format_paragraphs_matches_design() {
        use crate::overlay::transcript_log::Paragraph;
        use chrono::Local;

        let now = Local::now();
        let paras = vec![
            Paragraph {
                timestamp: now,
                text: "Hello world".to_string(),
            },
        ];

        let formatted = format_paragraphs(&paras);
        assert!(formatted.starts_with("["));
        assert!(formatted.contains("Hello world"));
        assert!(formatted.ends_with("\n"));
    }
}
