//! GTK4 modal dialog for assigning display names to diarization speakers.
//!
//! Sortformer produces up to 4 speaker IDs per session (0..=3). This dialog
//! exposes all 4 slots as text entries pre-filled with the current name (or
//! left empty to fall back to "Speaker {N}"). On Apply, the user-provided
//! names are sent to the overlay via `OverlayCommand::SetSpeakerNames`,
//! which updates the config, the live `CaptionBuffer`, and re-renders the
//! transcript window.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{ApplicationWindow, Box as GtkBox, Button, Entry, Label, Orientation, Window};
use std::collections::HashMap;

use crate::overlay::OverlayCommand;

/// Maximum speakers the Sortformer-4spk model can report.
const MAX_SPEAKERS: u32 = 4;

/// Show the speaker rename dialog as a transient toplevel parented to
/// `parent`. The dialog is non-modal in the GTK4 sense (no separate event
/// loop) — it returns immediately and dispatches `SetSpeakerNames` on Apply.
///
/// `current_names` is the snapshot of existing speaker name overrides taken
/// from `Config::speaker_names` at call time. Slots with no override show an
/// empty entry whose placeholder is the fallback label ("Speaker {N}").
pub fn show_rename_dialog(
    parent: &ApplicationWindow,
    current_names: HashMap<u32, String>,
    cmd_tx: async_channel::Sender<OverlayCommand>,
) {
    let win = Window::builder()
        .title("Rename Speakers")
        .transient_for(parent)
        .modal(false)
        .default_width(360)
        .resizable(false)
        .build();

    let outer = GtkBox::new(Orientation::Vertical, 12);
    outer.set_margin_top(16);
    outer.set_margin_bottom(16);
    outer.set_margin_start(16);
    outer.set_margin_end(16);

    let intro = Label::new(Some(
        "Set display names for diarization speakers. \
         Leave blank to use the default \"Speaker N\" label.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    outer.append(&intro);

    // One row per speaker slot. Collect entries so we can read them on Apply.
    let entries: Vec<Entry> = (0..MAX_SPEAKERS)
        .map(|id| {
            let row = GtkBox::new(Orientation::Horizontal, 8);
            let label = Label::new(Some(&format!("Speaker {}:", id + 1)));
            label.set_xalign(0.0);
            label.set_width_chars(11);

            let entry = Entry::new();
            entry.set_hexpand(true);
            entry.set_placeholder_text(Some(&format!("Speaker {}", id + 1)));
            if let Some(name) = current_names.get(&id) {
                entry.set_text(name);
            }

            row.append(&label);
            row.append(&entry);
            outer.append(&row);
            entry
        })
        .collect();

    // Action row: [Reset Names] [spacer] [Cancel] [Apply].
    let actions = GtkBox::new(Orientation::Horizontal, 8);
    actions.set_margin_top(8);
    let reset = Button::with_label("Reset Names");
    let spacer = GtkBox::new(Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let cancel = Button::with_label("Cancel");
    let apply = Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    actions.append(&reset);
    actions.append(&spacer);
    actions.append(&cancel);
    actions.append(&apply);
    outer.append(&actions);

    win.set_child(Some(&outer));

    // Wire up actions. Both close the window; Apply also dispatches the
    // SetSpeakerNames command. Entries are captured by clone-into-closure.
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let entries = entries.clone();
        reset.connect_clicked(move |_| {
            for entry in &entries {
                entry.set_text("");
            }
        });
    }
    {
        let win = win.clone();
        let entries = entries.clone();
        apply.connect_clicked(move |_| {
            let mut names: HashMap<u32, String> = HashMap::new();
            for (id, entry) in entries.iter().enumerate() {
                let text = entry.text().to_string();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    names.insert(id as u32, trimmed.to_string());
                }
            }
            // send_blocking on an unbounded async-channel returns immediately;
            // this runs on the GTK main thread so we don't want to await.
            let _ = cmd_tx.send_blocking(OverlayCommand::SetSpeakerNames(names));
            win.close();
        });
    }

    // Enter key on any entry = Apply; Escape = Cancel.
    for entry in &entries {
        let apply = apply.clone();
        entry.connect_activate(move |_| apply.emit_clicked());
    }
    {
        let cancel = cancel.clone();
        let key = gtk4::EventControllerKey::new();
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk4::gdk::Key::Escape {
                cancel.emit_clicked();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        win.add_controller(key);
    }

    win.present();
    // Focus the first entry for immediate typing.
    if let Some(first) = entries.first() {
        first.grab_focus();
    }
}
