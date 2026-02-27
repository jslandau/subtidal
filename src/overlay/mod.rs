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

/// Represents one line of caption text with a timestamp for expiry.
struct CaptionLine {
    text: String,
    last_active: Instant,
}

/// Buffer that accumulates caption text in lines with fill-and-shift model.
/// Lines are filled word-by-word up to max_chars_per_line. When all lines are full
/// and new text arrives, the oldest line is removed, all lines shift up, and new
/// text fills the freed bottom line. Individual lines expire after idle_secs of silence.
struct CaptionBuffer {
    /// Ordered lines from oldest (top, shown first) to newest (bottom, shown last).
    lines: Vec<CaptionLine>,
    max_lines: usize,
    max_chars_per_line: usize,
    expire_secs: u64,
    /// Track the last few words to detect and skip repeated output from the RNNT decoder.
    last_tail: String,
}

impl CaptionBuffer {
    fn new(max_lines: usize, max_chars_per_line: usize, expire_secs: u64) -> Self {
        CaptionBuffer {
            lines: Vec::new(),
            max_lines,
            max_chars_per_line,
            expire_secs,
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

        // Determine if this is a continuation fragment (no leading space and lines are not empty).
        let is_continuation = !fragment.starts_with(char::is_whitespace) && !self.lines.is_empty();

        if is_continuation {
            // Continuation: join with the last word on the current line.
            let idx = self.lines.len() - 1;
            let combined = format!("{}{}", self.lines[idx].text.clone(), fragment);

            if combined.len() <= self.max_chars_per_line {
                // Fits on current line: append directly.
                self.lines[idx].text = combined;
                self.lines[idx].last_active = Instant::now();
            } else {
                // Would overflow current line: move partial word to next line.
                if let Some(last_space_pos) = self.lines[idx].text.rfind(' ') {
                    // Split at last space: keep everything up to and including the space,
                    // move the partial word after the space.
                    let partial_word = self.lines[idx].text[last_space_pos + 1..].to_string();
                    self.lines[idx].text = self.lines[idx].text[..=last_space_pos].trim_end().to_string();

                    // Add new line with partial + continuation joined.
                    self.add_new_line(format!("{}{}", partial_word, fragment));
                } else {
                    // Entire line is one word with no space: start fresh on new line.
                    // Remove the old line before calling add_new_line to avoid stale index
                    // if add_new_line shifts (when buffer is at max_lines capacity).
                    let old_text = self.lines.remove(idx).text;
                    self.add_new_line(format!("{}{}", old_text, fragment));
                }
            }
        } else {
            // Not a continuation: split into words and fill lines normally.
            let words: Vec<&str> = fragment.split_whitespace().collect();
            for word in words {
                if word.is_empty() {
                    continue;
                }

                if self.lines.is_empty() {
                    // Start a new line with this word.
                    self.add_new_line(word.to_string());
                } else {
                    let idx = self.lines.len() - 1;

                    if self.lines[idx].text.is_empty() {
                        // Current line is empty: place word directly (no space prefix).
                        self.lines[idx].text = word.to_string();
                    } else if self.lines[idx].text.len() + 1 + word.len() <= self.max_chars_per_line {
                        // Room on current line: append with space.
                        self.lines[idx].text.push(' ');
                        self.lines[idx].text.push_str(word);
                    } else {
                        // Overflow: start new line (shifts if at max_lines).
                        self.add_new_line(word.to_string());
                    }
                }
            }
        }

        // Update last_active on the last line (most recent text).
        if !self.lines.is_empty() {
            let idx = self.lines.len() - 1;
            self.lines[idx].last_active = Instant::now();
        }

        // Rebuild tail for overlap detection.
        let display = self.all_text();
        let tail_start = display.len().saturating_sub(60);
        self.last_tail = display[tail_start..].to_string();
    }

    /// Add a new line, shifting off the oldest line if at max_lines capacity.
    fn add_new_line(&mut self, text: String) {
        if self.lines.len() >= self.max_lines {
            self.lines.remove(0); // Remove oldest (top) line.
        }
        self.lines.push(CaptionLine {
            text,
            last_active: Instant::now(),
        });
    }

    /// Join all line text with empty string. Each line's text is properly spaced already.
    fn all_text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("")
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

    /// Remove the oldest line if its last_active timestamp is older than expire_secs.
    /// Only removes one line per call (gradual drain). Returns true if a line was removed.
    fn expire(&mut self) -> bool {
        if self.lines.is_empty() {
            return false;
        }

        let cutoff = Instant::now() - std::time::Duration::from_secs(self.expire_secs);
        if self.lines[0].last_active <= cutoff {
            self.lines.remove(0);
            // Rebuild tail after removal.
            let display = self.all_text();
            let tail_start = display.len().saturating_sub(60);
            self.last_tail = display[tail_start..].to_string();
            true
        } else {
            false
        }
    }

    /// Join all lines with newline separators for display.
    fn display_text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
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
        let max_chars_per_line = estimate_max_chars(cfg.appearance.width, cfg.appearance.font_size) as usize;
        let caption_buffer = Rc::new(RefCell::new(CaptionBuffer::new(
            cfg.appearance.max_lines as usize,
            max_chars_per_line,
            8, // expire_secs
        )));

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
fn estimate_max_chars(width_px: i32, font_size_pt: f32) -> i32 {
    if width_px <= 0 || font_size_pt <= 0.0 {
        return 80; // fallback
    }
    // Average char width ≈ 0.6 × font size in points (heuristic for proportional fonts).
    // Subtract padding (8px + 12px = 20px per side from CSS).
    let usable_width = (width_px - 24).max(100) as f32;
    let avg_char_width = font_size_pt * 0.6;
    // Apply 0.85× conservative multiplier for visual padding with proportional fonts.
    (usable_width / avg_char_width * 0.85).floor() as i32
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
            label.set_lines(appearance.max_lines as i32);
            window.set_width_request(appearance.width);
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

    // CaptionBuffer line-fill tests

    /// AC1.1: Text fills line 1 left-to-right, word by word, up to max_chars_per_line.
    #[test]
    fn ac1_1_fill_single_line() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Push words with leading spaces (word boundaries).
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());
        buf.push(" this".to_string());

        let display = buf.display_text();
        assert_eq!(display, "Hello world this", "Words should fill single line");
        assert!(!display.contains('\n'), "Should not have newline separator");
    }

