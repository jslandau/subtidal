//! Safe wrappers around Core Audio process enumeration.
//!
//! Core Audio maintains a global registry of audio-producing processes.
//! This module provides helpers to enumerate them, translate PIDs, and check
//! process liveness. Used at UI time (source selection) and at runtime
//! (watchdog detection of source disappearance).

use anyhow::Result;
use coreaudio_sys::*;
use core_foundation::string::CFString;
use core_foundation::base::FromVoid;

/// Basic info about an audio-producing process.
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    pub audio_object_id: AudioObjectID,
    pub pid: std::ffi::c_int,
    pub bundle_id: Option<String>,
}

/// Enumerate all process AudioObjects known to Core Audio.
/// Returns processes that have a bundle ID. Does NOT filter by
/// `kAudioProcessPropertyIsRunning`: that property only reports "audio IO
/// is occurring at this exact instant", which is the wrong question for a
/// source picker — it's empty whenever the user has paused playback.
/// The presence of a process AudioObject is itself the signal that the
/// process has instantiated an audio engine and is tap-capturable.
///
/// # Errors
/// Returns Err if Core Audio property reads fail (e.g., communication with auditd broken).
pub fn enumerate_audio_processes() -> Result<Vec<ProcessInfo>> {
    unsafe {
        // Step 1: Read kAudioHardwarePropertyProcessObjectList size.
        let mut size: u32 = 0;
        let status = AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject,
            &AudioObjectPropertyAddress {
                mSelector: kAudioHardwarePropertyProcessObjectList,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            },
            0,
            std::ptr::null(),
            &mut size,
        );
        if status != 0 {
            anyhow::bail!("AudioObjectGetPropertyDataSize failed: status={}", status);
        }

        let n_processes = (size as usize) / std::mem::size_of::<AudioObjectID>();
        if n_processes == 0 {
            return Ok(vec![]);
        }

        // Step 2: Allocate and read the process object IDs.
        let mut process_ids = vec![0_u32; n_processes];
        let status = AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &AudioObjectPropertyAddress {
                mSelector: kAudioHardwarePropertyProcessObjectList,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            },
            0,
            std::ptr::null(),
            &mut size,
            process_ids.as_mut_ptr() as *mut std::ffi::c_void,
        );
        if status != 0 {
            anyhow::bail!("AudioObjectGetPropertyData (list) failed: status={}", status);
        }

        // Step 3: For each process, read PID and bundle ID. Skip entries with no
        // bundle ID (system kernel daemons, etc. — not user-selectable). Do NOT
        // filter by IsRunning — see function docstring.
        let mut out = vec![];
        for &obj_id in &process_ids {
            let pid = read_process_pid(obj_id)?;
            let bundle_id = read_process_bundle_id(obj_id)?;
            if bundle_id.is_none() {
                continue;
            }
            out.push(ProcessInfo {
                audio_object_id: obj_id,
                pid,
                bundle_id,
            });
        }

        Ok(out)
    }
}

/// Translate a PID into the corresponding Core Audio process AudioObjectID.
/// Currently unused — kept around because it's the natural inverse of the
/// PID→object_id mapping we expose via `enumerate_audio_processes()` and may
/// be useful for future tray/source-picker work in Phase 6.
///
/// # Errors
/// Returns Err if the PID is not found or Core Audio communication fails.
#[allow(dead_code)]
pub fn translate_pid_to_process_object(pid: std::ffi::c_int) -> Result<AudioObjectID> {
    unsafe {
        let mut out_id: AudioObjectID = 0;
        let mut out_size = std::mem::size_of::<AudioObjectID>() as u32;
        let status = AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &AudioObjectPropertyAddress {
                mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            },
            std::mem::size_of_val(&pid) as u32,
            &pid as *const _ as *const std::ffi::c_void,
            &mut out_size,
            &mut out_id as *mut _ as *mut std::ffi::c_void,
        );
        if status != 0 {
            anyhow::bail!("AudioObjectGetPropertyData (PID translate) failed: status={}", status);
        }
        Ok(out_id)
    }
}

/// Whether the given process AudioObject reports `kAudioProcessPropertyIsRunning = true`.
///
/// NOTE: this is NOT a "process is alive" check — it means "audio I/O is
/// occurring at this instant", which goes false whenever the user pauses
/// playback. The watchdog uses POSIX `kill(pid, 0)` for liveness instead.
/// Retained for the smoke test and any future use that needs the activity
/// signal specifically.
#[allow(dead_code)]
pub fn process_is_running(obj: AudioObjectID) -> bool {
    unsafe {
        let mut is_running: u32 = 0;
        let mut size = std::mem::size_of_val(&is_running) as u32;
        let status = AudioObjectGetPropertyData(
            obj,
            &AudioObjectPropertyAddress {
                mSelector: kAudioProcessPropertyIsRunning,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            },
            0,
            std::ptr::null(),
            &mut size,
            &mut is_running as *mut _ as *mut std::ffi::c_void,
        );
        status == 0 && is_running != 0
    }
}

/// Read the PID property from a process AudioObject.
unsafe fn read_process_pid(obj: AudioObjectID) -> Result<std::ffi::c_int> {
    let mut pid: std::ffi::c_int = 0;
    let mut size = std::mem::size_of_val(&pid) as u32;
    let status = AudioObjectGetPropertyData(
        obj,
        &AudioObjectPropertyAddress {
            mSelector: kAudioProcessPropertyPID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        },
        0,
        std::ptr::null(),
        &mut size,
        &mut pid as *mut _ as *mut std::ffi::c_void,
    );
    if status != 0 {
        anyhow::bail!("read_process_pid failed: status={}", status);
    }
    Ok(pid)
}

/// Read the bundle ID property from a process AudioObject.
/// Returns Ok(None) if the process has no bundle ID.
unsafe fn read_process_bundle_id(obj: AudioObjectID) -> Result<Option<String>> {
    let mut cf_string: CFStringRef = std::ptr::null_mut();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;
    let status = AudioObjectGetPropertyData(
        obj,
        &AudioObjectPropertyAddress {
            mSelector: kAudioProcessPropertyBundleID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        },
        0,
        std::ptr::null(),
        &mut size,
        &mut cf_string as *mut _ as *mut std::ffi::c_void,
    );

    if status != 0 {
        // Property not available for this process.
        return Ok(None);
    }

    if cf_string.is_null() {
        return Ok(None);
    }

    // Convert CFString to Rust String using core_foundation's TCFType trait.
    let cf_str = CFString::from_void(cf_string as *const std::ffi::c_void);
    let bundle = cf_str.to_string();

    if bundle.is_empty() {
        Ok(None)
    } else {
        Ok(Some(bundle))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn enumerate_audio_processes_returns_vec() {
        // Basic smoke test: enumerate should not crash.
        // On a running system, we expect at least the system process.
        let result = enumerate_audio_processes();
        assert!(result.is_ok(), "enumerate_audio_processes should not fail");
    }

    #[test]
    #[ignore = "requires a running macOS system with audio processes"]
    fn enumerate_audio_processes_includes_system() {
        let procs = enumerate_audio_processes().expect("enumerate should succeed");
        // On any graphical macOS session, there should be at least one process
        // with audio activity (kernel, Finder, or similar).
        assert!(!procs.is_empty(), "expected at least one audio process on a live system");
    }

    #[test]
    fn process_is_running_on_invalid_object() {
        // Invalid object ID should return false, not crash.
        let result = process_is_running(0xdeadbeef);
        assert!(!result, "invalid object should report as not running");
    }
}
