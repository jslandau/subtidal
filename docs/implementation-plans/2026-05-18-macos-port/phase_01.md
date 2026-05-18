# macOS Port — Phase 1: Skeleton wiring + `.app` bundle

**Goal:** Produce a launchable `.app` bundle from `cargo build` on macOS, with all cfg-gating in place so the cross-target CI check stays green and Phase 2+ can drop in real implementations behind stable module boundaries.

**Architecture:** Mirror the existing Linux scaffolding. Add empty `impl_macos.rs` / `macos/mod.rs` siblings, a `main_macos` stub entry point, and a minimal `Subtidal.app` wrapper script. Un-gate the `models` module (and `hf-hub` dep) so cross-platform model download is possible; cfg-gate config and model directory paths to per-OS conventions. No functional behavior yet.

**Tech Stack:** `objc2 = "0.6"`, `objc2-foundation = "0.3"`, `objc2-app-kit = "0.3"`, `objc2-core-foundation = "0.3"`, `dispatch2 = "0.3"`, `block2 = "0.6"`, `hf-hub = "0.5"` (now cross-platform), Bash for bundle script, plist XML.

**Scope:** Phase 1 of 8.

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

This phase is infrastructure-only. **Verifies: None.**

Verification is operational per the design's "Done when": `cargo build --release` succeeds on macOS; `scripts/bundle-mac.sh` produces `target/release/Subtidal.app`; launching it prints the hello message and exits; `cargo check --lib --target x86_64-apple-darwin` remains green; `plutil -lint` passes.

**Note on Phase 0 sequencing:** Phase 0 Task 4 (run the spike on Apple Silicon) imports `subtidal::models`, which Task 2 of this phase un-gates from Linux-only. If Phase 0 Task 4 has not yet been run on hardware, complete Phase 1 Task 2 first.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Extend Cargo.toml macOS dependency block

**Files:**
- Modify: `Cargo.toml` — extend the macOS-conditional block created in Phase 0 Task 1.

**Implementation:**

Replace the Phase 0 macOS block with the fuller set below. Adds objc2 family, GCD bindings, and `hf-hub` (so `models::ensure_nemotron_models` is available on macOS). `objc2-screen-capture-kit` and `objc2-core-media` are intentionally deferred to Phase 4.

```toml
[target.'cfg(target_os = "macos")'.dependencies]
parakeet-rs = { version = "0.3.4", features = ["webgpu"] }
ort = { version = "2.0.0-rc.12", features = ["webgpu"] }
hound = "3.5"
# AppKit bindings (used Phase 2+); declared up-front so cross-target check exercises them.
objc2 = "0.6"
objc2-foundation = "0.3"
objc2-app-kit = "0.3"
objc2-core-foundation = "0.3"
# Grand Central Dispatch for caption-bridge main-thread marshaling (Phase 2+).
dispatch2 = "0.3"
block2 = "0.6"
# Model download via HuggingFace Hub (cross-platform now; was Linux-only).
hf-hub = { version = "0.5", features = ["tokio"] }
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin --verbose
cargo check --lib
```
Both must succeed. If `hf-hub` fails to cross-compile (the existing Cargo.toml comment hints at rustls/ring issues), switch to `features = ["tokio", "native-tls"]` and try again; capture the resolution in the commit message.

**Commit:** `macos: add objc2 family, dispatch2, hf-hub to target deps`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Un-gate models module and cfg-gate platform-specific paths

**Files:**
- Modify: `src/lib.rs:13-14` — remove the `#[cfg(target_os = "linux")]` attribute from `pub mod models;`
- Modify: `src/models/mod.rs` — cfg-gate `nemotron_model_dir()` per OS
- Modify: `src/config.rs` — cfg-gate the config-path helper per OS

**Implementation:**

**`src/lib.rs`:** change

```rust
#[cfg(target_os = "linux")]
pub mod models;
```

to:

```rust
pub mod models;
```

**`src/models/mod.rs`:** read the current `nemotron_model_dir()` (or equivalent) helper. Replace with a cfg-gated pair:

```rust
#[cfg(target_os = "linux")]
pub fn nemotron_model_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("subtidal")
        .join("models")
        .join("nemotron")
}

#[cfg(target_os = "macos")]
pub fn nemotron_model_dir() -> PathBuf {
    // ~/Library/Application Support/Subtidal/models/nemotron/
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Subtidal")
        .join("models")
        .join("nemotron")
}
```

