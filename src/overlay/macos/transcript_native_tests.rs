//! Main-thread AppKit checks; invoked by the harness-free native test runner.
use super::transcript_window::*;
use crate::overlay::transcript_log::TranscriptLog;
use objc2::{msg_send, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::NSApplication;
use objc2_foundation::NSRange;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

fn pump_until(mtm: MainThreadMarker, condition: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let app = NSApplication::sharedApplication(mtm);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "native layout condition timed out"
        );
        {
            let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                objc2_app_kit::NSEventMask::Any,
                Some(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(
                    0.01,
                )),
                &objc2_foundation::NSString::from_str("kCFRunLoopDefaultMode"),
                true,
            );
            if let Some(event) = event {
                app.sendEvent(&event);
            }
        }
    }
}

fn native_follow_scroll_matrix(mtm: MainThreadMarker) {
    let log = Arc::new(Mutex::new(TranscriptLog::new(Duration::from_millis(1500))));
    let bundle = build_transcript_window(mtm, log.clone());
    let state = &bundle.state;
    order_front(state, mtm);
    for sample in 0..120 {
        let (fragment, kind) = {
            let mut log = log.lock().unwrap();
            let kind = log.push_with_speaker_and_sample(
                format!(
                    " wrapped history {sample} — hé🙂 {}",
                    "caption words ".repeat(12)
                ),
                Some(sample % 2),
                sample as u64,
            );
            (log.fragments().last().unwrap().clone(), kind)
        };
        append_fragment_to_view(state, &fragment, kind, &HashMap::new());
    }
    pump_until(mtm, || native_layout_drained(state));
    let visible = state.scroll.contentView().bounds();
    assert!(
        state.text_view.bounds().size.height > visible.size.height * 2.0,
        "matrix must exercise actual overflow after settled layout"
    );
    assert!(
        native_layout_drained(state),
        "follow callbacks must be drained before bottom assertion"
    );
    assert!(
        crate::overlay::transcript_follow::at_bottom(
            visible.origin.y,
            state.text_view.bounds().size.height,
            visible.size.height
        ),
        "settled native view must follow bottom"
    );
    assert!(
        state.follow.borrow().following(),
        "native_follow_scroll_matrix: default following"
    );
    state
        .window
        .setContentSize(objc2_core_foundation::CGSize::new(540.0, 420.0));
    pump_until(mtm, || native_layout_drained(state));
    let resized = state.scroll.contentView().bounds();
    assert!(
        crate::overlay::transcript_follow::at_bottom(
            resized.origin.y,
            state.text_view.bounds().size.height,
            resized.size.height
        ),
        "resize preserves following"
    );
    native_click_autoscroll(state);
    assert!(
        !state.follow.borrow().enabled,
        "native switch turns preference off"
    );
    native_click_jump(state);
    assert!(
        !state.follow.borrow().enabled,
        "native Jump must not enable preference"
    );
    let origin = state.scroll.contentView().bounds().origin;
    rebuild_view(state, mtm, &HashMap::new());
    assert!(!state.follow.borrow().enabled);
    assert!((state.scroll.contentView().bounds().origin.y - origin.y).abs() < 30.0);
    order_out(state, mtm);
    println!("native_follow_scroll_matrix passed");
}

fn append(state: &TranscriptWindowState, sample: u64) {
    let (fragment, kind) = {
        let mut log = state.log.lock().unwrap();
        let kind = log.push_with_speaker_and_sample(
            format!(" row {sample} hé🙂 {}", "wrapped words ".repeat(20)),
            Some((sample % 2) as u32),
            sample,
        );
        (log.fragments().last().unwrap().clone(), kind)
    };
    append_fragment_to_view(state, &fragment, kind, &HashMap::new());
}

fn fixture(mtm: MainThreadMarker) -> TranscriptWindow {
    let bundle = build_transcript_window(
        mtm,
        Arc::new(Mutex::new(TranscriptLog::new(Duration::from_millis(1500)))),
    );
    NSApplication::sharedApplication(mtm).activate();
    order_front(&bundle.state, mtm);
    unsafe {
        let _: () = msg_send![&*bundle.state.scroll, setScrollsDynamically: true];
    }
    for sample in 0..80 {
        append(&bundle.state, sample);
    }
    pump_until(mtm, || native_layout_drained(&bundle.state));
    assert!(
        bundle.state.text_view.bounds().size.height
            > bundle.state.scroll.contentView().bounds().size.height * 2.0
    );
    bundle
}

