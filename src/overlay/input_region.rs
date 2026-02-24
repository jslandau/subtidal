//! Click-through input region management for GTK4 overlay window.
//!
//! GTK4 exposes input region control via `gdk4::Surface::set_input_region()`.
//! An empty `cairo::Region` (zero rectangles) means no part of the surface accepts
//! pointer input — all events pass through to the window below (AC3.3, AC3.4).
//!
//! Additionally, `gtk4_layer_shell::LayerShell::set_keyboard_mode(KeyboardMode::None)`
//! prevents the overlay from stealing keyboard focus.

use gtk4::prelude::*;
use gtk4::ApplicationWindow;
use gtk4::cairo;
use gtk4_layer_shell::{KeyboardMode, LayerShell};

/// Make the window click-through: set an empty GDK surface input region.
///
/// Uses `gdk4::Surface::set_input_region()` with an empty `cairo::Region`.
/// Must be called after the window has been mapped (via `connect_map` signal).
///
/// Also sets `KeyboardMode::None` via gtk4-layer-shell to prevent focus stealing.
pub fn set_empty_input_region(window: &ApplicationWindow) {
    use gtk4::prelude::SurfaceExt;

    // Set keyboard mode to None (layer-shell API) — prevents focus stealing.
    window.set_keyboard_mode(KeyboardMode::None);

    let Some(surface) = window.surface() else {
        eprintln!("warn: set_empty_input_region: window has no GDK surface (not yet mapped?)");
        return;
    };

    // An empty cairo::Region (no rectangles added) means zero input area.
    // When set on the GDK surface, pointer events pass through to windows below.
    let empty_region = cairo::Region::create();
    surface.set_input_region(&empty_region);
}

/// Restore the default (full) input region: window accepts all pointer events.
///
/// Creates a region covering the full window bounds and sets it on the GDK surface.
/// Called when unlocking the floating overlay.
pub fn clear_input_region(window: &ApplicationWindow) {
    use gtk4::prelude::SurfaceExt;

    let Some(surface) = window.surface() else {
        return;
    };

    // A region covering the full window dimensions restores normal pointer handling.
    let width = window.width();
    let height = window.height();
    let full_rect = cairo::RectangleInt::new(0, 0, width, height);
    let full_region = cairo::Region::create_rectangle(&full_rect);
    surface.set_input_region(&full_region);
}
