# Live Captions Implementation Plan — Phase 7: Config Persistence and Hot-reload

**Goal:** All runtime state changes persist to config.toml; editing background color, text color, or font size in config.toml applies to the live overlay within 1 second without restart.

**Architecture:** `src/config.rs` gains a `start_hot_reload` function that uses `notify-debouncer-mini` to watch `config.toml`. When a modification event fires (debounced to 500ms), it reads the new config, extracts the appearance fields, and sends `OverlayCommand::UpdateAppearance` via the existing glib channel. Tray state changes call `Config::save()` directly on each mutation.

**Tech Stack:** notify 8, notify-debouncer-mini (add to Cargo.toml).

**Scope:** Phase 7 of 8. Depends on Phases 5, 6.

**Codebase verified:** 2026-02-22 — src/config.rs established in Phase 1.

---

## Acceptance Criteria Coverage

### live-captions.AC6: Configuration persists and hot-reloads
- **live-captions.AC6.1 Success:** Audio source, engine, overlay mode and position persist across restarts
- **live-captions.AC6.2 Success:** Editing background color, text color, or font size in config.toml applies to the live overlay within 1 second
- **live-captions.AC6.3 Failure:** A malformed config.toml causes a warning log but the app starts with defaults

---

**Additional Cargo.toml dependency for Phase 7:**

```toml
notify-debouncer-mini = "0.4"
```

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add hot-reload watcher to src/config.rs

**Files:**
- Modify: `src/config.rs` (extend the file from Phase 1)

**Verifies:** live-captions.AC6.2, live-captions.AC6.3

**Step 1: Add dependency to Cargo.toml**

```toml
notify-debouncer-mini = "0.4"
```

**Note on `notify` direct dep:** The `Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>` type annotation in `main.rs` directly names `notify::RecommendedWatcher`. The `notify = "8"` crate is already declared in Phase 1's Cargo.toml — confirm it is present. Without it as a direct dependency, `rustc` may not resolve the type even though `notify-debouncer-mini` pulls it in transitively.

**Step 2: Add the following to src/config.rs**

Add at the top of `src/config.rs`:

```rust
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use std::time::Duration;
```

Add this function after the existing `Config` impl block:

```rust
/// Start watching config.toml for changes. When appearance fields change,
/// sends an UpdateAppearance command to the overlay.
///
/// Returns the debouncer watcher (must be kept alive for the lifetime of the watch).
/// Drop the returned watcher to stop watching.
pub fn start_hot_reload(
    overlay_tx: glib::Sender<crate::overlay::OverlayCommand>,
) -> anyhow::Result<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> {
    let config_path = Config::config_path();

    // Ensure the config directory exists (it should from startup, but guard here).
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Debounce at 500ms: multiple rapid writes (e.g. from an editor) collapse into one event.
    let mut debouncer = new_debouncer(Duration::from_millis(500), move |result: DebounceEventResult| {
        match result {
            Ok(_events) => {
                // Config file changed: reload and extract appearance.
                match Config::load_from(&Config::config_path()) {
                    Ok(new_cfg) => {
                        let _ = overlay_tx.send(
                            crate::overlay::OverlayCommand::UpdateAppearance(new_cfg.appearance)
                        );
                    }
                    Err(e) => {
                        // AC6.3: malformed TOML → warn and keep current appearance.
                        eprintln!("warn: config hot-reload failed (malformed TOML): {e}");
                        eprintln!("warn: keeping current overlay appearance");
                    }
                }
            }
            Err(e) => {
                eprintln!("warn: config file watch error: {e:?}");
            }
        }
    })?;

    // Watch the config file itself (NonRecursive = only the file).
    debouncer.watcher().watch(
        &config_path,
        notify::RecursiveMode::NonRecursive,
    )?;

    Ok(debouncer)
}
```

