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
use std::sync::OnceLock;

/// Niri doesn't honor oversized input regions set via `set_input_region()`.
/// Passing `None` (unset) works on Niri but may not on all compositors.
/// KDE/Sway work with an explicit large rectangle.
pub fn is_niri() -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase()
            .contains("niri")
    })
}

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
/// On Niri: passes `None` to unset the region entirely (Niri doesn't honor explicit large regions).
/// On KDE/Sway/others: sets a large explicit rectangle (known working behavior).
pub fn clear_input_region(window: &ApplicationWindow) {
    use gtk4::prelude::SurfaceExt;

    let Some(surface) = window.surface() else {
        return;
    };

    if is_niri() {
        // On Niri, use actual window dimensions — Niri clips input regions to the
        // surface bounds and doesn't honor oversized rectangles.
        let w = window.width().max(1);
        let h = window.height().max(1);
        let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(0, 0, w, h));
        surface.set_input_region(&region);
    } else {
        // KDE/Sway: large rectangle works and avoids races with layout.
        let full_rect = cairo::RectangleInt::new(0, 0, 16384, 16384);
        let full_region = cairo::Region::create_rectangle(&full_rect);
        surface.set_input_region(&full_region);
    }
}
