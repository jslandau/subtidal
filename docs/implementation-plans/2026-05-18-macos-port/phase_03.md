# macOS Port — Phase 3: STT engine on macOS (WebGPU + CPU fallback)

**Goal:** Wire the real Nemotron engine for macOS with WebGPU primary and CPU fallback. Replace Phase 2's hardcoded test caption harness with the real STT pipeline thread driven by a fixture-WAV harness (real audio capture lands in Phase 4).

**Architecture:** `NemotronEngine::new` gains a macOS branch that attempts `ExecutionProvider::WebGPU` first and falls back to `ExecutionProvider::Cpu` on init failure, logging the chosen provider in both cases. The `stt::spawn_stt_thread` and `stt::build_engine` functions (currently Linux-gated) are widened to `any(target_os = "linux", target_os = "macos")`. `main_macos::main` constructs `AudioWake`, ring buffer, `ArcSwap<Engine>`, and `PipelineConfig { use_cuda: true, ... }`, spawns the STT thread via the existing neutral `spawn_stt_thread`, and spawns a Phase-3-only fixture-WAV harness that feeds the ring buffer at real-time pacing.

**Tech Stack:** `parakeet-rs` 0.3 (`webgpu` feature, `ExecutionProvider::{WebGPU, Cpu}`), neutral `stt::AudioWake` / `stt::PipelineConfig` / `stt::SttEngine` (unchanged), `ringbuf` (re-used from Linux side), `arc-swap`, `tokio` for one-shot model-download `block_on`, `rubato` for fixture→48kHz-stereo conversion.

**Scope:** Phase 3 of 8.

**Codebase verified:** 2026-05-18.

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### macos-port.AC4: STT engine on macOS (WebGPU primary, CPU fallback)
- **macos-port.AC4.1 Success:** On Apple Silicon, `NemotronEngine::new` selects `ExecutionProvider::WebGpu` and engine init succeeds; a log line confirms `WebGpu` as the chosen provider.
- **macos-port.AC4.2 Success:** If `WebGpu` init fails (e.g., simulated by injected fault), `NemotronEngine::new` retries with `ExecutionProvider::Cpu`; a log line confirms fallback occurred.
- **macos-port.AC4.3 Success:** Transcript accuracy on the committed test fixture WAV (`tests/fixtures/macos-webgpu-smoke.wav`) matches the Linux baseline within tokenizer-level tolerance (identical token sequence; small whitespace differences acceptable).
- **macos-port.AC4.5 Edge:** Engine swap via the tray (single-engine for now, but the code path exists) reads `ArcSwap<Engine>` on the next chunk boundary; no concurrent session construction occurs (verified by code review of the STT thread).

**Note on naming:** the design doc spells the variant `WebGpu`; the actual parakeet-rs API is `ExecutionProvider::WebGPU` (capital GPU, verified Phase 0). Log messages and code use `WebGPU`. AC text is reproduced verbatim from the design.

**Design decision documented:** `PipelineConfig.use_cuda: bool` is kept as-is. Its semantic on macOS is "request platform GPU acceleration" — main_macos passes `true`; the WebGPU→CPU fallback lives inside `NemotronEngine::new`. Refactoring to an enum is deferred until a third platform lands.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Add WebGPU + CPU fallback to NemotronEngine::new

**Verifies:** macos-port.AC4.1, macos-port.AC4.2

**Files:**
- Modify: `src/stt/nemotron.rs:21-39` (cfg-dispatch the execution provider selection per OS)

**Implementation:**

Refactor `NemotronEngine::new` so the Linux branch is unchanged behaviorally, and the macOS branch attempts WebGPU first with CPU fallback. Extract the macOS-only logic into a testable seam `build_macos_with` that takes a closure for the WebGPU attempt (so Task 4 can inject a failure).

