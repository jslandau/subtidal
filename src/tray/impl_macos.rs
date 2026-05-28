//! macOS system tray: NSStatusItem + NSMenu with Obj-C action targets.
//!
//! ## Canonical objc2 0.6 `define_class!` pattern
//!
//! AppKit menu items dispatch clicks by sending `[target selector:item]` —
//! the target must be a real Obj-C object whose class implements the
//! selector. Pointing `setTarget:` at an arbitrary Rust struct or at an
//! unrelated AppKit object (e.g. NSStatusItem) will raise
//! `NSInvalidArgumentException` at first click. The fix is a small custom
//! NSObject subclass declared via `objc2::define_class!`.
//!
//! This file is intentionally the only place in the codebase that does
//! this; future macOS work (drag observers in `overlay/macos/drag.rs`, a
//! transcript-window save button, etc.) should copy this structure rather
//! than reinvent it. Key points:
//!
//! - Ivars are a plain Rust struct (`TrayIvars`) accessed inside methods
//!   via `self.ivars()`.
//! - Construction is `Self::alloc(mtm).set_ivars(ivars)` followed by
//!   `msg_send![super(allocated), init]`. The `mtm` argument carries the
//!   main-thread guarantee that makes `Retained<NSMenuItem>` ivars sound.
//! - Methods are declared with `#[unsafe(method(selector:))]`; arguments
//!   match the Obj-C signature — menu actions take
//!   `Option<&NSMenuItem>`, NSTimer callbacks take `Option<&NSTimer>`.
//! - `setTarget(Some(&*actions))` is wired *after* the menu is built. The
//!   `Retained<TrayActions>` is kept alive for the app lifetime via the
//!   tuple returned from `install_tray`.

use crate::audio::{AudioCommand, AudioSourceInfo};
use crate::config::{AudioSource, Config, Engine, OverlayMode};
use crate::overlay::OverlayCommand;
use arc_swap::ArcSwap;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventMask, NSEventType,
    NSImage, NSMenu, NSMenuItem, NSSquareStatusItemLength, NSStatusBar, NSStatusItem,
};
use objc2_foundation::{NSData, NSObject, NSPoint, NSString, NSTimer};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

/// Shared state passed in by `main_macos.rs`.
pub struct TrayState {
    pub config: Arc<Mutex<Config>>,
    pub captions_enabled: Arc<AtomicBool>,
    pub cmd_tx: async_channel::Sender<OverlayCommand>,
    pub audio_cmd_tx: SyncSender<AudioCommand>,
    pub engine_choice: Arc<ArcSwap<Engine>>,
    pub audio_sources: Arc<Mutex<Vec<AudioSourceInfo>>>,
}

/// Ivars for the `TrayActions` NSObject subclass.
///
/// `Retained<NSMenuItem>` ivars are sound because `TrayActions` is
/// `MainThreadOnly` — AppKit always invokes menu targets on the main
/// thread, and the NSTimer fires on the main run loop. `RefCell` gives
/// interior mutability for re-pointing the Audio Source submenu when the
/// running-process list changes.
pub struct TrayIvars {
    state: TrayState,
    audio_item: RefCell<Retained<NSMenuItem>>,
    lock_item: RefCell<Retained<NSMenuItem>>,
    captions_item: RefCell<Retained<NSMenuItem>>,
    above_item: RefCell<Retained<NSMenuItem>>,
    mode_items: RefCell<Vec<Retained<NSMenuItem>>>,
    font_item: RefCell<Retained<NSMenuItem>>,
    lines_item: RefCell<Retained<NSMenuItem>>,
    context_menu: RefCell<Retained<NSMenu>>,
    status_item: RefCell<Retained<NSStatusItem>>,
    icon_on: Retained<NSImage>,
    icon_off: Retained<NSImage>,
}

