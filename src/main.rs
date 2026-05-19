//! Subtidal binary entry point.
//!
//! Platform-specific startup logic lives in `main_linux` (Linux) and `main_macos` (macOS),
//! both gated with `#[cfg(target_os = "...")]`. This binary crate acts as a thin dispatcher.

#[cfg(target_os = "linux")]
mod main_linux;

#[cfg(target_os = "macos")]
mod main_macos;

// Hard-fail the binary build on unsupported platforms with a clear message.
// `cargo check --lib --target ...` does NOT compile this binary and so does
// not trip this error — that is how the CI cross-target-check stays green while
// `cargo build` on unsupported targets fails fast with a single, clear message.
//
// Placement note: this MUST come AFTER all `mod` declarations, per Rust's mod-resolution order.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("Subtidal supports Linux and macOS only.");

/// Dispatch to the platform-specific main function.
fn main() {
    #[cfg(target_os = "linux")]
    main_linux::main();

    #[cfg(target_os = "macos")]
    main_macos::main();
}
