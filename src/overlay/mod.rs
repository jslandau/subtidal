//! GTK4 overlay window: docked (wlr-layer-shell) and floating modes with caption display.

use crate::config::{AppearanceConfig, Config, DockPosition, OverlayMode, ScreenEdge};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};
use gtk4::glib;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::sync::{Arc, atomic::{AtomicBool, AtomicI32, Ordering}};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

/// Buffer that accumulates recent caption fragments with timestamps for expiry.
struct CaptionBuffer {
    /// Each entry is (timestamp, fragment_text). Fragments are displayed as
    /// continuous flowing text separated by spaces.
    fragments: Vec<(Instant, String)>,
    max_fragments: usize,
    expire_secs: u64,
    /// Track the last few words to detect and skip repeated output from the RNNT decoder.
    last_tail: String,
}

impl CaptionBuffer {
    fn new(max_fragments: usize) -> Self {
        CaptionBuffer {
            fragments: Vec::new(),
            max_fragments,
            expire_secs: 8,
            last_tail: String::new(),
        }
    }

    /// Add a new caption fragment, deduplicating overlapping text from streaming RNNT.
    /// Preserves leading/trailing whitespace from the engine — these signal word
    /// boundaries (e.g. " ve" = new word, "ve" = continuation of previous word).
    fn push(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }

        // Deduplicate: if the new text starts with the end of what we already have,
        // skip the overlapping prefix. Streaming RNNT decoders sometimes re-emit
        // the tail of the previous output as the start of the next.
        let deduped = Self::remove_overlap(&self.last_tail, text.trim());
        if deduped.is_empty() {
            return;
        }

        // Preserve the leading space from the original engine output if present.
        // This signals a word boundary vs. a mid-word continuation.
        let fragment = if text.starts_with(char::is_whitespace) && !deduped.starts_with(char::is_whitespace) {
            format!(" {deduped}")
        } else {
            deduped.clone()
        };

        self.fragments.push((Instant::now(), fragment));
        if self.fragments.len() > self.max_fragments {
            self.fragments.remove(0);
        }

        // Update tail: keep last ~60 chars for overlap detection.
        let display = self.display_text();
        let tail_start = display.len().saturating_sub(60);
        self.last_tail = display[tail_start..].to_string();
    }

    /// Remove overlapping prefix between existing tail and new text.
    /// Only triggers on overlaps of 4+ characters to avoid false positives
    /// from coincidental single-character matches.
    fn remove_overlap(tail: &str, new: &str) -> String {
        if tail.is_empty() {
            return new.to_string();
        }
        let tail_lower = tail.to_lowercase();
        let new_lower = new.to_lowercase();

        // Only consider overlaps of 4+ characters to avoid false positives.
        let max_check = tail_lower.len().min(new_lower.len());
        for overlap_len in (4..=max_check).rev() {
            let tail_suffix = &tail_lower[tail_lower.len() - overlap_len..];
            let new_prefix = &new_lower[..overlap_len];
            if tail_suffix == new_prefix {
                let remainder = new[overlap_len..].trim_start();
                if !remainder.is_empty() {
                    return remainder.to_string();
                }
            }
        }
        new.to_string()
    }

    /// Remove fragments older than expire_secs. Returns true if any were removed.
    fn expire(&mut self) -> bool {
        let cutoff = Instant::now() - std::time::Duration::from_secs(self.expire_secs);
        let before = self.fragments.len();
        self.fragments.retain(|(ts, _)| *ts > cutoff);
        if self.fragments.len() != before {
            // Rebuild tail after expiry.
            let display = self.display_text();
            let tail_start = display.len().saturating_sub(60);
            self.last_tail = display[tail_start..].to_string();
            true
        } else {
            false
        }
    }

    /// Join all buffered fragments into a single display string.
    /// Fragments carry their own leading whitespace from the engine's tokenizer:
    /// " Hello" = new word, "llo" = mid-word continuation. So we concatenate directly.
    fn display_text(&self) -> String {
        let mut result = String::new();
        for (_, frag) in &self.fragments {
            result.push_str(frag);
        }
        // Trim leading whitespace from the combined result.
        result.trim_start().to_string()
    }
}

pub mod input_region;

