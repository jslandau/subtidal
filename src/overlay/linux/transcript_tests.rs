//! Display-server tests that exercise the production Linux routing functions.
use super::*;
use crate::overlay::{transcript_log::TranscriptLog, CaptionEvent};
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Mutex},
    time::Duration,
};

#[test]
#[ignore = "requires a GTK display server; run explicitly with --ignored --test-threads=1"]
fn linux_transcript_native_routing() {
    gtk4::init().expect("GTK display required");
    let app = Application::builder()
        .application_id("com.subtidal.transcript-test")
        .build();
    app.register(None::<&gtk4::gio::Cancellable>).unwrap();
    let log = Rc::new(RefCell::new(TranscriptLog::new(Duration::from_secs(7))));
    let state = transcript_window::build_transcript_window(
        &app,
        log.clone(),
        "test".into(),
        chrono::Local::now(),
    );
    native_panel_controls(&state);
    assert_eq!(
        state.buffer.char_count(),
        0,
        "placeholder is not selectable transcript content"
    );
    assert!(
        log.borrow().fragments().is_empty(),
        "placeholder never enters export log"
    );
    let caption_label = gtk4::Label::new(Some("old caption"));
    caption_label.set_widget_name("caption-label");
    let overlay = ApplicationWindow::builder()
        .application(&app)
        .child(&caption_label)
        .build();
    let config = Arc::new(Mutex::new(Config::default()));
    let dragging = Rc::new(Cell::new(false));
    let buffer = Rc::new(RefCell::new(CaptionBuffer::new(3, 80, 8)));
    let enabled = Arc::new(AtomicBool::new(true));
    let mode = Rc::new(RefCell::new(OverlayMode::Transcript));
    let (tx, _rx) = async_channel::unbounded();
    let command = |cmd| {
        handle_overlay_command(
            &overlay, cmd, &config, &dragging, &buffer, &enabled, &mode, &state, &log, &tx,
        )
    };
    let names = HashMap::new();
    route_transcript_event(
        &state,
        &log,
        &CaptionEvent::Append {
            text: " hé🙂".into(),
            speaker_id: Some(0),
            emit_sample: 10,
        },
        &names,
    );
    assert_eq!(log.borrow().fragments().len(), 1);
    let text = || {
        state
            .buffer
            .text(&state.buffer.start_iter(), &state.buffer.end_iter(), false)
            .to_string()
    };
    assert!(text().contains("Speaker 1:  hé🙂"));
    route_transcript_event(
        &state,
        &log,
        &CaptionEvent::Relabel {
            from_sample: 0,
            new_speaker_id: 1,
        },
        &names,
    );
    assert!(text().contains("Speaker 2:  hé🙂"));
    command(OverlayCommand::SetSpeakerNames(HashMap::from([(
        1,
        "長い名前".into(),
    )])));
    assert!(text().contains("長い名前:  hé🙂"));
    command(OverlayCommand::SetMode(OverlayMode::Transcript));
    assert!(state.window.is_visible());
    command(OverlayCommand::SetVisible(false));
    assert!(!state.window.is_visible());
    native_follow_scroll_matrix(&state, &log, &names);
    native_selected_body_correction(&state, &command);
    buffer.borrow_mut().push(" stale overlay".into());
    let controllers = state.scrolled.observe_controllers();
    let wheel = (0..controllers.n_items())
        .filter_map(|i| {
            controllers
                .item(i)?
                .downcast::<gtk4::EventControllerScroll>()
                .ok()
        })
        .next()
        .unwrap();
    wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
    assert_eq!(state.test_cleanup_counts().1, 1);
    command(OverlayCommand::SetCaptionsEnabled(false));
    assert!(text().is_empty());
    assert!(log.borrow().fragments().is_empty());
    assert!(!enabled.load(Ordering::Relaxed));
    assert!(
        buffer.borrow().display_text().is_empty(),
        "disable clears caption buffer"
    );
    assert!(
        caption_label.text().is_empty(),
        "disable clears overlay label"
    );
    assert!(!state.test_pending(), "disable cancels native callback");
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "disable clears physical cleanup source"
    );
    state.window.close();
    overlay.close();
}

fn native_panel_controls(state: &transcript_window::TranscriptWindowState) {
    fn walk(widget: &gtk4::Widget, labels: &mut Vec<String>) {
        if let Ok(button) = widget.clone().downcast::<gtk4::Button>() {
            if let Some(label) = button.label() {
                labels.push(label.to_string());
                assert!(button.tooltip_text().is_some());
            }
        }
        if let Ok(button) = widget.clone().downcast::<gtk4::CheckButton>() {
            if let Some(label) = button.label() {
                labels.push(label.to_string());
                assert!(button.tooltip_text().is_some());
            }
        }
        let mut child = widget.first_child();
        while let Some(item) = child {
            walk(&item, labels);
            child = item.next_sibling();
        }
    }
    let mut labels = Vec::new();
    walk(state.window.upcast_ref(), &mut labels);
    for required in ["Autoscroll", "Jump to latest", "Save…"] {
        assert!(
            labels.iter().any(|label| label == required),
            "native labeled control {required}"
        );
    }
    assert_eq!(state.scrolled.hscrollbar_policy(), gtk4::PolicyType::Never);
}