```rust
pub fn new(model_dir: &Path, use_cuda: bool) -> Result<Self> {
    #[cfg(target_os = "linux")]
    let inner = {
        let exec_config = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(if use_cuda {
                parakeet_rs::ExecutionProvider::Cuda
            } else {
                parakeet_rs::ExecutionProvider::Cpu
            });
        let provider = if use_cuda { "Cuda" } else { "Cpu" };
        eprintln!("info: Nemotron using execution provider: {provider}");
        parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec_config))
            .with_context(|| format!("loading Nemotron from {} (provider={provider})", model_dir.display()))?
    };

    #[cfg(target_os = "macos")]
    let inner = build_macos(model_dir, use_cuda)?;

    Ok(NemotronEngine {
        inner,
        chunk_buf: Vec::with_capacity(NEMOTRON_CHUNK_SAMPLES),
    })
}

#[cfg(target_os = "macos")]
fn build_macos(model_dir: &Path, use_cuda: bool) -> Result<parakeet_rs::Nemotron> {
    build_macos_with(model_dir, use_cuda, |dir| {
        let exec = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(parakeet_rs::ExecutionProvider::WebGPU);
        parakeet_rs::Nemotron::from_pretrained(dir, Some(exec))
            .map_err(anyhow::Error::from)
    })
}

#[cfg(target_os = "macos")]
fn build_macos_with<F>(model_dir: &Path, use_cuda: bool, try_webgpu: F) -> Result<parakeet_rs::Nemotron>
where
    F: FnOnce(&Path) -> Result<parakeet_rs::Nemotron, anyhow::Error>,
{
    // `use_cuda` here means "request GPU acceleration"; macOS uses WebGPU
    // (backed by Metal via wgpu) as the GPU provider. CPU is the fallback
    // both when the caller explicitly requested CPU AND when WebGPU init fails.
    if use_cuda {
        match try_webgpu(model_dir) {
            Ok(inner) => {
                eprintln!("info: Nemotron using execution provider: WebGPU");
                return Ok(inner);
            }
            Err(e) => {
                eprintln!("warn: WebGPU init failed ({e}); falling back to CPU");
            }
        }
    }
    let exec = parakeet_rs::ExecutionConfig::new()
        .with_execution_provider(parakeet_rs::ExecutionProvider::Cpu);
    eprintln!("info: Nemotron using execution provider: Cpu");
    parakeet_rs::Nemotron::from_pretrained(model_dir, Some(exec))
        .with_context(|| format!("loading Nemotron from {} (provider=Cpu)", model_dir.display()))
}
```

Update the `NemotronEngine::new` doc comment to note: "On macOS, `use_cuda` requests GPU acceleration via WebGPU; CPU fallback is automatic on WebGPU init failure."

If `parakeet_rs::Error` is not freely convertible to `anyhow::Error`, adapt the closure return type accordingly — the goal is the closure can be substituted in tests.

**Verification:**

```bash
cargo check --lib
cargo check --lib --target x86_64-apple-darwin
cargo test --lib
```

**Commit:** `macos: Nemotron WebGPU primary + CPU fallback in engine constructor`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Widen stt module cfg gating for macOS

**Files:**
- Modify: `src/stt/mod.rs` — change `#[cfg(target_os = "linux")]` to `#[cfg(any(target_os = "linux", target_os = "macos"))]` on platform-bound items the macOS port reuses unchanged.

**Implementation:**

Read `src/stt/mod.rs` end-to-end. Items to widen:
- `pub mod nemotron;` (currently line 3-4)
- `pub fn spawn_stt_thread(...)` (currently line 112-117)
- `fn build_engine(...)` (currently line 237)
- Any imports that exclusively serve the above

Neutral items (`SttEngine` trait, `AudioWake`, `PipelineConfig`, `Engine` re-export) stay unguarded.

**Verification:**

```bash
cargo check --lib
cargo check --lib --target x86_64-apple-darwin
cargo test --lib
```
Linux behavior unchanged; cross-target check now exercises the nemotron/build_engine/spawn_stt_thread surface on macOS.

**Commit:** `macos: widen stt module cfg gating to include macOS`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Wire real STT pipeline into main_macos.rs

