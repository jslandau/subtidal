# Native transcript tests

This document describes the native GTK/AppKit test targets and the CI prerequisites. These tests exercise transcript-window behavior without starting audio capture or downloading inference models.

## Current coverage

| Platform | Test entry point | Current checks | Limitations |
| --- | --- | --- | --- |
| Linux | `src/overlay/linux/transcript_tests.rs::linux_transcript_native_routing` (ignored library test) | Runs the GTK routing scenario; checks Unicode append and single log write, retroactive speaker relabeling and rename, transcript mode/visibility, settled follow-scroll behavior through a long wrapped-caption/resize matrix, paused reading position, append-only text-buffer mutations, selection preservation through speaker-name correction, and caption-disable clearing of the transcript log/view, caption buffer, overlay label, and pending callback. | Exercises substantial native routing and scroll behavior, but does not cover every transcript acceptance criterion or native save-dialog interaction. |
| macOS | `tests/native_transcript.rs` (harness-free executable) calling `src/overlay/macos/transcript_native_tests.rs::run` | Runs AppKit setup on the executable's main thread; checks Unicode text and one log write, selection preservation when rebuilding, clear/follow-state behavior, and a native follow/scroll scenario with wrapped history and disabled-follow preservation. | Does not cover every transcript acceptance criterion or interact with a native save dialog. |

The macOS target is `harness = false` so Rust's test harness does not move AppKit setup to a worker thread. `Cargo.toml` gates it behind the opt-in `native-transcript-tests` feature. Do not replace it with ordinary `cargo test --lib` for AppKit UI coverage. The existing AppKit unit tests using `MainThreadMarker::new()` can silently skip their assertions if run on worker threads.

## Linux: Ubuntu 24.04

### System requirements

- Ubuntu 24.04 (Noble), GTK 4.10 or newer (`gtk4` development package and pkg-config metadata).
- PipeWire development files (`libpipewire-0.3-dev`) for compiling the Linux target.
- GTK4 Layer Shell development library. Noble does not provide `libgtk4-layer-shell-dev`; CI builds upstream gtk4-layer-shell **v1.0.2** from source using Meson/Ninja against Noble's GTK4 and Wayland packages.
- `libclang-dev` for PipeWire Rust bindings, compiler/build tools, `libssl-dev`, and pkg-config.
- Xvfb and `xauth` to provide an X11 display for the GTK test. Missing display initialization is an error, not a skip.
- No running PipeWire daemon, Wayland compositor, microphone, model, or GPU is needed by the transcript test itself. PipeWire development files remain build dependencies for the crate.

### CI commands

The authoritative setup is `.github/workflows/native-transcript.yml`. Its native test commands are:

```sh
cargo fmt --all -- --check
cargo test --lib
xvfb-run --auto-servernum --server-args='-screen 0 1280x1024x24' \
  env GDK_BACKEND=x11 cargo test --lib linux_transcript_native_routing -- --ignored --test-threads=1
```

The GTK Layer Shell build uses the pinned upstream source and disables optional components not needed by this Rust dependency. The installed library must expose the `gtk4-layer-shell-0` pkg-config module at version 1.0 or newer, matching the locked `gtk4-layer-shell-sys 0.5.2` system-deps metadata; upstream v1.0.2 meets that requirement.

```sh
git clone --depth 1 --branch v1.0.2 \
  https://github.com/wmww/gtk4-layer-shell.git /tmp/gtk4-layer-shell
meson setup /tmp/gtk4-layer-shell/build /tmp/gtk4-layer-shell \
  --prefix=/usr/local \
  -Dexamples=false -Ddocs=false -Dtests=false -Dsmoke-tests=false \
  -Dintrospection=false -Dvapi=false
ninja -C /tmp/gtk4-layer-shell/build
sudo ninja -C /tmp/gtk4-layer-shell/build install
sudo ldconfig
```

Before compiling, CI requires `pkg-config --atleast-version=4.10 gtk4`, `pkg-config --atleast-version=1.0 gtk4-layer-shell-0`, and `xvfb-run` to succeed. CI lists the filtered ignored test and requires exactly one matching test before executing it, so a renamed or missing test cannot report success with zero tests run.

`build.rs` finds the ORT provider library produced by `ort-sys` in the Cargo target output/cache and follows it to its canonical runtime distribution. CI applies the same dependency-output discovery pattern: after `cargo test --lib --no-run`, resolve `libonnxruntime_providers_cuda.so` under `target/debug`, require the adjacent `libonnxruntime.so`, and add that directory to `LD_LIBRARY_PATH`. This is runtime loader setup only; CI must not use or fetch model files to locate ORT.

### Local run

Install the Ubuntu prerequisites above, build/install gtk4-layer-shell v1.0.2, and run the same test command under Xvfb. If Cargo reports a missing ORT runtime, locate the runtime via Cargo's `target/debug` provider link as CI does; do not treat a model directory as an ORT runtime location. The test must fail if Xvfb/GTK initialization fails.

## macOS: Apple Silicon

CI uses `macos-latest` (Apple Silicon) and runs the harness-free target:

```sh
cargo fmt --all -- --check
cargo test --lib
cargo test --features native-transcript-tests --test native_transcript
```

`tests/native_transcript.rs` is a plain `main` entry point. The AppKit adapter runner checks `MainThreadMarker::new()` with `expect`, initializes `NSApplication`, and prints `native transcript checks passed` after all assertions. A failed setup/assertion exits unsuccessfully; the workflow does not treat a normal test count, a main-thread skip, or a missing runner as success. This native job supplements and does not replace `.github/workflows/macos-check.yml`, which remains the macOS target compile check.

## Verification status

See [Transcript implementation verification](transcript-verification.md) for executed results and outstanding acceptance requirements. The full macOS runner currently fails its strict synthetic wheel-movement scenario; CI is intentionally not made green by skipping that assertion. Linux scenarios currently exercise native controller signals, not compositor-delivered GDK events, and have not been executed on the development macOS host. The operator has cleared the queued-update race for current use and asked that it be monitored in ordinary use; trackpad momentum/rubberbanding were reported working. Remaining light/dark, scrollbar and accessibility checks are recorded verification limitations, not completed checks.

## Extending coverage

Before claiming broader transcript acceptance coverage, add explicit assertions to the native platform adapters and keep the entry points above aligned with Cargo configuration and CI. Continue to isolate model inference and audio capture from transcript UI checks. Any test requiring those runtime inputs belongs in a separate test path with its own declared prerequisites.
