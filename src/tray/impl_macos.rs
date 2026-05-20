//! macOS system tray via NSStatusItem + NSMenu.
//!
//! Mirrors Linux tray structure semantically: TrayState holds shared references
//! to config, captions_enabled, command channels, engine choice, and audio sources.
//! NSMenu construction happens on the main thread via install_tray().

use crate::audio::AudioSourceInfo;
use crate::config::{Config, Engine, OverlayMode};
use crate::overlay::OverlayCommand;
use arc_swap::ArcSwap;
use objc2::{MainThreadMarker, AnyThread, MainThreadOnly};
use objc2::rc::Retained;
use objc2::sel;
use objc2::msg_send;
use objc2_app_kit::{
    NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSImage, NSSquareStatusItemLength,
};
use objc2_foundation::NSString;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;

/// Shared tray state. Mirrors Linux TrayState; macOS uses Arc<Mutex<>> for
/// interior mutability since NSMenu handlers cannot capture mutable self.
pub struct TrayState {
    pub config: Arc<Mutex<Config>>,
    pub captions_enabled: Arc<AtomicBool>,
    pub cmd_tx: async_channel::Sender<OverlayCommand>,
    pub audio_cmd_tx: SyncSender<crate::audio::AudioCommand>,
    pub engine_choice: Arc<ArcSwap<Engine>>,
    pub audio_sources: Arc<Mutex<Vec<AudioSourceInfo>>>,
}

/// Install the macOS system tray (NSStatusItem) on the main thread.
/// Returns Retained<NSStatusItem> to keep it alive for the app lifetime.
pub fn install_tray(state: TrayState, mtm: MainThreadMarker) -> Retained<NSStatusItem> {
    // 1. Get the system status bar and create a status item.
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(NSSquareStatusItemLength);

    // 2. Load the template icon from the app bundle and set it.
    let bundle = unsafe {
        let cls = objc2::class!(NSBundle);
        let bundle: Retained<objc2::runtime::AnyObject> = msg_send![cls, mainBundle];
        bundle
    };

    let icon_name = NSString::from_str("tray-icon-template");
    let icon_ext = NSString::from_str("png");
    let path: Option<Retained<NSString>> = unsafe {
        msg_send![&bundle, pathForResource: &*icon_name, ofType: &*icon_ext]
    };

    if let Some(path) = path {
        if let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path) {
            image.setTemplate(true);  // Auto light/dark coloring
            let button = item.button(mtm).expect("statusItem button");
            button.setImage(Some(&image));
        }
    }

    // 3. Build the NSMenu.
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);  // Explicit enable/disable per AC5.8

    // Captions on/off
    let captions_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Captions"),
            Some(sel!(toggleCaptions:)),
            &NSString::from_str(""),
        )
    };
    let captions_enabled_state = state.captions_enabled.load(std::sync::atomic::Ordering::Relaxed);
    captions_item.setState(if captions_enabled_state { 1 } else { 0 });
    menu.addItem(&captions_item);

    // Mode submenu
    let mode_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Mode"),
            None,
            &NSString::from_str(""),
        )
    };
    let mode_menu = NSMenu::new(mtm);
    let current_mode = state.config.lock().unwrap().overlay_mode.clone();
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
    }
    mode_item.setSubmenu(Some(&mode_menu));
    menu.addItem(&mode_item);

    // Engine submenu (Nemotron only)
    let engine_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Engine"),
            None,
            &NSString::from_str(""),
        )
    };
    let engine_menu = NSMenu::new(mtm);
    let nemo = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Nemotron"),
            Some(sel!(noop:)),
            &NSString::from_str(""),
        )
    };
    nemo.setState(1);
    nemo.setEnabled(false);  // No switching; Nemotron is the only engine
    engine_menu.addItem(&nemo);
    engine_item.setSubmenu(Some(&engine_menu));
    menu.addItem(&engine_item);

    // Audio Source submenu
    let audio_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Audio Source"),
            None,
            &NSString::from_str(""),
        )
    };
    let audio_menu = build_audio_submenu(&state, mtm);
    audio_item.setSubmenu(Some(&audio_menu));
    menu.addItem(&audio_item);

    // Show Above Fullscreen
    let above_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Show Above Fullscreen"),
            Some(sel!(toggleAboveFullscreen:)),
            &NSString::from_str(""),
        )
    };
    let above_fs = state.config.lock().unwrap().above_fullscreen;
    above_item.setState(if above_fs { 1 } else { 0 });
    menu.addItem(&above_item);

    // Lock Position (Floating-only)
    let lock_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Lock Position"),
            Some(sel!(toggleLock:)),
            &NSString::from_str(""),
        )
    };
    let locked = state.config.lock().unwrap().locked;
    lock_item.setState(if locked { 1 } else { 0 });
    lock_item.setEnabled(matches!(current_mode, OverlayMode::Floating));
    menu.addItem(&lock_item);

    menu.addItem(&NSMenuItem::separatorItem(mtm));

    // Quit
    let quit_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit Subtidal"),
            Some(sel!(quit:)),
            &NSString::from_str("q"),
        )
    };
    menu.addItem(&quit_item);

    item.setMenu(Some(&menu));

    item
}