fn settle(state: &transcript_window::TranscriptWindowState) {
    let context = glib::MainContext::default();
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    let previous = Cell::new(None);
    let frames = Cell::new(0);
    let adj = state.scrolled.vadjustment();
    let settling_state = state.clone();
    state.text_view.add_tick_callback(move |_, _| {
        let geometry = (adj.upper(), adj.page_size(), adj.value());
        frames.set(frames.get() + 1);
        if frames.get() >= 3
            && previous.get() == Some(geometry)
            && !settling_state.test_input_active()
            && !settling_state.test_pending()
        {
            flag.set(true);
            return glib::ControlFlow::Break;
        }
        assert!(frames.get() < 60, "native layout failed to settle");
        previous.set(Some(geometry));
        glib::ControlFlow::Continue
    });
    let expired = Rc::new(Cell::new(false));
    let timeout_flag = expired.clone();
    let timeout =
        glib::timeout_add_local_once(Duration::from_secs(3), move || timeout_flag.set(true));
    while !done.get() && !expired.get() {
        context.iteration(true);
    }
    if !expired.get() {
        timeout.remove();
    }
    assert!(
        done.get(),
        "native layout did not produce settled frames within 3 seconds"
    );
}

fn native_follow_scroll_matrix(
    state: &transcript_window::TranscriptWindowState,
    log: &Rc<RefCell<TranscriptLog>>,
    names: &HashMap<u32, String>,
) {
    state.window.present();
    settle(state);
    for i in 0..150 {
        route_transcript_event(
            state,
            log,
            &CaptionEvent::Append {
                text: format!(
                    " wrapped fragment {i} hé🙂 {}",
                    "long caption words ".repeat(12)
                ),
                speaker_id: Some(1),
                emit_sample: 20 + i,
            },
            names,
        );
    }
    assert!(
        state.test_pending(),
        "burst has a coalesced native callback"
    );
    settle(state);
    let adj = state.scrolled.vadjustment();
    assert!(
        adj.upper() > adj.page_size(),
        "real native document overflows"
    );
    assert!(
        (adj.value() - (adj.upper() - adj.page_size())).abs() <= 2.0,
        "following reaches settled bottom"
    );
    assert!(!state.test_pending());
    for (width, height) in [(380, 350), (950, 650), (520, 450)] {
        state.window.set_default_size(width, height);
        settle(state);
        assert!(
            (adj.value() - (adj.upper() - adj.page_size())).abs() <= 2.0,
            "resize keeps following settled wrapped text"
        );
    }
    native_storage_edit_ranges(state, log, names);
    settle(state);
    native_pointer_input_race(state, log, names);
    state.test_user_scroll(0.0);
    let before = adj.value();
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " paused append".into(),
            speaker_id: Some(1),
            emit_sample: 200,
        },
        names,
    );
    settle(state);
    assert!(
        (adj.value() - before).abs() <= 2.0,
        "reading history stays anchored"
    );
    assert!(!state.test_pending());
    state.test_user_scroll(123.0);
    let anchor = state.test_anchor().expect("reading fragment anchor");
    for (width, height) in [(410, 360), (850, 600)] {
        state.window.set_default_size(width, height);
        settle(state);
        assert!(
            (state.test_anchor_pixel(anchor.0) - anchor.1).abs() <= 2,
            "resize preserves reading fragment and intra-line pixels"
        );
    }
    state.test_follow_enabled(false);
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " manual append".into(),
            speaker_id: Some(1),
            emit_sample: 201,
        },
        names,
    );
    assert!(!state.test_pending(), "disabled control does not schedule");
    state.test_follow_enabled(true);
    assert!(state.test_pending());
    state.window.close();
    assert!(!state.test_pending(), "close physically cancels callback");
}

