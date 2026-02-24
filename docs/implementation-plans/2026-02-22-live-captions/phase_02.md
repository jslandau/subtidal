# Live Captions Implementation Plan — Phase 2: Model Management

**Goal:** Auto-download STT model weight files from HuggingFace on first run; verify presence on subsequent runs; exit with a clear error if download fails.

**Architecture:** `src/models/mod.rs` gains async download functions using `hf-hub`'s tokio API. The download copies files from hf-hub's cache to `~/.local/share/live-captions/models/{parakeet,moonshine}/`. Phase 1's `*_models_present()` predicates are used to skip downloads on subsequent runs.

**Tech Stack:** hf-hub 0.5 (tokio API), tokio 1 (async runtime for download).

**Scope:** Phase 2 of 8. Depends on Phase 1.

**Codebase verified:** 2026-02-22 — Phase 1 plan establishes `src/models/mod.rs` with path helpers; this phase extends it.

---

## Acceptance Criteria Coverage

### live-captions.AC5: STT engine management
- **live-captions.AC5.1 Success:** On first run, required model files are downloaded automatically before captions start
- **live-captions.AC5.2 Success:** Subsequent runs skip download when model files are already present
- **live-captions.AC5.4 Failure:** If model download fails, app exits with a clear error message pointing to the missing model

---

**Note on Parakeet model:** The design references "Parakeet TDT 0.6B". The parakeet-rs crate's EOU model available on HuggingFace is `realtime_eou_120m-v1-onnx` (120M parameters) from `altunenes/parakeet-rs`. This is what `ParakeetEOU::from_pretrained` expects. A larger model may be added in a future release of parakeet-rs — confirm this at implementation time by checking `https://github.com/altunenes/parakeet-rs`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Extend src/models/mod.rs with download logic

**Files:**
- Modify: `src/models/mod.rs` (extend the file created in Phase 1)

**Step 1: Add download functions to src/models/mod.rs**

Add the following after the existing content in `src/models/mod.rs`:

```rust
use anyhow::{Context, Result};
use std::path::Path;

/// HuggingFace repo and file paths for the Parakeet EOU model.
/// Repo: altunenes/parakeet-rs
/// Subfolder: realtime_eou_120m-v1-onnx/
const PARAKEET_REPO: &str = "altunenes/parakeet-rs";
const PARAKEET_FILES: &[(&str, &str)] = &[
    ("realtime_eou_120m-v1-onnx/encoder.onnx", "encoder.onnx"),
    ("realtime_eou_120m-v1-onnx/decoder_joint.onnx", "decoder_joint.onnx"),
    ("realtime_eou_120m-v1-onnx/tokenizer.json", "tokenizer.json"),
];

/// HuggingFace repo and file paths for the Moonshine tiny quantized model.
/// Repo: onnx-community/moonshine-tiny-ONNX
const MOONSHINE_REPO: &str = "onnx-community/moonshine-tiny-ONNX";
const MOONSHINE_FILES: &[(&str, &str)] = &[
    ("onnx/encoder_model_quantized.onnx", "encoder_model_quantized.onnx"),
    ("onnx/decoder_model_merged_quantized.onnx", "decoder_model_merged_quantized.onnx"),
    ("tokenizer.json", "tokenizer.json"),
];

/// Download all Parakeet EOU model files to `~/.local/share/live-captions/models/parakeet/`.
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_parakeet_models() -> Result<()> {
    let dest_dir = parakeet_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new()
        .context("initializing HuggingFace API")?;
    let repo = api.model(PARAKEET_REPO.to_string());

    for (remote_path, local_name) in PARAKEET_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!("info: parakeet model file already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo.get(remote_path).await
            .with_context(|| format!("downloading {remote_path} from {PARAKEET_REPO}"))?;
        copy_model_file(&cached, &dest)
            .with_context(|| format!("copying {remote_path} to {}", dest.display()))?;
        eprintln!("info: saved to {}", dest.display());
    }
    Ok(())
}

/// Download all Moonshine model files to `~/.local/share/live-captions/models/moonshine/`.
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_moonshine_models() -> Result<()> {
    let dest_dir = moonshine_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new()
        .context("initializing HuggingFace API")?;
    let repo = api.model(MOONSHINE_REPO.to_string());

    for (remote_path, local_name) in MOONSHINE_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!("info: moonshine model file already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo.get(remote_path).await
            .with_context(|| format!("downloading {remote_path} from {MOONSHINE_REPO}"))?;
        copy_model_file(&cached, &dest)
            .with_context(|| format!("copying {remote_path} to {}", dest.display()))?;
        eprintln!("info: saved to {}", dest.display());
    }
    Ok(())
}

fn copy_model_file(src: &Path, dest: &Path) -> Result<()> {
    // Try hardlink first (free if on same filesystem as HF cache).
    // Fall back to copy if hardlink fails (different filesystem).
    if std::fs::hard_link(src, dest).is_err() {
        std::fs::copy(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    }
    Ok(())
}
```