`dirs::data_dir()` already returns the per-platform base (`~/.local/share` on Linux, `~/Library/Application Support` on macOS). The only delta is subdirectory casing: lowercase `subtidal` on Linux (XDG convention), capitalized `Subtidal` on macOS (Apple convention).

**`src/config.rs`:** locate the existing config-path helper (likely a `config_path()` or `Config::load_path` free function) and apply the same cfg-gated split:

```rust
#[cfg(target_os = "linux")]
fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
        .join("subtidal")
        .join("config.toml")
}

#[cfg(target_os = "macos")]
fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
        .join("Subtidal")
        .join("config.toml")
}
```

(`dirs::config_dir()` returns `~/.config` on Linux and `~/Library/Application Support` on macOS.) Adapt to the existing helper's exact signature.

**Verification:**

```bash
cargo check --lib
cargo check --lib --target x86_64-apple-darwin
cargo test --lib
```
Linux build + tests stay green; cross-target check passes.

**Commit:** `macos: cross-platform models + cfg-gated config/model paths`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Create empty macOS implementation stubs

**Files:**
- Create: `src/audio/impl_macos.rs`
- Create: `src/tray/impl_macos.rs`
- Create: `src/overlay/macos/mod.rs`
- Create: `src/main_macos.rs`

**Implementation:**

Each file is a minimal skeleton. Real bodies arrive in later phases.

**`src/audio/impl_macos.rs`:**
```rust
// macOS audio capture implementation (ScreenCaptureKit).
// Skeleton only; populated in Phase 4 (SystemOutput capture) and Phase 5 (per-app).
```

**`src/tray/impl_macos.rs`:**
```rust
// macOS tray implementation (NSStatusItem).
// Skeleton only; populated in Phase 6.
```

**`src/overlay/macos/mod.rs`:**
```rust
// macOS overlay orchestration (NSPanel / NSWindow).
// Skeleton only; populated in Phase 2 (panel + caption bridge) and Phase 6 (full modes).
```

**`src/main_macos.rs`:**
```rust
// macOS startup entry point. Phase 1 stub: prints a hello message and exits.
// Phase 2 replaces this with full NSApplication startup orchestration.

pub fn main() {
    println!("Hello from macOS Subtidal");
}
```

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
```
Expected: green.

**Commit:** `macos: add empty platform-impl stubs`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Wire cfg-gated mod declarations and main.rs dispatch

**Files:**
- Modify: `src/audio/mod.rs:11-17`
- Modify: `src/overlay/mod.rs:12-16`
- Modify: `src/tray/mod.rs:8-12`
- Modify: `src/main.rs:7, 16-17, fn main` (current Linux-only body around line 87)
- Possibly modify: `src/main_linux.rs` — extract the Linux startup body into a `pub fn main()` if it doesn't already exist

**Implementation:**

**`src/audio/mod.rs`** — append after the existing Linux gate:
```rust
#[cfg(target_os = "macos")]
mod impl_macos;
// No re-exports yet; macOS public surface is built up in Phases 4-5.
```

**`src/overlay/mod.rs`** — append after the existing Linux subtree gate:
```rust
#[cfg(target_os = "macos")]
mod macos;
// No re-exports yet; macOS public surface is built up in Phase 2.
```

**`src/tray/mod.rs`** — append:
```rust
#[cfg(target_os = "macos")]
mod impl_macos;
// No re-exports yet; macOS public surface is built up in Phase 6.
```

**`src/main.rs`** — change the area around lines 7 and 16-17 from:
```rust
mod main_linux;

// ... possible other lines ...

#[cfg(not(target_os = "linux"))]
compile_error!("Subtidal currently only supports Linux. macOS support is planned.");
```

to:
```rust
#[cfg(target_os = "linux")]
mod main_linux;