fn native_pointer_input_race(
    state: &transcript_window::TranscriptWindowState,
    log: &Rc<RefCell<TranscriptLog>>,
    names: &HashMap<u32, String>,
) {
    let controllers = state.scrolled.observe_controllers();
    let click = (0..controllers.n_items())
        .filter_map(|i| controllers.item(i)?.downcast::<gtk4::GestureClick>().ok())
        .next()
        .expect("production pointer controller");
    click.emit_by_name::<()>("pressed", &[&1i32, &0f64, &0f64]);
    state.scrolled.vadjustment().set_value(30.0);
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " during pointer navigation".into(),
            speaker_id: Some(2),
            emit_sample: 191,
        },
        names,
    );
    click.emit_by_name::<()>("released", &[&1i32, &0f64, &0f64]);
    settle(state);
    assert!(
        (state.scrolled.vadjustment().value() - 30.0).abs() <= 2.0,
        "held pointer movement pauses stale follow"
    );
    state.test_follow_enabled(false);
    state.test_follow_enabled(true);
    settle(state);
    click.emit_by_name::<()>("pressed", &[&1i32, &0f64, &0f64]);
    click.emit_by_name::<()>("released", &[&1i32, &0f64, &0f64]);
    settle(state);
    let adj = state.scrolled.vadjustment();
    assert!(
        (adj.value() - (adj.upper() - adj.page_size())).abs() <= 2.0,
        "non-scrolling click re-arms following"
    );
    let wheel = (0..controllers.n_items())
        .filter_map(|i| {
            controllers
                .item(i)?
                .downcast::<gtk4::EventControllerScroll>()
                .ok()
        })
        .next()
        .unwrap();
    let registrations = state.test_cleanup_counts().0;
    for _ in 0..32 {
        wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
        assert_eq!(
            state.test_cleanup_counts().1,
            1,
            "rapid input retains exactly one physical cleanup source before dispatch"
        );
    }
    assert_eq!(state.test_cleanup_counts().0, registrations + 32);
    state.test_follow_enabled(false);
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "off removes source without waiting for dispatch"
    );
    settle(state);
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "removed sources cannot reappear"
    );
    state.test_follow_enabled(true);
    settle(state);
    fn jump_button(widget: &gtk4::Widget) -> Option<gtk4::Button> {
        if let Ok(button) = widget.clone().downcast::<gtk4::Button>() {
            if button.label().as_deref() == Some("Jump to latest") {
                return Some(button);
            }
        }
        let mut child = widget.first_child();
        while let Some(item) = child {
            if let Some(button) = jump_button(&item) {
                return Some(button);
            }
            child = item.next_sibling();
        }
        None
    }
    wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
    jump_button(state.window.upcast_ref())
        .unwrap()
        .emit_clicked();
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "Jump physically removes cleanup source"
    );
    settle(state);
    wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
    state.window.set_visible(false);
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "unmap physically removes cleanup source"
    );
    state.window.present();
    settle(state);
    wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
    adj.set_value(60.0);
    let reader = state.test_anchor().unwrap();
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " growth before input cleanup ".repeat(300),
            speaker_id: Some(2),
            emit_sample: 192,
        },
        names,
    );
    assert!(state.test_input_active());
    assert!(
        !state.test_pending(),
        "append cannot re-arm follow during wheel input"
    );
    settle(state);
    assert!(
        !state.test_input_active(),
        "layout invalidation does not strand input session"
    );
    assert!((state.test_anchor_pixel(reader.0) - reader.1).abs() <= 2);
    assert!(
        (adj.value() - 60.0).abs() <= 2.0,
        "wheel controller movement pauses"
    );
    state.test_follow_enabled(false);
    state.test_follow_enabled(true);
    settle(state);
    let key = (0..controllers.n_items())
        .filter_map(|i| {
            controllers
                .item(i)?
                .downcast::<gtk4::EventControllerKey>()
                .ok()
        })
        .next()
        .unwrap();
    key.emit_by_name::<bool>(
        "key-pressed",
        &[
            &gtk4::gdk::Key::Page_Up,
            &0u32,
            &gtk4::gdk::ModifierType::empty(),
        ],
    );
    adj.set_value(90.0);
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " key growth before context drain ".repeat(300),
            speaker_id: Some(2),
            emit_sample: 193,
        },
        names,
    );
    assert!(
        !state.test_pending(),
        "navigation input suspends append follow"
    );
    settle(state);
    assert!(!state.test_input_active());
    assert!(
        (adj.value() - 90.0).abs() <= 2.0,
        "navigation key controller movement pauses"
    );
    wheel.emit_by_name::<bool>("scroll", &[&0f64, &-1f64]);
    state.test_follow_enabled(false);
    assert!(
        !state.test_input_active(),
        "off cleans pending input immediately"
    );
    settle(state);
    assert!(
        !state.test_pending(),
        "old input cleanup cannot override off"
    );
    state.test_follow_enabled(true);
    settle(state);
    wheel.emit_by_name::<()>("scroll-begin", &[]);
    adj.set_value(120.0);
    wheel.emit_by_name::<()>("scroll-end", &[]);
    wheel.emit_by_name::<()>("decelerate", &[&0f64, &1f64]);
    adj.set_value(adj.upper() - adj.page_size());
    settle(state);
    assert!(!state.test_input_active(), "momentum cleanup completes");
    assert_eq!(
        state.test_cleanup_counts().1,
        0,
        "natural completion releases physical source"
    );
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " resumed after momentum".into(),
            speaker_id: Some(2),
            emit_sample: 194,
        },
        names,
    );
    assert!(state.test_pending(), "return to bottom resumes following");
    settle(state);
}

