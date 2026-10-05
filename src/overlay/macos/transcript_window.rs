//! Transcript NSWindow: NSScrollView + NSTextView with autoscroll and NSSavePanel export.
//!
//! Mirrors the Linux GTK4 transcript window implementation, providing a regular
//! window (not layer-shell overlay) with timestamped caption display and Save dialog.

use crate::overlay::transcript_follow::FollowState;
use crate::overlay::transcript_log::{AppendKind, Fragment, TranscriptLog};
use objc2::rc::Retained;
use objc2::{
    define_class, msg_send, sel, AnyThread, ClassType, DefinedClass, MainThreadMarker,
    MainThreadOnly,
};
use objc2_app_kit::NSTextField;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSSavePanel, NSScrollView, NSTextView,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::NSAttributedString;
use objc2_foundation::NSNotificationCenter;
use objc2_foundation::{NSObject, NSRange, NSString};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

pub struct TranscriptScrollIvars {
    state: RefCell<Option<TranscriptWindowState>>,
}
define_class!(
    #[unsafe(super(NSScrollView))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalTranscriptScrollView"]
    #[ivars = TranscriptScrollIvars]
    struct TranscriptScrollView;
    impl TranscriptScrollView {
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &objc2_app_kit::NSEvent) {
            let state = self.ivars().state.borrow().clone();
            let before = state.as_ref().map(begin_reader_input);
            if let Some(state) = state.as_ref() { state.wheel_active.set(true); }
            unsafe { let _: () = msg_send![super(self), scrollWheel: event]; }
            if let (Some(state), Some(before)) = (state, before) {
                end_reader_input(&state, before);
                use objc2_app_kit::NSEventPhase;
                let phase = event.phase();
                let momentum = event.momentumPhase();
                let finished = momentum.intersects(NSEventPhase::Ended | NSEventPhase::Cancelled)
                    || (momentum.is_empty() && (phase.is_empty() || phase.intersects(NSEventPhase::Ended | NSEventPhase::Cancelled)));
                state.wheel_terminal.set(finished);
                if finished { settle_reader_intent(&state); }
            }
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &objc2_app_kit::NSEvent) {
            let before = self.contentView().bounds().origin;
            unsafe { let _: () = msg_send![super(self), keyDown: event]; }
            if self.contentView().bounds().origin != before {
                if let Some(state) = self.ivars().state.borrow().as_ref() { observe_position(state); update_status(state); }
            }
        }
    }
);

define_class!(
    #[unsafe(super(NSTextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalTranscriptTextView"]
    #[ivars = TranscriptScrollIvars]
    struct TranscriptTextView;
    impl TranscriptTextView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            let state = self.ivars().state.borrow().clone();
            let before = state.as_ref().map(begin_reader_input);
            unsafe { let _: () = msg_send![super(self), mouseDown: event]; }
            if let (Some(state), Some(before)) = (state, before) { end_reader_input(&state, before); }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: CGSize) {
            let state = self.ivars().state.borrow().clone();
            let anchor = state.as_ref().and_then(|state| {
                if state.mutating.get() || state.follow.borrow().following() { None }
                else { capture_reading_anchor(state) }
            });
            unsafe { let _: () = msg_send![super(self), setFrameSize: size]; }
            if let Some(state) = state {
                if let Some(anchor) = anchor { restore_reading_anchor(&state, anchor); }
                else if !state.mutating.get() { schedule_follow(&state); }
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &objc2_app_kit::NSEvent) {
            let state = self.ivars().state.borrow().clone();
            let before = state.as_ref().map(begin_reader_input);
            unsafe { let _: () = msg_send![super(self), keyDown: event]; }
            if let (Some(state), Some(before)) = (state, before) { end_reader_input(&state, before); }
        }
    }
);

define_class!(
    #[unsafe(super(objc2_app_kit::NSScroller))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalTranscriptScroller"]
    #[ivars = TranscriptScrollIvars]
    struct TranscriptScroller;
    impl TranscriptScroller {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &objc2_app_kit::NSEvent) {
            let state = self.ivars().state.borrow().clone();
            let before = state.as_ref().map(begin_reader_input);
            unsafe { let _: () = msg_send![super(self), mouseDown: event]; }
            if let (Some(state), Some(before)) = (state, before) { end_reader_input(&state, before); }
        }
    }
);

