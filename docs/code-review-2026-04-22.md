# Code review — 2026-04-22

Critique of the Copilot-generated codebase. Issues numbered for cross-reference.
Status: `open` / `fixed` / `wontfix`.

## Session summary (2026-04-22)

Collapsed the bridge+inference threads into a single `stt-pipeline` thread driven
by a condvar wake from the PipeWire RT callback; replaced the mpsc polling on the
GTK side with `async_channel` + `glib::MainContext::spawn_local`.

Fixed outright: **#1, #2, #3, #4, #7, #10, #11, #13, #16, #19**.

Remaining open: **#5, #6, #8, #9, #12, #14, #15, #17, #18, #20**.


## Correctness bugs / latent problems

### 1. Audio bridge busy-polls at 5ms with a mutex on every chunk — `src/main.rs:314-347`
Status: **fixed (2026-04-22)** — collapse of bridge+inference thread, condvar wake from RT callback, `ArcSwap` for engine choice.

### 2. Lost chunks on engine switch — `src/main.rs:332-337`
When `tx.send` fails mid-switch, the remaining chunks in the batch are silently dropped.
Status: **fixed (2026-04-22)** — obviated by collapse; engine swap is a local `Box<dyn SttEngine>` replacement, no channel to fail on.

### 3. Disappeared-node cleanup uses `try_lock` and silently skips — `src/audio/mod.rs:230-251`
If contended, the whole disappearance-event cleanup is deferred to next iteration. Works in practice but brittle.
Status: **fixed (2026-04-22)** — `Arc<Mutex<Vec<u32>>>` swapped for `Rc<RefCell<Vec<u32>>>`. Producer (`global_remove` closure) and consumer (post-iterate sweep) both run on the pipewire-audio thread between mainloop iterations, so same-thread interior mutability is the structurally correct primitive. `add_listener_local`'s bound is `Fn + 'static` without `Send`, so the `!Send` `Rc`/`RefCell` captures cleanly; `disappeared_node_ids` is created inside the spawned thread and never crosses a thread boundary. Consumer now drains into a stack-local `Vec` before touching `node_list` (which is itself a `Mutex`), keeping the `RefCell` borrow scope minimal. Skip-on-contention is gone; an accidental double-borrow would panic loudly instead of silently dropping disappearance events.

### 4. `AudioResampler::push_interleaved` allocates heavily per chunk — `src/audio/resampler.rs:55-112`
Per 160ms chunk: `drain().collect()`, two channel `Vec`s, two output `Vec`s, `Vec<Vec<f32>>` wrappers, two adapters, another `drain().collect()` per output chunk. ~8 allocations per 160ms tick.
Status: **fixed (2026-04-22)** — all working buffers (input accumulator, deinterleaved channel vecs, rubato output vecs) are now owned fields on `AudioResampler`, pre-sized at construction and reused via `clear()`/in-place indexing. Input draining replaced with index-based consume + one trailing `drain(..consumed)`. Return type changed from `Vec<Vec<f32>>` to a `FnMut(&[f32])` callback so produced chunks are handed off by slice reference with no heap allocation at the boundary. The only remaining per-call allocations are the two `SequentialSliceOfVecs` adapter structs, which are stack-sized views over the owned vecs.

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
Status: **fixed (2026-04-22)** — split into four submodules along dependency-boundary lines: `caption_buffer.rs` (pure std, 200 lines of logic + 350 of tests), `drag.rs` (GTK + compositor-quirk compensation, 128 lines), `window.rs` (GTK + layer-shell construction/styling, 266 lines), and `mod.rs` (239 lines of orchestration, command dispatch, and public API). Pure movement; no behavior changes. Tests travelled with their modules: CaptionBuffer tests to `caption_buffer.rs`, CSS + `estimate_max_chars` tests to `window.rs`. `mod.rs` dropped from 1185 → 239 lines.

### 11. `Engine` enum has one variant but full polymorphic plumbing
Parser accepts both "nemotron" and "parakeet" → same variant; engine switch channel, handle retention vec, restart machinery all for one engine.
Status: **fixed (2026-04-22)** — the costly coordination machinery was deleted in the earlier collapse. The enum, CLI alias, `parse_engine`, `ArcSwap<Engine>`, and `build_engine` dispatch are intentionally retained as the seam for adding Parakeet/Whisper/etc.; their ongoing cost is negligible and they're now load-bearing scaffolding rather than cruft.

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
Status: **fixed (2026-04-22)** — module-level `#![allow(dead_code)]` removed from `audio/mod.rs` and `models/mod.rs`. Fallout deleted: `AudioNode::is_monitor` (written, never read), `list_nodes`, `AudioResampler::flush`. Instead of deleting `nemotron_model_files`, the three-way filename duplication (`NEMOTRON_FILES`, the presence check, and the getter) was consolidated: `NEMOTRON_FILES` is now the single source of truth and `nemotron_model_files_in` / `nemotron_models_present_in` both derive from it. `CaptureStream::stream`/`listener` retain targeted `#[allow(dead_code)]` with a comment explaining they exist for their Drop only.

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
