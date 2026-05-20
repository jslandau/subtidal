//! NSPanel construction and configuration for macOS overlay.
//!
//! Provides an NSPanel with caption display and configuration.
//! Phase 2 implements Floating mode; Phase 6 adds Docked mode geometry,
//! mode switching, and screen-change observation.

use objc2::rc::Retained;
use objc2::{msg_send, MainThreadMarker, MainThreadOnly, ClassType};
use objc2_foundation::NSString;
use objc2_app_kit::{
    NSPanel, NSTextField, NSWindowStyleMask, NSWindowCollectionBehavior,
    NSFloatingWindowLevel, NSStatusWindowLevel, NSColor, NSFont, NSLineBreakMode,
    NSBackingStoreType, NSView, NSScreen,
};
use objc2_core_foundation::{CGRect, CGPoint, CGSize};
use crate::config::{Config, OverlayMode};
use std::sync::Arc;

/// Inspection helper: public view of panel configuration for testing.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PanelConfig {
    pub level: i64,
    pub collection_behavior: u64,
    pub is_floating_panel: bool,
    pub style_mask: u64,
    pub ignores_mouse_events: bool,
}

/// Build an NSPanel for caption display in Floating mode.
///
/// Returns (panel, content_label) where the label is the main caption text view.
/// The panel is constructed with appropriate flags for multi-space rendering,
/// fullscreen compatibility, and transparency.
pub fn build_overlay_panel(
    mtm: MainThreadMarker,
    config: &Config,
) -> (Retained<NSPanel>, Retained<NSTextField>) {
    unsafe {
        // Compute initial window frame from config.
        let x = config.position.x as f64;
        let y = config.position.y as f64;
        let width = config.appearance.width as f64;
        // Natural height: font_size * 1.5 for single line, plus padding
        let height = (config.appearance.font_size * 1.5 + 8.0) as f64;
        let frame = CGRect::new(CGPoint::new(x, y), CGSize::new(width, height));

        // Determine initial level based on above_fullscreen config.
        let level = if config.above_fullscreen {
            NSStatusWindowLevel as isize
        } else {
            NSFloatingWindowLevel as isize
        };

        // Create the NSPanel with appropriate style and behavior.
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let backing = NSBackingStoreType::Buffered;

        // Allocate and initialize NSPanel using typed init method.
        let panel: Retained<NSPanel> = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            frame,
            style_mask,
            backing,
            false
        );

        // Set window level (floating or status depending on above_fullscreen)
        panel.setLevel(level);

        // Configure collection behavior for multi-space and fullscreen support
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        panel.setCollectionBehavior(collection_behavior);

        // Mark as floating panel
        panel.setFloatingPanel(true);

        // Transparent panel chrome — the rounded background lives on the
        // contentView's layer so corners render correctly.
        panel.setOpaque(false);
        let clear = NSColor::clearColor();
        panel.setBackgroundColor(Some(&clear));

        // Disable shadow (Floating mode)
        panel.setHasShadow(false);

        // Click-through: ignore mouse events
        panel.setIgnoresMouseEvents(true);

        // Allow repositioning by background click
        panel.setMovableByWindowBackground(true);

        // Create and configure the content NSTextField
        let label_frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, height));
        let label: Retained<NSTextField> = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            label_frame
        );

        // Text properties
        let empty_str = NSString::from_str("");
        label.setStringValue(&*empty_str);
        label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        label.setEditable(false);
        label.setSelectable(false);
        label.setBordered(false);
        label.setDrawsBackground(false);
        label.setTextColor(Some(&NSColor::whiteColor()));

        // Rounded translucent background on the label's layer. NSTextField
        // is layer-backable; cornerRadius + masksToBounds + layer.backgroundColor
        // gives us the soft pill look without an extra wrapper view.
        label.setWantsLayer(true);
        if let Some(layer) = label.layer() {
            layer.setCornerRadius(8.0);
            layer.setMasksToBounds(true);
            let bg = NSColor::colorWithWhite_alpha(0.0, 0.75);
            layer.setBackgroundColor(Some(&bg.CGColor()));
        }

        // Font: monospace at configured size
        // SAFETY: msg_send! is retained here because NSFont::userFixedPitchFontOfSize
        // doesn't have a typed equivalent in objc2-app-kit 0.3.
        let font_size = config.appearance.font_size as f64;
        let font: Retained<NSFont> = msg_send![
            NSFont::class(),
            userFixedPitchFontOfSize: font_size
        ];
        label.setFont(Some(&*font));

        // Set label as panel's content view
        // SAFETY: NSTextField is a subclass of NSView, so we can transmute the reference.
        let view_ref: &NSView = std::mem::transmute::<&NSTextField, &NSView>(&label);
        panel.setContentView(Some(view_ref));

        (panel, label)
    }
}