define_class!(
    /// Custom NSObject subclass holding shared tray state in ivars and
    /// implementing every menu/timer selector as an Obj-C method.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SubtidalTrayActions"]
    #[ivars = TrayIvars]
    pub struct TrayActions;

    impl TrayActions {
        #[unsafe(method(toggleCaptions:))]
        fn toggle_captions(&self, _sender: Option<&NSMenuItem>) {
            self.do_toggle_captions();
        }

        /// Fires on left- or right-click of the status-bar icon (registered
        /// via `sendActionOn`). Left click toggles captions; right click pops
        /// the context menu at the button's origin.
        #[unsafe(method(statusItemClicked:))]
        fn status_item_clicked(&self, _sender: Option<&AnyObject>) {
            let mtm = MainThreadMarker::from(self);
            let is_right = NSApplication::sharedApplication(mtm).currentEvent()
                .map(|e| e.r#type() == NSEventType::RightMouseUp)
                .unwrap_or(false);
            if is_right {
                let ivars = self.ivars();
                let menu = ivars.context_menu.borrow();
                let si = ivars.status_item.borrow();
                if let Some(btn) = si.button(mtm) {
                    unsafe {
                        let _: bool = msg_send![
                            &**menu,
                            popUpMenuPositioningItem: std::ptr::null::<NSMenuItem>(),
                            atLocation: NSPoint::new(0.0, 0.0),
                            inView: &*btn
                        ];
                    }
                }
            } else {
                self.do_toggle_captions();
            }
        }

        #[unsafe(method(selectMode:))]
        fn select_mode(&self, sender: Option<&NSMenuItem>) {
            let Some(sender) = sender else { return };
            let title = sender.title().to_string();
            let new_mode = match title.as_str() {
                "Docked" => OverlayMode::Docked,
                "Floating" => OverlayMode::Floating,
                "Transcript" => OverlayMode::Transcript,
                _ => return,
            };
            let ivars = self.ivars();
            {
                let mut cfg = ivars.state.config.lock().unwrap();
                cfg.overlay_mode = new_mode.clone();
                let _ = cfg.save();
            }
            let _ = ivars
                .state
                .cmd_tx
                .send_blocking(OverlayCommand::SetMode(new_mode.clone()));

            for mi in ivars.mode_items.borrow().iter() {
                let mi_title = mi.title().to_string();
                mi.setState(if mi_title == title { 1 } else { 0 });
            }
            ivars
                .lock_item
                .borrow()
                .setEnabled(matches!(new_mode, OverlayMode::Floating));
        }

        #[unsafe(method(selectAudioSource:))]
        fn select_audio_source(&self, sender: Option<&NSMenuItem>) {
            let Some(sender) = sender else { return };
            let title = sender.title().to_string();
            let ivars = self.ivars();
            let new_source = if title == "System Output" {
                AudioSource::SystemOutput
            } else {
                let sources = ivars.state.audio_sources.lock().unwrap();
                match sources.iter().find(|s| s.label == title) {
                    Some(info) => info.source.clone(),
                    None => return,
                }
            };
            {
                let mut cfg = ivars.state.config.lock().unwrap();
                cfg.audio_source = new_source.clone();
                let _ = cfg.save();
            }
            let _ = ivars
                .state
                .audio_cmd_tx
                .send(AudioCommand::SwitchSource(new_source));

            let audio = ivars.audio_item.borrow();
            if let Some(submenu) = audio.submenu() {
                let n = submenu.numberOfItems();
                for i in 0..n {
                    if let Some(mi) = submenu.itemAtIndex(i) {
                        let on = mi.title().to_string() == title;
                        mi.setState(if on { 1 } else { 0 });
                    }
                }
            }
        }

        #[unsafe(method(toggleAboveFullscreen:))]
        fn toggle_above_fullscreen(&self, _sender: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            let next = {
                let mut cfg = ivars.state.config.lock().unwrap();
                cfg.above_fullscreen = !cfg.above_fullscreen;
                let _ = cfg.save();
                cfg.above_fullscreen
            };
            let _ = ivars
                .state
                .cmd_tx
                .send_blocking(OverlayCommand::SetAboveFullscreen(next));
            ivars.above_item.borrow().setState(if next { 1 } else { 0 });
        }

        #[unsafe(method(toggleLock:))]
        fn toggle_lock(&self, _sender: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            let in_floating = {
                let cfg = ivars.state.config.lock().unwrap();
                matches!(cfg.overlay_mode, OverlayMode::Floating)
            };
            if !in_floating {
                return;
            }
            let next = {
                let mut cfg = ivars.state.config.lock().unwrap();
                cfg.locked = !cfg.locked;
                let _ = cfg.save();
                cfg.locked
            };
            let _ = ivars
                .state
                .cmd_tx
                .send_blocking(OverlayCommand::SetLocked(next));
            ivars.lock_item.borrow().setState(if next { 1 } else { 0 });
        }

        #[unsafe(method(selectFont:))]
        fn select_font(&self, sender: Option<&NSMenuItem>) {
            let Some(sender) = sender else { return };
            let title = sender.title().to_string();
            // "System" is a sentinel resolved by panel::resolve_font; every
            // other title is a literal NSFont family name.
            let family = if title == "System" { "system".to_string() } else { title.clone() };

            let appearance = {
                let mut cfg = self.ivars().state.config.lock().unwrap();
                cfg.appearance.font_family = family;
                let _ = cfg.save();
                cfg.appearance.clone()
            };
            let _ = self.ivars().state.cmd_tx.send_blocking(
                OverlayCommand::UpdateAppearance(appearance),
            );

            // Walk every item in both the curated list and the All Fonts
            // submenu (if present) to update checkmarks. Cheap because the
            // curated list is short and "All Fonts" is only walked when the
            // user has clicked into it at least once.
            let mtm = MainThreadMarker::from(self);
            let font_item = self.ivars().font_item.borrow();
            if let Some(submenu) = font_item.submenu() {
                update_font_checkmarks(&submenu, &title, mtm);
            }
        }

        #[unsafe(method(selectLines:))]
        fn select_lines(&self, sender: Option<&NSMenuItem>) {
            let Some(sender) = sender else { return };
            let Ok(n) = sender.title().to_string().trim().parse::<u32>() else { return };
            let appearance = {
                let mut cfg = self.ivars().state.config.lock().unwrap();
                cfg.appearance.max_lines = n;
                let _ = cfg.save();
                cfg.appearance.clone()
            };
            let _ = self.ivars().state.cmd_tx.send_blocking(
                OverlayCommand::UpdateAppearance(appearance),
            );
            let lines_item = self.ivars().lines_item.borrow();
            if let Some(submenu) = lines_item.submenu() {
                let count = submenu.numberOfItems();
                for i in 0..count {
                    if let Some(mi) = submenu.itemAtIndex(i) {
                        let on = mi.title().to_string().trim().parse::<u32>().ok() == Some(n);
                        mi.setState(if on { 1 } else { 0 });
                    }
                }
            }
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            let _ = ivars.state.audio_cmd_tx.send(AudioCommand::Shutdown);
            let _ = ivars.state.cmd_tx.send_blocking(OverlayCommand::Quit);
        }

        /// No-op for the disabled "Nemotron" engine entry. NSMenuItem
        /// requires a non-nil action for the checkmark to render under
        /// `setAutoenablesItems(false)`.
        #[unsafe(method(noop:))]
        fn noop(&self, _sender: Option<&NSMenuItem>) {}

        /// AC5.4: every 5 s, re-poll audio process list and rebuild the
        /// Audio Source submenu if it has changed.
        #[unsafe(method(refreshAudioSources:))]
        fn refresh_audio_sources(&self, _timer: Option<&NSTimer>) {
            let ivars = self.ivars();
            let new_sources = match crate::audio::list_sources() {
                Ok(s) => s,
                Err(_) => return,
            };
            let changed = {
                let mut cached = ivars.state.audio_sources.lock().unwrap();
                if new_sources == *cached {
                    false
                } else {
                    *cached = new_sources;
                    true
                }
            };
            if !changed {
                return;
            }
            let mtm = MainThreadMarker::from(self);
            let new_submenu = build_audio_submenu_for(&ivars.state, mtm, self);
            ivars.audio_item.borrow().setSubmenu(Some(&new_submenu));
        }
    }
);

impl TrayActions {
    fn new(
        mtm: MainThreadMarker,
        state: TrayState,
        captions_item: Retained<NSMenuItem>,
        mode_items: Vec<Retained<NSMenuItem>>,
        audio_item: Retained<NSMenuItem>,
        above_item: Retained<NSMenuItem>,
        lock_item: Retained<NSMenuItem>,
        font_item: Retained<NSMenuItem>,
        lines_item: Retained<NSMenuItem>,
        context_menu: Retained<NSMenu>,
        status_item: Retained<NSStatusItem>,
        icon_on: Retained<NSImage>,
        icon_off: Retained<NSImage>,
    ) -> Retained<Self> {
        let ivars = TrayIvars {
            state,
            audio_item: RefCell::new(audio_item),
            lock_item: RefCell::new(lock_item),
            captions_item: RefCell::new(captions_item),
            above_item: RefCell::new(above_item),
            mode_items: RefCell::new(mode_items),
            font_item: RefCell::new(font_item),
            lines_item: RefCell::new(lines_item),
            context_menu: RefCell::new(context_menu),
            status_item: RefCell::new(status_item),
            icon_on,
            icon_off,
        };
        let allocated = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(allocated), init] }
    }

    /// Shared captions toggle logic, called from both the menu item and the
    /// status-bar left-click handler.
    fn do_toggle_captions(&self) {
        let ivars = self.ivars();
        let prev = ivars.state.captions_enabled.load(Ordering::Relaxed);
        let next = !prev;
        ivars.state.captions_enabled.store(next, Ordering::Relaxed);
        let in_transcript = matches!(
            ivars.state.config.lock().unwrap().overlay_mode,
            OverlayMode::Transcript
        );
        if !in_transcript {
            let _ = ivars.state.cmd_tx.send_blocking(OverlayCommand::SetVisible(next));
        }
        let _ = ivars.state.cmd_tx.send_blocking(OverlayCommand::SetCaptionsEnabled(next));
        ivars.captions_item.borrow().setState(if next { 1 } else { 0 });
        let mtm = MainThreadMarker::from(self);
        let icon = if next { &ivars.icon_on } else { &ivars.icon_off };
        if let Some(btn) = ivars.status_item.borrow().button(mtm) {
            btn.setImage(Some(icon));
        }
    }
}

