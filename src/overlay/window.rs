//! Overlay window construction, docked/floating layout, and CSS styling.

use crate::config::{AppearanceConfig, Config, DockPosition, OverlayMode, ScreenEdge};
use crate::overlay::input_region;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;

/// Build the overlay window for the given config.
/// Uses gtk4-layer-shell for both docked and floating modes (Layer::Top).
pub fn build_overlay_window(app: &Application, cfg: &Config) -> ApplicationWindow {
    let window = ApplicationWindow::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .title("subtidal")
        .build();

    // Initialize layer shell.
    window.init_layer_shell();
    window.set_layer(if cfg.above_fullscreen { Layer::Overlay } else { Layer::Top });
    window.set_exclusive_zone(0); // don't push other windows aside

    match cfg.overlay_mode {
        OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge, &cfg.dock_position),
        OverlayMode::Floating => configure_floating(&window, cfg),
        OverlayMode::Transcript => {
            // Transcript mode hides the layer-shell overlay entirely; configure as
            // docked so if the user switches back to Docked mid-session the surface
            // is in a known good state. The window's visibility is gated separately
            // by the activation closure in overlay/mod.rs.
            configure_docked(&window, &cfg.screen_edge, &cfg.dock_position);
        }
    }

    // Build caption label with wrapping.
    // max_width_chars caps the label's natural width, forcing GTK to wrap text
    // instead of expanding the label/window to fit one long line.
    let max_chars = estimate_max_chars(cfg.appearance.width, cfg.appearance.font_size, cfg.appearance.effective_char_width_fraction());
    let label = Label::builder()
        .label("")
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .max_width_chars(max_chars)
        .lines(cfg.appearance.max_lines as i32)
        .xalign(0.0) // left-align text
        .build();
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_widget_name("caption-label");
    window.set_child(Some(&label));
    window.set_width_request(cfg.appearance.width);

    // Set click-through after window maps.
    let is_locked = cfg.locked || cfg.overlay_mode == OverlayMode::Docked;
    window.connect_map(move |win| {
        if is_locked {
            input_region::set_empty_input_region(win);
        } else {
            input_region::clear_input_region(win);
        }
    });

    window
}

pub fn configure_docked(window: &ApplicationWindow, edge: &ScreenEdge, dock_pos: &DockPosition) {
    // Always anchor to the selected edge.
    let anchor_edge = match edge {
        ScreenEdge::Bottom => Edge::Bottom,
        ScreenEdge::Top    => Edge::Top,
        ScreenEdge::Left   => Edge::Left,
        ScreenEdge::Right  => Edge::Right,
    };

    // For Stretch, anchor both perpendicular edges (fills the edge).
    // For Center/Offset, anchor only the primary edge — the compositor
    // centers the window on that edge (layer-shell spec). We use margins
    // to offset from center if needed.
    match dock_pos {
        DockPosition::Stretch => {
            let stretch_edges = match edge {
                ScreenEdge::Bottom | ScreenEdge::Top => vec![Edge::Left, Edge::Right],
                ScreenEdge::Left | ScreenEdge::Right => vec![Edge::Top, Edge::Bottom],
            };
            window.set_anchor(anchor_edge, true);
            for e in stretch_edges {
                window.set_anchor(e, true);
            }
        }
        DockPosition::Center => {
            // Only anchor the primary edge — compositor centers on that edge.
            window.set_anchor(anchor_edge, true);
        }
        DockPosition::Offset(px) => {
            // Anchor primary edge + the "start" perpendicular edge, use margin for offset.
            window.set_anchor(anchor_edge, true);
            match edge {
                ScreenEdge::Bottom | ScreenEdge::Top => {
                    window.set_anchor(Edge::Left, true);
                    window.set_margin(Edge::Left, *px);
                }
                ScreenEdge::Left | ScreenEdge::Right => {
                    window.set_anchor(Edge::Top, true);
                    window.set_margin(Edge::Top, *px);
                }
            }
        }
    }

    // Keyboard and pointer click-through: handled by keyboard_mode + empty input region.
    window.set_keyboard_mode(KeyboardMode::None);
}

fn configure_floating(window: &ApplicationWindow, cfg: &Config) {
    // Anchor to top-left so that Left/Top margins position the window absolutely.
    // Without anchors, layer-shell centers the surface and margins are relative to
    // center — which varies by compositor (KDE/Plasma doesn't support margin-from-center).
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);

    window.set_keyboard_mode(if cfg.locked {
        KeyboardMode::None
    } else {
        KeyboardMode::OnDemand
    });

    // Position the window via margins from the anchored edges.
    window.set_margin(Edge::Left, cfg.position.x);
    window.set_margin(Edge::Top, cfg.position.y);
}