/// Handles needed by the orchestration layer to drive the transcript window.
#[derive(Clone)]
pub struct TranscriptWindowState {
    pub window: Retained<NSWindow>,
    pub text_view: Retained<NSTextView>,
    pub log: Arc<Mutex<TranscriptLog>>,
    pub scroll: Retained<NSScrollView>,
    pub follow: Rc<RefCell<FollowState>>,
    pub status: Retained<NSTextField>,
    pub waiting: Retained<NSTextField>,
    pub autoscroll: Retained<NSButton>,
    pub jump_button: Retained<NSButton>,
    mutating: Rc<Cell<bool>>,
    input_depth: Rc<Cell<usize>>,
    wheel_active: Rc<Cell<bool>>,
    wheel_terminal: Rc<Cell<bool>>,
    intent_generation: Rc<Cell<u64>>,
    input_geometry: Rc<Cell<Option<(f64, f64, f64)>>>,
    live_scroll: Rc<Cell<bool>>,
    intent_callback_queued: Rc<Cell<bool>>,
    callback_queued: Rc<Cell<bool>>,
    callback_count: Rc<Cell<usize>>,
    callback_registrations: Rc<Cell<usize>>,
    presentation: Rc<RefCell<crate::overlay::transcript_presentation::Presentation>>,
}

/// Bundle returned by `build_transcript_window`: the state used to drive
/// the window plus the Retained<TranscriptActions> that owns the Save
/// button's action target. NSButton holds setTarget weakly, so the actions
/// object must outlive the window — keep this whole bundle alive.
pub struct TranscriptWindow {
    pub state: TranscriptWindowState,
    pub actions: Retained<TranscriptActions>,
}