/// Commands sent to the overlay from the tray / main integration.
#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Show or hide the overlay.
    SetVisible(bool),
    /// Switch overlay mode (docked ↔ floating).
    SetMode(OverlayMode),
    /// Lock or unlock the floating overlay.
    SetLocked(bool),
    /// Update appearance from config.
    UpdateAppearance(AppearanceConfig),
    /// Update caption text (also sent as plain String via glib channel in normal flow).
    #[allow(dead_code)]
    SetCaption(String),
    /// Quit the application cleanly (sent by tray Quit and SIGTERM handler).
    Quit,
}

/// Shared visibility flag (AtomicBool for tray ↔ overlay signaling).
pub type CaptionsEnabled = Arc<AtomicBool>;

/// Build and run the GTK4 application.
///
/// This function must be called on the main thread. It blocks until the GTK4
/// main loop exits.
///
/// Parameters:
/// - `config`: initial configuration
/// - `caption_rx`: mpsc channel receiver delivering caption strings from inference thread
/// - `cmd_rx`: mpsc channel receiver delivering OverlayCommand from tray
/// - `captions_enabled`: shared bool for left-click tray toggle
pub fn run_gtk_app(
    config: Config,
    caption_rx: std::sync::mpsc::Receiver<String>,
    cmd_rx: std::sync::mpsc::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    let app = Application::builder()
        .application_id("com.subtidal.app")
        .build();

    let config = Arc::new(std::sync::Mutex::new(config));
    let config_clone = Arc::clone(&config);
    let captions_enabled_clone = Arc::clone(&captions_enabled);

    // Wrap channels in Arc so they can be shared with closures
    let caption_rx = Arc::new(std::sync::Mutex::new(caption_rx));
    let cmd_rx = Arc::new(std::sync::Mutex::new(cmd_rx));

    app.connect_activate(move |app| {
        let cfg = config_clone.lock().unwrap().clone();
        let window = build_overlay_window(app, &cfg);

        // Apply initial appearance.
        apply_appearance(&cfg.appearance);

        // Dragging flag: when true, suppress all GTK mutations except margin updates.
        // Any relayout (caption text, CSS reload, widget resize) during a drag causes
        // the compositor to momentarily reposition the layer-shell surface, producing jitter.
        let is_dragging = Rc::new(Cell::new(false));

        // Initial drag handler for floating + unlocked.
        if cfg.overlay_mode == OverlayMode::Floating && !cfg.locked {
            add_drag_handler(&window, &is_dragging);
        }

        // Wire up caption receiver using glib timeout_add to poll.
        let label = find_caption_label(&window);
        let window_clone = window.clone();
        let enabled = Arc::clone(&captions_enabled_clone);
        let caption_rx_clone = Arc::clone(&caption_rx);
        let caption_buffer = Rc::new(RefCell::new(CaptionBuffer::new(cfg.appearance.max_lines as usize)));

        // Poll for new captions and append to buffer.
        let buf_for_poll = Rc::clone(&caption_buffer);
        let label_for_poll = label.clone();
        let window_for_poll = window_clone.clone();
        let dragging_for_caption = Rc::clone(&is_dragging);
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if let Ok(rx) = caption_rx_clone.try_lock() {
                let mut buf = buf_for_poll.borrow_mut();
                while let Ok(text) = rx.try_recv() {
                    if enabled.load(Ordering::Relaxed) {
                        buf.push(text);
                        if !dragging_for_caption.get() {
                            label_for_poll.set_text(&buf.display_text());
                            window_for_poll.set_visible(true);
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        // Timer to expire old caption lines every second.
        let buf_for_expire = Rc::clone(&caption_buffer);
        let label_for_expire = label.clone();
        let dragging_for_expire = Rc::clone(&is_dragging);
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            if !dragging_for_expire.get() {
                let mut buf = buf_for_expire.borrow_mut();
                if buf.expire() {
                    label_for_expire.set_text(&buf.display_text());
                }
            }
            glib::ControlFlow::Continue
        });

        // Wire up command receiver using glib timeout_add to poll.
        let window_clone2 = window.clone();
        let config_for_cmd = Arc::clone(&config_clone);
        let cmd_rx_clone = Arc::clone(&cmd_rx);
        let dragging_for_cmd = Rc::clone(&is_dragging);

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if let Ok(rx) = cmd_rx_clone.try_lock() {
                while let Ok(cmd) = rx.try_recv() {
                    if !dragging_for_cmd.get() {
                        handle_overlay_command(&window_clone2, cmd, &config_for_cmd, &dragging_for_cmd);
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        window.present();
    });

    app.run_with_args::<&str>(&[]);
}

/// Build the overlay window for the given config.
/// Uses gtk4-layer-shell for both docked and floating modes (Layer::Top).
fn build_overlay_window(app: &Application, cfg: &Config) -> ApplicationWindow {
    let window = ApplicationWindow::builder()
        .application(app)
        .decorated(false)
        .resizable(false)
        .title("subtidal")
        .build();

    // Initialize layer shell.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_exclusive_zone(0); // don't push other windows aside

    match cfg.overlay_mode {
        OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge, &cfg.dock_position),
        OverlayMode::Floating => configure_floating(&window, cfg),
    }

    // Build caption label with wrapping.
    // max_width_chars caps the label's natural width, forcing GTK to wrap text
    // instead of expanding the label/window to fit one long line.
    let max_chars = estimate_max_chars(cfg.appearance.width, cfg.appearance.font_size);
    let label = Label::builder()
        .label("")
        .wrap(true)
        .wrap_mode(gtk4::pango::WrapMode::WordChar)
        .max_width_chars(max_chars)
        .xalign(0.0) // left-align text
        .build();
    label.set_widget_name("caption-label");
    window.set_child(Some(&label));

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

fn configure_docked(window: &ApplicationWindow, edge: &ScreenEdge, dock_pos: &DockPosition) {
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

/// Build CSS string from appearance config.
/// AC3.7: Verify CSS contains configured appearance settings.
/// This is a pure function that can be tested without GTK display.
fn build_css(appearance: &AppearanceConfig) -> String {
    format!(
        r#"
        window {{
            background-color: {bg};
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
fn estimate_max_chars(width_px: i32, font_size_pt: f32) -> i32 {
    if width_px <= 0 || font_size_pt <= 0.0 {
        return 80; // fallback
    }
    // Average char width ≈ 0.6 × font size in points (heuristic for proportional fonts).
    // Subtract padding (8px + 12px = 20px per side from CSS).
    let usable_width = (width_px - 24).max(100) as f32;
    let avg_char_width = font_size_pt * 0.6;
    (usable_width / avg_char_width).floor() as i32
}

fn find_caption_label(window: &ApplicationWindow) -> Label {
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

fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
    is_dragging: &Rc<Cell<bool>>,
) {
    match cmd {
        OverlayCommand::SetVisible(v) => window.set_visible(v),
        OverlayCommand::SetMode(mode) => {
            // Reconfigure the existing layer-shell window for the new mode.
            // gtk4-layer-shell allows changing anchors/keyboard mode on a realized window.
            let mut cfg = config.lock().unwrap();
            cfg.overlay_mode = mode.clone();
            match mode {
                OverlayMode::Docked => {
                    // Clear any floating anchors, set docked anchors.
                    // First, clear all anchors.
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    configure_docked(window, &cfg.screen_edge, &cfg.dock_position);
                    // Docked mode is always click-through.
                    input_region::set_empty_input_region(window);
                }
                OverlayMode::Floating => {
                    // Clear all anchors, then set top-left for margin-based positioning.
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
                    window.set_anchor(Edge::Top, true);
                    window.set_anchor(Edge::Left, true);
                    // Restore position from config.
                    window.set_margin(Edge::Left, cfg.position.x);
                    window.set_margin(Edge::Top, cfg.position.y);
                    window.set_keyboard_mode(if cfg.locked {
                        KeyboardMode::None
                    } else {
                        KeyboardMode::OnDemand
                    });
                    if cfg.locked {
                        input_region::set_empty_input_region(window);
                    } else {
                        input_region::clear_input_region(window);
                        add_drag_handler(window, is_dragging);
                    }
                }
            }
        }
        OverlayCommand::SetLocked(locked) => {
            if locked {
                input_region::set_empty_input_region(window);
                window.set_keyboard_mode(KeyboardMode::None);
            } else {
                input_region::clear_input_region(window);
                window.set_keyboard_mode(KeyboardMode::OnDemand);
                add_drag_handler(window, is_dragging);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            apply_appearance(&appearance);
            let label = find_caption_label(window);
            label.set_max_width_chars(estimate_max_chars(appearance.width, appearance.font_size));
        }
        OverlayCommand::SetCaption(text) => {
            let label = find_caption_label(window);
            label.set_text(&text);
        }
        OverlayCommand::Quit => {
            // Quit the GTK4 application cleanly so all cleanup (Drop impls) runs.
            if let Some(app) = window.application() {
                app.quit();
            }
        }
    }
}

fn remove_drag_handlers(window: &ApplicationWindow) {
    // Remove existing GestureDrag controllers to prevent accumulation.
    // On repeated calls to add_drag_handler (e.g., SetLocked(false), SetMode(Floating)),
    // we must clean up previous gesture controllers to avoid erratic drag behavior.
    let controllers = window.observe_controllers();
    let n = controllers.n_items();
    for i in (0..n).rev() {
        if let Some(obj) = controllers.item(i) {
            if obj.downcast_ref::<gtk4::GestureDrag>().is_some() {
                if let Ok(ctrl) = obj.downcast::<gtk4::EventController>() {
                    window.remove_controller(&ctrl);
                }
            }
        }
    }
}

fn add_drag_handler(window: &ApplicationWindow, is_dragging: &Rc<Cell<bool>>) {
    // Remove any existing drag handlers first to prevent accumulation.
    remove_drag_handlers(window);

    // For gtk4-layer-shell floating windows, position is controlled by margins
    // (not compositor-managed coordinates). We use GestureDrag to track delta
    // and update set_margin() on each drag update.
    //
    // Note: begin_move_drag() is a GTK3 API that does not exist in GTK4.
    // On Wayland with layer-shell, the compositor positions the surface via margins.
    let gesture = gtk4::GestureDrag::new();

    // Capture starting margins when drag begins and set the dragging flag.
    // While dragging, all other GTK mutations (captions, CSS, commands) are
    // suppressed to prevent relayout-induced jitter on the layer-shell surface.
    let start_x = Arc::new(AtomicI32::new(0));
    let start_y = Arc::new(AtomicI32::new(0));
    // How much we've moved the window so far (cumulative).
    // GestureDrag reports offsets in widget-local coords. When set_margin() moves
    // the window, the coordinate system shifts by the same amount, so GTK's reported
    // offset becomes: real_mouse_movement - accumulated_window_movement.
    // Therefore: true_total = accumulated + reported_offset.
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
    let sx2 = Arc::clone(&start_x);
    let sy2 = Arc::clone(&start_y);
    let mx2 = Arc::clone(&moved_x);
    let my2 = Arc::clone(&moved_y);
    let win_update = window.clone();
    gesture.connect_drag_update(move |_, dx, dy| {
        let total_x = mx2.load(Ordering::Relaxed) + dx as i32;
        let total_y = my2.load(Ordering::Relaxed) + dy as i32;
        let new_x = (sx2.load(Ordering::Relaxed) + total_x).max(0);
        let new_y = (sy2.load(Ordering::Relaxed) + total_y).max(0);
        win_update.set_margin(Edge::Left, new_x);
        win_update.set_margin(Edge::Top, new_y);
        // Record how much we actually moved (clamped position - start position).
        mx2.store(new_x - sx2.load(Ordering::Relaxed), Ordering::Relaxed);
        my2.store(new_y - sy2.load(Ordering::Relaxed), Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// AC3.7: Caption text respects configured appearance (CSS).
    /// Test that build_css generates CSS containing configured colors and font size.
    #[test]
    fn build_css_contains_appearance_settings() {
        let appearance = AppearanceConfig {
            background_color: "rgba(255,0,0,0.5)".to_string(),
            text_color: "#00ff00".to_string(),
            font_size: 24.0,
            max_lines: 5,
            width: 800,
            height: 0,
        };
        let css = build_css(&appearance);

        // Verify CSS contains the background color
        assert!(css.contains("rgba(255,0,0,0.5)"), "CSS should contain background_color");

        // Verify CSS contains the text color
        assert!(css.contains("#00ff00"), "CSS should contain text_color");

        // Verify CSS contains the font size
        assert!(css.contains("24"), "CSS should contain font_size");
    }

    /// AC3.7: Test with default appearance settings.
    #[test]
    fn build_css_with_default_appearance() {
        let appearance = AppearanceConfig::default();
        let css = build_css(&appearance);

        // Verify CSS contains the default colors and font size
        assert!(css.contains("rgba(0,0,0,0.7)"), "CSS should contain default background_color");
        assert!(css.contains("#ffffff"), "CSS should contain default text_color");
        assert!(css.contains("16"), "CSS should contain default font_size");
    }
}
