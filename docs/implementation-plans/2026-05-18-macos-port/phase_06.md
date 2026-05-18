# macOS Port — Phase 6: Tray + full overlay modes

**Goal:** Implement the NSStatusItem tray and complete all three overlay modes (Docked / Floating / Transcript) with drag persistence, above-fullscreen toggle, lock-position, captions-disable 4-surface clear, and Transcript Save dialog. After this phase, Subtidal on macOS has feature parity with Linux at the user-visible level.

**Architecture:** `tray::impl_macos::install_tray(state, mtm) -> Retained<NSStatusItem>` builds the menu bar item with a template icon and the full NSMenu (Captions / Mode / Engine / Audio Source / Show Above Fullscreen / Lock Position / Quit). Menu actions use the same `OverlayCommand` / `AudioCommand` / `ArcSwap<Engine>` channels Linux uses. `overlay/macos/panel.rs` gains `apply_geometry(panel, mtm, mode, config)` to reconfigure the same `Retained<NSPanel>` for Docked vs Floating (no rebuild). `overlay/macos/drag.rs` observes `NSWindowDidMoveNotification` and persists `frame.origin` via the existing hot-reload-safe config write path. `overlay/macos/transcript_window.rs` builds a regular `NSWindow` with `NSScrollView` + `NSTextView`, autoscroll-when-at-bottom, and `NSSavePanel` for `TranscriptLog::to_json` export. The Phase 2 caption-bridge is upgraded to route captions through `CaptionBuffer` (display) and `TranscriptLog` (history), enabling the 4-surface clear on captions-disable.

**Tech Stack:** `objc2-app-kit` (`NSStatusItem`, `NSStatusBar`, `NSMenu`, `NSMenuItem`, `NSScrollView`, `NSTextView`, `NSTextStorage`, `NSSavePanel`, `NSImage`, `NSControl`, `NSCell`, `NSBundle`, `NSWorkspace`), `objc2-foundation` (`NSURL`, `NSNotificationCenter`), existing neutral `CaptionBuffer` + `TranscriptLog`, `chrono` for timestamps.

**Scope:** Phase 6 of 8.

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

### macos-port.AC1: Three overlay modes function on macOS
- **macos-port.AC1.1 Success:** Selecting Docked from the tray menu positions an NSPanel at the top of `NSScreen.main.visibleFrame` spanning the full screen width.
- **macos-port.AC1.2 Success:** Selecting Floating from the tray menu shows an NSPanel at the position recorded in `config.toml` (or a sensible default for first run), draggable via click-and-drag on the panel background.
- **macos-port.AC1.3 Success:** Selecting Transcript from the tray menu shows a regular NSWindow with a scrollable NSTextView; captions append as timestamped paragraphs and the view autoscrolls to the bottom when the user is at the bottom.
- **macos-port.AC1.4 Success:** Switching modes via the tray is instant and does not require restart; both windows are constructed once at startup and visibility-toggled.
- **macos-port.AC1.5 Failure:** Switching to Transcript while no captions have been received produces an empty (not crashed) NSTextView; the Save dialog still functions and produces a valid (possibly empty-of-fragments) JSON file.
- **macos-port.AC1.6 Edge:** Resizing the screen (external display connect/disconnect) while in Docked mode re-positions the panel to the new `NSScreen.main.visibleFrame`.

### macos-port.AC2: NSPanel renders correctly across Spaces and fullscreen
- **macos-port.AC2.2 Success:** Panel is visible on every Space the user switches to (Mission Control verified).
- **macos-port.AC2.3 Success:** Panel is visible above another application's fullscreen window (Safari/Chrome in fullscreen confirmed).
- **macos-port.AC2.5 Success:** Caption modes (Docked, Floating) have `ignoresMouseEvents = true`; clicks pass through to the window below.
- **macos-port.AC2.6 Failure:** Transcript mode has `ignoresMouseEvents = false`; clicks land on the window and Save button works.
- **macos-port.AC2.7 Edge:** Panel does not collide with the menu bar (positioned via `visibleFrame`, not `frame`).

