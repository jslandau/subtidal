# Code review — 2026-04-22

Critique of the Copilot-generated codebase. Issues numbered for cross-reference.
Status: `open` / `fixed` / `wontfix`.

## Session summary (2026-04-22)

Collapsed the bridge+inference threads into a single `stt-pipeline` thread driven
by a condvar wake from the PipeWire RT callback; replaced the mpsc polling on the
GTK side with `async_channel` + `glib::MainContext::spawn_local`.

Fixed outright: **#1, #2, #7, #13, #19**.
Partially fixed: **#4** (per-chunk channel allocation gone; internal rubato
adapter churn remains), **#11** (coordination machinery deleted; enum + CLI
alias retained), **#16** (`SttEngine::sample_rate` deleted; module-level
allow-dead-code remains).

Remaining open: **#3, #5, #6, #8, #9, #10, #12, #14, #15, #17, #18, #20**.


## Correctness bugs / latent problems

### 1. Audio bridge busy-polls at 5ms with a mutex on every chunk — `src/main.rs:314-347`
Status: **fixed (2026-04-22)** — collapse of bridge+inference thread, condvar wake from RT callback, `ArcSwap` for engine choice.

### 2. Lost chunks on engine switch — `src/main.rs:332-337`
When `tx.send` fails mid-switch, the remaining chunks in the batch are silently dropped.
Status: **fixed (2026-04-22)** — obviated by collapse; engine swap is a local `Box<dyn SttEngine>` replacement, no channel to fail on.

### 3. Disappeared-node cleanup uses `try_lock` and silently skips — `src/audio/mod.rs:230-251`
If contended, the whole disappearance-event cleanup is deferred to next iteration. Works in practice but brittle.
Status: open.

### 4. `AudioResampler::push_interleaved` allocates heavily per chunk — `src/audio/resampler.rs:55-112`
Per 160ms chunk: `drain().collect()`, two channel `Vec`s, two output `Vec`s, `Vec<Vec<f32>>` wrappers, two adapters, another `drain().collect()` per output chunk. ~8 allocations per 160ms tick.
Status: **partially fixed (2026-04-22)** — per-chunk delivery allocation gone; internal rubato adapter churn remains.

### 5. `CaptionBuffer::remove_overlap` byte-slices lowercased strings — `src/overlay/mod.rs:172-192`
`to_lowercase()` can change byte length (ß→ss, İ→i̇); indexing across the original/lowercase boundary is unsound for non-ASCII. Latent because Nemotron is English-only.
Status: open.

### 6. `_exit(0)` loses buffered stderr — `src/main.rs:538`
Final shutdown messages from `eprintln!` can be dropped. Flush stdout/stderr before `_exit`.
Status: open.

### 7. Ctrl-C to GTK has up to 100ms latency via polling
Addressed by GTK sweep below.
Status: **fixed (2026-04-22)** — part of GTK glib-channel sweep.

### 8. `ensure_nemotron_models` downloads serially
Could be `join_all`'d. The ~600MB `encoder.onnx.data` dominates anyway.
Status: open.

### 9. `find_ort_cache_dir` picks by mtime, not ORT version
Stale cache dirs can be picked over the correct one.
Status: open.

## Architecture / design

### 10. `overlay/mod.rs` is 1176 lines, mixes four concerns
`CaptionBuffer` (pure, ~200 lines, well-tested) should live in its own module. Drag handler with compositor-quirk compensation too.
Status: open.

### 11. `Engine` enum has one variant but full polymorphic plumbing
Parser accepts both "nemotron" and "parakeet" → same variant; engine switch channel, handle retention vec, restart machinery all for one engine.
Status: **partially fixed (2026-04-22)** — coordination machinery deleted as part of collapse. The enum itself and CLI alias are retained for future Parakeet reintroduction, but at near-zero ongoing cost.

### 12. Config TOML write race
Multiple writers: hot-reload, tray, drag-end save, fallback handler. Writes are mostly idempotent but no serialization. Mitigated by debouncer "only send on change" logic.
Status: open.

### 13. GTK polls two mpsc channels every 100ms via `Arc<Mutex<Receiver>>`
`Mutex` around single-thread receiver is unnecessary. Should use glib async channels.
Status: **fixed (2026-04-22)** — replaced with `async_channel` + `glib::MainContext::spawn_local` futures; no more polling and no more `Arc<Mutex<Receiver>>`. The caption-bridge thread that forwarded between two mpsc channels is also gone (STT sends directly to the GTK-consumed `async_channel`).

### 14. CUDA probe subprocess loads full 600M-param model on every startup
Should cache result keyed on (ort version, cuda driver version).
Status: open.

## Cosmetic / code smell

### 15. Copilot narration comments ("Phase 2:", "AC1.4", etc.) reference a spec doc readers don't have
Violates own CLAUDE.md rule about not referencing task/fix in comments. Should mostly be deleted.
Status: open.

### 16. `#![allow(dead_code)]` + per-method `#[allow(dead_code)]` on `SttEngine::sample_rate` with "future phases" comment
If unused now, delete. `git` remembers.
Status: **partially fixed (2026-04-22)** — `sample_rate()` deleted from the trait and its impl. The module-level `#![allow(dead_code)]` in `audio/mod.rs` and `models/mod.rs` remains.

### 17. Tests over-cover trivia
`test_models_dir_is_valid_path`, `cuda_status_message_when_available`, etc. The `CaptionBuffer` tests are genuinely valuable; the AC-prefixed tests could be collapsed.
Status: open.

### 18. `_prefix` convention overloaded
Used for both "lifetime-holder" and "actual unused" without distinction. Named struct fields are clearer.
Status: open.

### 19. `handle_overlay_command` suppresses entire queue during drag
`Quit` and `SetVisible` should bypass drag-suppression; only layout-causing commands need gating.
Status: **fixed (2026-04-22)** — the new async command consumer matches `OverlayCommand::Quit | OverlayCommand::SetVisible(_)` and bypasses the drag gate; only layout-changing commands (`SetMode`, `SetLocked`, `UpdateAppearance`) are still deferred during a drag.

### 20. `compositor_shifts_coords_on_margin_change` is a one-shot env-var sniff
No user override for new compositors. Should be exposed in config.
Status: open.

## Priority

Originally identified top three:

1. Delete `Engine` indirection — **done** as part of #1 collapse.
2. Cache CUDA probe result — **#14**, open.
3. Extract `CaptionBuffer` — **#10**, open.