fn native_storage_edit_ranges(
    state: &transcript_window::TranscriptWindowState,
    log: &Rc<RefCell<TranscriptLog>>,
    names: &HashMap<u32, String>,
) {
    let old_end = state.buffer.char_count();
    let prefix = state
        .buffer
        .text(&state.buffer.start_iter(), &state.buffer.end_iter(), false)
        .to_string();
    let inserts = Rc::new(RefCell::new(Vec::new()));
    let deletes = Rc::new(Cell::new(0));
    let attributes = Rc::new(RefCell::new(Vec::new()));
    let records = inserts.clone();
    let insert_id = state.buffer.connect_insert_text(move |_, iter, text| {
        records
            .borrow_mut()
            .push((iter.offset(), text.chars().count()))
    });
    let records = deletes.clone();
    let delete_id = state
        .buffer
        .connect_delete_range(move |_, _, _| records.set(records.get() + 1));
    let records = attributes.clone();
    let tag_id = state
        .buffer
        .connect_apply_tag(move |_, _, a, b| records.borrow_mut().push((a.offset(), b.offset())));
    let count = log.borrow().fragments().len();
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " Unicode suffix hé🙂".into(),
            speaker_id: Some(2),
            emit_sample: 190,
        },
        names,
    );
    assert_eq!(
        log.borrow().fragments().len(),
        count + 1,
        "exactly one durable push"
    );
    assert_eq!(deletes.get(), 0, "append never replaces existing storage");
    assert_eq!(inserts.borrow().len(), 1, "one old-end insertion");
    assert_eq!(inserts.borrow()[0].0, old_end, "insert begins at old end");
    assert!(
        attributes
            .borrow()
            .iter()
            .all(|(a, b)| *a >= old_end && *b >= *a),
        "attributes touch only inserted suffix"
    );
    assert_eq!(
        state
            .buffer
            .text(
                &state.buffer.start_iter(),
                &state.buffer.iter_at_offset(old_end),
                false
            )
            .as_str(),
        prefix,
        "new paragraph preserves prefix text"
    );
    inserts.borrow_mut().clear();
    attributes.borrow_mut().clear();
    let continuation_end = state.buffer.char_count();
    let continuation_prefix = state
        .buffer
        .text(&state.buffer.start_iter(), &state.buffer.end_iter(), false)
        .to_string();
    route_transcript_event(
        state,
        log,
        &CaptionEvent::Append {
            text: " continuation hé🙂".into(),
            speaker_id: Some(2),
            emit_sample: 191,
        },
        names,
    );
    assert_eq!(log.borrow().fragments().len(), count + 2);
    assert_eq!(deletes.get(), 0);
    assert_eq!(inserts.borrow().len(), 1);
    assert_eq!(inserts.borrow()[0].0, continuation_end);
    assert!(
        attributes.borrow().is_empty(),
        "continuation never restyles old prefix"
    );
    assert_eq!(
        state
            .buffer
            .text(
                &state.buffer.start_iter(),
                &state.buffer.iter_at_offset(continuation_end),
                false
            )
            .as_str(),
        continuation_prefix,
        "continuation preserves prefix text"
    );
    state.buffer.disconnect(insert_id);
    state.buffer.disconnect(delete_id);
    state.buffer.disconnect(tag_id);
}

fn native_selected_body_correction(
    state: &transcript_window::TranscriptWindowState,
    command: &impl Fn(OverlayCommand),
) {
    let text = state
        .buffer
        .text(&state.buffer.start_iter(), &state.buffer.end_iter(), false)
        .to_string();
    let start = text.find("hé🙂").unwrap();
    let a = crate::overlay::transcript_presentation::gtk_offset(&text, start) as i32;
    state.buffer.select_range(
        &state.buffer.iter_at_offset(a),
        &state.buffer.iter_at_offset(a + 3),
    );
    command(OverlayCommand::SetSpeakerNames(HashMap::from([(
        1,
        "別のとても長い名前".into(),
    )])));
    let (a, b) = state.buffer.selection_bounds().expect("selection retained");
    assert_eq!(
        state.buffer.text(&a, &b, false).as_str(),
        "hé🙂",
        "selected Unicode body follows fragment position"
    );
}
