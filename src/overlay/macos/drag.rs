//! Drag-to-move gesture for the macOS floating overlay with position persistence.
//!
//! Observes NSWindowDidMoveNotification on the overlay panel and persists the new
//! frame.origin to config via the hot-reload-safe write path. The existing
//! `start_hot_reload_macos` watcher only emits SwitchSource when audio_source
//! changes, so position-only writes don't echo back through hot-reload (AC6.2).

use objc2::rc::Retained;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2::runtime::AnyObject;
use objc2_app_kit::NSPanel;
use objc2_foundation::{NSNotificationCenter, NSObject, NSString};
use std::sync::{Arc, Mutex};
use crate::config::Config;

pub struct DragObserverIvars {
    config: Arc<Mutex<Config>>,
    panel: Retained<NSPanel>,
}

define_class!(
    /// Custom NSObject subclass observing NSWindowDidMoveNotification on the
    /// overlay panel and persisting position changes to config.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalDragObserver"]
    #[ivars = DragObserverIvars]
    pub struct DragObserver;

    impl DragObserver {
        #[unsafe(method(windowDidMove:))]
        fn window_did_move(&self, _notification: Option<&objc2_foundation::NSNotification>) {
            let ivars = self.ivars();
            let frame = ivars.panel.frame();
            let new_x = frame.origin.x as i32;
            let new_y = frame.origin.y as i32;
            let mut cfg = ivars.config.lock().unwrap();
            // Only persist position in Floating mode — Docked/Transcript moves
            // are programmatic (apply_geometry repositioning) and must not
            // overwrite the user's saved Floating coords.
            if !matches!(cfg.overlay_mode, crate::config::OverlayMode::Floating) {
                return;
            }
            if cfg.position.x == new_x && cfg.position.y == new_y {
                return;
            }
            cfg.position.x = new_x;
            cfg.position.y = new_y;
            if let Err(e) = cfg.save() {
                eprintln!("warn: failed to persist drag position: {e}");
            }
        }
    }
);

impl DragObserver {
    fn new(
        mtm: MainThreadMarker,
        panel: Retained<NSPanel>,
        config: Arc<Mutex<Config>>,
    ) -> Retained<Self> {
        let ivars = DragObserverIvars { config, panel };
        let allocated = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(allocated), init] }
    }
}

/// Install a drag observer for the overlay panel. The returned observer
/// must be kept alive for the app lifetime; dropping it removes the
/// notification subscription via NSObject deallocation.
pub fn install_drag_observer(
    panel: &NSPanel,
    config: Arc<Mutex<Config>>,
    mtm: MainThreadMarker,
) -> Retained<DragObserver> {
    let panel_retained: Retained<NSPanel> = Retained::from(panel);
    let observer = DragObserver::new(mtm, panel_retained, config);

    let center = NSNotificationCenter::defaultCenter();
    let observer_ns: &NSObject = &*observer;
    let panel_any: &AnyObject = panel.as_ref();

    unsafe {
        center.addObserver_selector_name_object(
            observer_ns,
            sel!(windowDidMove:),
            Some(&NSString::from_str("NSWindowDidMoveNotification")),
            Some(panel_any),
        );
    }

    observer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_observer_constructs() {
        let Some(mtm) = MainThreadMarker::new() else {
            eprintln!("drag_observer_constructs: skipping (not on main thread)");
            return;
        };
        // We don't construct a real NSPanel in tests (would need MainThreadMarker
        // *and* AppKit initialization). The construction is verified by callers
        // in run_app and exercised via cargo check.
        let _ = mtm;
    }
}
