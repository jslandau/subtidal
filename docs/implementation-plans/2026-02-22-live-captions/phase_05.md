# Live Captions Implementation Plan — Phase 5: Overlay Window

**Goal:** Display captions in a GTK4 overlay window with docked (wlr-layer-shell, anchored to screen edge) and floating (freely positioned) modes. Click-through when locked. Drag-to-reposition when unlocked.

**Architecture:** `src/overlay/mod.rs` creates the GTK4 Application and Window. Both modes use `gtk4-layer-shell` (`Layer::Top`) — this is the only reliable way to appear above all windows on KDE Plasma 6 / Wayland without compositor-specific extensions. A `glib::MainContext` channel receives caption strings from the inference thread and calls `label.set_text()` on the GTK main thread. Click-through uses empty `wl_surface` input region set after the `map` signal.

**Tech Stack:** gtk4 0.10, gtk4-layer-shell 0.7, cairo-rs (transitive via gtk4).

**Scope:** Phase 5 of 8. Depends on Phase 1 (Config, AppearanceConfig).

**Codebase verified:** 2026-02-22 — greenfield, no src/overlay/ exists.

---

## Acceptance Criteria Coverage

### live-captions.AC3: Overlay displays captions correctly
- **live-captions.AC3.1 Success:** Overlay appears above all other windows in docked mode (anchored to configured edge)
- **live-captions.AC3.2 Success:** Overlay appears above all other windows in floating mode
- **live-captions.AC3.3 Success:** Docked mode passes all pointer and keyboard events through to windows below
- **live-captions.AC3.4 Success:** Floating mode in locked state passes all pointer events through
- **live-captions.AC3.5 Success:** Floating mode in unlocked state can be dragged to any screen position; position persists across restart
- **live-captions.AC3.6 Success:** Switching between docked and floating mode at runtime works without restart
- **live-captions.AC3.7 Success:** Caption text respects configured font size, text color, and background color/transparency
- **live-captions.AC3.8 Failure:** Toggling captions off hides the overlay entirely

---

**Additional Cargo.toml dependencies for Phase 5:**

No new Cargo.toml entries are needed for Phase 5. `gtk4`, `gtk4-layer-shell`, and `cairo-rs` (transitive via `gtk4`) are already declared in Phase 1.

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Create src/overlay/mod.rs — OverlayState and GTK4 Application setup

**Files:**
- Create: `src/overlay/mod.rs`
- Create: `src/overlay/` directory

**Step 1: Create src/overlay/mod.rs**

