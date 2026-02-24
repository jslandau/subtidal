# Live Captions Implementation Plan — Phase 6: System Tray

**Goal:** System tray icon with left-click toggle and full right-click menu: audio source, overlay mode, STT engine, lock toggle, settings, quit.

**Architecture:** `TrayState` implements `ksni::Tray`. Its `menu()` method builds the menu from current state fields. Menu item callbacks send commands via `glib::Sender<OverlayCommand>` (overlay) and `std::sync::mpsc::SyncSender<AudioCommand>` (audio). The tray spawns on the existing Tokio runtime as an async task. `handle.update()` is called whenever any subsystem changes state.

**Tech Stack:** ksni 0.3, tokio 1 (already in Cargo.toml).

**Scope:** Phase 6 of 8. Depends on Phases 3, 4, 5.

**Codebase verified:** 2026-02-22 — greenfield, no src/tray/ exists.

---

## Acceptance Criteria Coverage

### live-captions.AC4: System tray controls work correctly
- **live-captions.AC4.1 Success:** Left-clicking tray icon toggles captions on and off
- **live-captions.AC4.2 Success:** Right-click menu reflects current state (active source, engine, overlay mode, lock state)
- **live-captions.AC4.3 Success:** Audio source submenu shows all available PipeWire sinks and application streams as a radio group
- **live-captions.AC4.4 Success:** STT engine can be switched at runtime via tray menu without restart
- **live-captions.AC4.5 Success:** Overlay lock/unlock menu item is greyed out in docked mode

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Create src/tray/mod.rs — TrayState and menu structure

**Files:**
- Create: `src/tray/mod.rs`
- Create: `src/tray/` directory

**Step 1: Create src/tray/mod.rs**

