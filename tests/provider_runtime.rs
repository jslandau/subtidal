//! Pure distribution-identity and startup-staging regression guards.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../src/main_linux.rs"]
mod main_linux;

#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

#[cfg(target_os = "linux")]
mod staging_tests {
    use super::main_linux::stage_provider_pair;
    use std::fs;
    use tempfile::tempdir;

    const CUDA: &str = "libonnxruntime_providers_cuda.so";
    const SHARED: &str = "libonnxruntime_providers_shared.so";

    fn source_pair(dir: &std::path::Path) {
        fs::write(dir.join(CUDA), b"matching CUDA provider").unwrap();
        fs::write(dir.join(SHARED), b"matching shared provider").unwrap();
    }

    #[test]
    fn missing_source_fails_before_replacing_any_peer() {
        let source = tempdir().unwrap();
        let exe = tempdir().unwrap();
        fs::write(source.path().join(CUDA), b"only half the source pair").unwrap();
        let stale = exe.path().join(CUDA);
        fs::write(&stale, b"stale installed provider").unwrap();

        assert!(stage_provider_pair(source.path(), exe.path()).is_err());
        assert_eq!(fs::read(stale).unwrap(), b"stale installed provider");
        assert!(!exe.path().join(SHARED).exists());
    }

    #[test]
    fn stale_peer_is_replaced_with_matching_source_pair() {
        let source = tempdir().unwrap();
        let exe = tempdir().unwrap();
        source_pair(source.path());
        fs::write(exe.path().join(CUDA), b"stale provider").unwrap();
        fs::write(exe.path().join(SHARED), b"stale shared provider").unwrap();

        stage_provider_pair(source.path(), exe.path()).unwrap();
        assert_eq!(
            fs::canonicalize(exe.path().join(CUDA)).unwrap(),
            fs::canonicalize(source.path().join(CUDA)).unwrap()
        );
        assert_eq!(
            fs::canonicalize(exe.path().join(SHARED)).unwrap(),
            fs::canonicalize(source.path().join(SHARED)).unwrap()
        );
    }

    #[test]
    fn partial_pair_failure_is_returned_and_successful_first_stage_remains_valid() {
        let source = tempdir().unwrap();
        let exe = tempdir().unwrap();
        source_pair(source.path());
        // A directory cannot be replaced by the second provider symlink.
        fs::create_dir(exe.path().join(SHARED)).unwrap();

        assert!(stage_provider_pair(source.path(), exe.path()).is_err());
        assert_eq!(
            fs::canonicalize(exe.path().join(CUDA)).unwrap(),
            fs::canonicalize(source.path().join(CUDA)).unwrap()
        );
        assert!(exe.path().join(SHARED).is_dir());
    }

    #[test]
    fn complete_pair_is_staged_from_the_same_source() {
        let source = tempdir().unwrap();
        let exe = tempdir().unwrap();
        source_pair(source.path());

        stage_provider_pair(source.path(), exe.path()).unwrap();
        for name in [CUDA, SHARED] {
            assert_eq!(
                fs::canonicalize(exe.path().join(name)).unwrap(),
                fs::canonicalize(source.path().join(name)).unwrap()
            );
        }
    }
}

#[test]
fn rc14_cuda_distributions_are_pinned_for_supported_linux_targets() {
    assert_eq!(
        build_script::distribution_hash("x86_64-unknown-linux-gnu"),
        Some("535872087d5a5921aad090fdb46c65bb11c4c50b900dfcf6657ad3d5c465986f")
    );
    assert_eq!(
        build_script::distribution_hash("aarch64-unknown-linux-gnu"),
        Some("40c6f87b145b1fbe7df73f92c5ef3cbb886c6a45133ac1a3bd5177987ef72c75")
    );
    assert_eq!(
        build_script::distribution_hash("x86_64-unknown-linux-musl"),
        None
    );
    assert_eq!(
        build_script::distribution_hash("aarch64-apple-darwin"),
        None
    );
    let manifest = include_str!("../Cargo.toml");
    assert!(
        manifest.contains("ort = { version = \"=2.0.0-rc.14\", features = [\"cuda\"] }"),
        "review distribution mapping when changing ORT features/version"
    );
}

#[test]
fn provider_identity_does_not_depend_on_cache_mtime_or_sibling_libraries() {
    let build = include_str!("../build.rs");
    assert!(!build.contains("modified()"));
    assert!(!build.contains("read_dir("));
    assert!(!build.contains("ancestors()"));
    let linux = include_str!("../src/main_linux.rs");
    let discovery = linux
        .split("fn find_provider_dir()")
        .nth(1)
        .unwrap()
        .split("/// ORT's `GetRuntimePath`")
        .next()
        .unwrap();
    assert!(!discovery.contains("current_exe"));
    assert!(discovery.contains("complete_provider_dir"));
    assert!(linux.contains("root.join(\"dfbin\").join(target).join(hash)"));
    assert!(linux.contains("std::fs::rename(&tmp, &dst)"));
    assert!(linux.contains("std::env::set_var(\"__SUBTIDAL_PROVIDER_STAGING_FAILED\", \"1\")"));
    assert!(linux.contains("std::env::remove_var(\"__SUBTIDAL_PROVIDER_STAGING_FAILED\")"));
    let startup = linux.split("let use_cuda =").nth(1).unwrap();
    assert!(startup
        .split("ProbeKey::from_parts")
        .next()
        .unwrap()
        .contains("__SUBTIDAL_PROVIDER_STAGING_FAILED"));
    let probe = linux.split("pub fn run_cuda_probe").nth(1).unwrap();
    assert!(probe.contains("require_gpu_registration()"));
}