fn key(state: &TranscriptWindowState, character: &str, code: u16) {
    let chars = objc2_foundation::NSString::from_str(character);
    let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        objc2_app_kit::NSEventType::KeyDown, objc2_core_foundation::CGPoint::new(0.0, 0.0), objc2_app_kit::NSEventModifierFlags::empty(), 0.0, state.window.windowNumber(), None, &chars, &chars, false, code).unwrap();
    state.text_view.keyDown(&event);
}

objc2::define_class!(
    #[unsafe(super(objc2_app_kit::NSEvent))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalNativeTestWheelEvent"]
    #[ivars = (objc2::rc::Retained<objc2_app_kit::NSWindow>, objc2_core_foundation::CGPoint)]
    struct NativeWheelEvent;
    impl NativeWheelEvent {
        #[unsafe(method(window))] fn window(&self) -> *const objc2_app_kit::NSWindow { &*objc2::DefinedClass::ivars(self).0 }
        #[unsafe(method(windowNumber))] fn window_number(&self) -> isize { objc2::DefinedClass::ivars(self).0.windowNumber() }
        #[unsafe(method(locationInWindow))] fn location(&self) -> objc2_core_foundation::CGPoint { objc2::DefinedClass::ivars(self).1 }
        #[unsafe(method(type))] fn event_type(&self) -> objc2_app_kit::NSEventType { objc2_app_kit::NSEventType::ScrollWheel }
        #[unsafe(method(deltaY))] fn delta_y(&self) -> f64 { -12.0 }
        #[unsafe(method(scrollingDeltaY))] fn scrolling_delta_y(&self) -> f64 { -12.0 }
        #[unsafe(method(deltaX))] fn delta_x(&self) -> f64 { 0.0 }
        #[unsafe(method(scrollingDeltaX))] fn scrolling_delta_x(&self) -> f64 { 0.0 }
        #[unsafe(method(hasPreciseScrollingDeltas))] fn precise(&self) -> bool { false }
        #[unsafe(method(phase))] fn phase(&self) -> objc2_app_kit::NSEventPhase { objc2_app_kit::NSEventPhase::None }
        #[unsafe(method(momentumPhase))] fn momentum_phase(&self) -> objc2_app_kit::NSEventPhase { objc2_app_kit::NSEventPhase::None }
    }
);

fn wheel(state: &TranscriptWindowState) {
    use objc2::MainThreadOnly;
    let event: objc2::rc::Retained<NativeWheelEvent> = unsafe {
        msg_send![
            super(
                NativeWheelEvent::alloc(MainThreadMarker::new().unwrap()).set_ivars((
                    state.window.clone(),
                    state
                        .scroll
                        .convertPoint_toView(objc2_core_foundation::CGPoint::new(50.0, 50.0), None)
                ))
            ),
            init
        ]
    };
    NSApplication::sharedApplication(state.window.mtm()).sendEvent(&event);
}

fn native_input_race(mtm: MainThreadMarker) {
    for keyboard in [true, false] {
        let bundle = fixture(mtm);
        let state = &bundle.state;
        state.text_view.setSelectedRange(NSRange {
            location: state.text_view.string().length(),
            length: 0,
        });
        append(state, 80); // Queue follow, then deliver reader input before its callback.
        let before = state.scroll.contentView().bounds().origin.y;
        if keyboard {
            key(state, "\u{f72c}", 116);
        } else {
            wheel(state);
        }
        if !keyboard {
            pump_until(mtm, || {
                state.scroll.contentView().bounds().origin.y < before - 2.0
            });
        }
        let reading = state.scroll.contentView().bounds().origin.y;
        assert!(
            reading < before - 2.0,
            "native input must actually move upward ({keyboard}): {before} -> {reading}"
        );
        assert!(state.follow.borrow().enabled);
        assert!(
            !state.follow.borrow().following(),
            "reader movement pauses queued follow"
        );
        pump_until(mtm, || native_layout_drained(state));
        assert!(
            (state.scroll.contentView().bounds().origin.y - reading).abs() < 2.0,
            "stale follow must not undo input"
        );
        order_out(state, mtm);
    }
    println!("native_input_race passed");
}

