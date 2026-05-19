//! NSPanel construction and configuration for macOS overlay.
//!
//! Provides a Floating-mode NSPanel with caption display and configuration.
//! Phase 2 implements Floating mode only; Phase 6 adds Docked mode and
//! Transcript window.

use objc2::rc::Retained;
use objc2::{msg_send, MainThreadMarker, ClassType, MainThreadOnly};
use objc2_foundation::NSString;
use objc2_app_kit::{
    NSPanel, NSTextField, NSWindowStyleMask, NSWindowCollectionBehavior,
    NSFloatingWindowLevel, NSStatusWindowLevel, NSColor, NSFont, NSLineBreakMode,
};
use objc2_core_foundation::{CGRect, CGPoint, CGSize};
use crate::config::Config;

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
            NSStatusWindowLevel as i64
        } else {
            NSFloatingWindowLevel as i64
        };

        // Create the NSPanel with appropriate style and behavior.
        let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let backing = 2u64; // NSBackingStoreBuffered

        // Allocate and initialize NSPanel
        let panel: Retained<NSPanel> = msg_send![
            NSPanel::alloc(mtm),
            initWithContentRect: frame,
            styleMask: style_mask.bits() as u64,
            backing: backing,
            defer: false
        ];

        // Set window level (floating or status depending on above_fullscreen)
        let _: () = msg_send![&panel, setLevel: level];

        // Configure collection behavior for multi-space and fullscreen support
        let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary;
        let _: () = msg_send![&panel, setCollectionBehavior: collection_behavior.bits()];

        // Mark as floating panel
        let _: () = msg_send![&panel, setFloatingPanel: true];

        // Set transparent background
        let clear_color = NSColor::clearColor();
        let _: () = msg_send![&panel, setBackgroundColor: &*clear_color];

        // Disable shadow (Floating mode)
        let _: () = msg_send![&panel, setHasShadow: false];

        // Click-through: ignore mouse events
        let _: () = msg_send![&panel, setIgnoresMouseEvents: true];

        // Allow repositioning by background click
        let _: () = msg_send![&panel, setMovableByWindowBackground: true];

        // Create and configure the content NSTextField
        let label_frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, height));
        let label: Retained<NSTextField> = msg_send![
            NSTextField::alloc(mtm),
            initWithFrame: label_frame
        ];

        // Text properties
        let empty_str = NSString::from_str("");
        let _: () = msg_send![&label, setStringValue: &*empty_str];
        let _: () = msg_send![&label, setLineBreakMode: NSLineBreakMode::ByWordWrapping.0];
        let _: () = msg_send![&label, setEditable: false];
        let _: () = msg_send![&label, setSelectable: false];
        let _: () = msg_send![&label, setBordered: false];
        let _: () = msg_send![&label, setDrawsBackground: false];

        // Font: monospace at configured size
        let font_size = config.appearance.font_size as f64;
        let font: Retained<NSFont> = msg_send![
            NSFont::class(),
            userFixedPitchFontOfSize: font_size
        ];
        let _: () = msg_send![&label, setFont: &*font];

        // Set label as panel's content view
        let _: () = msg_send![&panel, setContentView: &*label];

        (panel, label)
    }
}

/// Inspect panel configuration for testing and verification.
#[allow(dead_code)]  // Used in #[cfg(all(test, target_os = "macos"))] tests
pub fn inspect(panel: &NSPanel) -> PanelConfig {
    unsafe {
        let level: i64 = msg_send![panel, level];
        let collection_behavior: u64 = msg_send![panel, collectionBehavior];
        let is_floating_panel: bool = msg_send![panel, isFloatingPanel];
        let style_mask: u64 = msg_send![panel, styleMask];
        let ignores_mouse_events: bool = msg_send![panel, ignoresMouseEvents];

        PanelConfig {
            level,
            collection_behavior,
            is_floating_panel,
            style_mask,
            ignores_mouse_events,
        }
    }
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
    unsafe {
        let level = if on {
            NSStatusWindowLevel as i64
        } else {
            NSFloatingWindowLevel as i64
        };
        let _: () = msg_send![panel, setLevel:level];
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
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
}
