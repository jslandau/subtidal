# Nemotron CPU Fallback Implementation Plan — Phase 1

**Goal:** Remove all Moonshine code paths, Engine variant, CLI option, and stt module declaration so the codebase is Nemotron-only.

**Architecture:** Remove the `Moonshine` variant from the `Engine` enum, strip all match arms and code paths that reference it across config, CLI, main startup, and engine switching. Replace the CUDA fallback block (which switched to Moonshine) with a simple info log. Remove the `pub mod moonshine` declaration from `stt/mod.rs` so the file is no longer compiled.

**Tech Stack:** Rust, serde, clap

**Scope:** 5 phases from original design (restructured from 6 design phases into 5 compilable phases). This phase covers design phases 1 (partial: mod removal), 3 (config/CLI), and 4 (CUDA fallback).

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### nemotron-cpu-fallback.AC2: Config and CLI updated
- **nemotron-cpu-fallback.AC2.1 Success:** Config with `engine = "moonshine"` logs warning and defaults to Nemotron
- **nemotron-cpu-fallback.AC2.2 Success:** CLI `--engine nemotron` and `--engine parakeet` still work
- **nemotron-cpu-fallback.AC2.3 Failure:** CLI `--engine moonshine` prints helpful error and exits

### nemotron-cpu-fallback.AC3: CUDA fallback simplified
- **nemotron-cpu-fallback.AC3.1 Success:** When CUDA unavailable, Nemotron starts on CPU and logs info message
- **nemotron-cpu-fallback.AC3.2 Success:** When CUDA available, Nemotron starts with GPU (no behavioral change)

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->

<!-- START_TASK_1 -->
### Task 1: Remove Moonshine from Engine enum and add unknown-variant handling

**Verifies:** nemotron-cpu-fallback.AC2.1

**Files:**
- Modify: `src/config.rs:8-15` (Engine enum)

**Implementation:**

Replace the Engine enum (lines 8-15) with:

```rust
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    #[serde(alias = "parakeet")]
    Nemotron,
}
```

Remove line 14 (`Moonshine,`). The serde `rename_all = "snake_case"` combined with `Config::load()` returning defaults on parse error naturally handles old configs with `engine = "moonshine"` — `toml::from_str` will fail on the unknown variant, and `Config::load()` (line 183-196) already catches parse errors, logs a warning, and returns defaults.

**Verification:**
Run: `cargo check 2>&1 | head -50`
Expected: Compile errors about exhaustive match patterns for `Engine::Moonshine` (these are fixed in subsequent tasks)

**Commit:** Do not commit yet — build is broken until Task 4
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update CLI engine parsing in main.rs

**Verifies:** nemotron-cpu-fallback.AC2.2, nemotron-cpu-fallback.AC2.3

**Files:**
- Modify: `src/main.rs:21` (help text)
- Modify: `src/main.rs:48-57` (CLI engine match)

**Implementation:**

Update the help text on line 21 from:
```rust
    /// Override STT engine for this session (parakeet|moonshine)
```
to:
```rust
    /// Override STT engine for this session (nemotron|parakeet)
```

Replace the CLI engine match block (lines 48-57) with:

```rust
    if let Some(engine_str) = args.engine {
        cfg.engine = match engine_str.to_lowercase().as_str() {
            "nemotron" | "parakeet" => config::Engine::Nemotron,
            other => {
                eprintln!("Unknown engine '{other}'. Valid engines: nemotron, parakeet.");
                std::process::exit(1);
            }
        };
    }
```

This removes the `"moonshine"` arm. The `other` arm now catches it with a helpful error.

**Verification:**
Run: `cargo check 2>&1 | head -20`
Expected: Still compile errors from other Moonshine references (fixed in Tasks 3-4)

**Commit:** Do not commit yet
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Simplify CUDA fallback and remove Moonshine from main.rs code paths

**Verifies:** nemotron-cpu-fallback.AC3.1, nemotron-cpu-fallback.AC3.2

**Files:**
- Modify: `src/main.rs:78-110` (model download block)
- Modify: `src/main.rs:134-146` (CUDA fallback block)
- Modify: `src/main.rs:195-217` (engine instantiation)
- Modify: `src/main.rs:244-263` (engine switch handler)
- Modify: `src/main.rs:309-320` (TrayState construction)
- Modify: `src/stt/mod.rs:4` (remove `pub mod moonshine;`)

**Implementation:**

**3a. Remove `pub mod moonshine;` from `src/stt/mod.rs`:**

Delete line 4 (`pub mod moonshine;`). The file becomes:
```rust
pub mod nemotron;

use anyhow::Result;
```