    /// AC1.2: When line 1 is full, text continues on line 2 (up to max_lines).
    #[test]
    fn ac1_2_overflow_to_second_line() {
        let mut buf = CaptionBuffer::new(3, 15, 8);

        // Fill line 1 with "Hello world" (11 chars).
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());

        // Next word "this" (4 chars) won't fit (11 + 1 + 4 = 16 > 15).
        buf.push(" this".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should have 2 lines");
        assert_eq!(lines[0], "Hello world");
        assert_eq!(lines[1], "this");
    }

    /// AC1.3: When all lines are full and new text arrives, line 1 is removed,
    /// all lines shift up, and new text fills the freed bottom line.
    #[test]
    fn ac1_3_shift_when_all_lines_full() {
        let mut buf = CaptionBuffer::new(2, 7, 8);

        // Fill line 1: " Hello" (5 chars, fits in 7).
        buf.push(" Hello".to_string());

        // Add word that goes to line 2: "Hello world" = 11 chars > 7, so "world" goes to line 2 (5 chars).
        buf.push(" world".to_string());

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines filled");
        assert_eq!(buf.lines[0].text, "Hello");
        assert_eq!(buf.lines[1].text, "world");

        // Add third word: "Hello world test" = " test" (4 chars) won't fit on line 2 (5+1+4=10 > 7),
        // so it goes to new line. Since we're at max_lines=2, oldest line (line 1: "Hello") shifts off.
        buf.push(" test".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should still have max_lines=2 after shift");
        assert_eq!(lines[0], "world", "Line 1 should be old line 2");
        assert_eq!(lines[1], "test", "Line 2 should be new content");
    }

    /// AC1.4: Continuation fragments (no leading space) join the previous word
    /// on the same line without inserting a space.
    #[test]
    fn ac1_4_continuation_no_space() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Push " Hel" (word boundary).
        buf.push(" Hel".to_string());
        // Push "lo" (continuation, no leading space).
        buf.push("lo".to_string());

        let display = buf.display_text();
        assert_eq!(display, "Hello", "Continuation should join without space");
    }