fn native_non_scrolling_interaction_keeps_following(mtm: MainThreadMarker) {
    let bundle = fixture(mtm);
    let state = &bundle.state;
    state.text_view.setSelectedRange(NSRange {
        location: state.text_view.string().length(),
        length: 0,
    });
    let before = state.scroll.contentView().bounds().origin.y;
    key(state, "\u{f702}", 123); // Left changes selection, not viewport.
    let point = state.text_view.convertPoint_toView(
        objc2_core_foundation::CGPoint::new(40.0, before + 40.0),
        None,
    );
    let mouse = |kind| {
        objc2_app_kit::NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind, point, objc2_app_kit::NSEventModifierFlags::empty(), 0.0, state.window.windowNumber(), None, 1, 1, 1.0).unwrap()
    };
    let down = mouse(objc2_app_kit::NSEventType::LeftMouseDown);
    let up = mouse(objc2_app_kit::NSEventType::LeftMouseUp);
    NSApplication::sharedApplication(mtm).postEvent_atStart(&up, true);
    state.text_view.mouseDown(&down);
    assert!((state.scroll.contentView().bounds().origin.y - before).abs() < 2.0);
    assert!(
        state.follow.borrow().following(),
        "non-scrolling selection is not reader movement"
    );
    append(state, 80);
    pump_until(mtm, || native_layout_drained(state));
    assert!(state.follow.borrow().following());
    order_out(state, mtm);
    println!("native_non_scrolling_interaction_keeps_following passed");
}

fn native_stale_scroll_cancelled(mtm: MainThreadMarker) {
    let bundle = fixture(mtm);
    let state = &bundle.state;
    append(state, 80);
    native_click_autoscroll(state);
    let off_origin = state.scroll.contentView().bounds().origin.y;
    pump_until(mtm, || native_layout_drained(state));
    assert!(!state.follow.borrow().enabled);
    assert!((state.scroll.contentView().bounds().origin.y - off_origin).abs() < 2.0);
    native_click_autoscroll(state);
    append(state, 81);
    let generation = state.follow.borrow().generation;
    order_out(state, mtm);
    assert!(state.follow.borrow().generation > generation);
    pump_until(mtm, || native_layout_drained(state));
    assert!(!state.window.isVisible());
    order_front(state, mtm);
    append(state, 82);
    state.log.lock().unwrap().clear();
    clear_view(state, mtm);
    pump_until(mtm, || native_layout_drained(state));
    assert_eq!(state.text_view.string().length(), 0);
    assert!(state.follow.borrow().enabled, "clear retains preference");
    order_out(state, mtm);
    println!("native_stale_scroll_cancelled passed");
}

fn native_burst_coalesced(mtm: MainThreadMarker) {
    let bundle = fixture(mtm);
    let state = &bundle.state;
    let callbacks = native_follow_callback_count(state);
    let registrations = native_follow_callback_registrations(state);
    for sample in 80..180 {
        append(state, sample);
    }
    assert!(native_follow_callback_queued(state));
    assert_eq!(
        native_follow_callback_registrations(state) - registrations,
        1,
        "burst registers one callback"
    );
    pump_until(mtm, || native_layout_drained(state));
    assert_eq!(
        native_follow_callback_count(state) - callbacks,
        1,
        "burst executes one callback"
    );
    assert_eq!(state.log.lock().unwrap().fragments().len(), 180);
    assert!(state.text_view.string().to_string().contains("row 179"));
    let visible = state.scroll.contentView().bounds();
    assert!(crate::overlay::transcript_follow::at_bottom(
        visible.origin.y,
        state.text_view.bounds().size.height,
        visible.size.height
    ));
    order_out(state, mtm);
    println!("native_burst_coalesced passed");
}