/// Build the Audio Source submenu, wiring `selectAudioSource:` to the
/// given `TrayActions` target on each row. Used for the initial build and
/// the timer-driven refresh.
fn build_audio_submenu_for(
    state: &TrayState,
    mtm: MainThreadMarker,
    target: &TrayActions,
) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);

    let current_source = state.config.lock().unwrap().audio_source.clone();
    let sources = state.audio_sources.lock().unwrap().clone();
    let target_obj: &AnyObject = target.as_ref();

    let make_item = |label: &str, on: bool| -> Retained<NSMenuItem> {
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(selectAudioSource:)),
                &NSString::from_str(""),
            )
        };
        mi.setState(if on { 1 } else { 0 });
        unsafe { mi.setTarget(Some(target_obj)) };
        mi
    };

    let system_on = matches!(current_source, AudioSource::SystemOutput);
    menu.addItem(&make_item("System Output", system_on));
    for info in &sources {
        let on = info.source == current_source;
        menu.addItem(&make_item(&info.label, on));
    }
    menu
}

/// Curated font picks shown at the top of the Font submenu. "System" is
/// a sentinel resolved by `panel::resolve_font` to the system monospace.
const CURATED_FONTS: &[&str] = &["System", "SF Mono", "Menlo", "Monaco", "Courier New"];

