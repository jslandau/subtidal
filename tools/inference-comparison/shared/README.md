# Standalone inference runners

Each sibling runner is an independent Cargo workspace with its own committed lockfile and ignored `target/`. Build from the runner directory (or pass `--manifest-path`) with `cargo build --locked --release`; do not supply a shared CARGO_TARGET_DIR. No production crate is linked and no runner downloads models or audio.

## CLI

```
runner-baseline --model-dir PATH --diar-model FILE --fixture WAV \
  --provider cpu --mode combined --output result.json --warmup 1
```

The same flags apply to `runner-ort-v3` and `runner-burn`. Required: `--fixture`, `--provider cpu|cuda`, `--mode asr|diar|combined`, `--output`. ASR modes require `--model-dir` (Nemotron pretrained directory); diarization modes require `--diar-model` (exact model file, not directory). `--warmup N` defaults to one complete fixture pass. Every warmup is followed by Nemotron reset and Sortformer reset_state; measured framing and output counters are newly constructed. `--help` prints usage. Duplicate and unknown flags fail. WAV must be nonempty finite 16kHz mono PCM signed i16 or f32; i16 normalization divides by 32768.

ORT runners retain upstream default features, with CUDA and copy-dylibs: each builds/stages the core selected by its exact ort pin, rather than selecting a system core through load-dynamic. v3 creates an explicit environment and clones it into execution configurations. Burn has no ORT dependency and explicitly selects BurnCpu or BurnCuda. Burn 0.22 requires Rust 1.95; use cargo +1.95.0 for that standalone runner.

## Models and provider limitations

Baseline pins parakeet-rs 0.3.6 / ort rc12 and mirrors current production Callhome postprocessing with chunk_len=6, right_context=7, fifo_len=188, spkcache_len=188, reset_state. v3 and Burn pin parakeet-rs 0.4.1, use default postprocessing and set_profile(StreamingProfile::low_latency()). **0.4.1 requires Nemotron-3 diarization outputs preds_diar/preds_hires and rejects the production v2.1 Sortformer export.** Supply a compatible exported model; runners never replace it silently.

Both ORT CUDA paths first execute a deterministic Add graph (`2+2=4`) in a direct CUDA session with error_on_failure and session.disable_cpu_ep_fallback=1. A CUDA initialization failure therefore aborts, not silently downgrades the whole trial. **Upstream parakeet 0.3.6 and 0.4.1 nevertheless append CPU to their internal CUDA model sessions; their public API provides no per-session strict-fallback switch. Preflight does not prove all real model nodes execute on CUDA. Treat model-wide no-fallback as unresolved, not as verified by the provider label.** Burn uses only its explicitly selected backend. CUDA preflight is included in load_seconds, outside timed inference.

## Streaming and output schema (version 1)

Audio is fed unpaced in 2560-sample blocks (last block may be shorter), serial diarization first then ASR. ASR accumulates exactly 8960 samples per inference. At end, a nonempty residual is zero-padded to 8960 once; no extra ASR chunk is added if aligned. Sortformer is then flushed. Segment coordinates are upstream absolute sample offsets, never shifted by feed position; clip end to real_samples, discard empty segments and reject out-of-range IDs, allow speaker IDs 0..7. `padding_samples` reports the terminal ASR zero padding; `runtime_config` records actual streaming capacities/profile and the upstream ORT fallback limitation. Padding never contributes audio duration.

JSON fields:
- `schema_version`, `runner`, `provider`, `mode`, `fixture`, `warmup_runs`.
- `real_samples`, `duration_seconds` (real_samples/16000).
- `load_seconds`, `warmup_seconds` (excluded), `compute_seconds`, `rtf` (compute_seconds/duration_seconds), `timing_scope`.
- `transcript`: exact concatenation of emitted fragment text, no trimming, joining spaces, dedup or normalization.
- `fragments`: `{text, emit_sample, finish?}`; emission positions refer to admitted real audio, not a claimed word alignment.
- `segments`: `{start_sample,end_sample,speaker_id}` absolute coordinates. Streaming segments are recorded as returned, not merged.
- `ticks`: `{feed_index,audio_start_sample,audio_end_sample,diar_seconds,diar_feed_seconds,diar_inference_passes,asr_seconds,asr_inference}`. ASR buffering ticks are explicitly zero seconds / false. Diarization pass counts use the verified upstream window/stride cadence: (chunk_len+right_context)*1280 / chunk_len*1280. `diar_seconds` is zero on no-inference ticks; `diar_feed_seconds` retains total feed cost.
- `first_asr_output`, `first_diar_output`: null or `{compute_seconds,audio_sample}`. These are unpaced first-output compute and input-admission positions, not paced end-to-end latency.
- `finish_asr_seconds`, `finish_diar_seconds`: terminal work, included in compute_seconds.

The measured clock includes framing and output collection and synchronous returned host data, excludes WAV loading, model loading, warmup, JSON serialization/write. Burn calls return host text/segment data; timing includes that materialization. Upstream parakeet-rs 0.4.1 burn_backend/mod.rs::vec_f32 and vec_i64 call Tensor::into_data().convert().try_into_vec() (lines 154–167); burn_backend/nemotron.rs uses those helpers for encoded_len/cache lengths and decoder logits (lines 131, 136, 192). Host decoding therefore waits for the logits it consumes; this is source-level synchronization evidence, not a GPU trace or proof that unrelated queued kernels have completed. JSON uses serde_json only in the driver; contract.rs itself has no dependencies.

## Tests

`cargo test --locked` in each runner checks ASR framing and one-time padding, absolute offset clipping/ID validation, reset offsets, real-duration RTF, buffering tick schema and fragment whitespace fidelity. They do not load a model or verify GPU execution. Runtime validation must separately exercise actual model/backend combinations.
