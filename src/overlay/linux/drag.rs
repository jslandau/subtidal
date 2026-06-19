//! Drag-to-move gesture for the floating overlay, with compositor-aware
//! coordinate compensation.

use gtk4::prelude::*;
use gtk4::{ApplicationWindow, GestureDrag};
use gtk4_layer_shell::{Edge, LayerShell};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::{
    atomic::{AtomicI32, Ordering},
    Arc,
};

/// Whether the compositor shifts widget-local coordinates when layer-shell margins change mid-drag.
/// KDE, Sway, and Hyprland do this (GTK's drag offset shrinks as the surface moves), requiring
/// accumulated compensation. Niri (smithay-based) does not, so raw `start + offset` works directly.
fn compositor_shifts_coords_on_margin_change() -> bool {
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        // Niri is the known compositor that does NOT shift coords.
        // Default to compensation (safe fallback — worst case drag is sluggish, not flung off-screen).
        !desktop.contains("niri")
    })
}

/// Remove any existing GestureDrag controllers to prevent accumulation on
/// repeated calls (e.g., SetLocked(false) → SetMode(Floating)).
fn remove_drag_handlers(window: &ApplicationWindow) {
    let controllers = window.observe_controllers();
    let n = controllers.n_items();
    for i in (0..n).rev() {
        if let Some(obj) = controllers.item(i) {
            if obj.downcast_ref::<GestureDrag>().is_some() {
                if let Ok(ctrl) = obj.downcast::<gtk4::EventController>() {
                    window.remove_controller(&ctrl);
                }
            }
        }
    }
}

pub fn add_drag_handler(window: &ApplicationWindow, is_dragging: &Rc<Cell<bool>>) {
    // Remove any existing drag handlers first to prevent accumulation.
    remove_drag_handlers(window);

    // For gtk4-layer-shell floating windows, position is controlled by margins
    // (not compositor-managed coordinates). We use GestureDrag to track delta
    // and update set_margin() on each drag update.
    //
    // Note: begin_move_drag() is a GTK3 API that does not exist in GTK4.
    // On Wayland with layer-shell, the compositor positions the surface via margins.
    let gesture = GestureDrag::new();

    // Capture starting margins when drag begins and set the dragging flag.
    // While dragging, all other GTK mutations (captions, CSS, commands) are
    // suppressed to prevent relayout-induced jitter on the layer-shell surface.
    let start_x = Arc::new(AtomicI32::new(0));
    let start_y = Arc::new(AtomicI32::new(0));
    // Accumulated movement — only used on compositors that shift widget-local coords.
    let moved_x = Arc::new(AtomicI32::new(0));
    let moved_y = Arc::new(AtomicI32::new(0));

    let sx = Arc::clone(&start_x);
    let sy = Arc::clone(&start_y);
    let mx = Arc::clone(&moved_x);
    let my = Arc::clone(&moved_y);
    let win_begin = window.clone();
    let dragging_begin = Rc::clone(is_dragging);
    gesture.connect_drag_begin(move |_, _, _| {
        dragging_begin.set(true);
        sx.store(win_begin.margin(Edge::Left), Ordering::Relaxed);
        sy.store(win_begin.margin(Edge::Top), Ordering::Relaxed);
        mx.store(0, Ordering::Relaxed);
        my.store(0, Ordering::Relaxed);
    });

    // Update margins on each drag update.
    //
    // GestureDrag reports cumulative offset from the drag start point. However,
    // calling set_margin() repositions the layer-shell surface, and some compositors
    // (KDE, Sway, Hyprland) shift the widget-local coordinate origin accordingly —
    // making GTK's reported offset shrink by the amount the window moved. On these
    // compositors we must accumulate the real movement separately.
    //
    // Niri (smithay-based) does NOT shift coords, so raw start + offset works.
    let sx2 = Arc::clone(&start_x);
    let sy2 = Arc::clone(&start_y);
    let mx2 = Arc::clone(&moved_x);
    let my2 = Arc::clone(&moved_y);
    let win_update = window.clone();
    let needs_compensation = compositor_shifts_coords_on_margin_change();
    gesture.connect_drag_update(move |_, dx, dy| {
        let (new_x, new_y) = if needs_compensation {
            // KDE/Sway/Hyprland: offset is reduced by surface movement, so add accumulated delta.
            let total_x = mx2.load(Ordering::Relaxed) + dx as i32;
            let total_y = my2.load(Ordering::Relaxed) + dy as i32;
            let nx = (sx2.load(Ordering::Relaxed) + total_x).max(0);
            let ny = (sy2.load(Ordering::Relaxed) + total_y).max(0);
            mx2.store(nx - sx2.load(Ordering::Relaxed), Ordering::Relaxed);
            my2.store(ny - sy2.load(Ordering::Relaxed), Ordering::Relaxed);
            (nx, ny)
        } else {
            // Niri: offset is the true cumulative mouse delta.
            let nx = (sx2.load(Ordering::Relaxed) + dx as i32).max(0);
            let ny = (sy2.load(Ordering::Relaxed) + dy as i32).max(0);
            (nx, ny)
        };
        win_update.set_margin(Edge::Left, new_x);
        win_update.set_margin(Edge::Top, new_y);
    });

    // Clear dragging flag and save position on drag end.
    let win_for_release = window.clone();
    let dragging_end = Rc::clone(is_dragging);
    gesture.connect_drag_end(move |_, _offset_x, _offset_y| {
        dragging_end.set(false);
        let x = win_for_release.margin(Edge::Left);
        let y = win_for_release.margin(Edge::Top);
        eprintln!("info: overlay dragged to ({x}, {y})");
        let mut cfg = crate::config::Config::load();
        cfg.position.x = x;
        cfg.position.y = y;
        if let Err(e) = cfg.save() {
            eprintln!("warn: failed to save position: {e}");
        }
    });

    window.add_controller(gesture);
}
