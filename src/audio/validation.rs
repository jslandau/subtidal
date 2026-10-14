//! Platform-neutral validation helpers for the two real-time audio capture
//! callbacks (PipeWire process callback on Linux, Core Audio IOProc on macOS).
//!
//! This module is deliberately **not** cfg-gated: every helper takes plain
//! integers/pointers/byte slices so its unit tests compile and run on every
//! target, while the macOS/Linux call sites stay one-liners. Contract for all
//! RT-facing helpers: no allocation, no logging, no blocking, `try_lock` only;
//! invalid buffers are dropped silently (a lock-free counter is bumped where
//! noted so drop volume can be observed outside RT context).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use ringbuf::traits::Producer;

/// Lock-free count of PipeWire chunks rejected by `push_pw_chunk` (bad
/// offset/size/alignment metadata). Read from non-RT code via
/// [`pw_dropped_chunks`] — never logged from the RT callback itself.
static PW_DROPPED_CHUNKS: AtomicU32 = AtomicU32::new(0);

/// Core Audio `kAudioFormatLinearPCM` (`'lpcm'`) — ABI-stable since 10.0.
pub const K_AUDIO_FORMAT_LINEAR_PCM: u32 = 0x6C70_636D;
/// Core Audio `kAudioFormatFlagIsFloat` (1 << 0).
pub const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1 << 0;
/// Core Audio `kAudioFormatFlagIsBigEndian` (1 << 1).
pub const K_AUDIO_FORMAT_FLAG_IS_BIG_ENDIAN: u32 = 1 << 1;
/// Core Audio `kAudioFormatFlagIsSignedInteger` (1 << 2).
pub const K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER: u32 = 1 << 2;
/// Core Audio `kAudioFormatFlagIsNonInterleaved` (1 << 5).
pub const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;

/// How many PipeWire chunks have been dropped for invalid metadata.
pub fn pw_dropped_chunks() -> u32 {
    PW_DROPPED_CHUNKS.load(Ordering::Relaxed)
}

/// Extract an `f32` sample slice from a PipeWire data block, validating the
/// chunk metadata before any cast.
///
/// Returns `None` (instead of panicking or reading out of bounds) when:
/// - `offset + size` overflows or extends past `bytes.len()`,
/// - the byte count is not divisible by `size_of::<f32>()`,
/// - the slice start is not sufficiently aligned for `f32`.
pub fn extract_f32<'a>(bytes: &'a [u8], offset: usize, size: usize) -> Option<&'a [f32]> {
    let end = offset.checked_add(size)?;
    let range = bytes.get(offset..end)?;
    bytemuck::try_cast_slice(range).ok()
}

/// Callback-facing wrapper used by the PipeWire RT process closure: validate
/// the chunk metadata, then push the samples into the shared ring buffer.
///
/// Semantics match the pre-validation code exactly: the producer mutex is
/// taken with `try_lock` (skip the chunk when contended) and the return value
/// is `true` only when at least one sample was actually pushed — the caller
/// notifies the consumer wake on `true` only. Invalid metadata bumps the
/// lock-free drop counter and returns `false` silently.
///
/// RT contract: no allocation, no logging, no blocking.
pub fn push_pw_chunk(
    bytes: &[u8],
    offset: usize,
    size: usize,
    producer: &Arc<Mutex<ringbuf::HeapProd<f32>>>,
) -> bool {
    let Some(samples) = extract_f32(bytes, offset, size) else {
        PW_DROPPED_CHUNKS.fetch_add(1, Ordering::Relaxed);
        return false;
    };
    if let Ok(mut prod) = producer.try_lock() {
        let n = prod.push_slice(samples);
        return n > 0;
    }
    false
}

/// Build a validated `f32` sample view over a Core Audio IOProc buffer.
///
/// Rejects: null `ptr`, `byte_size == 0`, byte sizes not divisible by
/// `size_of::<f32>()`, and pointers not aligned to `align_of::<f32>()`.
/// On success the `f32` slice is produced via a byte view +
/// `bytemuck::try_cast_slice`; the `&[f32]` is never constructed directly
/// with `from_raw_parts` (an unaligned non-null pointer is UB there even
/// with a correct length).
///
/// # Safety
///
/// The caller guarantees `ptr` references `byte_size` readable bytes for
/// the lifetime `'a`. In Subtidal the Core Audio IOProc is the trusted
/// caller: Core Audio hands it a valid `AudioBufferList`. The checks below
/// validate layout, not memory validity — which is why this function is
/// `unsafe` despite checking everything it can.
pub unsafe fn audio_buffer_f32<'a>(
    ptr: *mut std::ffi::c_void,
    byte_size: usize,
) -> Option<&'a [f32]> {
    if ptr.is_null() || byte_size == 0 {
        return None;
    }
    if byte_size % std::mem::size_of::<f32>() != 0 {
        return None;
    }
    if (ptr as usize) % std::mem::align_of::<f32>() != 0 {
        return None;
    }
    // SAFETY: caller guarantee (see above) plus the four layout checks that
    // have already passed.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), byte_size) };
    bytemuck::try_cast_slice(bytes).ok()
}