```rust
//! System tray via ksni StatusNotifierItem.

use crate::audio::{AudioCommand, AudioNode, NodeList};
use crate::config::{AudioSource, Engine, OverlayMode};
use crate::overlay::OverlayCommand;
use ksni::{menu::*, Tray, TrayMethods};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::SyncSender,
    Arc, Mutex,
};

/// Full state of the tray — the menu is built fresh from these fields on every update.
pub struct TrayState {
    pub captions_enabled: Arc<AtomicBool>,
    pub active_source: AudioSource,
    pub overlay_mode: OverlayMode,
    pub locked: bool,
    pub active_engine: Engine,
    pub cuda_warning: Option<&'static str>,
    /// Available PipeWire nodes — refreshed from NodeList on each menu open.
    pub audio_nodes: Vec<AudioNode>,
    /// Channel to send OverlayCommand to the GTK4 main thread.
    pub overlay_tx: glib::Sender<OverlayCommand>,
    /// Channel to send AudioCommand to the PipeWire thread.
    pub audio_tx: SyncSender<AudioCommand>,
    /// Channel to send engine-switch command to the inference thread.
    pub engine_tx: SyncSender<EngineCommand>,
    /// Shared node list from audio thread.
    pub node_list: NodeList,
}

/// Commands for switching the STT engine at runtime.
pub enum EngineCommand {
    Switch(Engine),
}

impl TrayState {
    /// Toggle captions on/off and notify the overlay. Single source of truth for
    /// the toggle — called from both left-click (activate) and the Captions checkmark.
    fn toggle_captions(&mut self) {
        let enabled = self.captions_enabled.fetch_xor(true, Ordering::Relaxed);
        let _ = self.overlay_tx.send(OverlayCommand::SetVisible(!enabled));
    }
}

impl Tray for TrayState {
    fn icon_name(&self) -> String {
        if self.captions_enabled.load(Ordering::Relaxed) {
            "microphone-sensitivity-high-symbolic".to_string()
        } else {
            "microphone-disabled-symbolic".to_string()
        }
    }

    fn id(&self) -> String {
        "live-captions".to_string()
    }

    fn title(&self) -> String {
        if let Some(warn) = self.cuda_warning {
            format!("Live Captions — ⚠ {warn}")
        } else {
            "Live Captions".to_string()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left-click: toggle captions on/off (AC4.1).
        // Delegates to the same internal method as the Captions checkmark to avoid
        // double-toggle if ksni ever calls both activate() and menu item activate.
        self.toggle_captions();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        // Refresh audio node list from shared NodeList on each menu open.
        let nodes = self.node_list.lock().unwrap().clone();

        vec![
            // --- Captions on/off ---
            CheckmarkItem {
                label: "Captions".to_string(),
                checked: self.captions_enabled.load(Ordering::Relaxed),
                activate: Box::new(|tray: &mut TrayState| {
                    tray.toggle_captions();
                }),
                ..Default::default()
            }
            .into(),

            MenuItem::Separator,

            // --- Audio Source submenu ---
            SubMenu {
                label: "Audio Source".to_string(),
                submenu: build_audio_source_submenu(&self.active_source, &nodes),
                ..Default::default()
            }
            .into(),

            // --- Overlay submenu ---
            SubMenu {
                label: "Overlay".to_string(),
                submenu: build_overlay_submenu(self),
                ..Default::default()
            }
            .into(),

            // --- STT Engine submenu ---
            SubMenu {
                label: "STT Engine".to_string(),
                submenu: build_engine_submenu(&self.active_engine),
                ..Default::default()
            }
            .into(),

            MenuItem::Separator,

            // --- Settings ---
            StandardItem {
                label: "Settings...".to_string(),
                icon_name: "preferences-system-symbolic".to_string(),
                activate: Box::new(|_tray: &mut TrayState| {
                    let config_path = crate::config::Config::config_path();
                    let _ = std::process::Command::new("xdg-open")
                        .arg(config_path)
                        .spawn();
                }),
                ..Default::default()
            }
            .into(),

            // --- Quit ---
            StandardItem {
                label: "Quit".to_string(),
                icon_name: "application-exit-symbolic".to_string(),
                activate: Box::new(|tray: &mut TrayState| {
                    // Send shutdown to audio thread first, then tell GTK to quit cleanly.
                    let _ = tray.audio_tx.send(AudioCommand::Shutdown);
                    let _ = tray.overlay_tx.send(OverlayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn build_audio_source_submenu(
    active: &AudioSource,
    nodes: &[AudioNode],
) -> Vec<MenuItem<TrayState>> {
    // System output is always the first option (AC4.3).
    let system_selected = matches!(active, AudioSource::SystemOutput);

    let mut items: Vec<MenuItem<TrayState>> = vec![RadioGroup {
        selected: if system_selected { 0 } else {
            nodes.iter().position(|n| {
                if let AudioSource::Application { node_id, .. } = active {
                    n.node_id == *node_id
                } else {
                    false
                }
            })
            .map(|i| i + 1)
            .unwrap_or(0)
        },
        select: Box::new(|tray: &mut TrayState, idx: usize| {
            let nodes = tray.node_list.lock().unwrap().clone();
            let new_source = if idx == 0 {
                AudioSource::SystemOutput
            } else if let Some(node) = nodes.get(idx - 1) {
                AudioSource::Application {
                    node_id: node.node_id,
                    node_name: node.name.clone(),
                }
            } else {
                AudioSource::SystemOutput
            };
            tray.active_source = new_source.clone();
            let _ = tray.audio_tx.send(AudioCommand::SwitchSource(new_source));
        }),
        options: {
            let mut opts = vec![RadioItem {
                label: "System Output".to_string(),
                enabled: true,
                ..Default::default()
            }];
            for node in nodes {
                // Disambiguate duplicate names with PID (PipeWire node ID).
                let label = format!("{} (id:{})", node.description, node.node_id);
                opts.push(RadioItem {
                    label,
                    enabled: true,
                    ..Default::default()
                });
            }
            opts
        },
        ..Default::default()
    }
    .into()];

    items
}

fn build_overlay_submenu(tray: &TrayState) -> Vec<MenuItem<TrayState>> {
    let is_docked = tray.overlay_mode == OverlayMode::Docked;
    vec![
        // Docked / Floating radio.
        RadioGroup {
            selected: if is_docked { 0 } else { 1 },
            select: Box::new(|tray: &mut TrayState, idx: usize| {
                let mode = if idx == 0 { OverlayMode::Docked } else { OverlayMode::Floating };
                tray.overlay_mode = mode.clone();
                let _ = tray.overlay_tx.send(OverlayCommand::SetMode(mode));
            }),
            options: vec![
                RadioItem { label: "Docked".to_string(), enabled: true, ..Default::default() },
                RadioItem { label: "Floating".to_string(), enabled: true, ..Default::default() },
            ],
            ..Default::default()
        }
        .into(),

        MenuItem::Separator,

        // Lock overlay position (disabled in docked mode) — AC4.5.
        CheckmarkItem {
            label: "Lock Overlay Position".to_string(),
            checked: tray.locked,
            enabled: !is_docked, // greyed out in docked mode (AC4.5)
            activate: Box::new(|tray: &mut TrayState| {
                if tray.overlay_mode == OverlayMode::Floating {
                    tray.locked = !tray.locked;
                    let _ = tray.overlay_tx.send(OverlayCommand::SetLocked(tray.locked));
                }
            }),
            ..Default::default()
        }
        .into(),
    ]
}

fn build_engine_submenu(active: &Engine) -> Vec<MenuItem<TrayState>> {
    vec![RadioGroup {
        selected: if *active == Engine::Parakeet { 0 } else { 1 },
        select: Box::new(|tray: &mut TrayState, idx: usize| {
            let engine = if idx == 0 { Engine::Parakeet } else { Engine::Moonshine };
            tray.active_engine = engine.clone();
            let _ = tray.engine_tx.send(EngineCommand::Switch(engine));
        }),
        options: vec![
            RadioItem {
                label: "Parakeet (GPU)".to_string(),
                enabled: true,
                ..Default::default()
            },
            RadioItem {
                label: "Moonshine (CPU)".to_string(),
                enabled: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
    .into()]
}

/// Spawn the system tray on the Tokio runtime.
/// Returns a ksni Handle for calling `handle.update(...)` from other threads.
pub fn spawn_tray(
    tray_state: TrayState,
    runtime: &tokio::runtime::Runtime,
) -> ksni::Handle<TrayState> {
    runtime.block_on(async {
        tray_state.spawn().await.expect("spawning ksni tray")
    })
}
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Wire tray into main.rs

**Files:**
- Modify: `src/main.rs`

**Verifies:** live-captions.AC4.1, live-captions.AC4.2, live-captions.AC4.3, live-captions.AC4.4, live-captions.AC4.5

**Step 1: Add mod tray and EngineCommand channel to src/main.rs**

At the top: `mod tray;`

**Step 2: Add engine-switch channel and spawn tray before GTK4 app**

After the inference thread is spawned (Phase 4), before `run_gtk_app`:

```rust
// Phase 6: Set up engine-switch channel.
let (engine_switch_tx, engine_switch_rx) = std::sync::mpsc::sync_channel::<tray::EngineCommand>(4);

