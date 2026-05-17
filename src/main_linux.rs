//! Linux-only startup helpers for the Subtidal binary.
//!
//! All `std::os::unix::*` and `libc` usage lives in this file. The binary's
//! `src/main.rs` orchestrates startup by importing these helpers via
//! `#[cfg(target_os = "linux")] mod main_linux;`.

use ort::ep::ExecutionProvider as _;
use ort::ep::CUDA;
use std::path::Path;

/// Ensure the CUDA provider .so files live next to the installed binary.
///
/// ORT's provider discovery uses dladdr on its own static code, which resolves to
/// the binary's directory (not CWD, not LD_LIBRARY_PATH). We symlink the provider
/// .so files from the ORT build cache into the exe dir so ORT's GetRuntimePath
/// finds them regardless of how subtidal was launched (CLI, app launcher, systemd).
pub fn ensure_provider_libs_next_to_exe() {
    let exe_dir = match std::env::current_exe().ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        Some(d) => d,
        None => return,
    };

    if exe_dir.join("libonnxruntime_providers_cuda.so").exists() {
        return;
    }

    let source_dir = match find_provider_dir() {
        Some(d) => d,
        None => return,
    };

    if source_dir == exe_dir {
        return;
    }

    for name in ["libonnxruntime_providers_cuda.so", "libonnxruntime_providers_shared.so"] {
        let src = source_dir.join(name);
        let dst = exe_dir.join(name);
        if !src.exists() || dst.exists() { continue; }
        let _ = std::os::unix::fs::symlink(&src, &dst);
    }
}

/// Locate this binary's ORT provider dir inside `~/.cache/ort.pyke.io/dfbin/`.
///
/// Keys on the exact `dist.hash` that `build.rs` embedded into the binary —
/// not mtime — so a stale sibling build of a different ORT version can't be
/// picked up and cause ABI mismatch. If the build-time hash is unavailable
/// (e.g. built without ORT_PROVIDER_LIB_DIR resolution), returns None and lets
/// the other layers of `find_provider_dir` handle it.
fn find_ort_cache_dir() -> Option<std::path::PathBuf> {
    let expected_hash = option_env!("ORT_DIST_HASH")?;
    let cache_dir = dirs::cache_dir()?.join("ort.pyke.io/dfbin");
    if !cache_dir.is_dir() {
        return None;
    }
    for arch_entry in std::fs::read_dir(&cache_dir).ok()?.flatten() {
        let candidate = arch_entry.path().join(expected_hash);
        if candidate.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(candidate);
        }
    }
    None
}

/// Check next to the binary (cargo run with symlinks in target/release/)
/// or use the provider directory embedded at compile time by build.rs.
fn find_provider_dir() -> Option<std::path::PathBuf> {
    // Check next to the binary (cargo run with symlinks in target/release/)
    if let Some(exe_dir) = std::env::current_exe().ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        if exe_dir.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(exe_dir);
        }
    }

    // Use the provider directory embedded at compile time by build.rs
    if let Some(dir) = option_env!("ORT_PROVIDER_LIB_DIR") {
        let p = std::path::PathBuf::from(dir);
        if p.join("libonnxruntime_providers_cuda.so").exists() {
            return Some(p);
        }
    }

    // Fallback: scan ort cache at runtime
    find_ort_cache_dir()
}