fn native_reading_anchor_and_selection(mtm: MainThreadMarker) {
    let bundle = fixture(mtm);
    let state = &bundle.state;
    state.text_view.setSelectedRange(NSRange {
        location: state.text_view.string().length(),
        length: 0,
    });
    key(state, "\u{f72c}", 116);
    assert!(!state.follow.borrow().following());
    let text = state.text_view.string().to_string();
    let byte = text.find("hé🙂").unwrap();
    state.text_view.setSelectedRange(NSRange {
        location: text[..byte].encode_utf16().count(),
        length: "hé🙂".encode_utf16().count(),
    });
    let anchor = native_reading_anchor(state).expect("overflow has reading anchor");
    let names = HashMap::from([
        (0, "Long Unicode speaker 名🙂".to_owned()),
        (1, "Other speaker".to_owned()),
    ]);
    rebuild_view(state, mtm, &names);
    pump_until(mtm, || native_layout_drained(state));
    let selected = state.text_view.selectedRange();
    let utf16: Vec<_> = state
        .text_view
        .string()
        .to_string()
        .encode_utf16()
        .collect();
    assert_eq!(
        String::from_utf16(&utf16[selected.location..selected.location + selected.length]).unwrap(),
        "hé🙂"
    );
    let renamed_offset = native_anchor_screen_offset(state, anchor.0).unwrap();
    assert!(
        (renamed_offset + anchor.1).abs() < 2.0,
        "rename retains original fragment character's screen-relative pixel offset"
    );
    state.log.lock().unwrap().relabel_since(40, 0);
    rebuild_view(state, mtm, &names);
    pump_until(mtm, || native_layout_drained(state));
    let selected = state.text_view.selectedRange();
    let utf16: Vec<_> = state
        .text_view
        .string()
        .to_string()
        .encode_utf16()
        .collect();
    assert_eq!(
        String::from_utf16(&utf16[selected.location..selected.location + selected.length]).unwrap(),
        "hé🙂"
    );
    let relabeled_offset = native_anchor_screen_offset(state, anchor.0).unwrap();
    assert!(
        (relabeled_offset + anchor.1).abs() < 2.0,
        "relabel retains original stable character's pixel offset"
    );
    state
        .window
        .setContentSize(objc2_core_foundation::CGSize::new(540.0, 420.0));
    pump_until(mtm, || native_layout_drained(state));
    let resized_offset = native_anchor_screen_offset(state, anchor.0).unwrap();
    assert!(
        (resized_offset + anchor.1).abs() < 2.0,
        "paused resize retains original stable character's pixel offset"
    );
    assert!(!state.follow.borrow().following());
    order_out(state, mtm);
    println!("native_reading_anchor_and_selection passed");
}

fn native_panel_render_properties(mtm: MainThreadMarker) {
    let bundle = build_transcript_window(
        mtm,
        Arc::new(Mutex::new(TranscriptLog::new(Duration::from_millis(1500)))),
    );
    let state = &bundle.state;
    assert_eq!(
        state.scroll.scrollerStyle(),
        objc2_app_kit::NSScrollerStyle::Legacy
    );
    assert!(!state.scroll.autohidesScrollers());
    assert!(state.scroll.hasVerticalScroller());
    assert!(!state.text_view.isEditable());
    assert!(state.text_view.isSelectable());
    assert!(state.text_view.font().unwrap().pointSize() >= 14.0);
    assert_eq!(
        state.text_view.string().length(),
        0,
        "empty hint is not selectable transcript storage"
    );
    let label: objc2::rc::Retained<objc2_foundation::NSString> =
        unsafe { msg_send![&*state.text_view, accessibilityLabel] };
    assert_eq!(label.to_string(), "Transcript text");
    let content = state.window.contentView().unwrap();
    let views = content.subviews();
    let mut buttons = Vec::new();
    let mut waiting = None;
    for view in views.iter() {
        if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>() {
            buttons.push(button.title().to_string());
            assert!(button.toolTip().is_some(), "controls explain their action");
            assert!(
                button.target().is_some(),
                "weak action target retained by bundle"
            );
        }
        if let Some(field) = view.downcast_ref::<objc2_app_kit::NSTextField>() {
            if field.stringValue().to_string() == "Waiting for speech…" {
                waiting = Some(field.retain());
            }
        }
    }
    assert_eq!(buttons, ["Save…", "Autoscroll", "Jump to latest"]);
    let waiting = waiting.expect("empty state hint");
    assert!(!waiting.isHidden());
    append(state, 0);
    assert!(waiting.isHidden(), "speech hides empty hint");
    clear_view(state, mtm);
    assert!(!waiting.isHidden(), "clear restores empty hint");
    println!("native_panel_render_properties passed");
}