### macos-port.AC5: Tray (NSStatusItem) controls
- **macos-port.AC5.1 Success:** Tray icon appears in the menu bar with `isTemplate = true`; renders correctly in light and dark mode without code branching.
- **macos-port.AC5.2 Success:** "Captions On/Off" menu item toggles the `CaptionsEnabled` shared state; the checkmark reflects the current state.
- **macos-port.AC5.3 Success:** "Mode" submenu lists Docked, Floating, Transcript; the active mode is checkmarked; selection posts `OverlayCommand::SetMode`.
- **macos-port.AC5.4 Success:** "Audio Source" submenu is populated dynamically from `list_sources()` and lists System Output plus running apps that produce audio (identified by bundle ID).
- **macos-port.AC5.5 Success:** "Show Above Fullscreen" toggles `config.above_fullscreen` and posts `OverlayCommand::SetAboveFullscreen` live (no rebuild).
- **macos-port.AC5.6 Success:** "Lock Position" toggle is only enabled in Floating mode; locks the panel against drag.
- **macos-port.AC5.7 Success:** "Quit Subtidal" (Cmd-Q) terminates the application cleanly via `applicationWillTerminate`, signaling all worker threads to shut down.
- **macos-port.AC5.8 Failure:** Tray menu items reflect disabled state when the underlying feature is unavailable (e.g., "Lock Position" is disabled in Docked/Transcript modes).

### macos-port.AC6: Hot-reload config
- **macos-port.AC6.1 Success:** Editing `~/Library/Application Support/Subtidal/config.toml` and saving triggers a debounced reload; changed values apply within ~500ms.
- **macos-port.AC6.2 Success:** Drag-induced position writes to config do not trigger a config-reload feedback loop (existing change-detection invariant preserved).
- **macos-port.AC6.3 Failure:** Malformed TOML in `config.toml` is warned to stderr and ignored; the application continues with previous config.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Extend objc2-app-kit features + add 22×22 tray icon

**Files:**
- Modify: `Cargo.toml` macOS dep block
- Create: `resources/macos/tray-icon-template.png` (22×22 monochrome black+alpha PNG)
- Modify: `scripts/bundle-mac.sh` — copy the icon into `Subtidal.app/Contents/Resources/`

**Implementation:**

Extend the macOS dep block from Phase 2:

```toml
objc2-app-kit = { version = "0.3", features = [
    "NSApplication", "NSPanel", "NSWindow", "NSTextField", "NSScreen",
    "NSColor", "NSFont", "NSAttributedString", "NSEvent",
    "NSResponder", "NSView", "NSAppearance",
    # Phase 6 additions:
    "NSMenu", "NSMenuItem", "NSStatusBar", "NSStatusItem",
    "NSScrollView", "NSTextView", "NSTextStorage",
    "NSSavePanel", "NSImage", "NSControl", "NSCell",
    "NSBundle", "NSWorkspace",
] }
objc2-foundation = { version = "0.3", features = [
    "NSString", "NSNotification", "NSObject", "NSRunLoop",
    "NSURL",
] }
```

Produce the tray icon. Two paths; pick whichever produces a clean 22×22 monochrome with alpha.

```bash
mkdir -p resources/macos
# Path A (Linux/macOS, requires librsvg + ImageMagick):
rsvg-convert -w 22 -h 22 assets/icons/hicolor/scalable/apps/subtidal.svg /tmp/raw.png
magick /tmp/raw.png -alpha extract -threshold 50% +channel \
       -fill black -opaque white -transparent white \
       resources/macos/tray-icon-template.png

# Path B (macOS, sips + ImageMagick):
sips -s format png -z 22 22 assets/icons/hicolor/scalable/apps/subtidal.svg --out /tmp/raw.png
magick /tmp/raw.png -alpha extract -threshold 50% +channel \
       -fill black -opaque white -transparent white \
       resources/macos/tray-icon-template.png
```

