//! Embeds the ort-sys provider library directory so the installed binary can find
//! CUDA execution provider .so files at runtime.

use std::path::PathBuf;

fn main() {
    // `cfg!(target_os = "linux")` here would check the HOST OS, not the target.
    // Read the TARGET env var (set by Cargo for build scripts) to detect the
    // actual compilation target so `cargo check --target x86_64-apple-darwin`
    // from a Linux host correctly skips CUDA-provider scanning.
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("linux") {
        return;
    }

    // ort-sys downloads ORT binaries into the cargo build cache under OUT_DIR.
    // The copy-dylibs feature symlinks provider .so files into target/{profile}/,
    // but `cargo install` only copies the final binary. We find the actual .so
    // directory from the build cache and embed it so the binary can locate providers
    // at runtime regardless of install location.
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let ort_lib_dir = find_ort_provider_dir(&out_dir);
    if let Some(dir) = ort_lib_dir {
        println!("cargo:rustc-env=ORT_PROVIDER_LIB_DIR={}", dir.display());
        // The final path component of ORT_PROVIDER_LIB_DIR is ort-sys's `dist.hash`
        // (content hash of the upstream prebuilt tarball for this target+features).
        // Embedding it lets the runtime cache scan match the *exact* distribution
        // this binary was linked against, rather than guessing by mtime.
        if let Some(hash) = dir.file_name().and_then(|s| s.to_str()) {
            println!("cargo:rustc-env=ORT_DIST_HASH={}", hash);
        }
    }
}

/// Walk up from OUT_DIR to find the ort-sys build output containing provider .so files.
fn find_ort_provider_dir(out_dir: &str) -> Option<PathBuf> {
    // The ORT cache lives at ~/.cache/ort.pyke.io/dfbin/{arch}/{hash}/
    // The ort-sys build script symlinks .so files from there into target/{profile}/.
    // We can find the canonical path by following symlinks in target/{profile}/.

    // Strategy: go from OUT_DIR up to target/{profile}/ and look for provider symlinks.
    let out_path = PathBuf::from(out_dir);
    // OUT_DIR is typically target/{profile}/build/{crate}-{hash}/out
    // We want target/{profile}/
    let profile_dir = out_path.ancestors().nth(3)?;
    let cuda_provider = profile_dir.join("libonnxruntime_providers_cuda.so");
    if cuda_provider.exists() {
        // Follow the symlink to get the actual ORT cache directory
        if let Ok(resolved) = std::fs::canonicalize(&cuda_provider) {
            return resolved.parent().map(|p| p.to_path_buf());
        }
    }

    // Fallback: scan the ort cache directory directly
    let cache_dir = dirs_for_build()?;
    scan_ort_cache(&cache_dir)
}

fn dirs_for_build() -> Option<PathBuf> {
    // ~/.cache/ort.pyke.io/dfbin/
    let home = std::env::var("HOME").ok()?;
    let cache = PathBuf::from(home).join(".cache/ort.pyke.io/dfbin");
    if cache.is_dir() {
        Some(cache)
    } else {
        None
    }
}

fn scan_ort_cache(cache_dir: &PathBuf) -> Option<PathBuf> {
    let target = std::env::var("TARGET").ok()?;
    let arch_dir = cache_dir.join(&target);
    if !arch_dir.is_dir() {
        return None;
    }
    // Find the most recently modified hash dir containing the CUDA provider
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    if let Ok(entries) = std::fs::read_dir(&arch_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let provider = path.join("libonnxruntime_providers_cuda.so");
            if provider.exists() {
                if let Ok(meta) = provider.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if best.as_ref().map_or(true, |(_, t)| modified > *t) {
                            best = Some((path, modified));
                        }
                    }
                }
            }
        }
    }
    best.map(|(p, _)| p)
}
