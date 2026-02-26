# Nemotron CPU Fallback Design

## Summary

Subtidal currently ships two STT engines: Nemotron (a 600M-parameter RNNT model requiring CUDA) and Moonshine (a lighter encoder-decoder model used as the CPU fallback when no GPU is detected). The Moonshine engine was implemented as a stopgap and is now known to be redundant — parakeet-rs, the library that drives Nemotron, already supports CPU execution through its `ExecutionProvider` abstraction. This change removes Moonshine entirely and updates the fallback path to simply run Nemotron on CPU instead.

The implementation is a mechanical cleanup across six phases: delete the Moonshine source file and its exclusive `tokenizers` dependency, strip its model download and management code, remove it from the config and CLI surface, replace the CUDA-detection branch that previously switched engines with an informational log message, hide the tray engine submenu (now pointless with a single engine), and update documentation. The `SttEngine` trait, engine-switching channels, and `Engine` enum skeleton are all preserved so adding a future engine remains a localized change.

## Definition of Done

Remove the stubbed Moonshine STT engine entirely and simplify the CUDA-unavailable fallback to run Nemotron on CPU (which parakeet-rs already supports). Preserve the Engine trait and enum infrastructure for future engine additions.

**Deliverables:**
- `stt/moonshine.rs` deleted; all Moonshine references removed from `main.rs`, `models/mod.rs`, `config.rs`, tray menu, CLI args
- CUDA fallback path runs Nemotron on CPU instead of switching to Moonshine
- Engine enum retains its structure (Moonshine variant removed) for future extensibility
- Config TOML schema updated; existing configs referencing `moonshine` warned and defaulted to `nemotron`
- CLI `--engine moonshine` removed
- All existing tests pass; Moonshine-specific tests removed

## Acceptance Criteria

### nemotron-cpu-fallback.AC1: Moonshine code fully removed
- **nemotron-cpu-fallback.AC1.1 Success:** `stt/moonshine.rs` deleted, `tokenizers` crate removed, project builds
- **nemotron-cpu-fallback.AC1.2 Success:** All Moonshine model management functions and constants removed from `models/mod.rs`
- **nemotron-cpu-fallback.AC1.3 Success:** Case-insensitive grep for `moonshine` across codebase returns zero results (excluding design docs and git history)

### nemotron-cpu-fallback.AC2: Config and CLI updated
- **nemotron-cpu-fallback.AC2.1 Success:** Config with `engine = "moonshine"` logs warning and defaults to Nemotron
- **nemotron-cpu-fallback.AC2.2 Success:** CLI `--engine nemotron` and `--engine parakeet` still work
- **nemotron-cpu-fallback.AC2.3 Failure:** CLI `--engine moonshine` prints helpful error and exits

### nemotron-cpu-fallback.AC3: CUDA fallback simplified
- **nemotron-cpu-fallback.AC3.1 Success:** When CUDA unavailable, Nemotron starts on CPU and logs info message
- **nemotron-cpu-fallback.AC3.2 Success:** When CUDA available, Nemotron starts with GPU (no behavioral change)

### nemotron-cpu-fallback.AC4: Tray menu updated
- **nemotron-cpu-fallback.AC4.1 Success:** Engine submenu hidden when only one engine variant exists

## Glossary

- **Nemotron**: NVIDIA's RNNT-based speech recognition model (600M parameters). In Subtidal it runs via the parakeet-rs crate and the ONNX Runtime. Supports both GPU and CPU execution.
- **Moonshine**: A separate, lighter STT model (encoder-decoder architecture) previously used as the CPU fallback in Subtidal. Being removed by this change.
- **RNNT (Recurrent Neural Network Transducer)**: A streaming-capable neural network architecture for speech recognition that emits partial results as audio arrives.
- **CUDA**: NVIDIA's GPU compute platform. Absence of CUDA causes Nemotron to fall back to CPU execution.
- **parakeet-rs**: A Rust crate that wraps the Nemotron RNNT decoder. Its `ExecutionProvider` enum controls whether inference runs on CPU or GPU.
- **ONNX Runtime (ort)**: A cross-platform inference engine for ML models stored in ONNX format.
- **SttEngine trait**: Subtidal's internal Rust interface that all speech recognition backends implement. Defines `process_chunk` (accepts 160ms of 16kHz mono PCM, returns optional transcript) and `sample_rate`.
- **Engine enum**: A Rust enum in `config.rs` whose variants name the available STT backends. After this change it will have a single `Nemotron` variant; structure kept for future extension.
- **ExecutionProvider**: A parakeet-rs type that selects CPU or GPU execution. Currently only a `Cpu` variant exists in parakeet-rs 0.3, meaning Nemotron already runs on CPU without special configuration.
- **tokenizers**: A HuggingFace Rust crate used by Moonshine to decode ONNX output token IDs into text. Removed along with Moonshine.
- **StatusNotifierItem (tray)**: A D-Bus protocol for system tray icons on Linux. Subtidal uses `ksni` to expose a tray icon with an engine submenu that this change hides.
- **Config hot-reload**: Subtidal watches `~/.config/subtidal/config.toml` for changes at runtime using the `notify` crate and applies them without restart.

## Architecture

Remove the Moonshine STT engine and simplify the CUDA fallback path. Nemotron (via parakeet-rs) already runs on CPU — the `ExecutionProvider` enum in parakeet-rs 0.3 only has a `Cpu` variant. The current architecture switches to Moonshine when CUDA is unavailable; the new architecture keeps Nemotron and logs an informational message instead.

**Engine enum:** Remains as an enum with a single `Nemotron` variant. The `SttEngine` trait, `spawn_inference_thread`, `restart_inference_thread`, and engine-switching channel infrastructure are preserved unchanged. Adding a future engine means adding a variant and an impl — no structural changes needed.

