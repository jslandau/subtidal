//! RAII wrappers around Core Audio Process Taps, Aggregate Devices, and IOProcs.
//!
//! This module owns the lifecycle of:
//! 1. A process tap (`AudioObjectID` returned by `AudioHardwareCreateProcessTap`)
//! 2. An aggregate device wrapping the tap (`AudioObjectID` from `AudioHardwareCreateAggregateDevice`)
//! 3. An IOProc bound to the aggregate device (`AudioDeviceIOProcID` from `AudioDeviceCreateIOProcID`)
//!
//! The `AudioTap` type ensures correct teardown order in its `Drop` impl:
//! `AudioDeviceStop` → `AudioDeviceDestroyIOProcID` → `AudioHardwareDestroyAggregateDevice` → `AudioHardwareDestroyProcessTap`.
//!
//! The IOProc callback is RT-safe (no allocation, no blocking, try_lock only).
//!
//! Format: the tap's natural format is 48 kHz stereo f32 interleaved.
//! The downstream `audio/resampler.rs` handles 48 kHz stereo → 16 kHz mono.

use anyhow::{Context, Result};
use core_foundation::base::{FromVoid, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use coreaudio_sys::*;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{msg_send, ClassType};
use std::sync::{Arc, Mutex};

use crate::stt::AudioWake;
use ringbuf::traits::Producer;

/// Specifies which audio to capture.
#[derive(Debug, Clone)]
pub enum TapTarget {
    /// System-wide audio mix (all applications and system sounds).
    SystemMix,
    /// Audio from one or more specific processes. `object_ids` are Core
    /// Audio process AudioObjectIDs (NOT system PIDs — see the
    /// `initStereoMixdownOfProcesses:` contract). `watchdog_pids` are the
    /// corresponding system PIDs, used by the 1 Hz liveness watchdog to
    /// detect when ALL captured processes have exited.
    ///
    /// Multiple ids per target supports the common case of browsers, where
    /// audio is split across several WebKit content / GPU helper processes
    /// that all share one bundle id.
    Processes {
        object_ids: Vec<AudioObjectID>,
        watchdog_pids: Vec<std::ffi::c_int>,
    },
}

/// Owns a Core Audio process tap, its aggregate device, and its IOProc.
///
/// `Drop` tears all three down in the correct order:
/// 1. `AudioDeviceStop(aggregate_id, ioproc_id)`
/// 2. `AudioDeviceDestroyIOProcID(aggregate_id, ioproc_id)`
/// 3. `AudioHardwareDestroyAggregateDevice(aggregate_id)`
/// 4. `AudioHardwareDestroyProcessTap(tap_id)`
///
/// The `callback_context_ptr` is a raw pointer to a heap-allocated `CallbackContext`.
/// Safety: `AudioDeviceStop` is the relevant quiescence boundary — after it
/// returns, the IOProc is guaranteed not to fire again, so the
/// `CallbackContext` Box can be safely reclaimed before
/// `AudioDeviceDestroyIOProcID`. `DestroyIOProcID` only removes the
/// proc-id ↔ device binding; it does not affect callback safety.
/// On stop failure, the Box is intentionally leaked to avoid use-after-free from in-flight IOProc callbacks.
pub struct AudioTap {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    ioproc_id: AudioDeviceIOProcID,
    callback_context_ptr: *mut CallbackContext,
    /// PIDs the watchdog polls for liveness. Empty for SystemMix. When ALL
    /// of these PIDs are gone, the captured source is considered disappeared.
    captured_pids: Vec<std::ffi::c_int>,
}

/// Context passed to the RT-safe IOProc callback via `clientData`.
struct CallbackContext {
    producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    wake: Arc<AudioWake>,
}

impl AudioTap {
    /// Create a new tap targeting the specified source.
    ///
    /// # Arguments
    /// - `target`: which audio to capture (system mix or specific process)
    /// - `producer`: ring buffer producer (shared with the audio thread)
    /// - `wake`: audio wake notification (signaled on each IOProc callback)
    ///
    /// # Errors
    /// Returns Err if any Core Audio operation fails, typically due to:
    /// - Permission denied (Audio Capture TCC not granted)
    /// - Process not found (for `TapTarget::Process`)
    /// - Aggregate device creation conflict
    pub fn build(
        target: TapTarget,
        producer: Arc<Mutex<ringbuf::HeapProd<f32>>>,
        wake: Arc<AudioWake>,
    ) -> Result<Self> {
        let captured_pids: Vec<std::ffi::c_int> = match &target {
            TapTarget::SystemMix => vec![],
            TapTarget::Processes { watchdog_pids, .. } => watchdog_pids.clone(),
        };

        unsafe {
            // Step 1: Create a CATapDescription via Obj-C msg_send!.
            // No objc2 binding crate covers CATapDescription; use raw dispatch.
            // The tap_desc_owned must stay alive until after AudioHardwareCreateProcessTap returns.
            let tap_desc_owned =
                create_tap_description(&target).context("failed to create CATapDescription")?;

            // Step 2: Create the process tap.
            let mut tap_id: AudioObjectID = 0;
            let status = AudioHardwareCreateProcessTap(
                Retained::as_ptr(&tap_desc_owned) as *mut AnyObject,
                &mut tap_id,
            );
            // tap_desc_owned is still alive; it will drop after this block if an error occurs,
            // which is correct (the tap constructor doesn't retain the description).
            if status != 0 {
                anyhow::bail!(
                    "AudioHardwareCreateProcessTap failed: status={} (Audio Capture permission denied?)",
                    status
                );
            }

            // Step 3: Verify tap format early (right after tap creation, before aggregate device).
            // If verification fails, only the tap needs cleanup, not the aggregate device or IOProc.
            if let Err(e) = verify_tap_format(tap_id) {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
                return Err(e).context("tap format verification failed");
            }

            // Protect the tap_id from leaking on read_tap_uid or create_aggregate_device failures.
            // TapGuard is a scope guard that ensures AudioHardwareDestroyProcessTap is called
            // unless explicitly defused (which happens after the IOProc is successfully created).
            struct TapGuard {
                tap_id: AudioObjectID,
                defused: bool,
            }
            impl TapGuard {
                fn defuse(&mut self) {
                    self.defused = true;
                }
            }
            impl Drop for TapGuard {
                fn drop(&mut self) {
                    if !self.defused {
                        unsafe {
                            let _ = AudioHardwareDestroyProcessTap(self.tap_id);
                        }
                    }
                }
            }
            let mut tap_guard = TapGuard {
                tap_id,
                defused: false,
            };

            // Step 4: Read the tap's UID.
            let tap_uid = read_tap_uid(tap_guard.tap_id).context("failed to read tap UID")?;

            // Step 5: Create aggregate device.
            let aggregate_id =
                create_aggregate_device(&tap_uid).context("failed to create aggregate device")?;

            // Step 6: Create IOProc with a stable pointer to the callback context.
            // Use Box::into_raw to hand the Box's heap allocation to the IOProc,
            // then Box::from_raw in Drop to reclaim ownership when destroying the IOProc.
            let callback_context = Box::new(CallbackContext {
                producer: Arc::clone(&producer),
                wake: Arc::clone(&wake),
            });
            let context_ptr = Box::into_raw(callback_context) as *mut std::ffi::c_void;

            let mut ioproc_id: AudioDeviceIOProcID = None;
            let status = AudioDeviceCreateIOProcID(
                aggregate_id,
                Some(ioproc_fn),
                context_ptr,
                &mut ioproc_id,
            );
            if status != 0 {
                // Clean up post-tap resources; TapGuard (still armed) destroys the tap on unwind.
                let _ = Box::from_raw(context_ptr as *mut CallbackContext);
                let _ = AudioHardwareDestroyAggregateDevice(aggregate_id);
                anyhow::bail!("AudioDeviceCreateIOProcID failed: status={}", status);
            }

            // Step 7: Start the IOProc.
            let status = AudioDeviceStart(aggregate_id, ioproc_id);
            if status != 0 {
                // Clean up post-tap resources in reverse order; TapGuard destroys the tap on unwind.
                let _ = AudioDeviceDestroyIOProcID(aggregate_id, ioproc_id);
                let _ = Box::from_raw(context_ptr as *mut CallbackContext);
                let _ = AudioHardwareDestroyAggregateDevice(aggregate_id);
                anyhow::bail!("AudioDeviceStart failed: status={}", status);
            }

            // Defuse the guard — ownership of tap_id transfers to the AudioTap struct,
            // which will clean it up via Drop.
            tap_guard.defuse();

            Ok(AudioTap {
                tap_id: tap_guard.tap_id,
                aggregate_id,
                ioproc_id,
                callback_context_ptr: context_ptr as *mut CallbackContext,
                captured_pids,
            })
        }
    }

    /// PIDs the watchdog should poll. Empty for SystemMix; otherwise the PIDs
    /// of every process AudioObject included in the tap. The captured source
    /// is considered disappeared when ALL of these PIDs no longer exist.
    pub fn captured_pids(&self) -> &[std::ffi::c_int] {
        &self.captured_pids
    }
}

impl Drop for AudioTap {
    fn drop(&mut self) {
        unsafe {
            // Step 1: Stop the IOProc. If this fails, the IOProc may still be in flight
            // and could dereference the CallbackContext. In that case, we must leak the
            // context to avoid use-after-free.
            let stop_status = AudioDeviceStop(self.aggregate_id, self.ioproc_id);
            if stop_status != 0 {
                eprintln!(
                    "error: AudioDeviceStop failed (status {}); leaking CallbackContext to avoid use-after-free",
                    stop_status
                );
                // Intentionally leak the CallbackContext. The IOProc may still
                // be running with a pointer to it, so freeing would cause UB.
                std::mem::forget(Box::from_raw(self.callback_context_ptr));
                // Skip the rest of teardown — the aggregate device and tap may also be in-flight.
                return;
            }

            // Step 2: Reclaim and drop the CallbackContext now that IOProc is stopped.
            drop(Box::from_raw(self.callback_context_ptr));

            // Step 3: Destroy the IOProc.
            let status = AudioDeviceDestroyIOProcID(self.aggregate_id, self.ioproc_id);
            if status != 0 {
                eprintln!("warn: AudioDeviceDestroyIOProcID failed: status={}", status);
            }

            // Step 4: Destroy the aggregate device.
            let status = AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            if status != 0 {
                eprintln!(
                    "warn: AudioHardwareDestroyAggregateDevice failed: status={}",
                    status
                );
            }

            // Step 5: Destroy the tap.
            let status = AudioHardwareDestroyProcessTap(self.tap_id);
            if status != 0 {
                eprintln!(
                    "warn: AudioHardwareDestroyProcessTap failed: status={}",
                    status
                );
            }
        }
    }
}

/// RT-safe IOProc callback.
///
/// Called from a Core Audio thread with real-time constraints:
/// - No allocation
/// - No blocking locks (try_lock only)
/// - No logging
///
/// Reads input samples from the tap and pushes them into the ring buffer.
/// Signals the audio wake to notify the STT pipeline.
extern "C" fn ioproc_fn(
    _device: AudioObjectID,
    _now: *const AudioTimeStamp,
    input_data: *const AudioBufferList,
    _input_time: *const AudioTimeStamp,
    _output_data: *mut AudioBufferList,
    _output_time: *const AudioTimeStamp,
    client_data: *mut std::ffi::c_void,
) -> OSStatus {
    // SAFETY: client_data is guaranteed to point to a valid CallbackContext
    // as long as the AudioTap hasn't been dropped. The Drop impl ensures the
    // IOProc is stopped before the Box is dropped, so this pointer remains valid
    // for the lifetime of this callback.
    let ctx = unsafe { &*(client_data as *const CallbackContext) };

    unsafe {
        let buf_list = &*input_data;
        // The first (and only) buffer for a stereo mixdown tap.
        if buf_list.mNumberBuffers < 1 {
            return 0; // noErr
        }
        let buf = &buf_list.mBuffers[0];
        let n_frames = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
        let samples = std::slice::from_raw_parts(buf.mData as *const f32, n_frames);

        // Try to push samples into the ring buffer. If locked, skip this callback.
        // RT-safe: we use try_lock, not lock.
        if let Ok(mut prod) = ctx.producer.try_lock() {
            let _ = prod.push_slice(samples);
        }
    }

    // Notify the audio wake (atomic flag + condition variable; no allocation).
    ctx.wake.notify();

    0 // noErr
}

// FFI declarations for Core Audio Tap functions not in coreaudio-sys 0.2.17.
//
// These are declared in <CoreAudio/AudioHardwareTapping.h> (macOS 14.2+)
// but not exposed by the CoreAudio.h umbrella, so coreaudio-sys's bindgen-generated
// bindings don't include them. We declare them inline here; linking is automatic
// via CoreAudio.framework (already linked by coreaudio-sys).
extern "C" {
    // Create a process tap.
    //
    // Arguments:
    // - inDescription: pointer to a CATapDescription (Obj-C object)
    // - outTapID: pointer to AudioObjectID to receive the tap ID
    //
    // Returns:
    // noErr (0) on success; non-zero on error.
    fn AudioHardwareCreateProcessTap(
        inDescription: *mut AnyObject,
        outTapID: *mut AudioObjectID,
    ) -> OSStatus;

    // Destroy a process tap.
    //
    // Arguments:
    // - inTapID: the tap ID returned by AudioHardwareCreateProcessTap
    //
    // Returns:
    // noErr (0) on success; non-zero on error.
    fn AudioHardwareDestroyProcessTap(inTapID: AudioObjectID) -> OSStatus;
}

/// Verify that the tap delivers audio in the expected format.
///
/// Confirms:
/// - Sample rate is 48 kHz (resampler downstream expects this)
/// - Two channels (stereo)
/// - Float32 format
///
/// # Safety
/// `tap_id` must be a valid Core Audio object ID.
unsafe fn verify_tap_format(tap_id: AudioObjectID) -> Result<()> {
    let mut asbd = AudioStreamBasicDescription {
        mSampleRate: 0.0,
        mFormatID: 0,
        mFormatFlags: 0,
        mBytesPerPacket: 0,
        mFramesPerPacket: 0,
        mBytesPerFrame: 0,
        mChannelsPerFrame: 0,
        mBitsPerChannel: 0,
        mReserved: 0,
    };
    let mut size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;

    let status = AudioObjectGetPropertyData(
        tap_id,
        &AudioObjectPropertyAddress {
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        },
        0,
        std::ptr::null(),
        &mut size,
        &mut asbd as *mut _ as *mut std::ffi::c_void,
    );

    if status != 0 {
        anyhow::bail!(
            "kAudioTapPropertyFormat read failed: status={} (may not be a tap device)",
            status
        );
    }

    if (asbd.mSampleRate - 48000.0).abs() > 1.0 {
        anyhow::bail!(
            "tap delivers {} Hz; downstream resampler expects 48 kHz. \
             Tap format coercion not yet implemented.",
            asbd.mSampleRate
        );
    }

    if asbd.mChannelsPerFrame != 2 {
        anyhow::bail!(
            "tap delivers {} channels; downstream expects stereo",
            asbd.mChannelsPerFrame
        );
    }

    Ok(())
}

/// Create a CATapDescription via Obj-C msg_send!.
///
/// `CATapDescription` is an Obj-C class with purpose-built initializers:
/// - `initStereoGlobalTapButExcludeProcesses:` for system-wide mix
/// - `initStereoMixdownOfProcesses:` for per-process capture
///
/// Returns a `Retained<AnyObject>` to ensure proper lifetime management.
/// The caller must keep the `Retained` alive across the `AudioHardwareCreateProcessTap` call
/// (typically by binding it to a local variable at the call site).
///
/// # Safety
/// This function uses `objc2`'s `msg_send!` for raw Obj-C dispatch.
unsafe fn create_tap_description(target: &TapTarget) -> Result<objc2::rc::Retained<AnyObject>> {
    use objc2::rc::Retained;
    use objc2_foundation::{NSArray, NSNumber};

    // Use objc2 to look up the CATapDescription class.
    let tap_class = match AnyClass::get(c"CATapDescription") {
        Some(cls) => cls,
        None => anyhow::bail!("CATapDescription class not found (requires macOS 14.2+; check SDK)"),
    };

    // Allocate and initialize.
    let desc: Retained<AnyObject> = match target {
        TapTarget::SystemMix => {
            // Empty process list → system-wide mix.
            // Single init-family message capturing ownership in Retained.
            let allocated: *mut AnyObject = msg_send![tap_class, alloc];
            let empty_array: *mut AnyObject = msg_send![NSArray::<AnyObject>::class(), array];
            let desc: Option<Retained<AnyObject>> = Retained::from_raw(
                msg_send![allocated, initStereoGlobalTapButExcludeProcesses: empty_array],
            );
            desc.context("CATapDescription initStereoGlobalTapButExcludeProcesses: returned nil")?
        }
        TapTarget::Processes { ref object_ids, .. } => {
            // Per-app tap: NSArray of NSNumber<AudioObjectID> (NOT pids — Apple's
            // initStereoMixdownOfProcesses: docstring is explicit that it wants
            // Core Audio's process AudioObjectIDs, which differ from system PIDs).
            let allocated: *mut AnyObject = msg_send![tap_class, alloc];

            // Build NSArray of NSNumbers. NSNumber::new_u32 autoreleases.
            let number_objs: Vec<Retained<NSNumber>> = object_ids
                .iter()
                .map(|&oid| NSNumber::new_u32(oid))
                .collect();
            let ptr_refs: Vec<&NSNumber> = number_objs.iter().map(|n| &**n).collect();
            let arr: Retained<NSArray<NSNumber>> = NSArray::from_slice(&ptr_refs);

            let desc: Option<Retained<AnyObject>> =
                Retained::from_raw(msg_send![allocated, initStereoMixdownOfProcesses: &*arr]);
            desc.context("CATapDescription initStereoMixdownOfProcesses: returned nil")?
        }
    };

    Ok(desc)
}

/// Read the UID property from a tap.
///
/// # Safety
/// `tap_id` must be a valid Core Audio object ID.
unsafe fn read_tap_uid(tap_id: AudioObjectID) -> Result<String> {
    let mut cf_string: CFStringRef = std::ptr::null_mut();
    let mut size = std::mem::size_of::<CFStringRef>() as u32;

    let status = AudioObjectGetPropertyData(
        tap_id,
        &AudioObjectPropertyAddress {
            mSelector: kAudioTapPropertyUID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain,
        },
        0,
        std::ptr::null(),
        &mut size,
        &mut cf_string as *mut _ as *mut std::ffi::c_void,
    );

    if status != 0 {
        anyhow::bail!(
            "AudioObjectGetPropertyData (kAudioTapPropertyUID) failed: status={}",
            status
        );
    }

    if cf_string.is_null() {
        anyhow::bail!("kAudioTapPropertyUID returned null");
    }

    let cf_str = CFString::from_void(cf_string as *const std::ffi::c_void);
    Ok(cf_str.to_string())
}

/// Create an aggregate device containing the tap.
///
/// # Arguments
/// - `tap_uid`: the UID of the tap (from `read_tap_uid`)
///
/// # Returns
/// The AudioObjectID of the new aggregate device.
///
/// The aggregate device UID is constructed from a unique suffix (PID + counter)
/// to avoid collisions with other Subtidal instances.
unsafe fn create_aggregate_device(tap_uid: &str) -> Result<AudioObjectID> {
    use core_foundation::base::CFTypeRef;

    // The aggregate-device dictionary keys are well-known Core Audio strings.
    // coreaudio-sys exposes them as `b"..\0"` byte arrays (with trailing NUL);
    // wrap each in a CFString for use as a dictionary key. Using the SDK
    // constants (rather than hardcoded literals like "uid") guarantees we
    // track Apple's authoritative spelling.
    fn const_key(bytes: &'static [u8]) -> CFString {
        let s = std::str::from_utf8(&bytes[..bytes.len() - 1])
            .expect("Core Audio dict-key constants are ASCII");
        CFString::new(s)
    }

    // Generate a unique aggregate device UID.
    let pid = std::process::id();
    let aggregate_uid = format!("com.subtidal.tap.{}", pid);

    // Build the tap sub-dictionary: { kAudioSubTapUIDKey: tap_uid }
    let sub_uid_key = const_key(coreaudio_sys::kAudioSubTapUIDKey);
    let sub_uid_val = CFString::new(tap_uid);
    let tap_sub_dict =
        CFDictionary::from_CFType_pairs(&[(sub_uid_key.clone(), sub_uid_val.clone())]);

    // Build the tap list array containing the sub-dict.
    // Cast the dictionary to a generic CFDictionary with void pointers.
    let tap_list_val = core_foundation::array::CFArray::<
        CFDictionary<*const std::ffi::c_void, *const std::ffi::c_void>,
    >::from_CFTypes(&[
        // Cast tap_sub_dict to the expected type by going through CFTypeRef
        std::mem::transmute(tap_sub_dict),
    ]);

    // Build the aggregate device dictionary with all required keys.
    let uid_key = const_key(coreaudio_sys::kAudioAggregateDeviceUIDKey);
    let uid_val = CFString::new(&aggregate_uid);

    let name_key = const_key(coreaudio_sys::kAudioAggregateDeviceNameKey);
    let name_val = CFString::new("Subtidal Tap");

    let private_key = const_key(coreaudio_sys::kAudioAggregateDeviceIsPrivateKey);
    let private_val = CFNumber::from(1i32);

    let autostart_key = const_key(coreaudio_sys::kAudioAggregateDeviceTapAutoStartKey);
    let autostart_val = CFNumber::from(1i32);

    let tap_list_key = const_key(coreaudio_sys::kAudioAggregateDeviceTapListKey);

    // Create the top-level aggregate dictionary using CFTypeRef directly.
    let mut keys: Vec<CFTypeRef> = vec![];
    let mut vals: Vec<CFTypeRef> = vec![];

    keys.push(uid_key.as_CFTypeRef());
    vals.push(uid_val.as_CFTypeRef());

    keys.push(name_key.as_CFTypeRef());
    vals.push(name_val.as_CFTypeRef());

    keys.push(private_key.as_CFTypeRef());
    vals.push(private_val.as_CFTypeRef());

    keys.push(autostart_key.as_CFTypeRef());
    vals.push(autostart_val.as_CFTypeRef());

    keys.push(tap_list_key.as_CFTypeRef());
    vals.push(tap_list_val.as_CFTypeRef());

    // Create dictionary from raw CFTypeRefs using the FFI directly.
    let agg_dict = coreaudio_sys::CFDictionaryCreateMutable(
        std::ptr::null(),
        keys.len() as i64,
        std::ptr::null(),
        std::ptr::null(),
    );

    // Populate the dictionary.
    for (k, v) in keys.into_iter().zip(vals.into_iter()) {
        coreaudio_sys::CFDictionaryAddValue(agg_dict, k, v);
    }

    // Create the aggregate device.
    let mut aggregate_id: AudioObjectID = 0;
    let status = AudioHardwareCreateAggregateDevice(agg_dict, &mut aggregate_id);

    // Release the dictionary.
    coreaudio_sys::CFRelease(agg_dict as CFTypeRef);

    if status != 0 {
        anyhow::bail!(
            "AudioHardwareCreateAggregateDevice failed: status={}",
            status
        );
    }

    Ok(aggregate_id)
}

#[cfg(test)]
#[path = "./tap_test.rs"]
mod tests;