    /// AC1.5: When a continuation fragment would cause the combined word to overflow
    /// the current line, the partial word moves to the next line and joins there.
    /// Tests the "with space" branch where we split at last space.
    #[test]
    fn ac1_5_partial_word_overflow() {
        let mut buf = CaptionBuffer::new(3, 10, 8);

        // Set up: Line 1: "Hello" (5), Line 2: "world" (5)
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());

        // Line 2 is now "world" (5 chars). Add another word " more" (5 chars).
        // "world more" = 10 chars, exactly fits.
        buf.push(" more".to_string());

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines before overflow");
        assert_eq!(buf.lines[1].text, "world more");

        // Current line 2: "world more" (10 chars). Push continuation "text" (4 chars).
        // Appending "text" to last word "more": "moretext" (8 chars).
        // Adding to current line: 10 + 8 = 18 > 10, overflow!
        // Last space in "world more" at position 5.
        // Split: keep "world", move "more" to new line.
        // New line 3: "more" + "text" = "moretext" (8 chars).
        buf.push("text".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 3, "Should have 3 lines after split");
        assert_eq!(lines[0], "Hello", "Line 1 should have 'Hello'");
        assert_eq!(lines[1], "world", "Line 2 should have 'world' (split off)");
        assert_eq!(lines[2], "moretext", "Line 3 should have 'more' + 'text' joined");
    }

    /// AC1.5 extended: "no space" branch at full max_lines capacity.
    /// When last line is a single word and continuation overflows with no space,
    /// the old line is removed and replaced with the joined word.
    /// This tests the critical bug fix where stale index could clear the wrong line.
    #[test]
    fn ac1_5_continuation_no_space_at_full_capacity() {
        let mut buf = CaptionBuffer::new(3, 7, 8); // max_lines=3, max_chars=7

        // Create three single-word lines to fill buffer to max_lines.
        buf.push(" one".to_string());   // Line 1: "one" (3 chars)
        buf.push(" two".to_string());   // Line 1: "one two" = 7, fits exactly
        buf.push(" three".to_string()); // "one two three" = 13 > 7, goes to line 2: "three" (5 chars)
        buf.push(" four".to_string());  // "three four" = 10 > 7, goes to line 3: "four" (4 chars)

        assert_eq!(buf.lines.len(), 3, "Buffer should be full at max_lines=3");
        assert_eq!(buf.lines[0].text, "one two");
        assert_eq!(buf.lines[1].text, "three");
        assert_eq!(buf.lines[2].text, "four");

        // Now buffer is full and all 3 lines exist. Push continuation on last line that overflows.
        // Current line 3: "four" (4 chars). Continuation "more" (4 chars).
        // Combined: "fourmore" (8 chars) > 7. No space in "four", so the whole line moves.
        // add_new_line will remove line 0 and add new line, resulting in:
        // ["three", "four", "fourmore"]
        buf.push("more".to_string());

        // Verify: no empty lines and correct content.
        assert_eq!(buf.lines.len(), 3, "Should still have max_lines=3");
        assert_eq!(buf.lines[0].text, "one two", "Line 1 unchanged");
        assert_eq!(buf.lines[1].text, "three", "Line 2 unchanged");
        assert_eq!(buf.lines[2].text, "fourmore", "Line 3 has joined word replacing old 'four'");

        let display = buf.display_text();
        assert!(display.contains("one two"), "Should contain 'one two'");
        assert!(display.contains("three"), "Should contain 'three'");
        assert!(display.contains("fourmore"), "Should contain 'fourmore'");
        assert_eq!(display.lines().count(), 3, "Display should have 3 lines");
    }

    /// AC1.5 extended: "with space" continuation overflow branch.
    /// When last line has multiple words and continuation overflows, the partial word
    /// after the last space moves to next line and joins the continuation.
    #[test]
    fn ac1_5_continuation_with_space_overflow() {
        let mut buf = CaptionBuffer::new(3, 20, 8);

        // Set up line 1: "Hello world" (11 chars, fits in 20)
        buf.push(" Hello".to_string());
        buf.push(" world".to_string());
        assert_eq!(buf.lines[0].text, "Hello world");

        // Current line: "Hello world" (11 chars). Push continuation "ly" (2 chars).
        // Combined: "world" + "ly" = 7 chars, fits in 20. ✓
        buf.push("ly".to_string());
        assert_eq!(buf.lines[0].text, "Hello worldly");

        // Now make line nearly full and overflow. Reset for clearer setup.
        buf = CaptionBuffer::new(3, 18, 8);
        buf.push(" Hello".to_string());         // Line 1: "Hello" (5 chars)
        buf.push(" world".to_string());         // Line 1: "Hello world" (11 chars)

        // Current line: "Hello world" (11 chars). Push continuation "ly" (2 chars) that fits.
        buf.push("ly".to_string());             // Line 1: "Hello worldly" (13 chars)

        // Now push word that forces split. Current line: "Hello worldly" (13 chars).
        // Word " test" (5 chars): 13 + 1 + 5 = 19 > 18, doesn't fit.
        // Goes to line 2.
        buf.push(" test".to_string());          // Line 2: "test" (4 chars)

        // Current line 2: "test" (4 chars). Push continuation that overflows.
        // "test" + "something" = 13 chars > 18? No, 13 < 18, fits. Let's use longer continuation.
        // "test" + "ingsomething" = 16 chars, fits in 18. Hmm, still fits.
        // Let's be more aggressive: use continuation that definitely overflows.
        // "test" + "verylongcontinuation" = too long.
        buf.push("verylongcontinuation".to_string()); // "test" + "verylongcontinuation" = 24 > 18

        // This overflows. Line 2 is "test" (no space). Last space in "test"? None.
        // So the "no space" branch triggers, which just moves entire line to new line.
        // That's not the "with space" branch.

        // Let's retest more carefully to exercise "with space" branch:
        buf = CaptionBuffer::new(3, 18, 8);
        buf.push(" Hello".to_string());         // Line 1: "Hello" (5 chars)
        buf.push(" world".to_string());         // Line 1: "Hello world" (11 chars)
        buf.push(" more".to_string());          // Line 1: "Hello world more" (16 chars, fits)

        // Current line 1: "Hello world more" (16 chars, 2 chars left before max).
        // Push continuation "text" (4 chars).
        // "more" + "text" = 8 chars. 16 + 8 = 24 > 18. Overflow!
        // Last space in "Hello world more"? Yes, at position 11 (after "world").
        // Split: keep "Hello world " (12 chars), move "more" to next line.
        // New line: "moretext" (8 chars).
        buf.push("text".to_string());

        let display = buf.display_text();
        let lines: Vec<&str> = display.split('\n').collect();
        assert_eq!(lines.len(), 2, "Should have 2 lines after split");
        assert_eq!(lines[0], "Hello world", "First line should be trimmed to 'Hello world'");
        assert_eq!(lines[1], "moretext", "Second line should have partial word + continuation joined");
    }

    /// AC1.6: RNNT decoder overlap is deduplicated (4+ char matches).
    #[test]
    fn ac1_6_overlap_deduplication() {
        let mut buf = CaptionBuffer::new(3, 50, 8);

        buf.push(" The quick brown".to_string());
        // Simulating RNNT decoder re-emitting "brown fox" where "brown" already output.
        buf.push(" brown fox".to_string());

        let display = buf.display_text();
        assert_eq!(display, "The quick brown fox", "Overlap should be deduplicated");
        assert!(!display.contains("brownbrown"), "Should not duplicate 'brown'");
    }

    /// AC2.1: When no new text arrives for expire_secs, the oldest (top) line is removed
    /// and remaining lines shift up.
    #[test]
    fn ac2_1_oldest_line_expires() {
        let mut buf = CaptionBuffer::new(2, 7, 1); // expire_secs = 1, max_chars = 7

        buf.push(" line1".to_string()); // Creates line 1: "line1" (5 chars)
        buf.push(" line2".to_string()); // "line1 line2" = 11 chars > 7, so creates line 2: "line2" (5 chars)

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines");

        // Manually expire the oldest line by setting its timestamp to the past.
        let now = Instant::now();
        if !buf.lines.is_empty() {
            buf.lines[0].last_active = now - std::time::Duration::from_secs(2);
        }

        let expired = buf.expire();
        assert!(expired, "expire() should return true when a line is removed");

        let display = buf.display_text();
        assert_eq!(display, "line2", "Oldest line should be removed");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after expiry");
    }

    /// AC2.2: Expiry continues once per second until all lines are cleared during silence.
    #[test]
    fn ac2_2_expiry_gradual_drain() {
        let mut buf = CaptionBuffer::new(3, 5, 1); // max_chars = 5 to force separate lines

        buf.push(" one".to_string());   // Line 1: "one" (3 chars)
        buf.push(" two".to_string());   // Won't fit on line 1 (3+1+3=7 > 5), goes to line 2: "two" (3 chars)
        buf.push(" three".to_string()); // Won't fit on line 2 (3+1+5=9 > 5), goes to line 3: "three" (5 chars)

        assert_eq!(buf.lines.len(), 3, "Should have 3 separate lines");

        // Set all lines to expired state.
        let now = Instant::now();
        let expired_time = now - std::time::Duration::from_secs(2);
        for line in &mut buf.lines {
            line.last_active = expired_time;
        }

        // First expire call should remove one line.
        assert!(buf.expire(), "First expire should remove a line");
        assert_eq!(buf.lines.len(), 2, "Should have 2 lines after first expire");

        // Second expire call should remove another line.
        assert!(buf.expire(), "Second expire should remove another line");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after second expire");

        // Third expire call should remove the last line.
        assert!(buf.expire(), "Third expire should remove the last line");
        assert_eq!(buf.lines.len(), 0, "Should have 0 lines after third expire");

        // Fourth expire call should return false (no lines to expire).
        assert!(!buf.expire(), "expire() should return false when buffer is empty");
    }

    /// AC2.3: Active lines (receiving new text) do not expire — last_active resets on each push.
    #[test]
    fn ac2_3_active_lines_dont_expire() {
        let now = Instant::now();
        let mut buf = CaptionBuffer::new(2, 20, 1);

        // Manually construct two lines: one expired and one active.
        buf.lines.push(CaptionLine {
            text: "old_content".to_string(),
            last_active: now - std::time::Duration::from_secs(2),
        });
        buf.lines.push(CaptionLine {
            text: "recent_content".to_string(),
            last_active: Instant::now(),
        });

        assert_eq!(buf.lines.len(), 2, "Should have 2 lines");

        // Expire should only remove the first (expired) line.
        assert!(buf.expire(), "Should remove the expired first line");
        assert_eq!(buf.lines.len(), 1, "Should have 1 line after expiry");
        assert_eq!(buf.lines[0].text, "recent_content");

        // The remaining line should have recent last_active and not expire on next call.
        assert!(!buf.expire(), "Active line should not expire");
    }

    /// AC4.1: estimate_max_chars applies 0.85× conservative multiplier for visual padding.
    #[test]
    fn ac4_1_conservative_multiplier() {
        let width_px = 800;
        let font_size_pt = 24.0;

        // Old formula: (usable_width / avg_char_width)
        // usable_width = (800 - 24).max(100) = 776
        // avg_char_width = 24.0 * 0.6 = 14.4
        // old_result = 776 / 14.4 = 53.88... → floor = 53
        // new_result = 53 * 0.85 = 45.05 → floor = 45
        let result = estimate_max_chars(width_px, font_size_pt);
        let expected_old = ((776.0_f32 / 14.4).floor()) as i32; // 53
        let expected_new = ((776.0_f32 / 14.4 * 0.85).floor()) as i32; // 45

        assert_eq!(expected_old, 53, "Sanity check: old formula should give 53");
        assert_eq!(expected_new, 45, "Sanity check: new formula should give 45");
        assert_eq!(result, 45, "Result should be approximately 85% of old formula");
        assert!(
            result < expected_old,
            "Conservative multiplier should make result smaller than old formula"
        );
    }
}