// Wire engine-switch receiver (restarts inference thread on switch — Phase 8 completes this).
{
    let audio_tx_for_engine = audio_cmd_tx.clone();
    std::thread::spawn(move || {
        for cmd in engine_switch_rx.iter() {
            match cmd {
                tray::EngineCommand::Switch(new_engine) => {
                    eprintln!("info: engine switch to {new_engine:?} — respawn inference (Phase 8)");
                    // Full respawn logic wired in Phase 8.
                }
            }
        }
    });
}

// Spawn the system tray.
let tray_state = tray::TrayState {
    captions_enabled: Arc::clone(&captions_enabled),
    active_source: cfg.audio_source.clone(),
    overlay_mode: cfg.overlay_mode.clone(),
    locked: cfg.locked,
    active_engine: active_engine.clone(),
    cuda_warning: cuda_fallback_warning,
    audio_nodes: Vec::new(),
    overlay_tx: glib_cmd_tx.clone(),
    audio_tx: audio_cmd_tx.clone(),
    engine_tx: engine_switch_tx,
    node_list: Arc::clone(&node_list),
};

// Use the already-built tokio runtime (from Phase 2 model download).
// Store runtime in an Arc so it lives past this scope.
let tray_handle = tray::spawn_tray(tray_state, &runtime);

// Tray handle is stored so Phase 8 can call handle.update() when state changes.
let _ = tray_handle; // used in Phase 8
```

**Step 3: Build**

```bash
cargo build
```

**Step 4: Test tray appears**

```bash
cargo run
```

Expected: System tray icon appears in KDE Plasma 6 notification area. Right-click shows the menu structure.

**Step 5: Test AC4.1 — left-click toggle**

Left-click the tray icon → captions overlay hides. Left-click again → overlay shows.

**Step 6: Test AC4.3 — audio source submenu**

Right-click → Audio Source: should show "System Output" as first radio option, followed by any running application audio streams.

**Step 7: Test AC4.5 — lock item greyed out in docked mode**

Config has `overlay_mode = "docked"` (default). Right-click → Overlay → "Lock Overlay Position" should appear greyed out.

**Step 8: Commit**

```bash
git add src/tray/ src/main.rs
git commit -m "feat: system tray — ksni StatusNotifierItem with full right-click menu"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