impl Drop for TranscriptWindow {
    fn drop(&mut self) {
        self.state.follow.borrow_mut().invalidate();
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(&self.actions);
            let scroll: &TranscriptScrollView = &*(self.state.scroll.as_ref() as *const NSScrollView
                as *const TranscriptScrollView);
            *scroll.ivars().state.borrow_mut() = None;
            let text: &TranscriptTextView =
                &*(self.state.text_view.as_ref() as *const NSTextView as *const TranscriptTextView);
            *text.ivars().state.borrow_mut() = None;
            if let Some(scroller) = self.state.scroll.verticalScroller() {
                let scroller: &TranscriptScroller = &*(scroller.as_ref()
                    as *const objc2_app_kit::NSScroller
                    as *const TranscriptScroller);
                *scroller.ivars().state.borrow_mut() = None;
            }
            let _: () = msg_send![&*self.state.window, setDelegate: None::<&NSObject>];
        }
        *self.actions.ivars().window_state.borrow_mut() = None;
    }
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

        #[unsafe(method(transcriptLiveScrollStarted:))]
        fn live_scroll_started(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            let state = self.ivars().window_state.borrow().clone();
            if let Some(state) = state {
                state.live_scroll.set(true);
                state.wheel_active.set(true);
                state.input_geometry.set(Some(reader_geometry(&state)));
                state.follow.borrow_mut().invalidate();
            }
        }

        #[unsafe(method(transcriptLiveScrollEnded:))]
        fn live_scroll_ended(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            let state = self.ivars().window_state.borrow().clone();
            if let Some(state) = state {
                state.live_scroll.set(false);
                state.wheel_terminal.set(true);
                settle_reader_intent(&state);
            }
        }

        #[unsafe(method(transcriptScrolled:))]
        fn transcript_scrolled(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            if let Some(state) = self.ivars().window_state.borrow().as_ref() {
                if !state.mutating.get() {
                    if state.input_depth.get() != 0 || state.wheel_active.get() || state.live_scroll.get() { observe_reader_movement(state); }
                    else { schedule_follow(state); }
                }
            }
        }

        #[unsafe(method(toggleAutoscroll:))]
        fn toggle_autoscroll(&self, _sender: Option<&NSButton>) {
            if let Some(state) = self.ivars().window_state.borrow().as_ref() {
                let enabled = !state.follow.borrow().enabled;
                cancel_reader_intent(state);
                state.follow.borrow_mut().set_enabled(enabled);
                if enabled { scroll_to_end(state); }
                update_status(state);
            }
        }

        #[unsafe(method(jumpToLatest:))]
        fn jump_to_latest(&self, _sender: Option<&NSButton>) {
            if let Some(state) = self.ivars().window_state.borrow().as_ref() {
                state.follow.borrow_mut().jump();
                scroll_to_end(state);
                update_status(state);
            }
        }

        /// NSWindowDelegate: intercept the close button so it hides the window
        /// instead of destroying it, allowing re-entry into Transcript mode to
        /// bring it back via makeKeyAndOrderFront.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: Option<&NSWindow>) -> bool {
            let state = self.ivars().window_state.borrow().clone();
            if let Some(state) = state {
                cancel_reader_intent(&state);
                state.follow.borrow_mut().invalidate();
            }
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
        window.setContentMinSize(CGSize::new(540.0, 300.0));

        // Create NSScrollView with NSTextView. The scroll view fills the
        // container above the save-button strip; both stretch with the window.
        let scroll_rect = CGRect::new(
            CGPoint::new(0.0, 34.0),
            CGSize::new(rect.size.width, rect.size.height - 90.0),
        );
        let allocated = TranscriptScrollView::alloc(mtm).set_ivars(TranscriptScrollIvars {
            state: RefCell::new(None),
        });
        let native_scroll: Retained<TranscriptScrollView> =
            msg_send![super(allocated), initWithFrame: scroll_rect];
        let scroll: Retained<NSScrollView> = Retained::from(native_scroll.as_super());
        scroll.setHasVerticalScroller(true);
        let allocated = TranscriptScroller::alloc(mtm).set_ivars(TranscriptScrollIvars {
            state: RefCell::new(None),
        });
        let native_scroller: Retained<TranscriptScroller> = msg_send![super(allocated), initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(15.0, scroll_rect.size.height))];
        scroll.setVerticalScroller(Some(native_scroller.as_super()));
        // A persistent scrollbar avoids flashing and fading on every live append.
        scroll.setScrollerStyle(objc2_app_kit::NSScrollerStyle::Legacy);
        scroll.setAutohidesScrollers(false);
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
        let allocated = TranscriptTextView::alloc(mtm).set_ivars(TranscriptScrollIvars {
            state: RefCell::new(None),
        });
        let native_text: Retained<TranscriptTextView> = msg_send![super(allocated), initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), scroll_rect.size)];
        let text_view: Retained<NSTextView> = Retained::from(native_text.as_super());
        text_view.setEditable(false);
        text_view.setSelectable(true);
        text_view.setRichText(false);
        text_view.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(14.0)));
        text_view.setTextColor(Some(&objc2_app_kit::NSColor::textColor()));
        text_view.setBackgroundColor(&objc2_app_kit::NSColor::textBackgroundColor());
        text_view.setTextContainerInset(CGSize::new(20.0, 20.0));
        let paragraph = objc2_app_kit::NSMutableParagraphStyle::new();
        paragraph.setParagraphSpacing(10.0);
        paragraph.setLineSpacing(3.0);
        text_view.setDefaultParagraphStyle(Some(&paragraph));
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
            CGPoint::new(rect.size.width - 110.0, rect.size.height - 43.0),
            CGSize::new(100.0, 30.0),
        );
        let save_button = NSButton::initWithFrame(NSButton::alloc(mtm), button_rect);
        save_button.setTitle(&NSString::from_str("Save…"));
        save_button.setToolTip(Some(&NSString::from_str("Save the transcript as JSON")));
        let _: () = msg_send![&*save_button, setAccessibilityLabel: &*NSString::from_str("Save transcript")];
        let _: () =
            msg_send![&*text_view, setAccessibilityLabel: &*NSString::from_str("Transcript text")];
        save_button.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );

        // Create action target.
        let actions = TranscriptActions::new(mtm);

        // Create container view to hold scroll view and button.
        let container = NSView::initWithFrame(NSView::alloc(mtm), rect);
        container.addSubview(&scroll);
        container.addSubview(&save_button);
        let status = NSTextField::labelWithString(&NSString::from_str("Following live"), mtm);
        status.setFrame(CGRect::new(
            CGPoint::new(20.0, 8.0),
            CGSize::new(rect.size.width - 40.0, 20.0),
        ));
        status.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(12.0)));
        status.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        container.addSubview(&status);
        let title = NSTextField::labelWithString(&NSString::from_str("Transcript"), mtm);
        title.setFrame(CGRect::new(
            CGPoint::new(20.0, rect.size.height - 39.0),
            CGSize::new(130.0, 24.0),
        ));
        title.setFont(Some(&objc2_app_kit::NSFont::boldSystemFontOfSize(16.0)));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        container.addSubview(&title);
        let waiting = NSTextField::labelWithString(&NSString::from_str("Waiting for speech…"), mtm);
        waiting.setFrame(CGRect::new(
            CGPoint::new(20.0, rect.size.height - 100.0),
            CGSize::new(rect.size.width - 40.0, 24.0),
        ));
        waiting.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(14.0)));
        waiting.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
        waiting.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
        let _: () =
            msg_send![&*waiting, setAccessibilityLabel: &*NSString::from_str("Waiting for speech")];
        let _: () = msg_send![&*status, setAccessibilityLabel: &*NSString::from_str("Transcript following status")];
        container.addSubview(&waiting);
        let mut controls = Vec::new();
        for (x, title, action) in [
            (
                rect.size.width - 370.0,
                "Autoscroll",
                sel!(toggleAutoscroll:),
            ),
            (
                rect.size.width - 250.0,
                "Jump to latest",
                sel!(jumpToLatest:),
            ),
        ] {
            let button = NSButton::initWithFrame(
                NSButton::alloc(mtm),
                CGRect::new(
                    CGPoint::new(x, rect.size.height - 43.0),
                    CGSize::new(120.0, 30.0),
                ),
            );
            button.setTitle(&NSString::from_str(title));
            button.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin
                    | NSAutoresizingMaskOptions::ViewMinYMargin,
            );
            button.setToolTip(Some(&NSString::from_str(if title == "Autoscroll" {
                "Follow new speech automatically"
            } else {
                "Move to the latest speech without changing Autoscroll"
            })));
            let _: () = msg_send![&*button, setAccessibilityLabel: &*NSString::from_str(title)];
            button.setTarget(Some(actions.as_ref()));
            button.setAction(Some(action));
            container.addSubview(&button);
            controls.push(button);
        }
        let autoscroll = controls.remove(0);
        autoscroll.setButtonType(objc2_app_kit::NSButtonType::Switch);
        autoscroll.setState(1);
        let jump_button = controls.remove(0);

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
            scroll,
            follow: Rc::new(RefCell::new(FollowState::default())),
            status,
            waiting,
            autoscroll,
            jump_button,
            mutating: Rc::new(Cell::new(false)),
            input_depth: Rc::new(Cell::new(0)),
            wheel_active: Rc::new(Cell::new(false)),
            wheel_terminal: Rc::new(Cell::new(false)),
            intent_generation: Rc::new(Cell::new(0)),
            input_geometry: Rc::new(Cell::new(None)),
            live_scroll: Rc::new(Cell::new(false)),
            intent_callback_queued: Rc::new(Cell::new(false)),
            callback_queued: Rc::new(Cell::new(false)),
            callback_count: Rc::new(Cell::new(0)),
            callback_registrations: Rc::new(Cell::new(0)),
            presentation: Rc::new(RefCell::new(
                crate::overlay::transcript_presentation::Presentation::default(),
            )),
        };
        actions.set_window_state(state.clone());
        *native_scroll.ivars().state.borrow_mut() = Some(state.clone());
        *native_text.ivars().state.borrow_mut() = Some(state.clone());
        *native_scroller.ivars().state.borrow_mut() = Some(state.clone());
        state
            .scroll
            .contentView()
            .setPostsBoundsChangedNotifications(true);
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &actions,
            sel!(transcriptScrolled:),
            Some(&NSString::from_str("NSViewBoundsDidChangeNotification")),
            Some(state.scroll.contentView().as_ref()),
        );

        for (name, selector) in [
            (
                "NSScrollViewWillStartLiveScrollNotification",
                sel!(transcriptLiveScrollStarted:),
            ),
            (
                "NSScrollViewDidEndLiveScrollNotification",
                sel!(transcriptLiveScrollEnded:),
            ),
        ] {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &actions,
                selector,
                Some(&NSString::from_str(name)),
                Some(state.scroll.as_ref()),
            );
        }
        update_status(&state);
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
    state.mutating.set(true);
    state.follow.borrow_mut().invalidate();
    let origin = state.scroll.contentView().bounds().origin;
    let selection = state.text_view.selectedRange();
    use crate::overlay::transcript_presentation::{byte_from_utf16, utf16_offset, Presentation};
    let viewport_anchor = capture_reading_anchor(state);
    let old = state.presentation.borrow();
    let start = old.position(byte_from_utf16(&old.text, selection.location));
    let end = old.position(byte_from_utf16(
        &old.text,
        selection.location.saturating_add(selection.length),
    ));
    drop(old);
    let presentation = Presentation::from_log(&state.log.lock().unwrap(), speaker_names);
    let unchanged = presentation.text == state.presentation.borrow().text;
    let location = if unchanged {
        selection.location
    } else {
        start
            .map(|p| utf16_offset(&presentation.text, presentation.resolve(p)))
            .unwrap_or(0)
    };
    let end = if unchanged {
        selection.location.saturating_add(selection.length)
    } else {
        end.map(|p| utf16_offset(&presentation.text, presentation.resolve(p)))
            .unwrap_or(location)
    };
    let ns = NSString::from_str(&presentation.text);
    state.text_view.setString(&ns);
    state.text_view.setSelectedRange(NSRange {
        location,
        length: end.saturating_sub(location),
    });
    *state.presentation.borrow_mut() = presentation;
    style_metadata(state, 0);
    if state.follow.borrow().following() {
        schedule_follow(state);
    } else {
        if let Some(anchor) = viewport_anchor {
            restore_reading_anchor(state, anchor);
        } else {
            state.scroll.contentView().scrollToPoint(origin);
            state
                .scroll
                .reflectScrolledClipView(&state.scroll.contentView());
        }
    }
    state.mutating.set(false);
    update_status(state);
}