**Verifies:** macos-port.AC4.5 (code-review verification: single STT thread, ArcSwap loads on chunk boundaries)

**Files:**
- Modify: `src/main_macos.rs` — replace Phase 2's hardcoded test-caption harness with real STT plumbing + fixture-WAV harness.

**Implementation:**

Read `src/main.rs:136-230` for the canonical Linux pipeline wiring order; mirror it. New shape:

1. **Model files present? else download.** Mirrors `src/main.rs:136-157`:
   ```text
   let model_dir = models::nemotron_model_dir();
   if !models::nemotron_models_present() {
       println!("Downloading Nemotron model files (first run)...");
       let rt = tokio::runtime::Builder::new_multi_thread()
           .enable_all()
           .build()
           .expect("tokio runtime");
       rt.block_on(async {
           models::ensure_nemotron_models().await
               .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1); });
       });
   }
   ```

2. **AudioWake + ring buffer.** Read `src/main.rs` near line 165 for exact `ringbuf` types (likely `HeapRb<f32>::new(N)` with `.split()` into producer/consumer). Mirror identical capacity.
   ```text
   let audio_wake = Arc::new(stt::AudioWake::new());
   let (ring_producer, ring_consumer) = /* same as Linux */;
   ```

3. **ArcSwap engine choice** seeded with `Engine::Nemotron`:
   ```text
   let engine_choice = Arc::new(arc_swap::ArcSwap::new(Arc::new(config::Engine::Nemotron)));
   ```

4. **PipelineConfig.** Read `src/main.rs:~216` for the exact field set and mirror; `use_cuda: true` on macOS:
   ```text
   let pipeline_cfg = stt::PipelineConfig {
       model_dir,
       use_cuda: true,
       engine_choice: Arc::clone(&engine_choice),
       // ... other fields copied verbatim from Linux construction ...
   };
   ```

5. **Spawn the STT thread:**
   ```text
   let _stt_handle = stt::spawn_stt_thread(
       ring_consumer,
       Arc::clone(&audio_wake),
       caption_tx.clone(),
       pipeline_cfg,
   );
   ```

6. **Replace Phase 2's test-caption harness with a Phase-3-only fixture-WAV harness.** Two options for the executor — pick (b) for simplicity:

   - **Option (a):** Use `rubato` to upsample the 16 kHz mono fixture → 48 kHz stereo (the pre-resampler contract), then push into `ring_producer`. Exercises the resampler.
   - **Option (b) [recommended]:** Ship a second fixture WAV at the pre-resampler format: `tests/fixtures/macos-webgpu-smoke-48k-stereo.wav` (48 kHz stereo PCM f32). Read it and push samples raw into `ring_producer`. Skips re-conversion noise.

   Either way, pace the pushes by sample timing (e.g., sleep ~10 ms between every 480 stereo samples for option (b)) so the STT pipeline sees realistic real-time input. Call `audio_wake.notify()` after each push.

   ```text
   // Phase 3 only — superseded by Phase 4's SCK capture.
   {
       let mut ring_producer = ring_producer;
       let wake = Arc::clone(&audio_wake);
       std::thread::Builder::new()
           .name("phase3-wav-harness".into())
           .spawn(move || {
               feed_fixture_wav(
                   "tests/fixtures/macos-webgpu-smoke-48k-stereo.wav",
                   &mut ring_producer,
                   &wake,
               ).unwrap_or_else(|e| eprintln!("warn: fixture harness exited: {e}"));
           })
           .expect("spawn phase3-wav-harness");
   }
   ```

   If shipping option (b), generate the second fixture from the existing one:
   ```bash
   ffmpeg -i tests/fixtures/macos-webgpu-smoke.wav -ar 48000 -ac 2 -sample_fmt flt \
          tests/fixtures/macos-webgpu-smoke-48k-stereo.wav
   ```
   And update `tests/fixtures/README.md` to document the new fixture.

7. **Ctrl-C handler and `overlay::macos::run_app` call** — unchanged from Phase 2.