Verify:
```bash
file resources/macos/tray-icon-template.png
# Expected: PNG image data, 22 x 22, 8-bit gray+alpha, non-interlaced
```

If the auto-conversion produces an unreadable icon, hand-design a 22×22 template by hand (acceptable; the design notes the existing icon's alpha channel may be visually adequate).

Extend `scripts/bundle-mac.sh` to copy the icon after the Info.plist copy:

```bash
cp resources/macos/tray-icon-template.png "$APP/Contents/Resources/"
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
scripts/bundle-mac.sh
ls target/release/Subtidal.app/Contents/Resources/tray-icon-template.png
```

**Commit:** `macos: tray icon template + extended objc2-app-kit features`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: NSStatusItem + full NSMenu construction

**Verifies:** macos-port.AC5.1, macos-port.AC5.2, macos-port.AC5.3, macos-port.AC5.4, macos-port.AC5.5, macos-port.AC5.6, macos-port.AC5.7, macos-port.AC5.8

**Files:**
- Modify: `src/tray/impl_macos.rs` — replace Phase 1 stub with full NSStatusItem + NSMenu

**Implementation:**

Read `src/tray/impl_linux.rs` end-to-end as the structural reference (state shape, menu hierarchy, callback wiring). macOS uses NSMenu instead of ksni::Menu and runs on the main thread (no separate tray thread — the NSStatusItem lives on the main run loop).

**Public surface (mirrors Linux `spawn_tray` semantically):**

```text
pub struct TrayState {
    pub config: Arc<Mutex<Config>>,
    pub captions_enabled: CaptionsEnabled,
    pub cmd_tx: async_channel::Sender<OverlayCommand>,
    pub audio_cmd_tx: SyncSender<AudioCommand>,
    pub engine_choice: Arc<ArcSwap<Engine>>,
    pub audio_sources: Arc<Mutex<Vec<AudioSourceInfo>>>,
}

/// Construct the NSStatusItem + NSMenu on the main thread.
/// Returns the Retained<NSStatusItem> for the caller to keep alive.
pub fn install_tray(state: TrayState, mtm: MainThreadMarker) -> Retained<NSStatusItem>;
```

**Inside `install_tray`:**

1. **NSStatusItem:**
   ```text
   let bar = NSStatusBar::systemStatusBar();
   let item = bar.statusItemWithLength(NSSquareStatusItemLength);
   ```

2. **Template icon from bundle:**
   ```text
   let bundle = NSBundle::mainBundle();
   let path = bundle
       .pathForResource_ofType(Some(&NSString::from_str("tray-icon-template")),
                               Some(&NSString::from_str("png")))
       .expect("tray-icon-template.png missing from bundle Resources");
   let image = NSImage::initWithContentsOfFile(&path);
   image.setTemplate(true);  // auto light/dark coloring
   item.button(mtm).expect("button").setImage(Some(&image));
   ```

3. **Build NSMenu** (mirror Linux structure):

   ```text
   let menu = NSMenu::new();
   menu.setAutoenablesItems(false);   // explicit enable/disable per AC5.8

   // Captions On/Off
   let captions = NSMenuItem::new_with_title_action_keyEquivalent(
       "Captions", Some(sel!(toggleCaptions:)), "");
   captions.setState(if state.captions_enabled.load(Relaxed) { 1 } else { 0 });
   menu.addItem(&captions);

   // Mode submenu
   let mode_item = NSMenuItem::new_with_title("Mode");
   let mode_menu = NSMenu::new();
   let current_mode = state.config.lock().overlay_mode.clone();
   for (label, mode_kind) in [
       ("Docked", OverlayMode::Docked),
       ("Floating", OverlayMode::Floating),
       ("Transcript", OverlayMode::Transcript),
   ] {
       let mi = NSMenuItem::new_with_title_action_keyEquivalent(label, Some(sel!(selectMode:)), "");
       mi.setRepresentedObject(/* boxed enum kind, retrievable in the handler */);
       mi.setState(if current_mode == mode_kind { 1 } else { 0 });
       mode_menu.addItem(&mi);
   }
   mode_item.setSubmenu(Some(&mode_menu));
   menu.addItem(&mode_item);

   // Engine submenu (single item, matches Linux)
   let engine_item = NSMenuItem::new_with_title("Engine");
   let engine_menu = NSMenu::new();
   let nemo = NSMenuItem::new_with_title("Nemotron");
   nemo.setState(1);
   engine_menu.addItem(&nemo);
   engine_item.setSubmenu(Some(&engine_menu));
   menu.addItem(&engine_item);

   // Audio Source submenu (dynamic; refreshed via NSTimer below)
   let audio_item = NSMenuItem::new_with_title("Audio Source");
   audio_item.setSubmenu(Some(&build_audio_submenu(&state, mtm)));
   menu.addItem(&audio_item);

   // Show Above Fullscreen
   let above = NSMenuItem::new_with_title_action_keyEquivalent(
       "Show Above Fullscreen", Some(sel!(toggleAboveFullscreen:)), "");
   above.setState(if state.config.lock().above_fullscreen { 1 } else { 0 });
   menu.addItem(&above);

   // Lock Position (Floating-only)
   let lock = NSMenuItem::new_with_title_action_keyEquivalent(
       "Lock Position", Some(sel!(toggleLock:)), "");
   lock.setState(if state.config.lock().locked { 1 } else { 0 });
   lock.setEnabled(matches!(current_mode, OverlayMode::Floating));   // AC5.6, AC5.8
   menu.addItem(&lock);

   menu.addItem(&NSMenuItem::separator());

   // Quit
   let quit = NSMenuItem::new_with_title_action_keyEquivalent(
       "Quit Subtidal", Some(sel!(quit:)), "q");
   menu.addItem(&quit);

   item.setMenu(Some(&menu));
   ```

4. **Action target class** — define `TrayActions: NSObject` via `objc2::define_class!`, store `TrayState` as an ivar, implement each `@objc fn` handler (`toggleCaptions:`, `selectMode:`, `selectAudioSource:`, `toggleAboveFullscreen:`, `toggleLock:`, `quit:`). Each handler posts the appropriate `OverlayCommand` and updates the menu item's `state` (checkmark).

5. **AC5.5 (live above-fullscreen):** the handler:
   ```text
   {
       let mut cfg = state.config.lock();
       cfg.above_fullscreen = !cfg.above_fullscreen;
       /* Config::save via existing hot-reload-safe write path */
       state.cmd_tx.send_blocking(OverlayCommand::SetAboveFullscreen(cfg.above_fullscreen)).ok();
       above.setState(if cfg.above_fullscreen { 1 } else { 0 });
   }
   ```

6. **AC5.4 (dynamic audio sources):** install an `NSTimer` firing every 5s that re-calls `audio::list_sources()` and rebuilds the submenu items if the list changed:
   ```text
   NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
       5.0, &actions, sel!(refreshAudioSources:), None, true);
   ```

7. **AC5.6/AC5.8 (Lock-Position only when Floating):** the `selectMode:` handler also calls `lock.setEnabled(matches!(new_mode, OverlayMode::Floating))`.

8. **AC5.7 (Quit):** the `quit:` handler posts `OverlayCommand::Quit`. The Phase 2 dispatch loop already handles `Quit` by calling `NSApplication.terminate(None)`. The `applicationWillTerminate` path is honored by terminate's default flow.

**`main_macos.rs` integration:** before `overlay::macos::run_app`, call `tray::install_tray(...)` on the main thread. The returned `Retained<NSStatusItem>` lives in a `let _tray = ...` binding for the duration of `main`.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```

Hardware walkthrough in Task 6.

**Commit:** `macos: NSStatusItem + full NSMenu (Captions/Mode/Engine/Audio/AboveFS/Lock/Quit)`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Docked/Floating geometry + drag persistence + 4-surface clear + full OverlayCommand handlers

**Verifies:** macos-port.AC1.1, macos-port.AC1.2, macos-port.AC1.4, macos-port.AC1.6, macos-port.AC2.5, macos-port.AC2.7, macos-port.AC6.2

**Files:**
- Modify: `src/overlay/macos/panel.rs` — add `apply_geometry`, NSScreen-change observer
- Create: `src/overlay/macos/drag.rs` — drag observer persisting `frame.origin` via hot-reload-safe write path
- Modify: `src/overlay/macos/app.rs` — fill in handlers Phase 2 stubbed

**Implementation:**

**`panel::apply_geometry`** reconfigures the same `Retained<NSPanel>` per mode without rebuild:

```text
pub fn apply_geometry(
    panel: &NSPanel,
    mtm: MainThreadMarker,
    mode: OverlayMode,
    config: &Config,
) {
    match mode {
        OverlayMode::Docked => {
            let screen = NSScreen::mainScreen().expect("main screen");
            let visible = screen.visibleFrame();  // AC2.7 — not frame
            let height = panel_height_for_font(config.appearance.font_size);
            let rect = NSRect::new(
                visible.origin.x,
                visible.origin.y + visible.size.height - height,
                visible.size.width,
                height,
            );
            unsafe {
                panel.setFrame_display(rect, true);
                panel.setIgnoresMouseEvents(true);  // AC2.5
                panel.setMovableByWindowBackground(false);
                panel.setHasShadow(true);
            }
        }
        OverlayMode::Floating => {
            let rect = NSRect::new(
                config.position.x as f64,
                config.position.y as f64,
                config.appearance.width as f64,
                panel_height_for_font(config.appearance.font_size),
            );
            unsafe {
                panel.setFrame_display(rect, true);
                panel.setIgnoresMouseEvents(true);  // AC2.5
                panel.setMovableByWindowBackground(!config.locked);
                panel.setHasShadow(false);
            }
        }
        OverlayMode::Transcript => {
            unsafe { panel.orderOut(None); }
        }
    }
}
```

**AC1.6 (external display change):** subscribe to `NSApplicationDidChangeScreenParametersNotification` and re-call `apply_geometry` for the current mode.

**`drag.rs` — drag persistence:**

```text
pub fn install_drag_observer(
    panel: &NSPanel,
    config: Arc<Mutex<Config>>,
    mtm: MainThreadMarker,
) -> Retained<NSObject>
{
    // define_class! a small NSObject subclass with ivars { config, panel_ptr }
    // and a @objc fn windowDidMove(_:) method that:
    //   1. Reads panel.frame.origin
    //   2. Locks config; updates config.position.{x,y}
    //   3. Calls Config::save via the existing hot-reload-safe write path
    //      (the same path the Linux drag uses — config.rs detects no-op writes
    //      and suppresses the SetMode/SetLocked echo, preserving AC6.2)
    //
    // Register with NSNotificationCenter::defaultCenter.addObserver_selector_name_object
    // for NSWindowDidMoveNotification scoped to this panel.
}
```

The hot-reload-safe write path already exists in `src/config.rs` (Linux drag uses it). Reuse the same `Config::save` function and let the existing change-detection logic suppress the echo.

**`app::handle_overlay_command`** — fill in Phase 2's stubs. Add the following arms (in addition to Phase 2's `Quit`, `SetAboveFullscreen`, `SetCaptionsEnabled`):

```text
OverlayCommand::SetVisible(visible) => {
    if visible { unsafe { panel.orderFront(None); } }
    else       { unsafe { panel.orderOut(None);   } }
}
OverlayCommand::SetMode(mode) => {
    let cfg = config.lock().clone();
    config.lock().overlay_mode = mode;
    panel::apply_geometry(&panel, mtm, mode, &cfg);
    match mode {
        OverlayMode::Docked | OverlayMode::Floating => {
            unsafe { panel.orderFront(None); }
            transcript_window::order_out(&transcript_state, mtm);
        }
        OverlayMode::Transcript => {
            transcript_window::order_front(&transcript_state, mtm);
            unsafe { panel.orderOut(None); }
        }
    }
}
OverlayCommand::SetLocked(locked) => {
    config.lock().locked = locked;
    if matches!(config.lock().overlay_mode, OverlayMode::Floating) {
        unsafe { panel.setMovableByWindowBackground(!locked); }
    }
}
OverlayCommand::UpdateAppearance(appearance) => {
    apply_appearance(&label, &appearance);
    caption_buffer.lock().update_config(
        appearance.max_lines as usize,
        derive_max_chars(&appearance),
        appearance.expire_secs,
    );
}
// SetCaptionsEnabled extended for 4-surface clear:
OverlayCommand::SetCaptionsEnabled(enabled) => {
    captions_enabled.store(enabled, Ordering::Relaxed);
    if !enabled {
        transcript_log.lock().clear();                                        // surface 1
        transcript_window::clear_view(&transcript_state, mtm);                // surface 2
        caption_buffer.lock().clear();                                        // surface 3
        let ns = NSString::from_str("");
        unsafe { label.setStringValue(&ns); }                                 // surface 4
    }
}
```

**Caption-bridge upgrade:** the Phase 2 caption-bridge wrote raw text to the label. Phase 6 routes through `CaptionBuffer` + `TranscriptLog`:

```text
// In the dispatched main-queue closure (caption bridge):
let mut buf = caption_buffer.lock();
buf.push(text.clone());
let display = buf.display_text();
let ns = NSString::from_str(&display);
unsafe { label.setStringValue(&ns); }

let mut log = transcript_log.lock();
log.push(text, chrono::Utc::now());
if matches!(config.lock().overlay_mode, OverlayMode::Transcript) {
    transcript_window::append_fragment(&transcript_state, mtm, /* see Task 4 */);
}
```

(`caption_buffer` and `transcript_log` are `Arc<Mutex<>>` constructed at `run_app` start and shared into the bridge closure.)

Also install a per-second NSTimer that calls `caption_buffer.lock().expire()` and updates the label when expiry trimmed a line — mirrors Linux's `glib::timeout_add_local` at `overlay/linux/mod.rs:166-179`.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```

Hardware walkthrough in Task 6.

**Commit:** `macos: panel geometry + drag persistence + 4-surface clear + full OverlayCommand handlers`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Transcript window (NSScrollView + NSTextView + NSSavePanel)

**Verifies:** macos-port.AC1.3, macos-port.AC1.5, macos-port.AC2.6

**Files:**
- Create: `src/overlay/macos/transcript_window.rs`
- Modify: `src/overlay/macos/mod.rs` — `pub mod transcript_window;`

**Implementation:**

Mirror `src/overlay/linux/transcript_window.rs` (`TranscriptWindowState`, `build_transcript_window`, `append_fragment`, `clear_view`, helpers). macOS counterpart:

```text
pub struct TranscriptWindowState {
    pub window: Retained<NSWindow>,
    pub text_view: Retained<NSTextView>,
    pub log: Arc<Mutex<TranscriptLog>>,
}

pub fn build_transcript_window(
    mtm: MainThreadMarker,
    log: Arc<Mutex<TranscriptLog>>,
) -> TranscriptWindowState {
    let style = NSWindowStyleMask::Titled
              | NSWindowStyleMask::Closable
              | NSWindowStyleMask::Miniaturizable
              | NSWindowStyleMask::Resizable;
    let rect = NSRect::new(200.0, 200.0, 800.0, 600.0);
    let window = NSWindow::alloc().initWithContentRect_styleMask_backing_defer(
        rect, style, NSBackingStoreType::Buffered, false,
    );
    window.setTitle(&NSString::from_str("Subtidal — Transcript"));
    // AC2.6: regular window; ignoresMouseEvents = false (default).

    let scroll = NSScrollView::alloc().initWithFrame(rect);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    let text_view = NSTextView::alloc().initWithFrame(rect);
    text_view.setEditable(false);
    text_view.setSelectable(true);
    text_view.setRichText(false);
    scroll.setDocumentView(Some(&text_view));

    // Save button at the bottom of the content view (or as a toolbar item;
    // simpler: a regular NSButton). The handler invokes save_transcript.
    let save_button = NSButton::alloc().initWithFrame(/* small rect, bottom-right */);
    save_button.setTitle(&NSString::from_str("Save…"));
    save_button.setTarget(/* action target object */);
    save_button.setAction(Some(sel!(saveTranscript:)));

    // Stack scroll + save_button into a container view.
    let container = NSView::alloc().initWithFrame(rect);
    container.addSubview(&scroll);
    container.addSubview(&save_button);
    window.setContentView(Some(&container));

    TranscriptWindowState { window, text_view, log }
}

pub fn append_fragment(
    state: &TranscriptWindowState,
    mtm: MainThreadMarker,
    text: String,
    ts: chrono::DateTime<chrono::Utc>,
) {
    state.log.lock().unwrap().push(text, ts);
    let formatted = format_paragraphs(&state.log.lock().unwrap());
    let storage = state.text_view.textStorage().expect("textStorage");
    let attr = NSAttributedString::initWithString(&NSString::from_str(&formatted));
    storage.setAttributedString(&attr);
    if user_is_at_bottom(&state.text_view) {
        let len = storage.length();
        state.text_view.scrollRangeToVisible(NSRange { location: len, length: 0 });
    }
}

pub fn clear_view(state: &TranscriptWindowState, mtm: MainThreadMarker) {
    let storage = state.text_view.textStorage().expect("textStorage");
    let empty = NSAttributedString::initWithString(&NSString::from_str(""));
    storage.setAttributedString(&empty);
}

pub fn order_front(state: &TranscriptWindowState, mtm: MainThreadMarker) {
    unsafe { state.window.makeKeyAndOrderFront(None); }
}
pub fn order_out(state: &TranscriptWindowState, mtm: MainThreadMarker) {
    unsafe { state.window.orderOut(None); }
}
```

**Save dialog (NSSavePanel):**

```text
unsafe fn save_transcript(state: &TranscriptWindowState, mtm: MainThreadMarker) {
    let panel = NSSavePanel::savePanel();
    let default_name = format!(
        "subtidal-transcript-{}.json",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S"),
    );
    panel.setNameFieldStringValue(&NSString::from_str(&default_name));
    panel.setAllowedFileTypes(&NSArray::from_vec(vec![NSString::from_str("json")]));
    if panel.runModal() == NSModalResponseOK {
        if let Some(url) = panel.URL().and_then(|u| u.path()) {
            let path = url.to_string();
            let json = state.log.lock().unwrap().to_json();
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("warn: transcript save failed: {e}");
            }
        }
    }
}
```

**AC1.5 (empty Save):** `TranscriptLog::to_json` is already pure; empty fragments produce valid JSON. No special-casing.

**`format_paragraphs`** and `user_is_at_bottom`: read `src/overlay/linux/transcript_window.rs` for the exact formatting expected (timestamped paragraphs); mirror it. `user_is_at_bottom`: `text_view.visibleRect()` vs `text_view.bounds()` — return true when the bottom of the visible rect is within ~10 pt of the bottom of the bounds.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```

Hardware test in Task 6.

**Commit:** `macos: transcript window (NSScrollView+NSTextView, autoscroll, NSSavePanel)`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->
<!-- START_TASK_5 -->
### Task 5: Integration tests — mode switch + transcript save round-trip

**Verifies:** macos-port.AC1.4 (mode-switch no rebuild), macos-port.AC1.5 (empty Save valid)

**Files:**
- Modify: `src/overlay/macos/panel.rs` — extend the Phase 2 `#[cfg(all(test, target_os = "macos"))] mod tests` with a mode-switch test
- Modify: `src/overlay/macos/transcript_window.rs` — add `#[cfg(all(test, target_os = "macos"))] mod tests`

**Implementation:**

```rust
#[test]
fn mode_switch_does_not_rebuild_panel() {
    let Some(mtm) = MainThreadMarker::new() else { return; };
    let config = Config::default();
    let (panel, _label) = build_overlay_panel(mtm, &config);
    let initial_ptr = Retained::as_ptr(&panel);
    apply_geometry(&panel, mtm, OverlayMode::Docked, &config);
    apply_geometry(&panel, mtm, OverlayMode::Floating, &config);
    assert_eq!(Retained::as_ptr(&panel), initial_ptr);
}
```

```rust
#[test]
fn empty_transcript_save_produces_valid_json() {
    let Some(mtm) = MainThreadMarker::new() else { return; };
    let log = Arc::new(Mutex::new(TranscriptLog::new()));
    let state = build_transcript_window(mtm, Arc::clone(&log));

    let json = state.log.lock().unwrap().to_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(parsed.is_object() || parsed.is_array(), "TranscriptLog::to_json shape");
}
```

Hot-reload integration tests reuse the existing Linux path; the macOS-specific config-dir resolution from Phase 1 Task 2 is exercised by the hardware walkthrough in Task 6 (AC6.1/AC6.3).

**Verification:**

```bash
cargo test --lib --target aarch64-apple-darwin
cargo check --lib --target x86_64-apple-darwin
```

**Commit:** `macos: integration tests for mode-switch and transcript save`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: End-to-end hardware walkthrough — AC1/AC2/AC5/AC6 surfaces

**Verifies:** all AC1/AC2/AC5/AC6 criteria operationally

**Files:** none

**Implementation:**

On the target Apple Silicon Mac:

```bash
scripts/bundle-mac.sh
open target/release/Subtidal.app
```

Walk each criterion:

- **AC1.1 (Docked):** tray → Mode → Docked. Panel snaps to top of visible-frame, full width.
- **AC1.2 (Floating + drag):** tray → Mode → Floating. Drag, release. Reopen tray → drag again. Confirm `~/Library/Application Support/Subtidal/config.toml` `position` updates.
- **AC1.3 (Transcript autoscroll):** tray → Mode → Transcript. Confirm timestamped paragraphs append; bottom-locked autoscroll continues; manual scroll up pauses autoscroll.
- **AC1.4 (instant switch):** rapid-fire Mode switches; no rebuild visible.
- **AC1.5 (empty Save):** with no captions, switch to Transcript → Save. JSON file is valid (empty fragments OK).
- **AC1.6 (display change):** plug/unplug external display in Docked; panel re-positions.
- **AC2.2 (Spaces):** Mission Control → switch Space; panel visible.
- **AC2.3 (above fullscreen):** "Show Above Fullscreen" on; Safari fullscreen → panel still visible.
- **AC2.5 (click-through Floating/Docked):** clicks pass to window below.
- **AC2.6 (Transcript clickable):** Save button works.
- **AC2.7 (no menu-bar collision):** Docked sits below menu bar.
- **AC5.1–AC5.8:** all menu items functional; checkmarks reflect state; Lock Position disabled in Docked/Transcript modes; Cmd-Q terminates cleanly.
- **AC6.1 (hot-reload):** edit config.toml; ≤500 ms apply.
- **AC6.2 (no drag loop):** drag rapidly; no mode reset / glitch.
- **AC6.3 (malformed TOML):** write invalid TOML; stderr warns; app continues.
- **AC3.3 (tray-triggered live source switch):** with audio actively flowing, open the Audio Source submenu and select a different running app. The tray-driven `AudioCommand::SetSource` must reach the audio thread via `SCStream.updateContentFilter` — observe no panel flicker, no caption gap > 1 sample, and no momentary stream stop. (Phase 5 verified this via injected `AudioCommand`; this re-verifies the same path is wired through the live tray UI.)

Surface any failure to the user before merging.

**Cross-target CI green:**
```bash
cargo check --lib --target x86_64-apple-darwin
```

**Commit:** none (verification only). Capture discovered macOS landmines (template-image quirks, NSPanel Space-attaching gotchas, etc.) in a memory note for the user.
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_C -->