/// ORT's `GetRuntimePath` locates provider .so files relative to argv[0]'s parent
/// directory. When launched by bare name from a shell (e.g. `subtidal`), argv[0]
/// has no directory component and the runtime path becomes empty, so the CUDA
/// provider library is not found and CUDA EP registration silently falls back to
/// CPU. Re-exec ourselves with the absolute path from `current_exe()` so argv[0]
/// always has a directory component matching the real binary location.
pub fn reexec_with_absolute_argv0_if_needed() {
    use std::os::unix::process::CommandExt as _;

    if std::env::var_os("__SUBTIDAL_REEXECED").is_some() {
        return;
    }
    // Probe subprocess is already spawned with an absolute path by cuda_available().
    if std::env::var_os("__SUBTIDAL_CUDA_PROBE").is_some() {
        return;
    }
    let argv0 = match std::env::args_os().next() {
        Some(a) => a,
        None => return,
    };
    if std::path::Path::new(&argv0).parent().map(|p| !p.as_os_str().is_empty()).unwrap_or(false) {
        return; // already has a directory component
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let rest: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let err = std::process::Command::new(&exe)
        .args(&rest)
        .env("__SUBTIDAL_REEXECED", "1")
        .arg0(&exe)
        .exec();
    eprintln!("warn: failed to re-exec with absolute path ({err}); continuing. GPU acceleration may be unavailable.");
}

/// Detect CUDA usability by loading the model with CUDA in a subprocess.
/// See the comment in `run_cuda_probe` for why this is a subprocess.
pub fn cuda_available(model_dir: &Path) -> bool {
    use std::io::Read as _;
    use std::process::{Command, Stdio};

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let result = Command::new(exe)
        .env("__SUBTIDAL_CUDA_PROBE", "1")
        .env("__SUBTIDAL_CUDA_PROBE_MODEL_DIR", model_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            let mut output = String::new();
            if let Some(ref mut stdout) = child.stdout {
                let _ = stdout.read_to_string(&mut output);
            }
            let status = child.wait()?;
            Ok((status, output))
        });

    match result {
        Ok((status, output)) => status.success() && output.trim() == "cuda:ok",
        Err(_) => false,
    }
}

/// Called when `__SUBTIDAL_CUDA_PROBE` env var is set. Attempts to load the
/// Nemotron model with CUDA; prints "cuda:ok" on success, then `_exit`s so
/// the ORT/CUDA destructors don't run (they can hang or crash).
pub fn run_cuda_probe() -> ! {
    use std::io::Write as _;

    let available = CUDA::default().is_available().unwrap_or(false);
    if !available {
        unsafe { libc::_exit(0) };
    }

    if let Some(model_dir) = std::env::var_os("__SUBTIDAL_CUDA_PROBE_MODEL_DIR") {
        let config = parakeet_rs::ExecutionConfig::new()
            .with_execution_provider(parakeet_rs::ExecutionProvider::Cuda);
        let loaded = parakeet_rs::Nemotron::from_pretrained(
            std::path::Path::new(&model_dir),
            Some(config),
        );
        if loaded.is_err() {
            unsafe { libc::_exit(1) };
        }
        let _ = std::io::stdout().write_all(b"cuda:ok");
        let _ = std::io::stdout().flush();
        std::mem::forget(loaded);
        unsafe { libc::_exit(0) };
    }

    let _ = std::io::stdout().write_all(b"cuda:ok");
    let _ = std::io::stdout().flush();
    unsafe { libc::_exit(0) };
}

/// Returns the appropriate CUDA status message based on availability.
/// AC3.1 and AC3.2: Testable CUDA status logging.
pub fn cuda_status_message(cuda_available: bool) -> &'static str {
    if cuda_available {
        "info: CUDA available, Nemotron will use GPU acceleration"
    } else {
        "info: CUDA not available, Nemotron will use CPU"
    }
}

/// Skip all atexit handlers (both Rust and C++). ORT's C++ atexit destructors
/// call cudaFreeHost after the CUDA driver has already shut down, causing
/// SIGABRT. std::process::exit still runs atexit handlers, so we bypass
/// them via libc::_exit.
pub fn exit_without_atexit(code: i32) -> ! {
    unsafe { libc::_exit(code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuda_status_message_when_available() {
        let msg = cuda_status_message(true);
        assert!(msg.contains("GPU acceleration"));
        assert!(msg.contains("CUDA available"));
    }

    #[test]
    fn cuda_status_message_when_unavailable() {
        let msg = cuda_status_message(false);
        assert!(msg.contains("CPU"));
        assert!(msg.contains("CUDA not available"));
    }
}