**CUDA detection:** `cuda_available()` stays in `stt/mod.rs`. Called at startup to log whether GPU acceleration is active. No behavioral branching — Nemotron runs regardless.

**Config compatibility:** When `Config::load()` encounters an unrecognized engine variant (e.g., `engine = "moonshine"` from an old config), it logs a warning and defaults to `Nemotron`. This uses serde's deserialization error handling in the existing load path, which already returns defaults on malformed TOML.

**Tray menu:** The engine submenu is hidden when only one engine variant exists. The `build_engine_submenu` function remains for future use but is not called when there's nothing to choose between.

**Dependency removal:** The `tokenizers` crate (0.20) is removed from `Cargo.toml` — it was only used by the Moonshine engine for decoding ONNX output tokens.

## Existing Patterns

Investigation found the existing engine infrastructure follows a clean pattern:
- `SttEngine` trait in `stt/mod.rs` with `process_chunk` and `sample_rate` methods
- Engine instantiation in `main.rs` via match on `config::Engine` variant
- Engine switching via `EngineCommand::Switch` sent through channels, handled in a dedicated thread
- Config hot-reload sends `SetMode`/`SetLocked`/`UpdateAppearance` only when values change

This design follows all existing patterns. The only change is reducing the number of match arms — no structural divergence.

The config load/save pattern (`Config::load()` returns defaults on error) already handles the compatibility case naturally.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Remove Moonshine Engine and Dependencies

**Goal:** Delete Moonshine engine code and its exclusive dependency.

**Components:**
- Delete `src/stt/moonshine.rs`
- Remove `pub mod moonshine;` from `src/stt/mod.rs`
- Remove `tokenizers = "0.20"` and its comment from `Cargo.toml`

**Dependencies:** None (first phase)

**Done when:** `cargo build` succeeds without moonshine module or tokenizers crate. Covers `nemotron-cpu-fallback.AC1.1`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Remove Moonshine Model Management

**Goal:** Clean up model download, presence checks, and HF repo constants for Moonshine.

**Components:**
- `src/models/mod.rs` — remove `moonshine_model_dir`, `moonshine_model_files`, `moonshine_models_present`, `moonshine_models_present_in`, `ensure_moonshine_models`, `MOONSHINE_REPO`, `MOONSHINE_FILES` constants, and all Moonshine-specific tests

**Dependencies:** Phase 1

**Done when:** `cargo build` succeeds. `cargo test` passes for remaining model tests. Covers `nemotron-cpu-fallback.AC1.2`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Update Config and CLI

**Goal:** Remove Moonshine from Engine enum and CLI args. Handle old configs gracefully.

**Components:**
- `src/config.rs` — remove `Moonshine` variant from `Engine` enum, update deserialization to warn and default on unknown variants, remove Moonshine-specific tests
- `src/main.rs` — remove `"moonshine"` arm from CLI parsing, update help text and error message to only mention `nemotron`/`parakeet`

**Dependencies:** Phase 1

**Done when:** Old config with `engine = "moonshine"` logs warning and defaults to Nemotron. CLI rejects `--engine moonshine` with helpful error. Tests pass. Covers `nemotron-cpu-fallback.AC2.1`, `nemotron-cpu-fallback.AC2.2`, `nemotron-cpu-fallback.AC2.3`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Simplify CUDA Fallback and Engine Instantiation

**Goal:** Replace the CUDA→Moonshine fallback with an informational log. Remove Moonshine instantiation paths.

**Components:**
- `src/main.rs` — replace CUDA fallback block (lines ~135-145) with `eprintln!("info: CUDA not available, Nemotron will use CPU")`, remove Moonshine instantiation arm (lines ~206-216), remove Moonshine arm from engine switch handler (lines ~254-261)

**Dependencies:** Phases 1-3

**Done when:** Application starts successfully without CUDA, logs the info message, runs Nemotron on CPU. Covers `nemotron-cpu-fallback.AC3.1`, `nemotron-cpu-fallback.AC3.2`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Update Tray Menu

**Goal:** Hide engine submenu when only one engine exists.

**Components:**
- `src/tray/mod.rs` — conditionally skip engine submenu in tray menu construction when Engine enum has a single variant. `build_engine_submenu` function preserved but not called.

**Dependencies:** Phase 3 (Engine enum updated)

**Done when:** Tray menu renders without engine submenu. Covers `nemotron-cpu-fallback.AC4.1`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Update Documentation and Cleanup

**Goal:** Update CLAUDE.md and remove stale references.

**Components:**
- `CLAUDE.md` — remove Moonshine from architecture listing, update `stt/moonshine.rs` entry, update Key Contracts, update Dependencies (remove tokenizers), update Invariants (remove CUDA→Moonshine fallback mention)
- `src/stt/mod.rs` — update doc comment referencing "Parakeet or Moonshine", remove Moonshine fallback test comment

**Dependencies:** Phases 1-5

**Done when:** `cargo test` passes. `CLAUDE.md` accurately reflects single-engine architecture. No remaining "moonshine" references in codebase (case-insensitive grep returns zero results). Covers `nemotron-cpu-fallback.AC1.3`.
<!-- END_PHASE_6 -->

## Additional Considerations

**Config migration:** No automated migration needed. Serde's error handling on unknown variants combined with `Config::load()` defaulting on errors provides natural backward compatibility. Users with old configs get a one-time warning at startup.

**Future engines:** Adding a new engine requires: (1) add variant to `Engine` enum, (2) implement `SttEngine` trait, (3) add match arms in `main.rs` for instantiation and switching, (4) engine submenu automatically appears when >1 variant exists.