/// Clear the displayed text. The caller is responsible for clearing the
/// underlying `TranscriptLog` separately (it is surface 1 of the
/// 4-surface clear; this is surface 2).
pub fn clear_view(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    cancel_reader_intent(state);
    let empty = NSString::from_str("");
    state.mutating.set(true);
    state.follow.borrow_mut().clear();
    *state.presentation.borrow_mut() = Default::default();
    state.text_view.setString(&empty);
    state.mutating.set(false);
    update_status(state);
}

/// Show the transcript window (make key and order front).
pub fn order_front(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    state.window.makeKeyAndOrderFront(None);
    schedule_follow(state);
    update_status(state);
}

/// Hide the transcript window.
pub fn order_out(state: &TranscriptWindowState, _mtm: MainThreadMarker) {
    cancel_reader_intent(state);
    state.follow.borrow_mut().invalidate();
    state.window.orderOut(None);
}

/// Format fragments as timestamped paragraphs with speaker labels.
///
/// Paragraph breaks mirror `TranscriptLog::push_at_with_speaker`: first
/// fragment, a gap greater than 1.5 seconds, or a speaker change when both
/// adjacent fragments have speaker IDs. Fragment text is preserved verbatim.
#[cfg(test)]
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

#[cfg(test)]
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

/// Record reader movement using the shared two-point bottom tolerance.
/// The notification handler calls this only for active input events, never
/// during guarded storage/layout mutations.
#[cfg(feature = "native-transcript-tests")]
pub fn native_reading_anchor(
    state: &TranscriptWindowState,
) -> Option<(
    crate::overlay::transcript_presentation::FragmentPosition,
    f64,
)> {
    capture_reading_anchor(state)
}