Also make `Config::load_from` pub (it's currently private):

```rust
// Change: fn load_from → pub fn load_from
pub fn load_from(path: &Path) -> Result<Config> {
    // ... existing implementation ...
}
```

**Step 3: Verify compilation**

```bash
cargo check
```
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Save config on tray state changes

**Files:**
- Modify: `src/tray/mod.rs` (extend each menu item activate callback to save config)

**Verifies:** live-captions.AC6.1

**Step 1: Add save-on-change to audio source selection**

In `build_audio_source_submenu`'s `select` closure, after setting `tray.active_source`, add:

```rust
// Persist audio source change to config.
let mut cfg = crate::config::Config::load();
cfg.audio_source = tray.active_source.clone();
if let Err(e) = cfg.save() {
    eprintln!("warn: failed to save config: {e}");
}
```

**Step 2: Add save-on-change to overlay mode selection**

In `build_overlay_submenu`'s mode `select` closure:

```rust
let mut cfg = crate::config::Config::load();
cfg.overlay_mode = tray.overlay_mode.clone();
if let Err(e) = cfg.save() {
    eprintln!("warn: failed to save config: {e}");
}
```

**Step 3: Add save-on-change to lock toggle**

In `build_overlay_submenu`'s lock `activate` closure:

```rust
let mut cfg = crate::config::Config::load();
cfg.locked = tray.locked;
if let Err(e) = cfg.save() {
    eprintln!("warn: failed to save config: {e}");
}
```

**Step 4: Add save-on-change to engine selection**

In `build_engine_submenu`'s `select` closure:

```rust
let mut cfg = crate::config::Config::load();
cfg.engine = tray.active_engine.clone();
if let Err(e) = cfg.save() {
    eprintln!("warn: failed to save config: {e}");
}
```

**Step 5: Add position save in overlay drag handler**

In `src/overlay/mod.rs`, in the `gesture.connect_released` callback (Phase 5, Task 4):

```rust
let x = win_for_release.margin(Edge::Left);
let y = win_for_release.margin(Edge::Top);
let mut cfg = crate::config::Config::load();
cfg.position.x = x;
cfg.position.y = y;
if let Err(e) = cfg.save() {
    eprintln!("warn: failed to save position: {e}");
}
```

**Step 6: Verify compilation**

```bash
cargo check
```
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Wire hot-reload into main.rs and test

**Files:**
- Modify: `src/main.rs`

**Verifies:** live-captions.AC6.1, live-captions.AC6.2, live-captions.AC6.3

**Step 1: Start hot-reload watcher before GTK4 app loop**

Before the `overlay::run_gtk_app(...)` call in `src/main.rs`:

```rust
// Phase 7: Start config hot-reload watcher.
// _config_watcher must stay in scope until process exit (drop = stop watching).
// Typed as Option so the failure path compiles without a dummy Debouncer.
let _config_watcher: Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>> =
    match config::start_hot_reload(glib_cmd_tx.clone()) {
        Ok(watcher) => {
            eprintln!("info: config hot-reload active (watching config.toml)");
            Some(watcher)
        }
        Err(e) => {
            eprintln!("warn: config hot-reload unavailable: {e}");
            eprintln!("warn: config.toml changes will require a restart to take effect");
            None
        }
    };
```

**⚠️ Note on watcher lifetime:** `_config_watcher` must remain in scope until the process exits. Place it in the same scope as `run_gtk_app` (which blocks), so it stays alive. The `Option` wrapper means the type annotation is required (the compiler cannot infer the inner type when the `None` arm is taken).

**Step 2: Build and run**

```bash
cargo build
cargo run
```

**Step 3: Test AC6.2 — hot-reload within 1 second**

While the app is running:

```bash
# Open config in editor:
nano ~/.config/live-captions/config.toml
# Change: background_color = "rgba(255,0,0,0.8)"
# Save file.
# Expected: Within 1 second, overlay background turns red.
```

**Step 4: Test AC6.3 — malformed config graceful handling**

```bash
echo "INVALID [[[ TOML" > ~/.config/live-captions/config.toml
# Expected: Warning in app output: "warn: config hot-reload failed (malformed TOML): ..."
# Expected: Overlay appearance unchanged (keeps previous values).
# Restore:
cargo run -- --reset-config
```

**Step 5: Test AC6.1 — persistence across restart**

```bash
# Via tray: switch audio source to an application node.
# Kill app (Ctrl-C).
# Restart:
cargo run
# Expected: same audio source is active (read from config.toml).
```

**Step 6: Commit**

```bash
git add src/config.rs src/tray/mod.rs src/overlay/mod.rs src/main.rs Cargo.toml Cargo.lock
git commit -m "feat: config persistence and hot-reload — notify watcher, save on tray changes"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
