# Test fixtures

## macos-webgpu-smoke.wav

Short English speech clip used by `examples/macos_webgpu_smoke.rs` (Phase 0 of
the macOS port) to verify that `parakeet_rs::ExecutionProvider::WebGPU` runs
on Apple Silicon Metal and produces a reasonable transcription.

- Source: synthesized locally via macOS `say` (default system voice),
  `say -o macos-webgpu-smoke.wav --file-format=WAVE --data-format=LEI16@16000 "<text>"`
- Format: 16 kHz mono PCM s16le, ~4.2 s
- Expected transcription (approximate): "The quick brown fox jumps over the lazy dog and runs through the forest"

Used only for the spike; not exercised by `cargo test`.
