//! CMSampleBuffer → 48kHz stereo f32 format normalization (Phase 4 Task 3).
//!
//! Extracts PCM audio from `CMSampleBuffer`, validates format (48 kHz, stereo,
//! packed float32), and returns a borrowed slice for direct ring-buffer push.

use objc2_core_audio_types::{
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM,
};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMSampleBuffer};

/// Extract PCM audio from a CMSampleBuffer.
///
/// Returns `Some(&[f32])` on a matching 48 kHz / stereo / packed float32 buffer,
/// or `None` on any format mismatch (silently dropped per RT-safety contract).
///
/// # Safety
///
/// The returned slice borrows from the `CMSampleBuffer`'s underlying
/// `CMBlockBuffer`, which remains valid for the lifetime of `sb`.
pub fn extract_pcm(sb: &CMSampleBuffer) -> Option<&[f32]> {
    unsafe {
        let fd = sb.format_description()?;
        let asbd_ptr = CMAudioFormatDescriptionGetStreamBasicDescription(&fd);
        if asbd_ptr.is_null() {
            return None;
        }
        let asbd = &*asbd_ptr;

        if (asbd.mSampleRate as u32) != 48_000 {
            return None;
        }
        if asbd.mChannelsPerFrame != 2 {
            return None;
        }
        if asbd.mFormatID != kAudioFormatLinearPCM {
            return None;
        }
        let flags = asbd.mFormatFlags;
        if flags & kAudioFormatFlagIsFloat == 0
            || flags & kAudioFormatFlagIsPacked == 0
            || asbd.mBitsPerChannel != 32
        {
            return None;
        }

        let bb = sb.data_buffer()?;
        let mut total_len: usize = 0;
        let mut ptr: *mut std::os::raw::c_char = std::ptr::null_mut();
        let status =
            bb.data_pointer(0, std::ptr::null_mut(), &mut total_len, &mut ptr);
        if status != 0 || ptr.is_null() {
            return None;
        }
        let n_samples = total_len / std::mem::size_of::<f32>();
        Some(std::slice::from_raw_parts(ptr as *const f32, n_samples))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    // Full unit tests for extract_pcm require programmatic CMSampleBuffer
    // construction (CMSampleBufferCreate + AudioStreamBasicDescription +
    // CMBlockBuffer). Phase 4 defers this to operational verification in
    // Task 6 per the plan's "If constructing a CMSampleBuffer programmatically
    // is impractical" branch.
}