```rust
//! GTK4 overlay window: docked (wlr-layer-shell) and floating modes with caption display.

use crate::config::{AppearanceConfig, Config, OverlayMode, ScreenEdge};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use glib::MainContext;
use std::sync::{Arc, atomic::{AtomicBool, AtomicI32, Ordering}};

pub mod input_region;

/// Commands sent to the overlay from the tray / main integration.
#[derive(Debug)]
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
/// - `caption_rx`: glib channel receiver delivering caption strings from inference thread
/// - `cmd_rx`: glib channel receiver delivering OverlayCommand from tray
/// - `captions_enabled`: shared bool for left-click tray toggle
pub fn run_gtk_app(
    config: Config,
    caption_rx: glib::Receiver<String>,
    cmd_rx: glib::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
) {
    let app = Application::builder()
        .application_id("com.example.live-captions")
        .build();

    let config = Arc::new(std::sync::Mutex::new(config));
    let config_clone = Arc::clone(&config);
    let captions_enabled_clone = Arc::clone(&captions_enabled);

    app.connect_activate(move |app| {
        let cfg = config_clone.lock().unwrap().clone();
        let window = build_overlay_window(app, &cfg);

        // Apply initial appearance.
        apply_appearance(&window, &cfg.appearance);

        // Wire up caption receiver.
        let label = find_caption_label(&window);
        let window_clone = window.clone();
        let enabled = Arc::clone(&captions_enabled_clone);
        caption_rx.attach(None, move |text| {
            if enabled.load(Ordering::Relaxed) {
                label.set_text(&text);
                window_clone.set_visible(true);
            }
            glib::ControlFlow::Continue
        });

        // Wire up command receiver.
        let window_clone2 = window.clone();
        let config_for_cmd = Arc::clone(&config_clone);
        cmd_rx.attach(None, move |cmd| {
            handle_overlay_command(&window_clone2, cmd, &config_for_cmd);
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
        .title("live-captions")
        .build();

    // Initialize layer shell.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_exclusive_zone(0); // don't push other windows aside

    match cfg.overlay_mode {
        OverlayMode::Docked => configure_docked(&window, &cfg.screen_edge),
        OverlayMode::Floating => configure_floating(&window, cfg),
    }

    // Build caption label.
    let label = Label::builder()
        .label("")
        .wrap(true)
        .max_width_chars(80)
        .build();
    label.set_widget_name("caption-label");
    window.set_child(Some(&label));

    // Set click-through after window maps.
    let is_locked = cfg.locked || cfg.overlay_mode == OverlayMode::Docked;
    window.connect_map(move |win| {
        if is_locked {
            input_region::set_empty_input_region(win);
        }
    });

    // Drag handle for floating + unlocked.
    if cfg.overlay_mode == OverlayMode::Floating && !cfg.locked {
        add_drag_handler(&window);
    }

    window
}

fn configure_docked(window: &ApplicationWindow, edge: &ScreenEdge) {
    // Anchor to the selected screen edge and stretch horizontally/vertically as appropriate.
    let (anchor_edge, stretch_edges) = match edge {
        ScreenEdge::Bottom => (Edge::Bottom, vec![Edge::Left, Edge::Right]),
        ScreenEdge::Top    => (Edge::Top,    vec![Edge::Left, Edge::Right]),
        ScreenEdge::Left   => (Edge::Left,   vec![Edge::Top, Edge::Bottom]),
        ScreenEdge::Right  => (Edge::Right,  vec![Edge::Top, Edge::Bottom]),
    };
    window.set_anchor(anchor_edge, true);
    for e in stretch_edges {
        window.set_anchor(e, true);
    }
    // Keyboard and pointer click-through: handled by keyboard_mode + empty input region.
    window.set_keyboard_mode(KeyboardMode::None);
}

fn configure_floating(window: &ApplicationWindow, cfg: &Config) {
    // Floating: no anchors (window positioned freely).
    // gtk4-layer-shell with no anchors centres the surface; we then move it.
    window.set_keyboard_mode(if cfg.locked {
        KeyboardMode::None
    } else {
        KeyboardMode::OnDemand
    });

    // Position the window after it realizes.
    let pos = cfg.position.clone();
    window.connect_realize(move |win| {
        // Note: direct position setting on layer-shell surfaces is not standard.
        // For Wayland, the compositor controls position for layer-shell surfaces.
        // The "floating" appearance can be achieved by using margins:
        win.set_margin(Edge::Left, pos.x);
        win.set_margin(Edge::Top, pos.y);
    });
}

/// Set CSS on the caption label and window to reflect appearance config.
pub fn apply_appearance(window: &ApplicationWindow, appearance: &AppearanceConfig) {
    let css = format!(
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
    );

    let provider = gtk4::CssProvider::new();
    provider.load_from_string(&css);

    gtk4::style_context_add_provider_for_display(
        // gdk4 is re-exported by gtk4: use gtk4::gdk as gdk4.
        // If rustc cannot resolve `gdk4::`, add `gdk4 = "0.10"` to Cargo.toml.
        &gdk4::Display::default().expect("no GDK display"),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn find_caption_label(window: &ApplicationWindow) -> Label {
    window
        .child()
        .and_downcast::<Label>()
        .expect("caption label not found")
}

fn handle_overlay_command(
    window: &ApplicationWindow,
    cmd: OverlayCommand,
    config: &Arc<std::sync::Mutex<Config>>,
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
                    configure_docked(window, &cfg.screen_edge);
                    // Docked mode is always click-through.
                    input_region::set_empty_input_region(window);
                }
                OverlayMode::Floating => {
                    // Clear all anchors (layer-shell will centre the surface).
                    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
                        window.set_anchor(edge, false);
                    }
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
                        add_drag_handler(window);
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
                add_drag_handler(window);
            }
        }
        OverlayCommand::UpdateAppearance(appearance) => {
            apply_appearance(window, &appearance);
        }
        OverlayCommand::SetCaption(text) => {
            if let Some(label) = window.child().and_downcast::<Label>() {
                label.set_text(&text);
            }
        }
        OverlayCommand::Quit => {
            // Quit the GTK4 application cleanly so all cleanup (Drop impls) runs.
            if let Some(app) = window.application() {
                app.quit();
            }
        }
    }
}

fn add_drag_handler(window: &ApplicationWindow) {
    // For gtk4-layer-shell floating windows, position is controlled by margins
    // (not compositor-managed coordinates). We use GestureDrag to track delta
    // and update set_margin() on each drag update.
    //
    // Note: begin_move_drag() is a GTK3 API that does not exist in GTK4.
    // On Wayland with layer-shell, the compositor positions the surface via margins.
    let gesture = gtk4::GestureDrag::new();

    // Capture starting margins when drag begins.
    let start_x = Arc::new(AtomicI32::new(0));
    let start_y = Arc::new(AtomicI32::new(0));
    let sx = Arc::clone(&start_x);
    let sy = Arc::clone(&start_y);
    let win_begin = window.clone();
    gesture.connect_drag_begin(move |_, _, _| {
        sx.store(win_begin.margin(Edge::Left), std::sync::atomic::Ordering::Relaxed);
        sy.store(win_begin.margin(Edge::Top), std::sync::atomic::Ordering::Relaxed);
    });

    // Update margins on each drag update.
    let sx2 = Arc::clone(&start_x);
    let sy2 = Arc::clone(&start_y);
    let win_update = window.clone();
    gesture.connect_drag_update(move |_, dx, dy| {
        let new_x = sx2.load(std::sync::atomic::Ordering::Relaxed) + dx as i32;
        let new_y = sy2.load(std::sync::atomic::Ordering::Relaxed) + dy as i32;
        win_update.set_margin(Edge::Left, new_x.max(0));
        win_update.set_margin(Edge::Top, new_y.max(0));
    });

    window.add_controller(gesture);
}
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Create src/overlay/input_region.rs — Wayland click-through

**Files:**
- Create: `src/overlay/input_region.rs`

**Step 1: Create src/overlay/input_region.rs**

```rust
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
use gtk4_layer_shell::{KeyboardMode, LayerShell};