**Step 2: Add hf-hub import at the top of src/models/mod.rs**

The functions above use `hf_hub`. Since hf-hub is declared in Cargo.toml, it's available. No additional use statement needed at the module level (the functions use the full path `hf_hub::api::tokio::Api`).

**Step 3: Verify compilation**

```bash
cd /home/jslandau/git/live_text
cargo check
```

Expected: No errors. Warnings about unused functions are acceptable (they'll be called in Phase 8's `main.rs`).
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Wire model check into main.rs and test end-to-end

**Files:**
- Modify: `src/main.rs` (extend the stub from Phase 1)

**Verifies:** live-captions.AC5.1, live-captions.AC5.2, live-captions.AC5.4

**Step 1: Add model download to main.rs**

Replace the subsystem stubs comment block in `src/main.rs` with:

```rust
// Phase 2: Ensure model files are present before starting
let runtime = tokio::runtime::Builder::new_multi_thread()
    .enable_all()
    .build()
    .context("building tokio runtime")?;

let engine = cfg.engine.clone();
runtime.block_on(async move {
    match engine {
        config::Engine::Parakeet => {
            if !models::parakeet_models_present() {
                println!("Downloading Parakeet model files (first run)...");
                models::ensure_parakeet_models().await
                    .unwrap_or_else(|e| {
                        eprintln!("error: failed to download Parakeet model: {e:#}");
                        eprintln!("hint: check network connectivity and disk space in ~/.local/share/live-captions/models/");
                        std::process::exit(1);
                    });
                println!("Parakeet models ready.");
            } else {
                println!("Parakeet models already present, skipping download.");
            }
        }
        config::Engine::Moonshine => {
            if !models::moonshine_models_present() {
                println!("Downloading Moonshine model files (first run)...");
                models::ensure_moonshine_models().await
                    .unwrap_or_else(|e| {
                        eprintln!("error: failed to download Moonshine model: {e:#}");
                        eprintln!("hint: check network connectivity and disk space in ~/.local/share/live-captions/models/");
                        std::process::exit(1);
                    });
                println!("Moonshine models ready.");
            } else {
                println!("Moonshine models already present, skipping download.");
            }
        }
    }
})?;

// --- Remaining subsystem stubs (filled in subsequent phases) ---
// Phase 3: PipeWire audio capture
// Phase 4: STT inference thread
// Phase 5: GTK4 overlay window
// Phase 6: ksni system tray
// Phase 7: config hot-reload
// Phase 8: full integration
```

Also add `use anyhow::Context;` and `mod models;` at the top of `src/main.rs` if not already present. Also add `use std::io::Write;` if needed by eprintln (it's not — eprintln goes to stderr natively).

**Step 2: Build**

```bash
cd /home/jslandau/git/live_text
cargo build
```

Expected: Builds cleanly.

**Step 3: Test first run (downloads models)**

```bash
cargo run
```

Expected: On first run, downloads Parakeet EOU model files and saves them to `~/.local/share/live-captions/models/parakeet/`. Progress shown on stderr. (This download is several hundred MB — allow time.)

Verify files appeared:
```bash
ls ~/.local/share/live-captions/models/parakeet/
```

Expected: `encoder.onnx  decoder_joint.onnx  tokenizer.json`

**Step 4: Test second run (skips download)**

```bash
cargo run
```

Expected:
```
Parakeet models already present, skipping download.
```

**Step 5: Test download failure path (AC5.4)**

Rename a model file to simulate failure, run, restore it:

```bash
mv ~/.local/share/live-captions/models/parakeet/encoder.onnx ~/.local/share/live-captions/models/parakeet/encoder.onnx.bak
# Delete the cached hf-hub file to force re-download attempt:
# (or disconnect network)
cargo run 2>&1 | head -5
# Expected: "error: failed to download Parakeet model: ..." then exit code 1
# Restore:
mv ~/.local/share/live-captions/models/parakeet/encoder.onnx.bak ~/.local/share/live-captions/models/parakeet/encoder.onnx
```

**Step 6: Commit**

```bash
git add src/models/mod.rs src/main.rs
git commit -m "feat: model management — download Parakeet EOU and Moonshine ONNX on first run"
```
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->