/// Describe the first violated requirement of the tap format contract, or
/// `None` when the layout is exactly what the IOProc and downstream
/// resampler assume: 48 kHz stereo f32 interleaved linear PCM
/// (`mBitsPerChannel == 32`, `mBytesPerFrame == 8`, little-endian host
/// order, float flag set, signed-integer/big-endian/non-interleaved clear).
///
/// The IOProc reads a single interleaved buffer from `mBuffers[0]`; a
/// non-interleaved stream would hand it one channel's plane mis-framed as
/// interleaved stereo, so the flag must be clear.
///
/// Takes plain integers so it is unit-testable on any target.
pub fn tap_format_violation(
    rate: f64,
    format_id: u32,
    flags: u32,
    channels: u32,
    bits_per_channel: u32,
    bytes_per_frame: u32,
) -> Option<&'static str> {
    if !rate.is_finite() {
        return Some("mSampleRate is not finite");
    }
    if (rate - 48_000.0).abs() > 1.0 {
        return Some("mSampleRate is not 48 kHz (±1 Hz)");
    }
    if format_id != K_AUDIO_FORMAT_LINEAR_PCM {
        return Some("mFormatID is not kAudioFormatLinearPCM");
    }
    if flags & K_AUDIO_FORMAT_FLAG_IS_FLOAT == 0 {
        return Some("mFormatFlags does not set kAudioFormatFlagIsFloat");
    }
    if flags & K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER != 0 {
        return Some("mFormatFlags sets kAudioFormatFlagIsSignedInteger");
    }
    if flags & K_AUDIO_FORMAT_FLAG_IS_BIG_ENDIAN != 0 {
        return Some("mFormatFlags sets kAudioFormatFlagIsBigEndian");
    }
    if flags & K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED != 0 {
        return Some("mFormatFlags sets kAudioFormatFlagIsNonInterleaved");
    }
    if bits_per_channel != 32 {
        return Some("mBitsPerChannel is not 32");
    }
    if channels != 2 {
        return Some("mChannelsPerFrame is not 2");
    }
    if bytes_per_frame != 8 {
        return Some("mBytesPerFrame is not 8");
    }
    None
}

