//! Pin provider discovery to the same prebuilt distribution as ort-sys rc14.
use std::path::PathBuf;

// From ort-sys 2.0.0-rc.14 build/download/dist.tsv (ORT 1.31.0).
// resolve.rs selects this superset for the Linux CUDA-only feature request.
// Update these identities together with the exact ort dependency pin.
pub fn distribution_hash(target: &str) -> Option<&'static str> {
    match target {
        "x86_64-unknown-linux-gnu" => {
            Some("535872087d5a5921aad090fdb46c65bb11c4c50b900dfcf6657ad3d5c465986f")
        }
        "aarch64-unknown-linux-gnu" => {
            Some("40c6f87b145b1fbe7df73f92c5ef3cbb886c6a45133ac1a3bd5177987ef72c75")
        }
        _ => None,
    }
}

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("apple-darwin") {
        // The static ORT CoreML objects reference Clang's availability runtime.
        // Rust links with -nodefaultlibs, so request that archive explicitly.
        let output = std::process::Command::new("clang")
            .arg("-print-file-name=libclang_rt.osx.a")
            .output()
            .expect("clang is required for macOS builds");
        assert!(output.status.success(), "failed to locate Clang runtime");
        let archive = String::from_utf8(output.stdout).expect("Clang runtime path is UTF-8");
        let archive = archive.trim();
        assert!(
            std::path::Path::new(archive).is_file(),
            "Clang runtime archive not found"
        );
        println!("cargo:rustc-link-arg={archive}");
        return;
    }
    if !target.contains("linux") {
        return;
    }
    // System/pkg-config builds cannot be assigned a prebuilt archive identity.
    // Fail closed rather than stage plugins from an unrelated cached runtime.
    for name in [
        "ORT_LIB_PATH",
        "ORT_LIB_LOCATION",
        "ORT_PREFER_DYNAMIC_LINK",
        "ORT_SKIP_DOWNLOAD",
        "ORT_OFFLINE",
        "ORT_CUDA_VERSION",
        "ORT_CACHE_DIR",
        "XDG_CACHE_HOME",
        "HOME",
        "ORT_USE_PKG_CONFIG",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    for name in [
        "ORT_LIB_PATH",
        "ORT_LIB_LOCATION",
        "ORT_PREFER_DYNAMIC_LINK",
        "ORT_USE_PKG_CONFIG",
    ] {
        assert!(std::env::var_os(name).is_none(), "{name} is unsupported: provider staging requires the pinned ort-sys rc14 prebuilt runtime");
    }
    if let Ok(version) = std::env::var("ORT_CUDA_VERSION") {
        assert!(
            version != "12",
            "ort-sys rc14 prebuilt providers require CUDA 13, not ORT_CUDA_VERSION=12"
        );
    }
    let hash =
        distribution_hash(&target).expect("no pinned rc14 CUDA distribution for this Linux target");
    // Match ort-sys src/internal/dirs.rs, including its XDG_CACHE_HOME semantics.
    let cache = std::env::var_os("ORT_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        })
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ort.pyke.io")));
    if let Some(cache) = cache {
        let dir = cache.join("dfbin").join(&target).join(hash);
        println!("cargo:rustc-env=ORT_PROVIDER_LIB_DIR={}", dir.display());
    }
    // Embed identity even when the cache does not yet exist: Cargo may run this
    // script before ort-sys downloads it, and cargo install moves only the exe.
    println!("cargo:rustc-env=ORT_DIST_HASH={hash}");
    println!("cargo:rustc-env=ORT_DIST_TARGET={target}");
    println!("cargo:rustc-env=ORT_DIST_VERSION=1.31.0");
    println!("cargo:rustc-env=ORT_DIST_FEATURES=cuda13,tensorrt,nvrtx");
}