#[cfg(feature = "native-transcript-tests")]
pub fn native_anchor_screen_offset(
    state: &TranscriptWindowState,
    position: crate::overlay::transcript_presentation::FragmentPosition,
) -> Option<f64> {
    let character = {
        let presentation = state.presentation.borrow();
        crate::overlay::transcript_presentation::utf16_offset(
            &presentation.text,
            presentation.resolve(position),
        )
    };
    Some(character_rect(state, character)?.origin.y - state.scroll.contentView().bounds().origin.y)
}

type ReadingAnchor = (
    crate::overlay::transcript_presentation::FragmentPosition,
    f64,
);

fn character_rect(state: &TranscriptWindowState, character: usize) -> Option<CGRect> {
    let layout = unsafe { state.text_view.layoutManager() }?;
    let container = unsafe { state.text_view.textContainer() }?;
    layout.ensureLayoutForTextContainer(&container);
    let length = state.text_view.string().length();
    let character = character.min(length);
    unsafe {
        let glyphs: NSRange = msg_send![&*layout, glyphRangeForCharacterRange: NSRange { location: character, length: usize::from(character < length) }, actualCharacterRange: std::ptr::null_mut::<NSRange>()];
        let mut rect: CGRect =
            msg_send![&*layout, boundingRectForGlyphRange: glyphs, inTextContainer: &*container];
        rect.origin.y += state.text_view.textContainerInset().height;
        Some(rect)
    }
}

