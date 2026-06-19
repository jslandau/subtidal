//! AppKit dialog for assigning display names to diarization speakers.
//!
//! Sortformer produces up to 4 speaker IDs per session (0..=3). This dialog
//! exposes all 4 slots as text fields pre-filled with the current name (or
//! left empty to fall back to "Speaker N"). On Apply, the user-provided names
//! are sent to the overlay via `OverlayCommand::SetSpeakerNames`.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSObject, NSString};

use crate::overlay::OverlayCommand;

/// Maximum speakers the Sortformer-4spk model can report.
const MAX_SPEAKERS: u32 = 4;

thread_local! {
    /// AppKit controls hold `setTarget:` weakly, so keep the current dialog
    /// bundle alive until Apply/Cancel/window close. A single rename dialog at
    /// a time is sufficient for tray-triggered UX; opening again replaces the
    /// prior retained bundle after closing it.
    static CURRENT_DIALOG: RefCell<Option<RenameDialog>> = const { RefCell::new(None) };
}

pub struct RenameDialog {
    window: Retained<NSWindow>,
    /// Retained solely to keep AppKit's weak button/window-delegate target alive.
    _actions: Retained<RenameDialogActions>,
}

pub struct RenameDialogActionsIvars {
    window: RefCell<Option<Retained<NSWindow>>>,
    fields: RefCell<Vec<Retained<NSTextField>>>,
    cmd_tx: RefCell<Option<async_channel::Sender<OverlayCommand>>>,
}

define_class!(
    /// Custom NSObject subclass for Apply/Cancel actions and window delegate.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalRenameDialogActions"]
    #[ivars = RenameDialogActionsIvars]
    pub struct RenameDialogActions;

    impl RenameDialogActions {
        #[unsafe(method(applyRenameSpeakers:))]
        fn apply(&self, _sender: Option<&NSButton>) {
            let mut names = HashMap::new();
            for (id, field) in self.ivars().fields.borrow().iter().enumerate() {
                let value = field.stringValue().to_string();
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    names.insert(id as u32, trimmed.to_string());
                }
            }
            if let Some(tx) = self.ivars().cmd_tx.borrow().as_ref() {
                let _ = tx.send_blocking(OverlayCommand::SetSpeakerNames(names));
            }
            self.close_and_release();
        }

        #[unsafe(method(resetRenameSpeakers:))]
        fn reset(&self, _sender: Option<&NSButton>) {
            for field in self.ivars().fields.borrow().iter() {
                field.setStringValue(&NSString::from_str(""));
            }
        }

        #[unsafe(method(cancelRenameSpeakers:))]
        fn cancel(&self, _sender: Option<&NSButton>) {
            self.close_and_release();
        }

        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, sender: Option<&NSWindow>) -> bool {
            if let Some(w) = sender {
                w.orderOut(None);
            }
            CURRENT_DIALOG.with(|slot| slot.borrow_mut().take());
            false
        }
    }
);

impl RenameDialogActions {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let ivars = RenameDialogActionsIvars {
            window: RefCell::new(None),
            fields: RefCell::new(Vec::new()),
            cmd_tx: RefCell::new(None),
        };
        let allocated = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(allocated), init] }
    }

    fn configure(
        &self,
        window: Retained<NSWindow>,
        fields: Vec<Retained<NSTextField>>,
        cmd_tx: async_channel::Sender<OverlayCommand>,
    ) {
        *self.ivars().window.borrow_mut() = Some(window);
        *self.ivars().fields.borrow_mut() = fields;
        *self.ivars().cmd_tx.borrow_mut() = Some(cmd_tx);
    }

    fn close_and_release(&self) {
        if let Some(window) = self.ivars().window.borrow().as_ref() {
            window.orderOut(None);
        }
        CURRENT_DIALOG.with(|slot| slot.borrow_mut().take());
    }
}