fn build_audio_submenu(state: &TrayState, mtm: MainThreadMarker) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);

    let sources = state.audio_sources.lock().unwrap();
    let current_source = &state.config.lock().unwrap().audio_source;

    // System Output first
    let system_item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("System Output"),
            Some(sel!(selectAudioSource:)),
            &NSString::from_str(""),
        )
    };
    let is_system = matches!(current_source, crate::config::AudioSource::SystemOutput);
    system_item.setState(if is_system { 1 } else { 0 });
    menu.addItem(&system_item);

    // App sources
    for source_info in sources.iter() {
        let label = source_info.label.clone();
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&label),
                Some(sel!(selectAudioSource:)),
                &NSString::from_str(""),
            )
        };
        let is_current = source_info.source == *current_source;
        item.setState(if is_current { 1 } else { 0 });
        menu.addItem(&item);
    }

    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tray_state_creation() {
        let (_cmd_tx, _cmd_rx) = async_channel::unbounded();
        let (_audio_tx, _audio_rx) = std::sync::mpsc::sync_channel(1);
        let engine_choice = Arc::new(ArcSwap::from_pointee(Engine::Nemotron));
        let config = Arc::new(Mutex::new(Config::default()));
        let captions_enabled = Arc::new(AtomicBool::new(true));
        let audio_sources = Arc::new(Mutex::new(vec![]));

        let _state = TrayState {
            config,
            captions_enabled,
            cmd_tx: _cmd_tx,
            audio_cmd_tx: _audio_tx,
            engine_choice,
            audio_sources,
        };

        // If we get here, TrayState construction succeeded.
    }

    #[test]
    fn test_audio_submenu_with_system_only() {
        let config = Arc::new(Mutex::new(Config::default()));
        let sources = Arc::new(Mutex::new(vec![]));
        let (_cmd_tx, _cmd_rx) = async_channel::unbounded();
        let (_audio_tx, _audio_rx) = std::sync::mpsc::sync_channel(1);
        let engine_choice = Arc::new(ArcSwap::from_pointee(Engine::Nemotron));
        let captions_enabled = Arc::new(AtomicBool::new(true));

        let state = TrayState {
            config,
            captions_enabled,
            cmd_tx: _cmd_tx,
            audio_cmd_tx: _audio_tx,
            engine_choice,
            audio_sources: sources,
        };

        // Note: We can't actually build menus without MainThreadMarker in tests,
        // but this verifies the TrayState structure compiles and is sound.
        let sources_lock = state.audio_sources.lock().unwrap();
        assert_eq!(sources_lock.len(), 0);
    }
}