fn capture_reading_anchor(state: &TranscriptWindowState) -> Option<ReadingAnchor> {
    let origin = state.scroll.contentView().bounds().origin;
    let character: usize = unsafe {
        msg_send![&*state.text_view, characterIndexForInsertionAtPoint: CGPoint::new(origin.x + 24.0, origin.y + 1.0)]
    };
    let position = {
        let presentation = state.presentation.borrow();
        presentation.position(crate::overlay::transcript_presentation::byte_from_utf16(
            &presentation.text,
            character,
        ))?
    };
    let rect = character_rect(state, character)?;
    Some((position, origin.y - rect.origin.y))
}

fn restore_reading_anchor(state: &TranscriptWindowState, (position, delta): ReadingAnchor) {
    let previous = state.mutating.replace(true);
    let character = {
        let presentation = state.presentation.borrow();
        crate::overlay::transcript_presentation::utf16_offset(
            &presentation.text,
            presentation.resolve(position),
        )
    };
    if let Some(rect) = character_rect(state, character) {
        let clip = state.scroll.contentView();
        let mut origin = clip.bounds().origin;
        origin.y = (rect.origin.y + delta).max(0.0);
        clip.scrollToPoint(origin);
        state.scroll.reflectScrolledClipView(&clip);
    }
    state.mutating.set(previous);
}

fn reader_geometry(state: &TranscriptWindowState) -> (f64, f64, f64) {
    let bounds = state.scroll.contentView().bounds();
    (
        bounds.origin.y,
        state.text_view.bounds().size.height,
        bounds.size.height,
    )
}

fn observe_reader_movement(state: &TranscriptWindowState) {
    let geometry = reader_geometry(state);
    let previous = state.input_geometry.replace(Some(geometry));
    if let Some((value, document, viewport)) = previous {
        if document == geometry.1 && viewport == geometry.2 && value != geometry.0 {
            state
                .follow
                .borrow_mut()
                .observe_user(geometry.0, geometry.1, geometry.2);
            update_status(state);
        }
    }
}

fn cancel_reader_intent(state: &TranscriptWindowState) {
    state
        .intent_generation
        .set(state.intent_generation.get().wrapping_add(1));
    state.wheel_active.set(false);
    state.live_scroll.set(false);
    state.input_geometry.set(None);
}