/// List every monospaced font family installed on the system, in
/// alphabetical order. Uses NSFontManager with the FixedPitch trait mask.
fn all_monospace_fonts(mtm: MainThreadMarker) -> Vec<String> {
    use objc2_app_kit::{NSFontManager, NSFontTraitMask};
    let mgr = NSFontManager::sharedFontManager(mtm);
    let names = mgr.availableFontNamesWithTraits(NSFontTraitMask::FixedPitchFontMask);
    let Some(names) = names else { return Vec::new() };
    let mut out = Vec::with_capacity(names.count() as usize);
    for i in 0..names.count() {
        let s = names.objectAtIndex(i);
        out.push(s.to_string());
    }
    out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    out
}

/// Currently-selected font family for checkmark purposes. The config
/// stores "system" (sentinel) which displays as "System" in the menu.
fn current_font_label(config: &Arc<Mutex<Config>>) -> String {
    let cfg = config.lock().unwrap();
    let f = cfg.appearance.font_family.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("system") || f.eq_ignore_ascii_case("monospace") {
        "System".to_string()
    } else {
        f.to_string()
    }
}

fn build_font_submenu(
    state: &TrayState,
    mtm: MainThreadMarker,
    target: &TrayActions,
) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let target_obj: &AnyObject = target.as_ref();
    let current = current_font_label(&state.config);

    let make_item = |label: &str, on: bool| -> Retained<NSMenuItem> {
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(selectFont:)),
                &NSString::from_str(""),
            )
        };
        mi.setState(if on { 1 } else { 0 });
        unsafe { mi.setTarget(Some(target_obj)) };
        mi
    };

    for label in CURATED_FONTS {
        menu.addItem(&make_item(label, *label == current));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // "All Fonts" cascading submenu — built eagerly because NSFontManager
    // enumeration is cheap (<5 ms even with hundreds of fonts) and we
    // benefit from checkmark consistency if the user's selection lives
    // there.
    let all_parent = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("All Fonts"),
            None,
            &NSString::from_str(""),
        )
    };
    let all_menu = NSMenu::new(mtm);
    all_menu.setAutoenablesItems(false);
    for name in all_monospace_fonts(mtm) {
        // Skip curated entries to avoid duplicate visual rows + checkmark drift.
        if CURATED_FONTS.iter().any(|c| c.eq_ignore_ascii_case(&name)) {
            continue;
        }
        all_menu.addItem(&make_item(&name, name == current));
    }
    all_parent.setSubmenu(Some(&all_menu));
    menu.addItem(&all_parent);
    menu
}