/// Build CSS string from appearance config. Pure — no GTK display needed,
/// so it's unit-testable.
fn build_css(appearance: &AppearanceConfig) -> String {
    format!(
        r#"
        window {{
            background-color: {bg};
            border-radius: 12px;
        }}
        #caption-label {{
            color: {fg};
            font-size: {fs}pt;
            padding: 8px 12px;
        }}
        "#,
        bg = appearance.background_color,
        fg = appearance.text_color,
        fs = appearance.font_size,
    )
}

/// Set CSS on the caption label and window to reflect appearance config.
///
/// Uses a thread-local provider to avoid resource leaks: old provider is removed
/// before creating a new one on each call.
pub fn apply_appearance(appearance: &AppearanceConfig) {
    thread_local! {
        static CSS_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
    }

    let css = build_css(appearance);

    let display = gtk4::gdk::Display::default().expect("no GDK display");

    CSS_PROVIDER.with(|provider_cell| {
        let mut provider_opt = provider_cell.borrow_mut();

        // Remove old provider if it exists
        if let Some(ref old_provider) = *provider_opt {
            gtk4::style_context_remove_provider_for_display(&display, old_provider);
        }

        // Create and add new provider
        let new_provider = gtk4::CssProvider::new();
        new_provider.load_from_data(&css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &new_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Store the new provider for next call
        *provider_opt = Some(new_provider);
    });
}

/// Estimate the number of characters that fit in the given pixel width at the given font size.
/// Uses an approximate average character width of 0.6 × font_size (reasonable for proportional fonts).
pub fn estimate_max_chars(width_px: i32, font_size_pt: f32, char_width_fraction: f32) -> i32 {
    if width_px <= 0 || font_size_pt <= 0.0 {
        return 80; // fallback
    }
    // Average char width ≈ 0.6 × font size in points (heuristic for proportional fonts).
    // Subtract padding (8px + 12px = 20px per side from CSS).
    let usable_width = (width_px - 24).max(100) as f32;
    let avg_char_width = font_size_pt * 0.6;
    (usable_width / avg_char_width * char_width_fraction).floor() as i32
}

pub fn find_caption_label(window: &ApplicationWindow) -> Label {
    // Label is inside ScrolledWindow → Viewport (auto-created by GTK4) → Label.
    // Search by widget name to avoid fragile tree traversal.
    fn find_by_name(widget: &gtk4::Widget, name: &str) -> Option<Label> {
        if widget.widget_name() == name {
            return widget.clone().downcast::<Label>().ok();
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(found) = find_by_name(&c, name) {
                return Some(found);
            }
            child = c.next_sibling();
        }
        None
    }
    find_by_name(window.upcast_ref(), "caption-label")
        .expect("caption label not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_css_contains_appearance_settings() {
        let appearance = AppearanceConfig {
            background_color: "rgba(255,0,0,0.5)".to_string(),
            text_color: "#00ff00".to_string(),
            font_size: 24.0,
            max_lines: 5,
            width: 800,
            height: 0,
            expire_secs: 8,
            char_width_fraction: 0.95,
        };
        let css = build_css(&appearance);

        assert!(css.contains("rgba(255,0,0,0.5)"), "CSS should contain background_color");
        assert!(css.contains("#00ff00"), "CSS should contain text_color");
        assert!(css.contains("24"), "CSS should contain font_size");
    }

    #[test]
    fn build_css_with_default_appearance() {
        let appearance = AppearanceConfig::default();
        let css = build_css(&appearance);

        assert!(css.contains("rgba(0,0,0,0.7)"), "CSS should contain default background_color");
        assert!(css.contains("#ffffff"), "CSS should contain default text_color");
        assert!(css.contains("16"), "CSS should contain default font_size");
    }

    /// AC4.1: estimate_max_chars applies conservative multiplier for visual padding.
    #[test]
    fn ac4_1_conservative_multiplier() {
        let width_px = 800;
        let font_size_pt = 24.0;

        let result = estimate_max_chars(width_px, font_size_pt, 0.95);
        let expected_full = ((776.0_f32 / 14.4).floor()) as i32; // 53
        let expected_95 = ((776.0_f32 / 14.4 * 0.95).floor()) as i32; // 51

        assert_eq!(expected_full, 53, "Sanity check: full formula should give 53");
        assert_eq!(expected_95, 51, "Sanity check: 0.95 formula should give 51");
        assert_eq!(result, 51, "Result with 0.95 fraction");
        assert!(
            result < expected_full,
            "Fraction should make result smaller than full formula"
        );
    }
}