fn settle_reader_intent(state: &TranscriptWindowState) {
    if state.intent_callback_queued.replace(true) {
        return;
    }
    let generation = state.intent_generation.get();
    let state = Box::into_raw(Box::new(state.clone())) as usize;
    dispatch2::DispatchQueue::main().exec_async(move || {
        let state = unsafe { Box::from_raw(state as *mut TranscriptWindowState) };
        state.intent_callback_queued.set(false);
        if state.intent_generation.get() != generation {
            if state.wheel_active.get() && state.wheel_terminal.get() && !state.live_scroll.get() {
                settle_reader_intent(&state);
            }
            return;
        }
        if state.live_scroll.get() || !state.wheel_terminal.get() {
            return;
        }
        observe_reader_movement(&state);
        state.wheel_active.set(false);
        state.input_geometry.set(None);
        schedule_follow(&state);
    });
}

fn begin_reader_input(state: &TranscriptWindowState) -> CGPoint {
    state
        .intent_generation
        .set(state.intent_generation.get().wrapping_add(1));
    state.input_depth.set(state.input_depth.get() + 1);
    state.follow.borrow_mut().invalidate();
    state.input_geometry.set(Some(reader_geometry(state)));
    state.scroll.contentView().bounds().origin
}

fn end_reader_input(state: &TranscriptWindowState, before: CGPoint) {
    state
        .input_depth
        .set(state.input_depth.get().saturating_sub(1));
    let _ = before;
    observe_reader_movement(state);
    update_status(state);
    schedule_follow(state);
}

fn observe_position(state: &TranscriptWindowState) {
    let visible = state.scroll.contentView().bounds();
    state.follow.borrow_mut().observe_user(
        visible.origin.y,
        state.text_view.bounds().size.height,
        visible.size.height,
    );
}

fn update_status(state: &TranscriptWindowState) {
    let (status, enabled, following) = {
        let follow = state.follow.borrow();
        (follow.status(), follow.enabled, follow.following())
    };
    state.status.setStringValue(&NSString::from_str(status));
    state.autoscroll.setState(if enabled { 1 } else { 0 });
    state.jump_button.setHidden(following);
    state
        .waiting
        .setHidden(state.text_view.string().length() != 0);
}

fn scroll_to_end(state: &TranscriptWindowState) {
    let previous = state.mutating.replace(true);
    state.text_view.scrollRangeToVisible(NSRange {
        location: state.text_view.string().length(),
        length: 0,
    });
    state.mutating.set(previous);
}

fn style_metadata(state: &TranscriptWindowState, from_byte: usize) {
    use crate::overlay::transcript_presentation::{utf16_offset, MetadataKind};
    let spans: Vec<_> = {
        let presentation = state.presentation.borrow();
        presentation
            .metadata
            .iter()
            .filter(|span| span.range.start >= from_byte)
            .map(|span| {
                (
                    utf16_offset(&presentation.text, span.range.start),
                    utf16_offset(&presentation.text, span.range.end),
                    span.kind,
                )
            })
            .collect()
    };
    let (body_start, body_end) = {
        let presentation = state.presentation.borrow();
        (
            utf16_offset(&presentation.text, from_byte),
            presentation.text.encode_utf16().count(),
        )
    };
    if let Some(storage) = unsafe { state.text_view.textStorage() } {
        let range = NSRange {
            location: body_start,
            length: body_end - body_start,
        };
        let font = objc2_app_kit::NSFont::systemFontOfSize(14.0);
        let color = objc2_app_kit::NSColor::textColor();
        let paragraph = objc2_app_kit::NSMutableParagraphStyle::new();
        paragraph.setParagraphSpacing(10.0);
        paragraph.setLineSpacing(3.0);
        unsafe {
            let _: () = msg_send![&*storage, addAttribute: &*NSString::from_str("NSFont"), value: &*font, range: range];
            let _: () = msg_send![&*storage, addAttribute: &*NSString::from_str("NSColor"), value: &*color, range: range];
            let _: () = msg_send![&*storage, addAttribute: &*NSString::from_str("NSParagraphStyle"), value: &*paragraph, range: range];
        }
        for (start, end, kind) in spans {
            let color = match kind {
                MetadataKind::Timestamp => objc2_app_kit::NSColor::secondaryLabelColor(),
                MetadataKind::Speaker => objc2_app_kit::NSColor::labelColor(),
            };
            unsafe {
                let range = NSRange {
                    location: start,
                    length: end - start,
                };
                let _: () = msg_send![&*storage, addAttribute: &*NSString::from_str("NSColor"), value: &*color, range: range];
                if kind == MetadataKind::Speaker {
                    let font = objc2_app_kit::NSFont::boldSystemFontOfSize(14.0);
                    let _: () = msg_send![&*storage, addAttribute: &*NSString::from_str("NSFont"), value: &*font, range: range];
                }
            }
        }
    }
}

