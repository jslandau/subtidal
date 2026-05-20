//! Platform-isolated system-tray subsystem.
//!
//! Public types (`TrayState`) and entry points (`spawn_tray`) are re-exported from
//! the Linux implementation in `impl_linux.rs`. To add a new platform, create a
//! sibling `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export
//! the same public surface here.

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{spawn_tray, TrayState};

#[cfg(target_os = "macos")]
pub mod impl_macos;

#[cfg(target_os = "macos")]
pub use impl_macos::{install_tray, TrayState};