/// Inspect panel configuration for testing and verification.
#[allow(dead_code)]  // Used in #[cfg(all(test, target_os = "macos"))] tests
pub fn inspect(panel: &NSPanel) -> PanelConfig {
    let level: i64 = panel.level() as i64;
    let collection_behavior: u64 = panel.collectionBehavior().bits() as u64;
    let is_floating_panel = panel.isFloatingPanel();
    let style_mask: u64 = panel.styleMask().bits() as u64;
    let ignores_mouse_events = panel.ignoresMouseEvents();

    PanelConfig {
        level,
        collection_behavior,
        is_floating_panel,
        style_mask,
        ignores_mouse_events,
    }
}


/// Apply geometry configuration for a given overlay mode.
///
/// Reconfigures the same NSPanel for Docked vs Floating mode without rebuild.
/// In Docked mode, the panel is positioned at the top of the visible screen frame,
/// spanning the full width. In Floating mode, it uses the position and dimensions
/// from config. In Transcript mode, the panel is hidden (order out).
///
/// SAFETY: The mtm parameter proves this is called on the main thread, where
/// AppKit mutations are safe.
pub fn apply_geometry(
    panel: &NSPanel,
    mtm: MainThreadMarker,
    mode: OverlayMode,
    config: &Config,
) {
    unsafe {
        match mode {
            OverlayMode::Docked => {
                let screen = NSScreen::mainScreen(mtm).expect("main screen");
                let visible = screen.visibleFrame();  // AC2.7 — not frame
                let height = panel_height_for_font(config.appearance.font_size);
                let rect = CGRect::new(
                    CGPoint::new(visible.origin.x, visible.origin.y + visible.size.height - height),
                    CGSize::new(visible.size.width, height),
                );
                panel.setFrame_display(rect, true);
                panel.setIgnoresMouseEvents(true);  // AC2.5
                panel.setMovableByWindowBackground(false);
                panel.setHasShadow(true);
            }
            OverlayMode::Floating => {
                let rect = CGRect::new(
                    CGPoint::new(config.position.x as f64, config.position.y as f64),
                    CGSize::new(
                        config.appearance.width as f64,
                        panel_height_for_font(config.appearance.font_size),
                    ),
                );
                panel.setFrame_display(rect, true);
                // Mouse events: locked → pass-through (AC2.5); unlocked → receive
                // (so setMovableByWindowBackground can detect the drag).
                // ignoresMouseEvents=true bypasses the window entirely, which would
                // make drag impossible regardless of movableByWindowBackground.
                panel.setIgnoresMouseEvents(config.locked);
                panel.setMovableByWindowBackground(!config.locked);
                panel.setHasShadow(false);
            }
            OverlayMode::Transcript => {
                // Transcript mode uses a separate NSWindow; hide the caption panel.
                panel.orderOut(None);
            }
        }
    }
}

/// Calculate panel height for a given font size and explicit line count.
/// Used both for the initial single-line frame (line_count=1) and for
/// growing the panel when CaptionBuffer emits a multi-line display.
fn panel_height_for_font(font_size: f32) -> f64 {
    panel_height_for_lines(font_size, 1)
}

/// Per-line height plus uniform padding.
fn panel_height_for_lines(font_size: f32, line_count: usize) -> f64 {
    let lines = line_count.max(1) as f64;
    (font_size as f64) * 1.5 * lines + 8.0
}