**3b. Simplify model download block** (lines 78-110):

Replace with Nemotron-only download (remove the `config::Engine::Moonshine` arm and the outer `match`):

```rust
    runtime.block_on(async {
        if !models::nemotron_models_present() {
            println!("Downloading Nemotron model files (first run)...");
            models::ensure_nemotron_models().await
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to download Nemotron model: {e:#}");
                    eprintln!("hint: check network connectivity and disk space in ~/.local/share/subtidal/models/");
                    std::process::exit(1);
                });
            println!("Nemotron models ready.");
        } else {
            println!("Nemotron models already present, skipping download.");
        }
    });
```

**3c. Replace CUDA fallback block** (lines 134-146):

Replace with:

```rust
    // Log CUDA availability (Nemotron runs on CPU when CUDA is unavailable).
    if stt::cuda_available() {
        eprintln!("info: CUDA available, Nemotron will use GPU acceleration");
    } else {
        eprintln!("info: CUDA not available, Nemotron will use CPU");
    }
```

Remove `active_engine` and `cuda_fallback_warning` variables entirely.

**3d. Simplify engine instantiation** (lines 195-217):

Replace with Nemotron-only instantiation (no match needed):

```rust
    // Instantiate the STT engine.
    let engine: Box<dyn stt::SttEngine> = {
        let model_dir = models::nemotron_model_dir();
        Box::new(
            stt::nemotron::NemotronEngine::new(&model_dir)
                .unwrap_or_else(|e| {
                    eprintln!("error: failed to load Nemotron model: {e:#}");
                    std::process::exit(1);
                })
        )
    };
```

**3e. Simplify engine switch handler** (lines 244-263):

Replace the match on `new_engine_choice` with Nemotron-only:

```rust
                        let new_engine: Box<dyn stt::SttEngine> = match new_engine_choice {
                            config::Engine::Nemotron => {
                                match stt::nemotron::NemotronEngine::new(&models::nemotron_model_dir()) {
                                    Ok(e) => Box::new(e),
                                    Err(e) => {
                                        eprintln!("error: failed to load Nemotron: {e:#}");
                                        continue;
                                    }
                                }
                            }
                        };
```

**3f. Update TrayState construction** (lines 309-320):

Remove `cuda_warning` field:
- Remove line 315 (`cuda_warning: cuda_fallback_warning,`)
- Change line 314 from `active_engine: active_engine.clone(),` to `active_engine: cfg.engine.clone(),`

**Verification:**
Run: `cargo check 2>&1 | head -20`
Expected: Compile errors about `cuda_warning` field in TrayState (fixed in Task 4)

**Commit:** Do not commit yet
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Remove cuda_warning from TrayState and update tray title

**Files:**
- Modify: `src/tray/mod.rs:20` (remove `cuda_warning` field)
- Modify: `src/tray/mod.rs:59-65` (simplify `title()`)

**Implementation:**

**4a.** Remove `cuda_warning` field from TrayState struct (line 20):
```rust
    pub cuda_warning: Option<&'static str>,
```
Delete this line.

**4b.** Simplify the `title()` method (lines 59-65):
```rust
    fn title(&self) -> String {
        "Live Captions".to_string()
    }
```

**4c.** Update test helpers that construct TrayState — remove `cuda_warning: None,` from both test functions at lines 362-372 and 394-405.

**4d.** Update config tests in `src/config.rs:320-370`:

Update `config_roundtrip` test (line 329): change `engine: Engine::Moonshine,` to `engine: Engine::Nemotron,`. Update the assertion on line 339 from `Engine::Moonshine` to `Engine::Nemotron`.

Update `config_partial_toml_fills_defaults` test (line 364): change `"engine = \"moonshine\"\n"` to `"engine = \"nemotron\"\n"`. Update assertion on line 366 from `Engine::Moonshine` to `Engine::Nemotron`.

**Verification:**
Run: `cargo build 2>&1 | tail -3`
Expected: Build succeeds

Run: `cargo test 2>&1 | tail -15`
Expected: All tests pass (moonshine model tests still exist and pass — they test file paths, not the Engine enum)

**Commit:**
```bash
git add src/config.rs src/main.rs src/stt/mod.rs src/tray/mod.rs
git commit -m "refactor: remove Moonshine engine variant, CLI option, and CUDA fallback

Remove the Moonshine variant from the Engine enum, strip all Moonshine
code paths from main.rs (model download, instantiation, engine switching,
CUDA fallback), remove the pub mod moonshine declaration, and update CLI
to only accept nemotron/parakeet. Old configs with engine=\"moonshine\"
now fall through to defaults via existing TOML error handling."
```
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_A -->