fn native_storage_edit_ranges(mtm: MainThreadMarker) {
    let bundle = fixture(mtm);
    let state = &bundle.state;
    let storage = unsafe { state.text_view.textStorage() }.unwrap();
    let edits = std::rc::Rc::new(std::cell::RefCell::new(Vec::<NSRange>::new()));
    let observed = edits.clone();
    let captured = storage.clone();
    let block = block2::RcBlock::new(
        move |_notification: std::ptr::NonNull<objc2_foundation::NSNotification>| {
            observed.borrow_mut().push(captured.editedRange());
        },
    );
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(&objc2_foundation::NSString::from_str(
                "NSTextStorageDidProcessEditingNotification",
            )),
            Some(&storage),
            None,
            &block,
        )
    };
    let old_length = state.text_view.string().length();
    let prefix = storage.attributedSubstringFromRange(NSRange {
        location: 0,
        length: old_length,
    });
    append(state, 80);
    assert!(
        !edits.borrow().is_empty(),
        "observe actual native storage edits"
    );
    assert!(
        edits
            .borrow()
            .iter()
            .all(|range| range.location >= old_length),
        "append may not rewrite history: {:?}",
        edits.borrow()
    );
    assert!(
        storage
            .attributedSubstringFromRange(NSRange {
                location: 0,
                length: old_length
            })
            .isEqualToAttributedString(&prefix),
        "paragraph append preserves prefix text and attributes"
    );
    edits.borrow_mut().clear();
    let continuation_start = storage.length();
    let (fragment, kind) = {
        let mut log = state.log.lock().unwrap();
        let kind = log.push_with_speaker_and_sample(" continuation hé🙂".to_owned(), Some(0), 81);
        assert_eq!(
            kind,
            crate::overlay::transcript_log::AppendKind::ContinueParagraph
        );
        (log.fragments().last().unwrap().clone(), kind)
    };
    append_fragment_to_view(state, &fragment, kind, &HashMap::new());
    assert!(!edits.borrow().is_empty());
    assert!(
        edits
            .borrow()
            .iter()
            .all(|range| range.location >= continuation_start),
        "continuation edits only native tail"
    );
    assert!(
        storage
            .attributedSubstringFromRange(NSRange {
                location: 0,
                length: old_length
            })
            .isEqualToAttributedString(&prefix),
        "continuation preserves prefix attributes"
    );
    edits.borrow_mut().clear();
    rebuild_view(state, mtm, &HashMap::from([(0, "Renamed".to_owned())]));
    assert!(
        edits
            .borrow()
            .iter()
            .any(|range| range.location == 0 && range.length > old_length),
        "rename performs explicit full rebuild"
    );
    unsafe {
        let _: () = msg_send![&*center, removeObserver: &*token];
    }
    pump_until(mtm, || native_layout_drained(state));
    order_out(state, mtm);
    println!("native_storage_edit_ranges passed");
}

pub fn run() {
    let mtm = MainThreadMarker::new().expect("native transcript runner must run on main thread");
    let _app = NSApplication::sharedApplication(mtm);
    let log = Arc::new(Mutex::new(TranscriptLog::new(Duration::from_millis(1500))));
    let bundle = build_transcript_window(mtm, log.clone());
    let state = &bundle.state;
    let names = HashMap::new();
    let (fragment, kind) = {
        let mut log = log.lock().unwrap();
        let kind = log.push_with_speaker_and_sample(" hé🙂".into(), Some(0), 1);
        (log.fragments().last().unwrap().clone(), kind)
    };
    append_fragment_to_view(state, &fragment, kind, &names);
    assert_eq!(
        log.lock().unwrap().fragments().len(),
        1,
        "adapter must not duplicate log writes"
    );
    assert!(state.text_view.string().to_string().contains("hé🙂"));
    state.text_view.setSelectedRange(NSRange {
        location: 0,
        length: 3,
    });
    state.follow.borrow_mut().set_enabled(false);
    rebuild_view(state, mtm, &names);
    assert_eq!(state.text_view.selectedRange().length, 3);
    order_front(state, mtm);
    let before = state.follow.borrow().generation;
    unsafe {
        let _: () = msg_send![&*state.window, performClose: None::<&objc2::runtime::AnyObject>];
    }
    assert!(!state.window.isVisible());
    assert!(state.follow.borrow().generation > before);
    clear_view(state, mtm);
    assert_eq!(state.text_view.string().length(), 0);
    assert!(
        !state.follow.borrow().enabled,
        "clear preserves autoscroll preference"
    );
    native_panel_render_properties(mtm);
    native_follow_scroll_matrix(mtm);
    native_non_scrolling_interaction_keeps_following(mtm);
    native_stale_scroll_cancelled(mtm);
    native_burst_coalesced(mtm);
    native_storage_edit_ranges(mtm);
    super::transcript_routing::run_native_routing_scenarios(mtm);
    native_reading_anchor_and_selection(mtm);
    native_input_race(mtm);
    println!("native transcript checks passed");
}