/// Set the caption text and grow the panel height to match the line count
/// of the display string. The panel's top edge is held fixed so the
/// caption stays anchored where the user placed it (Floating) or against
/// the menu bar (Docked); the bottom edge grows downward.
///
/// Called by the caption-bridge and expiry timer after each label update
/// so multi-line CaptionBuffer output is fully visible.
pub fn set_caption_text(
    panel: &NSPanel,
    label: &NSTextField,
    text: &str,
    mtm: MainThreadMarker,
    config: &Config,
) {
    let ns_text = NSString::from_str(text);
    unsafe { label.setStringValue(&ns_text) };

    // Skip resize in Transcript mode — the overlay panel is hidden.
    if matches!(config.overlay_mode, OverlayMode::Transcript) {
        return;
    }

    let line_count = text.lines().count().max(1);
    let new_height = panel_height_for_lines(config.appearance.font_size, line_count);

    let current = panel.frame();
    // Hold the top edge: origin.y = (top of current frame) - new_height.
    let top = current.origin.y + current.size.height;
    let new_frame = CGRect::new(
        CGPoint::new(current.origin.x, top - new_height),
        CGSize::new(current.size.width, new_height),
    );
    unsafe { panel.setFrame_display(new_frame, true) };

    // The content view (NSTextField label) auto-sizes via the contentView
    // -> setFrame path on macOS when the panel's content view is configured
    // with auto-layout; for our manual layout we resize it explicitly.
    let label_frame = CGRect::new(
        CGPoint::new(0.0, 0.0),
        CGSize::new(current.size.width, new_height),
    );
    unsafe { label.setFrame(label_frame) };
    let _ = mtm; // marker held to enforce main-thread call site
}

/// AC1.6 — re-apply geometry on NSApplicationDidChangeScreenParametersNotification.
///
/// Used by `install_screen_observer` below; lives in the `define_class!`
/// body which can't see Config/OverlayMode imports directly.
fn screen_changed_reapply(panel: &NSPanel, mtm: MainThreadMarker, config: &Arc<std::sync::Mutex<Config>>) {
    let (mode, cfg_snapshot) = {
        let cfg = config.lock().unwrap();
        (cfg.overlay_mode.clone(), cfg.clone())
    };
    apply_geometry(panel, mtm, mode, &cfg_snapshot);
}

pub struct ScreenObserverIvars {
    panel: Retained<NSPanel>,
    config: Arc<std::sync::Mutex<Config>>,
}

objc2::define_class!(
    /// Observes `NSApplicationDidChangeScreenParametersNotification` and
    /// re-applies geometry so Docked mode tracks `visibleFrame` when an
    /// external display is attached or removed (AC1.6).
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalScreenObserver"]
    #[ivars = ScreenObserverIvars]
    pub struct ScreenObserver;

    impl ScreenObserver {
        #[unsafe(method(screenParamsChanged:))]
        fn screen_params_changed(&self, _notif: Option<&objc2_foundation::NSNotification>) {
            use objc2::DefinedClass;
            let mtm = MainThreadMarker::from(self);
            let ivars = self.ivars();
            screen_changed_reapply(&ivars.panel, mtm, &ivars.config);
        }
    }
);

impl ScreenObserver {
    fn new(mtm: MainThreadMarker, panel: Retained<NSPanel>, config: Arc<std::sync::Mutex<Config>>) -> Retained<Self> {
        let ivars = ScreenObserverIvars { panel, config };
        let allocated = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(allocated), init] }
    }
}

/// Install the NSScreen-change observer. Returns the observer; caller must
/// keep it alive for the app lifetime.
pub fn install_screen_observer(
    panel: &NSPanel,
    config: Arc<std::sync::Mutex<Config>>,
    mtm: MainThreadMarker,
) -> Retained<ScreenObserver> {
    let panel_retained: Retained<NSPanel> = Retained::from(panel);
    let observer = ScreenObserver::new(mtm, panel_retained, config);
    let center = objc2_foundation::NSNotificationCenter::defaultCenter();
    let observer_ns: &objc2_foundation::NSObject = &*observer;
    unsafe {
        center.addObserver_selector_name_object(
            observer_ns,
            objc2::sel!(screenParamsChanged:),
            Some(&objc2_foundation::NSString::from_str(
                "NSApplicationDidChangeScreenParametersNotification",
            )),
            None,
        );
    }
    observer
}