/// Boolean form of [`tap_format_violation`] — the tap format guard predicate.
pub fn tap_format_ok(
    rate: f64,
    format_id: u32,
    flags: u32,
    channels: u32,
    bits_per_channel: u32,
    bytes_per_frame: u32,
) -> bool {
    tap_format_violation(
        rate,
        format_id,
        flags,
        channels,
        bits_per_channel,
        bytes_per_frame,
    )
    .is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Consumer, Split};

    /// The exact accepted layout: 48 kHz / stereo / f32 / interleaved / LE.
    fn exact_layout() -> (f64, u32, u32, u32, u32, u32) {
        (
            48_000.0,
            K_AUDIO_FORMAT_LINEAR_PCM,
            K_AUDIO_FORMAT_FLAG_IS_FLOAT,
            2,
            32,
            8,
        )
    }

    fn sample_bytes() -> Vec<u8> {
        let samples: [f32; 4] = [1.0, -1.0, 0.5, -0.5];
        bytemuck::cast_slice(&samples).to_vec()
    }

    /// A buffer whose start is deliberately misaligned to 4 bytes: the
    /// malloc'd Vec data is at least 8-aligned, so offsetting by 1 yields a
    /// slice whose pointer is congruent to 1 mod 4.
    fn misaligned_bytes() -> Vec<u8> {
        let mut v = vec![0u8; 17];
        let first = v.as_ptr() as usize;
        assert_eq!(first % 4, 0, "test precondition: Vec data 4-aligned");
        v[0] = 0xEE; // marker byte at the misaligned start
        v
    }

    // ------------------------------------------------------------------
    // AC.2 (F2): PipeWire RT callback validation.
    // ------------------------------------------------------------------

    #[test]
    fn extract_f32_returns_valid_slice() {
        let bytes = sample_bytes();
        let out = extract_f32(&bytes, 0, bytes.len()).expect("valid slice");
        assert_eq!(out, &[1.0, -1.0, 0.5, -0.5]);
        // Nonzero offset within bounds is fine.
        let out = extract_f32(&bytes, 4, 8).expect("valid sub-slice");
        assert_eq!(out, &[-1.0, 0.5]);
    }

    #[test]
    fn extract_f32_rejects_out_of_range() {
        let bytes = sample_bytes();
        assert!(extract_f32(&bytes, 0, bytes.len() + 1).is_none());
        assert!(extract_f32(&bytes, 4, bytes.len()).is_none()); // 4 + 16 > 16
        assert!(extract_f32(&bytes, usize::MAX, 8).is_none()); // overflow
    }

    #[test]
    fn extract_f32_rejects_non_divisible_size() {
        let bytes = sample_bytes();
        assert!(extract_f32(&bytes, 0, 6).is_none()); // 6 % 4 != 0
        assert!(extract_f32(&bytes, 1, 4).is_none()); // misaligned by 1
    }

    #[test]
    fn extract_f32_rejects_unaligned() {
        let v = misaligned_bytes();
        let bytes = &v[1..17]; // 16 bytes, pointer = data + 1 → misaligned
        assert_eq!(bytes.as_ptr() as usize % 4, 1);
        assert!(extract_f32(bytes, 0, 16).is_none());
    }

    fn ring() -> (Arc<Mutex<ringbuf::HeapProd<f32>>>, ringbuf::HeapCons<f32>) {
        let (prod, cons) = ringbuf::HeapRb::<f32>::new(64).split();
        (Arc::new(Mutex::new(prod)), cons)
    }

    #[test]
    fn push_pw_chunk_skips_invalid_metadata() {
        let bytes = sample_bytes();
        let before = pw_dropped_chunks();

        // Out-of-range metadata.
        let (producer, _cons) = ring();
        assert!(!push_pw_chunk(&bytes, 0, bytes.len() + 1, &producer));
        // Non-divisible size.
        assert!(!push_pw_chunk(&bytes, 0, 6, &producer));
        // Misaligned offset.
        assert!(!push_pw_chunk(&bytes, 1, 4, &producer));
        // Zero size: no samples pushed, no panic.
        assert!(!push_pw_chunk(&bytes, 0, 0, &producer));

        // Drop counter advanced and no samples leaked into the ring.
        assert!(pw_dropped_chunks() >= before + 1);
        let (producer, mut cons) = ring();
        drop(producer);
        let mut out = [0.0f32; 8];
        assert_eq!(cons.pop_slice(&mut out), 0);
    }

    #[test]
    fn push_pw_chunk_pushes_valid_metadata() {
        // Companion to the AC.2 skip test: valid metadata pushes samples and
        // reports true (wake notification contract).
        let bytes = sample_bytes();
        let (producer, mut cons) = ring();
        assert!(push_pw_chunk(&bytes, 0, bytes.len(), &producer));
        let mut out = [0.0f32; 4];
        assert_eq!(cons.pop_slice(&mut out), 4);
        assert_eq!(out, [1.0, -1.0, 0.5, -0.5]);
    }

    // ------------------------------------------------------------------
    // AC.4 (F3): Core Audio IOProc buffer guards.
    // ------------------------------------------------------------------

    #[test]
    fn audio_buffer_f32_accepts_aligned_valid() {
        let mut samples: Vec<f32> = vec![0.25, -0.75, 1.5];
        let ptr = samples.as_mut_ptr() as *mut std::ffi::c_void;
        let byte_size = samples.len() * 4;
        let view = unsafe { audio_buffer_f32(ptr, byte_size) }.expect("aligned valid buffer");
        assert_eq!(view, &[0.25, -0.75, 1.5]);
    }

    #[test]
    fn audio_buffer_f32_rejects_null() {
        assert!(unsafe { audio_buffer_f32(std::ptr::null_mut(), 16) }.is_none());
    }

    #[test]
    fn audio_buffer_f32_rejects_zero_size() {
        let mut samples = vec![0.0f32; 4];
        let ptr = samples.as_mut_ptr() as *mut std::ffi::c_void;
        assert!(unsafe { audio_buffer_f32(ptr, 0) }.is_none());
    }

    #[test]
    fn audio_buffer_f32_rejects_misaligned() {
        let v = misaligned_bytes();
        let ptr = (v.as_ptr() as usize + 1) as *mut std::ffi::c_void;
        assert_eq!(ptr as usize % 4, 1);
        assert!(unsafe { audio_buffer_f32(ptr, 16) }.is_none());
    }

    #[test]
    fn audio_buffer_f32_rejects_non_divisible_size() {
        let mut samples = vec![0.0f32; 8];
        let ptr = samples.as_mut_ptr() as *mut std::ffi::c_void;
        assert!(unsafe { audio_buffer_f32(ptr, 18) }.is_none()); // 18 % 4 != 0
    }

    // ------------------------------------------------------------------
    // AC.3 (F4): tap format guard matrix.
    // ------------------------------------------------------------------

    #[test]
    fn tap_format_ok_accepts_exact_layout() {
        let (rate, id, flags, ch, bits, bpf) = exact_layout();
        assert!(tap_format_ok(rate, id, flags, ch, bits, bpf));
        // ±1 Hz tolerance around 48 kHz.
        assert!(tap_format_ok(48_000.9, id, flags, ch, bits, bpf));
        assert!(tap_format_violation(48_000.9, id, flags, ch, bits, bpf).is_none());
    }

    #[test]
    fn tap_format_ok_rejects_integer_pcm() {
        let (rate, id, _, ch, bits, bpf) = exact_layout();
        // Signed-integer PCM without the float flag.
        let flags = K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER;
        let v = tap_format_violation(rate, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("Float"), "should name the float flag, got: {v}");
        // Signed-integer AND float set simultaneously: still rejected.
        let flags = K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_SIGNED_INTEGER;
        let v = tap_format_violation(rate, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("SignedInteger"), "got: {v}");
    }

    #[test]
    fn tap_format_ok_rejects_non_float() {
        let (rate, id, _, ch, bits, bpf) = exact_layout();
        assert!(!tap_format_ok(rate, id, 0, ch, bits, bpf));
        // Non-PCM format id.
        assert!(!tap_format_ok(
            rate,
            0x6163_3332,
            K_AUDIO_FORMAT_FLAG_IS_FLOAT,
            ch,
            bits,
            bpf
        ));
    }

    #[test]
    fn tap_format_ok_rejects_wrong_bits() {
        let (rate, id, flags, ch, _, bpf) = exact_layout();
        assert!(!tap_format_ok(rate, id, flags, ch, 16, bpf));
        let v = tap_format_violation(rate, id, flags, ch, 16, bpf).unwrap();
        assert!(v.contains("BitsPerChannel"), "got: {v}");
    }

    #[test]
    fn tap_format_ok_rejects_mono() {
        let (rate, id, flags, _, bits, _) = exact_layout();
        assert!(!tap_format_ok(rate, id, flags, 1, bits, 4));
        let v = tap_format_violation(rate, id, flags, 1, bits, 4).unwrap();
        assert!(v.contains("ChannelsPerFrame"), "got: {v}");
    }

    #[test]
    fn tap_format_ok_rejects_non_48k() {
        let (_, id, flags, ch, bits, bpf) = exact_layout();
        assert!(!tap_format_ok(44_100.0, id, flags, ch, bits, bpf));
        assert!(!tap_format_ok(48_002.0, id, flags, ch, bits, bpf));
        let v = tap_format_violation(44_100.0, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("SampleRate"), "got: {v}");
    }

    #[test]
    fn tap_format_ok_rejects_non_finite_rate() {
        let (_, id, flags, ch, bits, bpf) = exact_layout();
        assert!(!tap_format_ok(f64::NAN, id, flags, ch, bits, bpf));
        assert!(!tap_format_ok(f64::INFINITY, id, flags, ch, bits, bpf));
        let v = tap_format_violation(f64::NAN, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("finite"), "got: {v}");
    }

    #[test]
    fn tap_format_ok_rejects_non_interleaved() {
        let (rate, id, _, ch, bits, bpf) = exact_layout();
        let flags = K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED;
        assert!(!tap_format_ok(rate, id, flags, ch, bits, bpf));
        let v = tap_format_violation(rate, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("NonInterleaved"), "got: {v}");
        // Big-endian is also rejected.
        let flags = K_AUDIO_FORMAT_FLAG_IS_FLOAT | K_AUDIO_FORMAT_FLAG_IS_BIG_ENDIAN;
        let v = tap_format_violation(rate, id, flags, ch, bits, bpf).unwrap();
        assert!(v.contains("BigEndian"), "got: {v}");
        // Wrong bytes-per-frame.
        let v = tap_format_violation(rate, id, K_AUDIO_FORMAT_FLAG_IS_FLOAT, ch, bits, 4).unwrap();
        assert!(v.contains("BytesPerFrame"), "got: {v}");
    }
}