/// Make the window click-through: set an empty GDK surface input region.
///
/// Uses `gdk4::Surface::set_input_region()` with an empty `cairo::Region`.
/// Must be called after the window has been mapped (via `connect_map` signal).
///
/// Also sets `KeyboardMode::None` via gtk4-layer-shell to prevent focus stealing.
pub fn set_empty_input_region(window: &ApplicationWindow) {
    use gdk4::prelude::SurfaceExt;

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
    use gdk4::prelude::SurfaceExt;

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
```

**Note on `cairo` dependency:** `cairo-rs` is a transitive dependency of `gtk4`. No additional Cargo.toml entry is needed — `cairo` is re-exported through gdk4's dependency chain and is usable as `cairo::Region`. If rustc cannot resolve `cairo::`, add `cairo-rs = "0.19"` to Cargo.toml explicitly.

**Note on KDE Plasma 6 / gtk4-layer-shell:** In practice, `set_keyboard_mode(KeyboardMode::None)` (set in `configure_docked`) is sufficient for keyboard pass-through. The `set_input_region` call with an empty cairo region handles pointer pass-through. Both are needed for AC3.3 and AC3.4.

**Step 2: Verify compilation**

```bash
cargo check
```

Expected: No errors.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Wire overlay into main.rs via GTK4 Application

**Files:**
- Modify: `src/main.rs`

**Verifies:** live-captions.AC3.1, live-captions.AC3.3, live-captions.AC3.7, live-captions.AC3.8

**Step 1: Add mod overlay to src/main.rs**

At the top: `mod overlay;`

**Step 2: Replace the main.rs test consumer with the GTK4 application**

Remove the test consumer thread from Phase 4. Replace with:

```rust
// Phase 5: Set up glib channels for caption and command delivery.
let (glib_caption_tx, glib_caption_rx) = MainContext::channel::<String>(glib::PRIORITY_DEFAULT);
let (glib_cmd_tx, glib_cmd_rx) = MainContext::channel::<overlay::OverlayCommand>(glib::PRIORITY_DEFAULT);

// Bridge: forward inference thread captions to the glib channel.
let caption_rx_from_inference = caption_rx; // from Phase 4 spawn_inference_thread
std::thread::spawn(move || {
    for caption in caption_rx_from_inference.iter() {
        if glib_caption_tx.send(caption).is_err() {
            break;
        }
    }
});

// Shared captions-enabled flag (also used by tray in Phase 6).
let captions_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

// Store glib_cmd_tx for use by tray (Phase 6).
// For Phase 5, wire directly.
let cuda_warning = cuda_fallback_warning; // from Phase 4

// Run GTK4 main loop (blocks until application exits).
overlay::run_gtk_app(cfg, glib_caption_rx, glib_cmd_rx, Arc::clone(&captions_enabled));
```

**Step 3: Build and run**

```bash
cargo build
cargo run
```

Expected: A semi-transparent black overlay appears at the bottom of the screen. Caption text updates as speech is detected.

**Step 4: Test AC3.8 — toggle off**

Send `OverlayCommand::SetVisible(false)` from main thread before starting GTK to verify window hides:

```bash
# In a terminal, while app is running, this is verified manually:
# The glib_cmd_tx.send(OverlayCommand::SetVisible(false)) hides the window.
# This will be wired to the tray left-click in Phase 6.
```

**Step 5: Test AC3.7 — appearance**

Edit `~/.config/live-captions/config.toml`, change `background_color` to `"rgba(0,0,255,0.5)"`:

Expected: Blue background after restart (hot-reload in Phase 7).

**Step 6: Commit**

```bash
git add src/overlay/ src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: GTK4 overlay window — docked layer-shell, floating, click-through"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_4 -->
### Task 4: Verify AC3.5 — floating mode drag and position persistence

**Files:**
- Modify: `src/overlay/mod.rs` (extend configure_floating to save drag position)

**Verifies:** live-captions.AC3.5, live-captions.AC3.6

**Step 1: Add position-save callback after drag release**

In `add_drag_handler`, after `begin_move_drag` completes, save the new window position to config. GTK4 on Wayland doesn't expose window position directly from the xdg_toplevel; with gtk4-layer-shell margins we can read them back:

```rust
// After drag: detect window position change via margin query.
// gtk4-layer-shell provides get_margin(Edge) to read current margins.
let win_for_release = window.clone();
// Store position on drag end (connect_drag_end is the correct GestureDrag signal).
gesture.connect_drag_end(move |_, _offset_x, _offset_y| {
    let x = win_for_release.margin(Edge::Left);
    let y = win_for_release.margin(Edge::Top);
    eprintln!("info: overlay dragged to ({x}, {y}) — save to config in Phase 7");
    // Phase 7 wires this to Config::save().
});
```

**Step 2: Test mode switching (AC3.6)**

Send `OverlayCommand::SetMode(OverlayMode::Floating)` while in docked mode (in Phase 8 this is wired to a full window teardown/recreate):

```bash
# Manual test: change config.toml overlay_mode = "floating" and restart.
# Expected: window appears in floating position (no edge anchoring).
cargo run
```

**Step 3: Commit**

```bash
git add src/overlay/
git commit -m "feat: overlay drag handler and position persistence stub"
```
<!-- END_TASK_4 -->