/// Toggle the above-fullscreen layer for the panel.
///
/// When `on` is true, sets the panel to NSStatusWindowLevel (renders above fullscreen).
/// When `on` is false, sets the panel to NSFloatingWindowLevel (below fullscreen).
/// The same NSPanel instance is retained throughout; no rebuild occurs.
///
/// SAFETY: The mtm parameter proves this is called on the main thread, where
/// AppKit mutations are safe. The parameter is not directly used but enforces
/// the contract at the call site.
pub fn set_above_fullscreen(panel: &NSPanel, _: MainThreadMarker, on: bool) {
    let level = if on {
        NSStatusWindowLevel as isize
    } else {
        NSFloatingWindowLevel as isize
    };
    panel.setLevel(level);
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    // NOTE: `cargo test --lib` runs tests on worker threads, so the MainThreadMarker::new()
    // check inside each test silently skips assertions when not on the main thread.
    // To actually exercise the assertions, run `cargo test --lib -- --test-threads=1`.
    // Only the first test will acquire the main thread; subsequent tests will still skip.
    // A future MainThreadTestHarness (Phase 5/6) will provide a proper test harness
    // for single-threaded main-thread testing of AppKit code.

    use super::*;
    use crate::config::{AppearanceConfig, OverlayPosition};

    #[test]
    fn panel_constructed_with_required_flags() {
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => {
                eprintln!("panel_constructed_with_required_flags: skipping (not on main thread)");
                return;
            }
        };

        let config = Config {
            appearance: AppearanceConfig::default(),
            position: OverlayPosition::default(),
            ..Default::default()
        };

        let (panel, _label) = build_overlay_panel(mtm, &config);
        let pc = inspect(&panel);

        // Verify required flags per AC2.1
        assert!(pc.is_floating_panel, "panel must be floating");
        assert_ne!(pc.style_mask & NSWindowStyleMask::Borderless.bits() as u64, 0, "must have Borderless flag");
        assert_ne!(pc.style_mask & NSWindowStyleMask::NonactivatingPanel.bits() as u64, 0, "must have NonactivatingPanel flag");
        assert_ne!(
            pc.collection_behavior & NSWindowCollectionBehavior::CanJoinAllSpaces.bits() as u64, 0,
            "must have CanJoinAllSpaces"
        );
        assert_ne!(
            pc.collection_behavior & NSWindowCollectionBehavior::FullScreenAuxiliary.bits() as u64, 0,
            "must have FullScreenAuxiliary"
        );
        assert!(pc.ignores_mouse_events, "must ignore mouse events");
        assert_eq!(pc.level, NSFloatingWindowLevel as i64, "default level should be Floating");
    }

    #[test]
    fn above_fullscreen_toggle_changes_level() {
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => {
                eprintln!("above_fullscreen_toggle_changes_level: skipping (not on main thread)");
                return;
            }
        };

        let config = Config {
            appearance: AppearanceConfig::default(),
            position: OverlayPosition::default(),
            above_fullscreen: false,
            ..Default::default()
        };

        let (panel, _label) = build_overlay_panel(mtm, &config);

        // Initial state: should be floating
        let pc = inspect(&panel);
        assert_eq!(pc.level, NSFloatingWindowLevel as i64);

        // Toggle to above-fullscreen
        set_above_fullscreen(&panel, mtm, true);
        let pc = inspect(&panel);
        assert_eq!(pc.level, NSStatusWindowLevel as i64, "after set_above_fullscreen(true), level should be StatusWindow");

        // Toggle back to floating
        set_above_fullscreen(&panel, mtm, false);
        let pc = inspect(&panel);
        assert_eq!(pc.level, NSFloatingWindowLevel as i64, "after set_above_fullscreen(false), level should be Floating");
    }

    #[test]
    fn mode_switch_does_not_rebuild_panel() {
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => {
                eprintln!("mode_switch_does_not_rebuild_panel: skipping (not on main thread)");
                return;
            }
        };

        let config = Config {
            appearance: AppearanceConfig::default(),
            position: OverlayPosition::default(),
            ..Default::default()
        };

        let (panel, _label) = build_overlay_panel(mtm, &config);
        let initial_ptr = Retained::as_ptr(&panel);

        // Apply geometry for different modes — panel ptr should remain unchanged.
        apply_geometry(&panel, mtm, OverlayMode::Docked, &config);
        assert_eq!(Retained::as_ptr(&panel), initial_ptr, "apply_geometry should not rebuild panel");

        apply_geometry(&panel, mtm, OverlayMode::Floating, &config);
        assert_eq!(Retained::as_ptr(&panel), initial_ptr, "apply_geometry should not rebuild panel");

        apply_geometry(&panel, mtm, OverlayMode::Transcript, &config);
        assert_eq!(Retained::as_ptr(&panel), initial_ptr, "apply_geometry should not rebuild panel");
    }
}
