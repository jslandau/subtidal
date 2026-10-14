//! CUDA probe verdict cache (F10).
//!
//! The Linux startup runs a CUDA probe subprocess that loads the full 600M
//! model — hundreds of milliseconds to seconds per launch. The verdict only
//! changes when the runtime environment changes, so it is cached in
//! `~/.cache/subtidal/cuda-probe.json` keyed on:
//!
//! - `ORT_DIST_HASH` (embedded by `build.rs`; encodes the ort distribution),
//! - size + mtime of **every** model file in the model directories (not the
//!   directory mtime — in-place file replacement must change the key),
//! - the cheap in-process `CUDA::default().is_available()` result.
//!
//! No parakeet-rs driver-version accessor exists; the ORT hash plus
//! failure-driven invalidation (see `invalidate`, wired into the
//! `build_engine` failure path in `stt`) is the concrete substitute.
//!
//! Pure types + file IO only — compiled on every target, unit-tested with a
//! fake probe closure and a `tempfile` cache path (no subprocess, no CUDA).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One model file's identity-in-time: size and mtime. In-place replacement
/// changes at least the mtime, which directory mtimes would miss.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelFileStat {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_nanos: u128,
}

/// Everything a cached CUDA verdict depends on.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeKey {
    pub ort_dist_hash: String,
    pub model_files: Vec<ModelFileStat>,
    pub cuda_available_inproc: bool,
}

impl ProbeKey {
    /// Stat every model file under the given base model directories
    /// (Nemotron + diarization files, via the `models` module's tables).
    /// Missing files stat as `(0, 0)` — stable until they appear.
    pub fn from_parts(ort_hash: &str, model_dirs: &[PathBuf], cuda_inproc: bool) -> Self {
        let mut files = Vec::new();
        for base in model_dirs {
            for path in crate::models::nemotron_model_files_in(base)
                .into_iter()
                .chain(std::iter::once(crate::models::diarization_model_file_in(
                    base,
                )))
            {
                let (size, mtime_nanos) = stat_file(&path);
                files.push(ModelFileStat {
                    path,
                    size,
                    mtime_nanos,
                });
            }
        }
        ProbeKey {
            ort_dist_hash: ort_hash.to_string(),
            model_files: files,
            cuda_available_inproc: cuda_inproc,
        }
    }
}

fn stat_file(path: &Path) -> (u64, u128) {
    match std::fs::metadata(path) {
        Ok(md) => {
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            (md.len(), mtime)
        }
        Err(_) => (0, 0),
    }
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    key: ProbeKey,
    verdict: bool,
}

/// Load the cached verdict for exactly this key. `None` on any mismatch,
/// missing file, or corrupt JSON (callers then probe and rewrite).
pub fn load_cached_verdict(cache_path: &Path, key: &ProbeKey) -> Option<bool> {
    let raw = std::fs::read_to_string(cache_path).ok()?;
    let entry: CacheEntry = serde_json::from_str(&raw).ok()?;
    (entry.key == *key).then_some(entry.verdict)
}

/// Persist the verdict under this key (best-effort; cache failures are never
/// fatal — the worst case is an extra probe on the next launch).
pub fn write_cached_verdict(cache_path: &Path, key: &ProbeKey, verdict: bool) {
    let Some(parent) = cache_path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let entry = CacheEntry {
        key: key.clone(),
        verdict,
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        let tmp = cache_path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, cache_path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// Delete the cache so the next launch re-probes. Called from the
/// `build_engine` failure path when CUDA was requested: a stale "available"
/// verdict is the likely cause of the failure.
pub fn invalidate(cache_path: &Path) {
    if let Err(e) = std::fs::remove_file(cache_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "warn: could not remove CUDA probe cache {}: {e}",
                cache_path.display()
            );
        }
    }
}

/// Invalidation policy seam: `None` (macOS, or probing disabled) is a no-op;
/// `Some(path)` deletes the cache file.
pub fn invalidate_if_configured(cache_path: Option<&Path>) {
    if let Some(path) = cache_path {
        invalidate(path);
    }
}

/// Probe-through-cache: return the cached verdict for this key when present,
/// otherwise run the (real: subprocess) probe, cache the result, and return
/// it. The probe is injected so tests never spawn processes or need CUDA.
pub fn cuda_available_cached(
    key: &ProbeKey,
    cache_path: &Path,
    probe: impl FnOnce() -> bool,
) -> bool {
    if let Some(verdict) = load_cached_verdict(cache_path, key) {
        return verdict;
    }
    let verdict = probe();
    write_cached_verdict(cache_path, key, verdict);
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_with_hash(hash: &str) -> ProbeKey {
        ProbeKey {
            ort_dist_hash: hash.to_string(),
            model_files: Vec::new(),
            cuda_available_inproc: true,
        }
    }

    #[test]
    fn probe_cache_hit_returns_cached_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cuda-probe.json");
        let key = key_with_hash("abc");
        write_cached_verdict(&cache, &key, true);

        // The probe closure must NOT run on a cache hit.
        let verdict = cuda_available_cached(&key, &cache, || {
            panic!("probe must not be invoked on a cache hit")
        });
        assert!(verdict);

        // A hit with a false verdict also round-trips.
        write_cached_verdict(&cache, &key, false);
        let verdict = cuda_available_cached(&key, &cache, || {
            panic!("probe must not be invoked on a cache hit")
        });
        assert!(!verdict);
    }

    #[test]
    fn probe_cache_invalidated_by_model_change() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        let model_file = models.join("nemotron").join("encoder.onnx");
        std::fs::create_dir_all(model_file.parent().unwrap()).unwrap();
        std::fs::write(&model_file, b"v1").unwrap();

        let cache = dir.path().join("cuda-probe.json");
        let key1 = ProbeKey::from_parts("h", &[models.clone()], true);
        write_cached_verdict(&cache, &key1, false);

        // In-place replacement: same path, different size (and mtime).
        std::fs::write(&model_file, b"v2-longer").unwrap();
        let key2 = ProbeKey::from_parts("h", &[models.clone()], true);
        assert_ne!(key1, key2, "in-place replacement must change the key");

        let mut probed = false;
        let verdict = cuda_available_cached(&key2, &cache, || {
            probed = true;
            true
        });
        assert!(probed, "changed key must re-probe");
        assert!(verdict);
        // And the new verdict is now cached.
        assert_eq!(load_cached_verdict(&cache, &key2), Some(true));
    }

    #[test]
    fn probe_cache_corrupt_falls_back_to_probe() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cuda-probe.json");
        std::fs::write(&cache, "not json{").unwrap();

        let key = key_with_hash("k");
        let mut probed = false;
        let verdict = cuda_available_cached(&key, &cache, || {
            probed = true;
            true
        });
        assert!(probed, "corrupt cache must fall back to the probe");
        assert!(verdict);
        assert_eq!(load_cached_verdict(&cache, &key), Some(true));
    }

    #[test]
    fn probe_cache_invalidates_only_when_configured() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cuda-probe.json");
        std::fs::write(&cache, "{}").unwrap();

        // None: no-op (must not touch any file, must not panic).
        invalidate_if_configured(None);

        // Some: the cache file is deleted.
        invalidate_if_configured(Some(&cache));
        assert!(!cache.exists(), "Some(path) must delete the cache file");
    }

    #[test]
    fn probe_key_covers_ort_hash_and_inproc_flag() {
        let a = key_with_hash("h1");
        let b = key_with_hash("h2");
        assert_ne!(a, b);
        let c = ProbeKey {
            cuda_available_inproc: false,
            ..key_with_hash("h1")
        };
        assert_ne!(a, c);
    }
}
