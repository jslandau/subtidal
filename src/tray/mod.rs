//! Platform-isolated system-tray subsystem.
//!
//! Public types (`TrayState`) and entry points (`spawn_tray`) are re-exported from
//! the Linux implementation in `impl_linux.rs`. To add a new platform, create a
//! sibling `impl_<os>.rs`, gate it with `#[cfg(target_os = "<os>")]`, and re-export
//! the same public surface here.

pub mod icon;

// ---------------------------------------------------------------------
// Tray refresh handoff (F9): neutral, Linux-testable routing decision for
// the macOS 5 s audio-source refresh. Core Audio enumeration (blocking IPC
// per process) runs off the main thread; AppKit labeling and the submenu
// rebuild run on main. These plain-integer/string types make the routing
// decision testable on any target.
// ---------------------------------------------------------------------

/// One audio process as seen by Core Audio, in platform-neutral form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioProcessSnapshot {
    /// Core Audio process AudioObjectID.
    pub object_id: u32,
    /// Bundle identifier (never empty for entries in the snapshot).
    pub bundle_id: String,
    /// POSIX PID for the liveness watchdog.
    pub watchdog_pid: i32,
}

/// What the 5 s tray refresh must do after enumerating processes off-main.
#[derive(Clone, Debug)]
pub struct HandoffPlan {
    /// The snapshot changed (or is the first one): labeling + equality
    /// bookkeeping + submenu rebuild are required on the main thread.
    pub changed: bool,
    /// Raw processes to label on the main thread (empty when unchanged).
    pub label_on_main: Vec<AudioProcessSnapshot>,
}

/// Routing decision for the tray refresh handoff:
/// - Enumeration failed (`next: None`) → skip everything.
/// - Identical snapshots → skip everything (no labeling, no rebuild).
/// - Changed → marshal the raw snapshot to main, where `bundle_to_label`
///   (`NSRunningApplication`, AppKit) and the submenu rebuild happen.
///
/// Core Audio enumeration itself never runs on the main thread; only the
/// `label_on_main` payload crosses back.
pub fn plan_refresh_handoff(
    prev: &[AudioProcessSnapshot],
    next: Option<Vec<AudioProcessSnapshot>>,
) -> HandoffPlan {
    match next {
        None => HandoffPlan {
            changed: false,
            label_on_main: Vec::new(),
        },
        Some(next) => {
            if next == prev {
                HandoffPlan {
                    changed: false,
                    label_on_main: Vec::new(),
                }
            } else {
                HandoffPlan {
                    changed: true,
                    label_on_main: next,
                }
            }
        }
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;

    fn snap(bundle: &str, oid: u32, pid: i32) -> AudioProcessSnapshot {
        AudioProcessSnapshot {
            object_id: oid,
            bundle_id: bundle.to_string(),
            watchdog_pid: pid,
        }
    }

    /// AC.9: on a changed snapshot the raw processes are marshaled to main
    /// for AppKit labeling + submenu rebuild — Core Audio enumeration stays
    /// off-main (it already happened; the plan only carries results), and an
    /// enumeration failure skips everything.
    #[test]
    fn plan_refresh_handoff_keeps_appkit_on_main() {
        let prev = vec![snap("com.apple.Music", 1, 100)];
        let next = Some(vec![
            snap("com.apple.Music", 1, 100),
            snap("org.mozilla.firefox", 7, 900),
        ]);
        let plan = plan_refresh_handoff(&prev, next);
        assert!(plan.changed, "process set changed");
        assert_eq!(plan.label_on_main.len(), 2, "raw snapshot goes to main");
        assert_eq!(plan.label_on_main[1].bundle_id, "org.mozilla.firefox");

        // First run (empty prev) is a change: initial labeling on main.
        let plan = plan_refresh_handoff(&[], Some(vec![snap("a.b.c", 3, 5)]));
        assert!(plan.changed);
        assert_eq!(plan.label_on_main.len(), 1);

        // Enumeration failure: no work at all, main untouched.
        let plan = plan_refresh_handoff(&prev, None);
        assert!(!plan.changed);
        assert!(plan.label_on_main.is_empty());
    }

    /// AC.9: identical snapshots → changed=false, no labeling, no rebuild.
    #[test]
    fn plan_refresh_handoff_skips_when_unchanged() {
        let prev = vec![snap("a.x", 1, 10), snap("b.y", 2, 20)];
        let next = Some(prev.clone());
        let plan = plan_refresh_handoff(&prev, next);
        assert!(!plan.changed);
        assert!(plan.label_on_main.is_empty());

        // Order is part of identity (the producer canonicalizes the order).
        let reordered = Some(vec![prev[1].clone(), prev[0].clone()]);
        let plan = plan_refresh_handoff(&prev, reordered);
        assert!(plan.changed, "non-canonical order is treated as a change");

        // Both empty: unchanged.
        let plan = plan_refresh_handoff(&[], Some(vec![]));
        assert!(!plan.changed);
    }
}

#[cfg(target_os = "linux")]
mod impl_linux;

#[cfg(target_os = "linux")]
pub use impl_linux::{spawn_tray, TrayState};

#[cfg(target_os = "macos")]
pub mod impl_macos;

#[cfg(target_os = "macos")]
pub use impl_macos::{install_tray, TrayState};
