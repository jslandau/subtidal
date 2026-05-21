# macOS Port — Phase 2: NSApplication startup + minimal NSPanel

**Goal:** Run `NSApplication.run()` on the main thread with worker threads spawned, show an empty Floating-mode `NSPanel`, and demonstrate the caption-bridge pattern with a hardcoded test caption stream.

**Architecture:** `main_macos::main` acquires `MainThreadMarker`, builds shared state, spawns a Phase-2-only test-harness thread that pushes hardcoded captions onto an `async_channel<String>`, installs a Ctrl-C handler that posts `OverlayCommand::Quit`, then calls `overlay::macos::run_app` (analogue of Linux's `overlay::run_gtk_app`). `run_app` builds the `NSPanel`, spawns a caption-bridge thread that blocks on `caption_rx.recv_blocking()` and posts each result onto the main run loop via `dispatch2::Queue::main().exec_async`, spawns the `OverlayCommand` dispatch loop with the same shape, then calls `NSApplication.run()` which blocks until `terminate(None)`.

**Tech Stack:** `objc2-app-kit` 0.3 (`NSApplication`, `NSPanel`, `NSTextField`, `NSColor`, `NSFont`, `NSAttributedString`, `NSScreen`, `NSEvent`, `NSResponder`, `NSView`, `NSAppearance` features), `objc2-foundation` 0.3 (`NSString`, `NSNotification`, `NSObject`, `NSRunLoop` features), `dispatch2` 0.3 (`Queue::main().exec_async`), `objc2::MainThreadMarker`, `async_channel`, `ctrlc`.

**Scope:** Phase 2 of 8.

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### macos-port.AC2: NSPanel renders correctly across Spaces and fullscreen
- **macos-port.AC2.1 Success:** Panel is constructed with `level = .floating`, `collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]`, `isFloatingPanel = true`, `styleMask` including `.borderless` and `.nonactivatingPanel` (verifiable via inspection helper in unit test).
- **macos-port.AC2.4 Success:** Toggling "Show Above Fullscreen" via the tray switches the panel's `level` between `.floating` and `.statusBar` without panel rebuild; change observable within one OverlayCommand cycle.

### macos-port.AC8: Main-thread caption delivery and shutdown
- **macos-port.AC8.1 Success:** Captions sent from the STT thread arrive at the main thread via the caption bridge (`dispatch::Queue::main().exec_async`) and update the NSTextField; no AppKit thread-affinity panics in the AppKit-warnings log.
- **macos-port.AC8.2 Success:** Cmd-Q triggers a clean shutdown: SCK stream stopped, STT thread exits within 250ms (one `AudioWake::wait_timeout` cycle), audio thread exits, tray thread exits, `NSApplication.run()` returns, `main` exits with code 0.

Phase 2 verifies AC2.1/AC2.4 (panel construction + above-fullscreen toggle) and AC8.1/AC8.2 (main-thread marshaling + clean shutdown) using a **placeholder caption harness** instead of the real STT pipeline / SCK stream — those wire up in Phases 3–4. When the real workers exist they MUST observe the same shutdown signal Phase 2's placeholders observe.

**Codebase finding worth restating:** the Linux entry point lives in `src/main.rs:87-309`, not in `src/main_linux.rs` (which holds only CUDA helpers). Phase 1 Task 4 is therefore a real refactor: extract that body into `main_linux::main()` and make `src/main.rs::main()` a thin cfg-dispatcher. Phase 2's `main_macos::main()` is the macOS sibling of that extracted Linux body.

**Code-vs-contract note:** the objc2 + dispatch2 + AppKit ceremony is verbose. Tasks below specify contracts (which API to call, which feature flag, which thread, which `MainThreadMarker` proof) instead of literal objc2 boilerplate. The task-implementor generates actual objc2 code at execution time against current docs.rs/objc2-app-kit/0.3.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Enable objc2-app-kit / objc2-foundation feature flags

**Files:**
- Modify: `Cargo.toml` (macOS target dep block from Phase 1 Task 1)

**Implementation:**

`objc2-app-kit` 0.3.x is feature-gated per Obj-C class. Replace the bare `objc2-app-kit = "0.3"` and `objc2-foundation = "0.3"` lines added in Phase 1 with feature-enabled entries:

```toml
objc2-foundation = { version = "0.3", features = [
    "NSString", "NSNotification", "NSObject", "NSRunLoop",
] }
objc2-app-kit = { version = "0.3", features = [
    "NSApplication", "NSPanel", "NSWindow", "NSTextField", "NSScreen",
    "NSColor", "NSFont", "NSAttributedString", "NSEvent",
    "NSResponder", "NSView", "NSAppearance",
] }
```

Phase 6 extends this list with `NSMenu`, `NSStatusBar`, `NSStatusItem`, `NSScrollView`, `NSTextView`, `NSSavePanel`, `NSImage`.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin --verbose
```
Expected: green.

**Commit:** `macos: enable objc2-app-kit feature flags for Phase 2 classes`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Build the NSPanel construction module

**Files:**
- Modify: `src/overlay/macos/mod.rs` (replace Phase 1 stub)
- Create: `src/overlay/macos/panel.rs`

**Implementation:**

**`src/overlay/macos/mod.rs`** — replace the Phase 1 stub with:

```rust
// macOS overlay orchestration (NSPanel for caption modes; NSWindow for
// Transcript). Phase 2 ships only the Floating NSPanel + a caption-bridge
// dispatch path. Phase 6 adds Docked geometry, Transcript window, drag, and
// captions-disable surface-clearing.

pub mod panel;
mod app;
pub use app::run_app;
```

**`src/overlay/macos/panel.rs`** contract — one public constructor plus an inspection helper and an above-fullscreen toggle. Return type uses `objc2::rc::Retained<T>` for the NSPanel and content NSTextField.

```text
pub fn build_overlay_panel(mtm: MainThreadMarker, config: &Config)
    -> (Retained<NSPanel>, Retained<NSTextField>)
```

Inside, follow this exact contract (derived from design doc §"Overlay (NSPanel for caption modes, NSWindow for Transcript)"):

| Property | Phase 2 value (Floating mode only) |
|---|---|
| `styleMask` | `NSWindowStyleMask::Borderless \| NSWindowStyleMask::NonactivatingPanel` |
| `backing` | `NSBackingStoreType::Buffered` |
| `defer` | `false` |
| `level` | `NSFloatingWindowLevel`; switch to `NSStatusWindowLevel` when `config.above_fullscreen == true` (maps to the design's `.statusBar`) |
| `collectionBehavior` | `CanJoinAllSpaces \| FullScreenAuxiliary` |
| `isFloatingPanel` | `true` |
| `backgroundColor` | `NSColor::clearColor()` |
| `hasShadow` | `false` (Floating); Phase 6 toggles to `true` for Docked |
| `ignoresMouseEvents` | `true` |
| `isMovableByWindowBackground` | `true` (Floating); Phase 6 toggles per mode |
| Initial frame | `NSRect::new(config.position.x as f64, config.position.y as f64, config.appearance.width as f64, /*natural height from font_size*/ ...)` |

Add an `NSTextField` content view:
- Wrappable (`NSLineBreakMode::ByWordWrapping`)
- `isEditable = false`, `isSelectable = false`, `isBordered = false`, `drawsBackground = false`
- Font: monospace at `config.appearance.font_size` via `NSFont::userFixedPitchFontOfSize_`
- Initial `stringValue = ""`

Provide a public inspection helper (required by `macos-port.AC2.1`, "verifiable via inspection helper in unit test"):

```text
pub struct PanelConfig {
    pub level: i64,
    pub collection_behavior: u64,
    pub is_floating_panel: bool,
    pub style_mask: u64,
    pub ignores_mouse_events: bool,
}

pub fn inspect(panel: &NSPanel) -> PanelConfig
```

Provide an above-fullscreen toggle (required by `macos-port.AC2.4`):

```text
pub fn set_above_fullscreen(panel: &NSPanel, mtm: MainThreadMarker, on: bool)
```

Sets `panel.level` to `NSStatusWindowLevel` when `on`, else `NSFloatingWindowLevel`. Same `Retained<NSPanel>` throughout — no rebuild.

**Reference:** docs.rs/objc2-app-kit/0.3.2 → `NSPanel`, `NSWindowStyleMask`, `NSWindowCollectionBehavior`, `NSWindowLevel`. The objc2 idiom is `unsafe { method(mtm, ...) }` for main-thread-only methods.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```
Cross-target green.

**Commit:** `macos: NSPanel construction module with inspection helper`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Caption bridge + OverlayCommand dispatch loop

**Files:**
- Create: `src/overlay/macos/app.rs`

**Implementation:**

`app.rs` exposes one public function with the same signature as Linux's `run_gtk_app`:

```text
pub fn run_app(
    config: Config,
    caption_rx: async_channel::Receiver<String>,
    cmd_rx: async_channel::Receiver<OverlayCommand>,
    captions_enabled: CaptionsEnabled,
)
```

Inside:

1. **Acquire `MainThreadMarker`.** `MainThreadMarker::new()` returns `None` if not main; panic with a clear message in that case. (The caller in `main_macos::main` will have already proved main-thread-ness at process start, but be explicit at the boundary.)

2. **Get `NSApplication::sharedApplication(mtm)`** and call `setActivationPolicy(NSApplicationActivationPolicy::Accessory)`. Matches `LSUIElement = true` from the Info.plist — no Dock icon.

3. **Build the overlay panel** via `panel::build_overlay_panel(mtm, &config)`. Retain both the `NSPanel` and the content `NSTextField`. Call `panel.orderFront(None)` to make it visible (Floating mode default for Phase 2).

4. **Spawn the caption-bridge thread.** AppKit forbids non-main-thread UI updates, so we cannot run the consumer on a tokio executor. Instead, a dedicated `std::thread` blocks on `caption_rx.recv_blocking()` and posts each result onto the main run loop via `dispatch2`:

   ```text
   let label_ptr = SendablePtr(Retained::as_ptr(&label));
   std::thread::Builder::new()
       .name("caption-bridge".into())
       .spawn({
           let captions_enabled = Arc::clone(&captions_enabled);
           move || {
               while let Ok(text) = caption_rx.recv_blocking() {
                   if !captions_enabled.load(Ordering::Relaxed) { continue; }
                   let text = text.clone();
                   dispatch2::Queue::main().exec_async(move || {
                       let mtm = MainThreadMarker::new()
                           .expect("dispatch main queue runs on main thread");
                       let label: &NSTextField = unsafe { &*label_ptr.0 };
                       let ns = NSString::from_str(&text);
                       unsafe { label.setStringValue(&ns) };
                       // CaptionBuffer / TranscriptLog integration deferred to Phase 6.
                   });
               }
           }
       })?;
   ```

   `SendablePtr` is a small newtype:
   ```text
   struct SendablePtr<T>(*const T);
   unsafe impl<T> Send for SendablePtr<T> {}
   ```
   Justification: the bridge thread treats the pointer as an opaque handle. Only the dispatched main-queue closure dereferences it, and the main queue is AppKit-affine.

5. **Spawn the OverlayCommand dispatch loop** as a second worker thread, same shape:

   ```text
   let panel_ptr = SendablePtr(Retained::as_ptr(&panel));
   let label_ptr = SendablePtr(Retained::as_ptr(&label));
   std::thread::Builder::new()
       .name("overlay-cmd".into())
       .spawn({
           let captions_enabled = Arc::clone(&captions_enabled);
           move || {
               while let Ok(cmd) = cmd_rx.recv_blocking() {
                   let captions_enabled = Arc::clone(&captions_enabled);
                   dispatch2::Queue::main().exec_async(move || {
                       let mtm = MainThreadMarker::new().expect("main");
                       match cmd {
                           OverlayCommand::Quit => {
                               let app = NSApplication::sharedApplication(mtm);
                               unsafe { app.terminate(None) };
                           }
                           OverlayCommand::SetAboveFullscreen(on) => {
                               let panel: &NSPanel = unsafe { &*panel_ptr.0 };
                               panel::set_above_fullscreen(panel, mtm, on);
                           }
                           OverlayCommand::SetCaptionsEnabled(on) => {
                               captions_enabled.store(on, Ordering::Relaxed);
                               if !on {
                                   // Phase 2: clear the label only.
                                   // Phase 6 extends to all 4 surfaces.
                                   let label: &NSTextField = unsafe { &*label_ptr.0 };
                                   let ns = NSString::from_str("");
                                   unsafe { label.setStringValue(&ns) };
                               }
                           }
                           // Phase 2 stubs — Phase 6 fills in real handlers.
                           OverlayCommand::SetVisible(_)
                           | OverlayCommand::SetMode(_)
                           | OverlayCommand::SetLocked(_)
                           | OverlayCommand::UpdateAppearance(_)
                           | OverlayCommand::SetCaption(_) => {
                               eprintln!("info: OverlayCommand {:?} deferred to Phase 6", cmd);
                           }
                       }
                   });
               }
           }
       })?;
   ```

6. **Call `NSApplication::sharedApplication(mtm).run()`** — blocks until `terminate(None)` is called from any dispatched closure (the Quit handler).

7. **After `run()` returns:** workers exit on next iteration when the channels close (from `main_macos::main` dropping the senders). No explicit join needed — mirrors Linux's `_stt_handle` pattern in `src/main.rs:207`.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```
Cross-target green. End-to-end exercise in Task 6.

**Commit:** `macos: caption bridge + OverlayCommand dispatch loop`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Replace main_macos.rs stub with full startup orchestration

**Files:**
- Modify: `src/main_macos.rs` (replace Phase 1 hello-world stub)

**Implementation:**

`main_macos::main` does the following, in order:

1. **Acquire `MainThreadMarker`** at the very top. Panic on `None` (binaries always start on main, but be explicit).

2. **Load config** via the existing neutral `Config::load_or_default()` (or whatever helper name `src/config.rs` exposes; reuses the cfg-gated `config_path()` from Phase 1 Task 2).

3. **Construct shared state:** `captions_enabled: CaptionsEnabled = Arc::new(AtomicBool::new(true))`. (`AudioWake` and `ArcSwap<Engine>` are placeholders in Phase 2; Phases 3–4 wire them.)

4. **Build async channels** matching the Linux shape (`src/main.rs:203-204`):
   - `(caption_tx, caption_rx) = async_channel::unbounded::<String>()`
   - `(cmd_tx, cmd_rx) = async_channel::unbounded::<OverlayCommand>()`

5. **Spawn the Phase-2-only test caption harness** (proves AC8.1 end-to-end without the real STT pipeline):

   ```text
   // Phase 2 only — remove when STT thread lands in Phase 3.
   {
       let tx = caption_tx.clone();
       std::thread::Builder::new()
           .name("test-caption-harness".into())
           .spawn(move || {
               let samples = [
                   "Hello",
                   "Hello, world",
                   "Hello, world, from macOS Subtidal",
               ];
               for s in samples {
                   std::thread::sleep(std::time::Duration::from_millis(1000));
                   if tx.send_blocking(s.to_string()).is_err() {
                       return;
                   }
               }
               // Then idle — leaves the panel showing the last caption.
           })
           .expect("spawn test-caption-harness");
   }
   ```

6. **Install Ctrl-C handler** (mirrors `src/main.rs:287-300`):

   ```text
   let cmd_tx_signal = cmd_tx.clone();
   ctrlc::set_handler(move || {
       let _ = cmd_tx_signal.send_blocking(OverlayCommand::Quit);
   })
   .expect("install ctrlc handler");
   ```

7. **Call `overlay::macos::run_app(config, caption_rx, cmd_rx, captions_enabled)`** — blocks until shutdown.

8. **After `run_app` returns:** drop `caption_tx` and `cmd_tx` so worker threads see closure. Exit via `std::process::exit(0)` (macOS doesn't need the Linux `exit_without_atexit` quirk — there's no argv[0] CUDA-stub issue).

**Skeleton:**

```text
pub fn main() {
    let _mtm = objc2::MainThreadMarker::new()
        .expect("main_macos::main must run on the main thread");

    let config = config::Config::load_or_default();
    let captions_enabled: overlay::CaptionsEnabled =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    let (caption_tx, caption_rx) = async_channel::unbounded::<String>();
    let (cmd_tx, cmd_rx) = async_channel::unbounded::<overlay::OverlayCommand>();

    // Phase 2 only — remove when STT thread lands in Phase 3.
    spawn_test_caption_harness(caption_tx.clone());

    let cmd_tx_signal = cmd_tx.clone();
    ctrlc::set_handler(move || {
        let _ = cmd_tx_signal.send_blocking(overlay::OverlayCommand::Quit);
    })
    .expect("install ctrlc handler");

    overlay::macos::run_app(config, caption_rx, cmd_rx, captions_enabled);

    drop(caption_tx);
    drop(cmd_tx);
    std::process::exit(0);
}
```

Adapt imports and `Config::load_or_default()` to the actual existing helper name from `src/config.rs`.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
cargo check --lib
```
Cross-target green, Linux unaffected.

**Commit:** `macos: full main_macos startup orchestration with test caption harness`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->
<!-- START_TASK_5 -->
### Task 5: Unit test for panel configuration

**Verifies:** macos-port.AC2.1, macos-port.AC2.4

**Files:**
- Modify: `src/overlay/macos/panel.rs` — add `#[cfg(all(test, target_os = "macos"))] mod tests`

**Implementation:**

Two test cases, in-file per the codebase convention (mirrors `src/stt/mod.rs:247-288`).

1. **`panel_constructed_with_required_flags` (AC2.1):**
   - Acquire `MainThreadMarker::new()` — if `None` (rare in `cargo test`), skip the test with `eprintln!` and return; do not panic.
   - Build a default `Config` (use `Config::default()` if exists, else construct minimally).
   - Call `build_overlay_panel(mtm, &config)`.
   - Call `inspect(&panel)` and assert:
     - `pc.is_floating_panel == true`.
     - `pc.style_mask & NSWindowStyleMask::Borderless.0 != 0`.
     - `pc.style_mask & NSWindowStyleMask::NonactivatingPanel.0 != 0`.
     - `pc.collection_behavior & NSWindowCollectionBehavior::CanJoinAllSpaces.0 != 0`.
     - `pc.collection_behavior & NSWindowCollectionBehavior::FullScreenAuxiliary.0 != 0`.
     - `pc.ignores_mouse_events == true`.
     - `pc.level == NSFloatingWindowLevel as i64`.

2. **`above_fullscreen_toggle_changes_level` (AC2.4):**
   - Build the panel.
   - `inspect(&panel).level == NSFloatingWindowLevel`.
   - `set_above_fullscreen(&panel, mtm, true)`; re-inspect; level is now `NSStatusWindowLevel`.
   - `set_above_fullscreen(&panel, mtm, false)`; re-inspect; level is back to `NSFloatingWindowLevel`.
   - Same `Retained<NSPanel>` pointer throughout (no rebuild).

**Verification:**

On macOS:
```bash
cargo test --lib -- panel::tests
```
Expected: both tests pass.

On Linux:
```bash
cargo check --lib --target x86_64-apple-darwin
```
Expected: green (test module is target-os-gated and not built on Linux).

**Commit:** `macos: unit tests for NSPanel construction and above-fullscreen toggle`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: End-to-end manual verification on real hardware

**Verifies:** macos-port.AC2.1, macos-port.AC2.4, macos-port.AC8.1, macos-port.AC8.2

**Files:** none (operational verification only)

**Implementation:**

On the target Apple Silicon Mac:

```bash
scripts/bundle-mac.sh
open target/release/Subtidal.app
```

Observe:

1. **AC2.1 (panel construction):** A small Floating NSPanel appears at `config.position` (default `{x:100, y:100}` on first run). Borderless, transparent background, no shadow.

2. **AC8.1 (main-thread caption delivery):** Within ~3 seconds the panel cycles through:
   - "Hello"
   - "Hello, world"
   - "Hello, world, from macOS Subtidal"
   Open Console.app and filter on process "subtidal"/"Subtidal" — no AppKit thread-affinity warnings.

3. **AC2.4 (above-fullscreen toggle):** No tray exists yet in Phase 2; the unit test in Task 5 covers the construction path. Full end-to-end AC2.4 verification re-runs in Phase 6 once the tray menu can post `OverlayCommand::SetAboveFullscreen`.

4. **AC8.2 (clean shutdown):**
   - Launch from a terminal directly so signals reach the process: `./target/release/Subtidal.app/Contents/MacOS/subtidal`.
   - Send Ctrl-C; the panel disappears, the process exits with code 0 (`echo $?`), and `pgrep -f subtidal` returns nothing.
   - Cmd-Q via Activity Monitor's "Quit" also works (delivers a graceful terminate).

5. **AC9.1 (cross-target CI):** push the branch; `x86_64-apple-darwin` cross-target check passes. (`aarch64-apple-darwin` matrix entry lands in Phase 7.)

**If Cmd-Q does not terminate cleanly:** investigate `ctrlc`'s interaction with `NSApplication`'s run loop. The likely remediation is to install an `NSApplicationDelegate` with `applicationShouldTerminate` returning `NSTerminateNow` after posting `OverlayCommand::Quit`, instead of relying solely on `ctrlc`. Flag and surface to the user rather than silently working around.

**Commit:** none (verification only).
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_C -->