8. **After `run_app` returns:** call `audio_wake.shutdown()` to release the STT thread from its `wait_timeout`. Drop senders. `std::process::exit(0)`.

**Code-review verification for AC4.5 (in commit body):** re-read `src/stt/mod.rs` `spawn_stt_thread` and confirm:
1. Exactly ONE thread is spawned.
2. `build_engine(&desired, &cfg.model_dir, cfg.use_cuda)` runs only inside that thread (line ~173).
3. `engine_choice.load()` is read at chunk-batch start, not concurrently with construction.
Document the audit in the commit body.

**Verification:**

```bash
cargo check --lib --target x86_64-apple-darwin
cargo check --lib
cargo test --lib
```

On the target Mac:
```bash
scripts/bundle-mac.sh
./target/release/Subtidal.app/Contents/MacOS/subtidal 2>&1 | tee /tmp/subtidal-phase3.log
```
Expect in `/tmp/subtidal-phase3.log`:
```
info: Nemotron using execution provider: WebGPU
```
Expect in the NSPanel: the fixture's transcribed text.

**Commit:** `macos: wire real STT pipeline + fixture-WAV harness in main_macos`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Unit test for CPU fallback on simulated WebGPU failure

**Verifies:** macos-port.AC4.2

**Files:**
- Modify: `src/stt/nemotron.rs` — add `#[cfg(all(test, target_os = "macos"))] mod tests`

**Implementation:**

Use the `build_macos_with(model_dir, use_cuda, try_webgpu)` seam from Task 1. Inject a closure that returns `Err`; assert the function still produces an `Ok(Nemotron)` via the CPU branch.

Because the CPU branch still calls `parakeet_rs::Nemotron::from_pretrained` with real model files, mark the test `#[ignore]` so CI without model files won't run it. Local runs do `cargo test --lib -- --ignored`.

```rust
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires Nemotron model files at the conventional model dir"]
    fn cpu_fallback_on_simulated_webgpu_failure() {
        let model_dir = crate::models::nemotron_model_dir();
        let result = build_macos_with(&model_dir, true, |_dir| {
            Err(anyhow::anyhow!("simulated WebGPU init failure"))
        });
        assert!(
            result.is_ok(),
            "CPU fallback should produce a working Nemotron, got: {:?}",
            result.err()
        );
    }
}
```

**Verification:**

On macOS:
```bash
cargo test --lib -- nemotron::tests --ignored
```
Expected: pass (after model download).

```bash
cargo test --lib
```
Expected: pass (test excluded by default).

Cross-target:
```bash
cargo check --lib --target x86_64-apple-darwin
```
Expected: green.

**Commit:** `macos: unit test for CPU fallback on simulated WebGPU failure`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Hardware verification — fixture WAV produces captions

**Verifies:** macos-port.AC4.1, macos-port.AC4.3

**Files:** none (operational verification only)

**Implementation:**

On the target Apple Silicon Mac:

```bash
# Ensure model files are downloaded (first launch triggers download).
./target/release/Subtidal.app/Contents/MacOS/subtidal

scripts/bundle-mac.sh
./target/release/Subtidal.app/Contents/MacOS/subtidal 2>&1 | tee /tmp/subtidal-phase3.log
```

Observe:

1. **AC4.1 (WebGPU primary):** `/tmp/subtidal-phase3.log` contains
   ```
   info: Nemotron using execution provider: WebGPU
   ```
   near the start.

2. **AC4.3 (transcript parity):** captions appearing in the NSPanel match the fixture's expected transcription (per `tests/fixtures/README.md`). Compare token sequence against a Linux baseline run on the same fixture — small whitespace differences acceptable; identical token sequence required. If divergence is large, surface to the user before proceeding to Phase 4.

3. **Cross-target CI green:**
   ```bash
   cargo check --lib --target x86_64-apple-darwin
   ```

**Commit:** none (verification only). If the Linux baseline reveals tokenizer-tolerance issues worth remembering, capture them in a memory note for the user (analogue of `project_gpu_cuda_landmines.md`).
<!-- END_TASK_5 -->