#[cfg(feature = "native-transcript-tests")]
pub fn native_click_autoscroll(state: &TranscriptWindowState) {
    unsafe {
        state.autoscroll.performClick(None);
    }
}
#[cfg(feature = "native-transcript-tests")]
pub fn native_click_jump(state: &TranscriptWindowState) {
    unsafe {
        state.jump_button.performClick(None);
    }
}
#[cfg(feature = "native-transcript-tests")]
pub fn native_follow_callback_count(state: &TranscriptWindowState) -> usize {
    state.callback_count.get()
}
#[cfg(feature = "native-transcript-tests")]
pub fn native_follow_callback_queued(state: &TranscriptWindowState) -> bool {
    state.callback_queued.get()
}
#[cfg(feature = "native-transcript-tests")]
pub fn native_follow_callback_registrations(state: &TranscriptWindowState) -> usize {
    state.callback_registrations.get()
}
#[cfg(feature = "native-transcript-tests")]
pub fn native_layout_drained(state: &TranscriptWindowState) -> bool {
    if let (Some(layout), Some(container)) = (unsafe { state.text_view.layoutManager() }, unsafe {
        state.text_view.textContainer()
    }) {
        layout.ensureLayoutForTextContainer(&container);
    }
    !state.callback_queued.get() && !state.follow.borrow().pending
}

fn schedule_follow(state: &TranscriptWindowState) {
    if state.input_depth.get() != 0 || state.wheel_active.get() || state.live_scroll.get() {
        return;
    }
    state.follow.borrow_mut().request();
    if !state.follow.borrow().pending || state.callback_queued.replace(true) {
        return;
    }
    state
        .callback_registrations
        .set(state.callback_registrations.get() + 1);
    // The retained AppKit handles are created and consumed exclusively on the main queue.
    let state = Box::into_raw(Box::new(state.clone())) as usize;
    dispatch2::DispatchQueue::main().exec_async(move || {
        let state = unsafe { Box::from_raw(state as *mut TranscriptWindowState) };
        state.callback_count.set(state.callback_count.get() + 1);
        state.callback_queued.set(false);
        let generation = state.follow.borrow().generation;
        if state.input_depth.get() == 0
            && !state.wheel_active.get()
            && !state.live_scroll.get()
            && state.follow.borrow().eligible(generation)
            && state.window.isVisible()
        {
            if let (Some(layout), Some(container)) =
                (unsafe { state.text_view.layoutManager() }, unsafe {
                    state.text_view.textContainer()
                })
            {
                layout.ensureLayoutForTextContainer(&container);
            }
            scroll_to_end(&state);
        }
        state.follow.borrow_mut().complete(generation);
    });
}

/// Display an already-recorded fragment; never mutates the transcript log.
pub fn append_fragment_to_view(
    state: &TranscriptWindowState,
    fragment: &Fragment,
    kind: AppendKind,
    speaker_names: &HashMap<u32, String>,
) {
    state.mutating.set(true);
    let mut presentation = state.presentation.borrow_mut();
    let start = presentation.text.len();
    presentation.append(fragment, kind, speaker_names);
    let text = presentation.text[start..].to_string();
    drop(presentation);
    let selection = state.text_view.selectedRange();
    if let Some(storage) = unsafe { state.text_view.textStorage() } {
        let attributed = NSAttributedString::initWithString(
            NSAttributedString::alloc(),
            &NSString::from_str(&text),
        );
        storage.beginEditing();
        storage.appendAttributedString(&attributed);
        style_metadata(state, start);
        storage.endEditing();
    }
    state.text_view.setSelectedRange(selection);
    schedule_follow(state);
    state.mutating.set(false);
    update_status(state);
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