fn update_font_checkmarks(menu: &NSMenu, selected_title: &str, mtm: MainThreadMarker) {
    let _ = mtm;
    let n = menu.numberOfItems();
    for i in 0..n {
        if let Some(mi) = menu.itemAtIndex(i) {
            let t = mi.title().to_string();
            mi.setState(if t == selected_title { 1 } else { 0 });
            if let Some(sub) = mi.submenu() {
                update_font_checkmarks(&sub, selected_title, mtm);
            }
        }
    }
}

fn build_lines_submenu(
    state: &TrayState,
    mtm: MainThreadMarker,
    target: &TrayActions,
) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let target_obj: &AnyObject = target.as_ref();
    let current = state.config.lock().unwrap().appearance.max_lines;
    for n in 1u32..=5 {
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&n.to_string()),
                Some(sel!(selectLines:)),
                &NSString::from_str(""),
            )
        };
        mi.setState(if n == current { 1 } else { 0 });
        unsafe { mi.setTarget(Some(target_obj)) };
        menu.addItem(&mi);
    }
    menu
}

/// Install the macOS system tray on the main thread. Returns both the
/// status item and the actions object — the caller MUST keep both alive
/// for the app lifetime (a `let _binding = ...` in `main` suffices).
pub fn install_tray(
    state: TrayState,
    mtm: MainThreadMarker,
) -> (Retained<NSStatusItem>, Retained<TrayActions>) {
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSSquareStatusItemLength);

    // Load state-indicator icons from PNGs embedded at compile time
    // (generated by `cargo run --example gen_macos_icons`).
    let load_icon = |bytes: &[u8]| -> Retained<NSImage> {
        let data = NSData::from_vec(bytes.to_vec());
        NSImage::initWithData(NSImage::alloc(), &data)
            .expect("failed to decode embedded tray icon PNG")
    };
    let icon_on  = load_icon(include_bytes!("../../resources/macos/tray-icon-on.png"));
    let icon_off = load_icon(include_bytes!("../../resources/macos/tray-icon-off.png"));

    // Set the initial icon based on current captions state.
    let initial_on = state.captions_enabled.load(Ordering::Relaxed);
    if let Some(button) = item.button(mtm) {
        button.setImage(Some(if initial_on { &icon_on } else { &icon_off }));
    }

    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);

    let captions_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Captions"),
            Some(sel!(toggleCaptions:)),
            &NSString::from_str(""),
        )
    };
    captions_item.setState(if state.captions_enabled.load(Ordering::Relaxed) { 1 } else { 0 });
    menu.addItem(&captions_item);

    // Mode submenu
    let mode_parent = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Mode"),
            None,
            &NSString::from_str(""),
        )
    };
    let mode_menu = NSMenu::new(mtm);
    mode_menu.setAutoenablesItems(false);
    let current_mode = state.config.lock().unwrap().overlay_mode.clone();
    let mut mode_items: Vec<Retained<NSMenuItem>> = Vec::new();
    for (label, mode_kind) in [
        ("Docked", OverlayMode::Docked),
        ("Floating", OverlayMode::Floating),
        ("Transcript", OverlayMode::Transcript),
    ] {
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(label),
                Some(sel!(selectMode:)),
                &NSString::from_str(""),
            )
        };
        mi.setState(if current_mode == mode_kind { 1 } else { 0 });
        mode_menu.addItem(&mi);
        mode_items.push(mi);
    }
    mode_parent.setSubmenu(Some(&mode_menu));
    menu.addItem(&mode_parent);

    // Engine submenu (Nemotron only, disabled)
    let engine_parent = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Engine"),
            None,
            &NSString::from_str(""),
        )
    };
    let engine_menu = NSMenu::new(mtm);
    engine_menu.setAutoenablesItems(false);
    let nemo = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Nemotron"),
            Some(sel!(noop:)),
            &NSString::from_str(""),
        )
    };
    nemo.setState(1);
    nemo.setEnabled(false);
    engine_menu.addItem(&nemo);
    engine_parent.setSubmenu(Some(&engine_menu));
    menu.addItem(&engine_parent);

    // Audio Source — submenu populated after actions exists
    let audio_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Audio Source"),
            None,
            &NSString::from_str(""),
        )
    };
    menu.addItem(&audio_item);

    // Font submenu — built after actions exists so each row can target it.
    let font_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Font"),
            None,
            &NSString::from_str(""),
        )
    };
    menu.addItem(&font_item);

    // Lines submenu (1-5).
    let lines_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Lines"),
            None,
            &NSString::from_str(""),
        )
    };
    menu.addItem(&lines_item);

    let above_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Show Above Fullscreen"),
            Some(sel!(toggleAboveFullscreen:)),
            &NSString::from_str(""),
        )
    };
    let above_initial = state.config.lock().unwrap().above_fullscreen;
    above_item.setState(if above_initial { 1 } else { 0 });
    menu.addItem(&above_item);

    let lock_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Lock Position"),
            Some(sel!(toggleLock:)),
            &NSString::from_str(""),
        )
    };
    let locked_initial = state.config.lock().unwrap().locked;
    lock_item.setState(if locked_initial { 1 } else { 0 });
    lock_item.setEnabled(matches!(current_mode, OverlayMode::Floating));
    menu.addItem(&lock_item);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    let quit_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit Subtidal"),
            Some(sel!(quit:)),
            &NSString::from_str("q"),
        )
    };
    menu.addItem(&quit_item);

    // Build the action target. Clone Retained handles for ivars — NSMenu
    // retains its items independently, so these clones are sound.
    // `menu` and `item` are also stored so statusItemClicked: can pop the
    // menu and re-use the status item reference.
    let actions = TrayActions::new(
        mtm,
        state,
        captions_item.clone(),
        mode_items.clone(),
        audio_item.clone(),
        above_item.clone(),
        lock_item.clone(),
        font_item.clone(),
        lines_item.clone(),
        menu.clone(),
        item.clone(),
        icon_on.clone(),
        icon_off.clone(),
    );

    {
        let ivars = actions.ivars();
        let audio_submenu = build_audio_submenu_for(&ivars.state, mtm, &actions);
        audio_item.setSubmenu(Some(&audio_submenu));
        let font_submenu = build_font_submenu(&ivars.state, mtm, &actions);
        font_item.setSubmenu(Some(&font_submenu));
        let lines_submenu = build_lines_submenu(&ivars.state, mtm, &actions);
        lines_item.setSubmenu(Some(&lines_submenu));
    }

    let target_obj: &AnyObject = actions.as_ref();
    unsafe {
        captions_item.setTarget(Some(target_obj));
        above_item.setTarget(Some(target_obj));
        lock_item.setTarget(Some(target_obj));
        quit_item.setTarget(Some(target_obj));
        nemo.setTarget(Some(target_obj));
        for mi in &mode_items {
            mi.setTarget(Some(target_obj));
        }
    }

    // Left click → toggle captions; right click → context menu.
    // NSStatusItem with no setMenu set does not intercept clicks, so we
    // wire statusItemClicked: on the button and extend sendActionOn to
    // include right-mouse-up so both click types reach our handler.
    if let Some(button) = item.button(mtm) {
        unsafe {
            button.setAction(Some(sel!(statusItemClicked:)));
            button.setTarget(Some(target_obj));
            let _: objc2_foundation::NSInteger = msg_send![
                &*button,
                sendActionOn: NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp
            ];
        }
    }

    // AC5.4: schedule a repeating 5 s timer on the main run loop.
    unsafe {
        let _: Retained<NSTimer> =
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                5.0,
                target_obj,
                sel!(refreshAudioSources:),
                None,
                true,
            );
    }

    (item, actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_state_constructs() {
        let (cmd_tx, _cmd_rx) = async_channel::unbounded();
        let (audio_tx, _audio_rx) = std::sync::mpsc::sync_channel(1);
        let _state = TrayState {
            config: Arc::new(Mutex::new(Config::default())),
            captions_enabled: Arc::new(AtomicBool::new(true)),
            cmd_tx,
            audio_cmd_tx: audio_tx,
            engine_choice: Arc::new(ArcSwap::from_pointee(Engine::Nemotron)),
            audio_sources: Arc::new(Mutex::new(vec![])),
        };
    }
}