#[cfg(target_os = "macos")]
mod main_macos;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Subtidal supports Linux and macOS only.");
```

Replace the Linux-cfg-gated `fn main()` body (currently at lines 87–309) with a thin dispatcher; move the existing Linux logic into `pub fn main()` in `src/main_linux.rs` if it isn't already structured that way:

```rust
fn main() {
    #[cfg(target_os = "linux")]
    main_linux::main();

    #[cfg(target_os = "macos")]
    main_macos::main();
}
```

If the existing Linux `main()` body uses `clap::Parser::parse()` for `Args` (per `main.rs:67-85`), move that call into `main_linux::main()` too — keeping `fn main()` in `main.rs` as a pure dispatcher avoids cfg-noise around CLI parsing that's currently Linux-only anyway (macOS doesn't need CLI args in Phase 1).

**Verification:**

```bash
cargo check --lib                                        # Linux lib still green
cargo check --bin subtidal                               # Linux binary still builds
cargo check --lib --target x86_64-apple-darwin           # cross-target lib still green
cargo test --lib                                         # existing tests still pass
```

**Commit:** `macos: wire cfg-gated mod declarations and main.rs dispatch`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->
<!-- START_TASK_5 -->
### Task 5: Create Info.plist and .app bundle script

**Files:**
- Create: `resources/macos/Info.plist`
- Create: `scripts/bundle-mac.sh` (`chmod +x`)

**Implementation:**

**`resources/macos/Info.plist`:**

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>          <string>com.subtidal.app</string>
  <key>CFBundleExecutable</key>          <string>subtidal</string>
  <key>CFBundleName</key>                <string>Subtidal</string>
  <key>CFBundlePackageType</key>         <string>APPL</string>
  <key>CFBundleVersion</key>             <string>1.0</string>
  <key>CFBundleShortVersionString</key>  <string>0.2.2</string>
  <key>LSMinimumSystemVersion</key>      <string>14.4</string>
  <key>LSUIElement</key>                 <true/>
  <key>NSScreenCaptureUsageDescription</key>
    <string>Subtidal captures system audio to display live captions.</string>
  <key>NSMicrophoneUsageDescription</key>
    <string>Subtidal captures audio for speech-to-text processing.</string>
</dict>
</plist>
```

Notes:
- `LSUIElement = true` makes Subtidal a menu-bar-only app (no Dock icon), matching the Linux tray-only model.
- `LSMinimumSystemVersion = 14.4` per the design's macOS 14.4+ requirement.
- `CFBundleShortVersionString` mirrors workspace version today (0.2.2); keep loosely in sync as releases happen.

**`scripts/bundle-mac.sh`:**

```bash
#!/usr/bin/env bash
# scripts/bundle-mac.sh — build a minimal Subtidal.app bundle from cargo output.
#
# Usage (from repo root):
#   scripts/bundle-mac.sh                # release build
#   scripts/bundle-mac.sh --debug        # debug build
#
# Bundle ID `com.subtidal.app` and ad-hoc codesign identity must stay stable
# across rebuilds to preserve TCC (Screen Recording) grants. See
# docs/design-plans/2026-05-18-macos-port.md §"TCC permissions and the .app
# wrapper".

set -euo pipefail

PROFILE="release"
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE="debug"
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ "$PROFILE" == "release" ]]; then
  cargo build --release
else
  cargo build
fi

BIN="target/${PROFILE}/subtidal"
APP="target/${PROFILE}/Subtidal.app"

if [[ ! -x "$BIN" ]]; then
  echo "error: binary not found at $BIN" >&2
  exit 1
fi

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/subtidal"
cp resources/macos/Info.plist "$APP/Contents/Info.plist"

plutil -lint "$APP/Contents/Info.plist"

codesign --force --deep --sign - "$APP"

echo "Built $APP"
```

Then:

```bash
chmod +x scripts/bundle-mac.sh
```

**Verification:**

On macOS:
```bash
scripts/bundle-mac.sh
plutil -lint target/release/Subtidal.app/Contents/Info.plist
open target/release/Subtidal.app
```
Expected: plist lints OK; bundle launches, prints "Hello from macOS Subtidal", exits. No TCC prompt (Phase 1 doesn't touch Screen Recording APIs).

On Linux:
```bash
bash -n scripts/bundle-mac.sh
```
Expected: no syntax errors.

**Commit:** `macos: add Info.plist and .app bundle script`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Final verification — full skeleton builds and bundles

**Files:** none (verification only)

**Implementation:**

Checkpoint task: after all prior tasks, the Phase 1 skeleton must meet the design's "Done when" criteria.

**Verification (Linux host):**

```bash
cargo check --lib
cargo check --bin subtidal
cargo test --lib
cargo check --lib --target x86_64-apple-darwin --verbose
```
All four must pass. The cross-target check is the load-bearing one for `macos-port.AC9.1`.

**Verification (macOS host, if available):**

```bash
cargo build --release
scripts/bundle-mac.sh
open target/release/Subtidal.app
plutil -lint target/release/Subtidal.app/Contents/Info.plist
```
Expected: binary builds cleanly; `.app` is created at `target/release/Subtidal.app`; launch prints the hello message; plist lints cleanly.

**Commit:** none (verification only).
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_C -->