/// Show the speaker rename dialog. The dialog is non-modal and dispatches
/// `SetSpeakerNames` on Apply.
pub fn show_rename_dialog(
    current_names: HashMap<u32, String>,
    cmd_tx: async_channel::Sender<OverlayCommand>,
    mtm: MainThreadMarker,
) {
    unsafe {
        CURRENT_DIALOG.with(|slot| {
            if let Some(existing) = slot.borrow_mut().take() {
                existing.window.orderOut(None);
            }
        });

        let rect = CGRect::new(CGPoint::new(260.0, 260.0), CGSize::new(380.0, 270.0));
        let app = NSApplication::sharedApplication(mtm);
        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
        let window = NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        );
        window.setTitle(&NSString::from_str("Rename Speakers"));
        window.setReleasedWhenClosed(false);

        let container = NSView::initWithFrame(NSView::alloc(mtm), rect);

        let intro = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            CGRect::new(CGPoint::new(20.0, 218.0), CGSize::new(340.0, 34.0)),
        );
        intro.setStringValue(&NSString::from_str(
            "Set display names for diarization speakers. Leave blank to use the default Speaker N label.",
        ));
        intro.setEditable(false);
        intro.setSelectable(false);
        intro.setBordered(false);
        intro.setDrawsBackground(false);
        container.addSubview(&intro);

        let mut fields = Vec::new();
        for id in 0..MAX_SPEAKERS {
            let y = 178.0 - (id as f64 * 38.0);
            let label = NSTextField::initWithFrame(
                NSTextField::alloc(mtm),
                CGRect::new(CGPoint::new(20.0, y + 3.0), CGSize::new(90.0, 22.0)),
            );
            label.setStringValue(&NSString::from_str(&format!("Speaker {}:", id + 1)));
            label.setEditable(false);
            label.setSelectable(false);
            label.setBordered(false);
            label.setDrawsBackground(false);
            container.addSubview(&label);

            let field = NSTextField::initWithFrame(
                NSTextField::alloc(mtm),
                CGRect::new(CGPoint::new(115.0, y), CGSize::new(245.0, 26.0)),
            );
            if let Some(name) = current_names.get(&id) {
                field.setStringValue(&NSString::from_str(name));
            }
            container.addSubview(&field);
            fields.push(field);
        }

        let reset = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            CGRect::new(CGPoint::new(20.0, 18.0), CGSize::new(110.0, 30.0)),
        );
        reset.setTitle(&NSString::from_str("Reset Names"));
        let cancel = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            CGRect::new(CGPoint::new(170.0, 18.0), CGSize::new(90.0, 30.0)),
        );
        cancel.setTitle(&NSString::from_str("Cancel"));
        let apply = NSButton::initWithFrame(
            NSButton::alloc(mtm),
            CGRect::new(CGPoint::new(270.0, 18.0), CGSize::new(90.0, 30.0)),
        );
        apply.setTitle(&NSString::from_str("Apply"));

        let actions = RenameDialogActions::new(mtm);
        let target_obj: &AnyObject = actions.as_ref();
        reset.setTarget(Some(target_obj));
        reset.setAction(Some(sel!(resetRenameSpeakers:)));
        cancel.setTarget(Some(target_obj));
        cancel.setAction(Some(sel!(cancelRenameSpeakers:)));
        apply.setTarget(Some(target_obj));
        apply.setAction(Some(sel!(applyRenameSpeakers:)));
        let _: () = msg_send![&*window, setDelegate: target_obj];

        container.addSubview(&reset);
        container.addSubview(&cancel);
        container.addSubview(&apply);
        window.setContentView(Some(&container));
        actions.configure(window.clone(), fields, cmd_tx);

        window.center();
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
        let _: () = msg_send![&*window, orderFrontRegardless];
        window.makeKeyAndOrderFront(None);

        CURRENT_DIALOG.with(|slot| {
            *slot.borrow_mut() = Some(RenameDialog {
                window,
                _actions: actions,
            });
        });
    }
}
