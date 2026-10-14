//! Model download and installation with supply-chain integrity.
//!
//! Policy (F5): every model file is pinned to an expected SHA-256 digest and
//! a fixed HuggingFace revision. Files are verified (a) after download and
//! (b) on first load of an already-present local copy; mismatches are
//! quarantined (renamed to `<name>.bad`) and the file is re-downloaded.
//! Installation is atomic and no-replace: content is staged to a temp file in
//! the destination directory and committed with `hard_link` (which fails on
//! an existing destination instead of silently replacing it, unlike
//! `rename(2)`).
//!
//! Updating a model is now a deliberate code change: bump the revision,
//! remote path, and digest together. This is the standard pinned-supply-chain
//! posture; the residual trust is in the repo owner at pin time.

use anyhow::{Context, Result};
use sha2::{Digest as ShaDigest, Sha256};
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Engine;

/// Returns the base directory for downloaded model files.
#[cfg(target_os = "linux")]
pub fn models_dir() -> PathBuf {
    // ~/.local/share/subtidal/models/ (XDG convention)
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("subtidal")
        .join("models")
}

#[cfg(target_os = "macos")]
pub fn models_dir() -> PathBuf {
    // ~/Library/Application Support/Subtidal/models/ (Apple convention)
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Subtidal")
        .join("models")
}

/// Returns the directory for Nemotron ONNX model files.
#[cfg(target_os = "linux")]
pub fn nemotron_model_dir() -> PathBuf {
    // ~/.local/share/subtidal/models/nemotron/ (XDG convention)
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("subtidal")
        .join("models")
        .join("nemotron")
}

#[cfg(target_os = "macos")]
pub fn nemotron_model_dir() -> PathBuf {
    // ~/Library/Application Support/Subtidal/models/nemotron/ (Apple convention)
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Subtidal")
        .join("models")
        .join("nemotron")
}

/// One downloadable model file: remote path within the repo, local file name,
/// and the pinned SHA-256 digest (hex, lowercase).
#[derive(Clone, Copy, Debug)]
pub struct ModelFile {
    pub remote: &'static str,
    pub local: &'static str,
    pub sha256: &'static str,
}

/// HuggingFace repo for the canonical Nemotron engine and the Sortformer
/// diarizer.
const REPO: &str = "altunenes/parakeet-rs";

/// Pinned HuggingFace revision for every model file we install. Pinning the
/// commit is load-bearing: the repo's `main` has already moved since these
/// bytes were captured (a `nemotron-3.5-asr-streaming-0.6b-onnx/` directory
/// with different content now exists there). Fetching from `main` would
/// yield bytes that fail digest verification. Moving to a newer model is a
/// deliberate operator decision: change this revision together with the
/// paths and digests below.
const PINNED_REVISION: &str = "12bca7934f692a845057af06c5861d5e99495318";

/// The four files of the Nemotron streaming RNNT model, with the digests of
/// the bytes this build is pinned to (captured 2026-09-21 from the locally
/// installed, running-verified copies, whose content-ids match the HF LFS
/// oids at `PINNED_REVISION`).
const NEMOTRON_FILES: &[ModelFile] = &[
    ModelFile {
        remote: "nemotron-speech-streaming-en-0.6b/encoder.onnx",
        local: "encoder.onnx",
        sha256: "4c3acc1fe273b710b818012919a53ac0d535b5cbb614eb65e240ddd9480df27b",
    },
    ModelFile {
        remote: "nemotron-speech-streaming-en-0.6b/encoder.onnx.data",
        local: "encoder.onnx.data",
        sha256: "2c43a65deb934ad505bf3a5c9f2e47bc59ed43daf8a806c64597e2030fcc3278",
    },
    ModelFile {
        remote: "nemotron-speech-streaming-en-0.6b/decoder_joint.onnx",
        local: "decoder_joint.onnx",
        sha256: "3111f21cc147631e6038422239d974547c80915b8a7e5139a67fa5bed63d4a31",
    },
    ModelFile {
        remote: "nemotron-speech-streaming-en-0.6b/tokenizer.model",
        local: "tokenizer.model",
        sha256: "07d4e5a63840a53ab2d4d106d2874768143fb3fbdd47938b3910d2da05bfb0a9",
    },
];

/// The Sortformer v2.1 streaming diarization model file.
const SORTFORMER_FILE: ModelFile = ModelFile {
    remote: "diar_streaming_sortformer_4spk-v2.1.onnx",
    local: "diar_streaming_sortformer_4spk-v2.1.onnx",
    sha256: "f010a62654fd6d2adb70166265df2e53f5a766e354d2cce152b4c7966a04d02d",
};

#[derive(Clone, Debug)]
pub struct ModelPaths {
    pub nemotron_dir: PathBuf,
}

impl ModelPaths {
    pub fn from_conventional_dirs() -> Self {
        Self {
            nemotron_dir: nemotron_model_dir(),
        }
    }

    pub fn for_engine(&self, engine: &Engine) -> &Path {
        match engine {
            Engine::Nemotron => &self.nemotron_dir,
        }
    }
}

/// Returns the local paths for every required Nemotron model file inside the
/// given base models directory (i.e. the parent of the `nemotron/` subdir).
pub fn nemotron_model_files_in(base_dir: &Path) -> Vec<PathBuf> {
    let model_dir = base_dir.join("nemotron");
    NEMOTRON_FILES
        .iter()
        .map(|f| model_dir.join(f.local))
        .collect()
}

/// Returns the local path for the Sortformer model file inside the given base directory.
pub fn diarization_model_file_in(base_dir: &Path) -> PathBuf {
    base_dir.join("diarization").join(SORTFORMER_FILE.local)
}

/// Returns the directory for Sortformer diarization model files.
#[cfg(target_os = "linux")]
pub fn diarization_model_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("subtidal")
        .join("models")
        .join("diarization")
}

#[cfg(target_os = "macos")]
pub fn diarization_model_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Subtidal")
        .join("models")
        .join("diarization")
}

// ---------------------------------------------------------------------
// Digest verification
// ---------------------------------------------------------------------

/// Lowercase hex encoding without pulling a hex crate.
fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Stream a file through SHA-256 and compare against the expected hex digest
/// (case-insensitive). Streams in 1 MiB chunks so multi-GB model files don't
/// need to fit in memory.
pub fn verify_digest(path: &Path, expected_hex: &str) -> Result<bool> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("opening {} for digest verification", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {} for digest verification", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_encode(&hasher.finalize());
    Ok(actual.eq_ignore_ascii_case(expected_hex))
}

/// True when `path` matches the expected digest; I/O errors count as a
/// mismatch (used as a closure in the installer's EEXIST path).
fn digest_matches(path: &Path, expected_hex: &str) -> bool {
    verify_digest(path, expected_hex).unwrap_or(false)
}

// ---------------------------------------------------------------------
// Quarantine and stale-temp sweep
// ---------------------------------------------------------------------

/// Quarantine a bad file: rename `<name>` to `<name>.bad` (replacing any
/// previous quarantine). Keeps the evidence on disk for inspection.
fn quarantine(path: &Path) -> Result<()> {
    let bad = quarantine_path(path);
    if bad.exists() {
        std::fs::remove_file(&bad)
            .with_context(|| format!("removing previous quarantine {}", bad.display()))?;
    }
    std::fs::rename(path, &bad)
        .with_context(|| format!("quarantining {} to {}", path.display(), bad.display()))?;
    eprintln!(
        "warn: quarantined {} -> {} (digest mismatch)",
        path.display(),
        bad.display()
    );
    Ok(())
}

fn quarantine_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_owned()).unwrap_or_default();
    name.push(".bad");
    path.with_file_name(name)
}

/// True when `candidate` is a staging temp file for `dest`
/// (i.e. `<dest name>.tmp.*` in the same directory).
fn is_temp_for(dest_file_name: &std::ffi::OsStr, candidate: &Path) -> bool {
    let Some(cand) = candidate.file_name() else {
        return false;
    };
    let mut prefix = dest_file_name.to_owned();
    prefix.push(".tmp.");
    cand.as_encoded_bytes()
        .starts_with(prefix.as_encoded_bytes())
}

/// Remove `<name>.tmp.*` leftovers in the destination directory from crashed
/// installs, so a torn run can never leave junk that looks like progress.
fn sweep_stale_temps(dest: &Path) {
    let Some(dir) = dest.parent() else {
        return;
    };
    let Some(name) = dest.file_name() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_temp_for(name, &path) {
            match std::fs::remove_file(&path) {
                Ok(()) => eprintln!("info: swept stale temp {}", path.display()),
                Err(e) => eprintln!("warn: could not remove stale temp {}: {e}", path.display()),
            }
        }
    }
}

// ---------------------------------------------------------------------
// No-replace atomic install
// ---------------------------------------------------------------------

/// Outcome of committing a model file into place. The three-variant shape
/// makes "accidentally treated as installed" a type error: a mismatched
/// existing destination can only surface as `ExistingReplaced` (after
/// quarantine) or an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Staged temp committed as a new destination.
    Installed,
    /// Destination already existed and passed verification; temp discarded.
    ExistingVerified,
    /// Destination existed but failed verification; it was quarantined and
    /// the staged temp committed in its place (single retry).
    ExistingReplaced,
}

/// Process-global counter for temp-file names so concurrent installs never
/// share a staging path.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// RAII cleanup for the staged temp file: guarantees "every failure path
/// unlinks the temp" without repeating cleanup at each error return.
struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("warn: could not remove temp {}: {e}", self.0.display());
            }
        }
    }
}

/// Streaming byte-for-byte equality of two files (1 MiB chunks, early exit on
/// first difference). The default verifier for the digest-less
/// [`install_model`] entry point.
fn files_equal(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let mut ba = vec![0u8; 1024 * 1024];
    let mut bb = vec![0u8; 1024 * 1024];
    loop {
        match (read_full(&mut fa, &mut ba), read_full(&mut fb, &mut bb)) {
            (Ok(na), Ok(nb)) => {
                if na != nb || ba[..na] != bb[..nb] {
                    return false;
                }
                if na == 0 {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}

/// Production commit: `hard_link` fails with `EEXIST` when the destination
/// exists — it never silently replaces (unlike `rename(2)`). The temp file
/// lives in the destination directory, so same-filesystem linking holds.
fn hard_link_commit(tmp: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::hard_link(tmp, dest)
}

/// Atomic, no-replace installation of `src` as `dest`.
///
/// In order:
/// 1. `canonicalize(src)` — resolves hf-hub's symlinks into the blob store
///    before any copy/link (a hardlink to a symlink would carry a relative
///    target that doesn't resolve from the models dir).
/// 2. Stage content to `dest.tmp.<pid>.<counter>` in the destination
///    directory. Prefers hardlinking the resolved source (hf-hub blobs are
///    immutable and content-addressed; today's zero-copy fast path), falling
///    back to a full copy + `fsync` when the link is impossible (cross-fs).
/// 3. Commit with `hard_link(tmp, dest)` and unlink the temp.
/// 4. On `EEXIST`: verify the existing destination — match discards the temp
///    (`ExistingVerified`); mismatch quarantines the destination and retries
///    the commit exactly once (`ExistingReplaced`). Success is never returned
///    for a mismatched destination.
///
/// Every failure path unlinks the temp file.
///
/// The EEXIST digest check and the commit are injectable for tests via
/// [`install_model_with_hooks`].
pub fn install_model(src: &Path, dest: &Path) -> Result<InstallOutcome> {
    install_model_with_hooks(src, dest, &|p| files_equal(p, src), &hard_link_commit)
}

/// Hooked form of [`install_model`]: `verifier(existing_dest) -> bool` decides
/// the EEXIST path; `committer(tmp, dest)` performs the atomic commit.
pub fn install_model_with_hooks(
    src: &Path,
    dest: &Path,
    verifier: &dyn Fn(&Path) -> bool,
    committer: &dyn Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<InstallOutcome> {
    let real_src = std::fs::canonicalize(src)
        .with_context(|| format!("resolving symlink {}", src.display()))?;

    let dest_dir = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dest_dir.join(format!(
        "{}.tmp.{}.{}",
        dest.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        counter
    ));

    // Install the cleanup guard before staging so copy/open/fsync failures also
    // remove a partially-created temp file.
    let _tmp_guard = TempGuard(tmp.clone());

    // Stage: prefer a zero-copy hardlink of the resolved source (hf-hub blobs
    // are immutable); fall back to copy + fsync.
    if std::fs::hard_link(&real_src, &tmp).is_err() {
        std::fs::copy(&real_src, &tmp)
            .with_context(|| format!("staging {} to {}", real_src.display(), tmp.display()))?;
        let f = std::fs::File::open(&tmp)
            .with_context(|| format!("opening staged temp {} for fsync", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsyncing staged temp {}", tmp.display()))?;
    }

    // Commit (no-replace).
    match committer(&tmp, dest) {
        Ok(()) => Ok(InstallOutcome::Installed),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if verifier(dest) {
                // Existing destination is good; drop our staging copy.
                eprintln!(
                    "info: {} already present and verified; discarded staged temp",
                    dest.display()
                );
                return Ok(InstallOutcome::ExistingVerified);
            }
            eprintln!(
                "warn: {} exists but failed verification; quarantining and replacing",
                dest.display()
            );
            quarantine(dest)?;
            // Exactly one retry: a second EEXIST means someone else is
            // installing concurrently — surface it instead of looping.
            committer(&tmp, dest).map_err(|e| {
                anyhow::Error::new(e).context(format!(
                    "committing {} after quarantine of {}",
                    tmp.display(),
                    dest.display()
                ))
            })?;
            Ok(InstallOutcome::ExistingReplaced)
        }
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "committing {} to {}",
            tmp.display(),
            dest.display()
        ))),
    }
    // TempGuard drops here: the temp file is unlinked on every path.
}

// ---------------------------------------------------------------------
// Download source seam
// ---------------------------------------------------------------------

/// Seam over the model fetch path so the orchestration policy
/// (`ensure_one_model`) is unit-testable without network access. Production
/// uses [`HfHubSource`]; tests serve fixture files.
pub trait ModelSource: Sync {
    fn get<'a>(
        &'a self,
        repo: &'a str,
        file: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>>;
}

/// Production source: hf-hub async API against the pinned revision.
pub struct HfHubSource;

impl ModelSource for HfHubSource {
    fn get<'a>(
        &'a self,
        repo: &'a str,
        file: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>> {
        Box::pin(async move {
            let api = hf_hub::api::tokio::Api::new().context("initializing HuggingFace API")?;
            let pinned = hf_hub::Repo::with_revision(
                repo.to_string(),
                hf_hub::RepoType::Model,
                PINNED_REVISION.to_string(),
            );
            let repo_api = api.repo(pinned);
            repo_api
                .get(file)
                .await
                .with_context(|| format!("downloading {file} from {repo}@{PINNED_REVISION}"))
        })
    }
}

// ---------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------

/// Full digest-pinned policy for one model file:
/// 1. Sweep stale `dest.tmp.*` leftovers from crashed runs.
/// 2. Verify an already-present `dest`; on success return `ExistingVerified`.
/// 3. On mismatch, quarantine (`<name>.bad`) and re-download via the seam.
/// 4. Verify the fetched file, then hand off to the no-replace installer
///    (whose EEXIST path re-verifies against the same digest).
pub async fn ensure_one_model(
    source: &dyn ModelSource,
    repo: &str,
    remote: &str,
    dest: &Path,
    expected_digest: &str,
) -> Result<InstallOutcome> {
    sweep_stale_temps(dest);

    if dest.exists() {
        if digest_matches(dest, expected_digest) {
            eprintln!("info: {} present and digest-verified", dest.display());
            return Ok(InstallOutcome::ExistingVerified);
        }
        quarantine(dest)?;
    }

    let fetched = source
        .get(repo, remote)
        .await
        .with_context(|| format!("fetching {remote}"))?;
    if !digest_matches(&fetched, expected_digest) {
        anyhow::bail!(
            "downloaded {} does not match the pinned digest; refusing to install \
             (repo content changed? update the pin in src/models/mod.rs)",
            fetched.display()
        );
    }

    install_model_with_hooks(
        &fetched,
        dest,
        &|p| digest_matches(p, expected_digest),
        &hard_link_commit,
    )
}

/// Ensure every Nemotron model file is present and digest-verified, fetching
/// the four files concurrently (`tokio::join!`) with per-file error context.
pub async fn ensure_nemotron_models() -> Result<()> {
    let source = HfHubSource;
    let dest_dir = nemotron_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    // Fixed four-file set: join! gives real concurrency without buffering.
    let [a, b, c, d] = NEMOTRON_FILES else {
        unreachable!("NEMOTRON_FILES is a fixed four-file table")
    };
    let (dest_a, dest_b, dest_c, dest_d) = (
        dest_dir.join(a.local),
        dest_dir.join(b.local),
        dest_dir.join(c.local),
        dest_dir.join(d.local),
    );
    let (ra, rb, rc, rd) = tokio::join!(
        ensure_one_model(&source, REPO, a.remote, &dest_a, a.sha256),
        ensure_one_model(&source, REPO, b.remote, &dest_b, b.sha256),
        ensure_one_model(&source, REPO, c.remote, &dest_c, c.sha256),
        ensure_one_model(&source, REPO, d.remote, &dest_d, d.sha256),
    );
    let results = [(a.local, ra), (b.local, rb), (c.local, rc), (d.local, rd)];
    let mut failed = false;
    for (name, r) in results {
        if let Err(e) = r {
            eprintln!("error: nemotron model file `{name}`: {e:#}");
            failed = true;
        }
    }
    if failed {
        anyhow::bail!("one or more Nemotron model files failed verification/installation");
    }
    Ok(())
}

/// Ensure the Sortformer diarization model is present and digest-verified.
/// Failure policy (non-fatal) is decided by the caller.
pub async fn ensure_diarization_models() -> Result<()> {
    let source = HfHubSource;
    let dest_dir = diarization_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let dest = dest_dir.join(SORTFORMER_FILE.local);
    ensure_one_model(
        &source,
        REPO,
        SORTFORMER_FILE.remote,
        &dest,
        SORTFORMER_FILE.sha256,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn test_models_dir_is_valid_path() {
        let dir = models_dir();
        assert!(dir.components().count() > 0);
        assert!(dir.as_os_str().len() > 0);
    }

    #[test]
    fn test_nemotron_model_dir_contains_models_dir() {
        let nemotron_dir = nemotron_model_dir();
        let models_base = models_dir();
        assert!(nemotron_dir.starts_with(&models_base));
    }

    #[test]
    fn nemotron_model_files_match_required_set() {
        let files = nemotron_model_files_in(Path::new("/base"));
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "encoder.onnx",
                "encoder.onnx.data",
                "decoder_joint.onnx",
                "tokenizer.model"
            ]
        );
        for p in &files {
            assert!(p.starts_with("/base/nemotron"));
        }
    }

    #[test]
    fn every_pinned_file_table_entry_has_a_digest() {
        for f in NEMOTRON_FILES
            .iter()
            .chain(std::iter::once(&SORTFORMER_FILE))
        {
            assert_eq!(f.sha256.len(), 64, "{}: digest must be sha256 hex", f.local);
            assert!(
                f.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "{}: digest must be lowercase hex",
                f.local
            );
            assert!(!f.remote.is_empty() && !f.local.is_empty());
        }
        // All files share one pinned revision.
        assert_eq!(PINNED_REVISION.len(), 40, "revision must be a git sha1 hex");
    }

    // ------------------------------------------------------------------
    // Verifier (AC.5)
    // ------------------------------------------------------------------

    const FIXTURE: &[u8] = b"hello world";
    const FIXTURE_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    #[test]
    fn verify_digest_accepts_good_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.bin");
        std::fs::write(&p, FIXTURE).unwrap();
        assert!(verify_digest(&p, FIXTURE_SHA256).unwrap());
        // Case-insensitive hex.
        assert!(verify_digest(&p, &FIXTURE_SHA256.to_uppercase()).unwrap());
    }

    #[test]
    fn verify_digest_rejects_wrong_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("m.bin");
        std::fs::write(&p, b"hello worlt").unwrap();
        assert!(!verify_digest(&p, FIXTURE_SHA256).unwrap());
        // Larger-than-buffer file whose digest still mismatches (exercises
        // the streaming loop across chunk boundaries).
        let big = vec![7u8; 3 * 1024 * 1024];
        std::fs::write(&p, &big).unwrap();
        assert!(!verify_digest(&p, FIXTURE_SHA256).unwrap());
        // Missing file is an error, not `false` — callers must distinguish.
        assert!(verify_digest(&dir.path().join("absent"), FIXTURE_SHA256).is_err());
    }

    // ------------------------------------------------------------------
    // Installer (AC.5, sync `install_model` with hooks)
    // ------------------------------------------------------------------

    #[cfg(unix)]
    fn inode_of(p: &Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(p).unwrap().ino()
    }

    #[test]
    fn install_canonicalizes_symlinked_source() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("blob.real");
        std::fs::write(&real, FIXTURE).unwrap();
        let link = dir.path().join("snapshot-link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let dest_dir = dir.path().join("dest");
        let dest = dest_dir.join("model.onnx");
        let out = install_model(&link, &dest).unwrap();
        assert_eq!(out, InstallOutcome::Installed);
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
        // The committed destination aliases the resolved blob, not the symlink.
        #[cfg(unix)]
        assert_eq!(inode_of(&dest), inode_of(&real));
    }

    #[test]
    fn install_does_not_overwrite_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, FIXTURE).unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(&dest, FIXTURE).unwrap();
        let inode_before = {
            #[cfg(unix)]
            {
                inode_of(&dest)
            }
            #[cfg(not(unix))]
            {
                ()
            }
        };

        let out = install_model(&src, &dest).unwrap();
        assert_eq!(out, InstallOutcome::ExistingVerified);
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
        #[cfg(unix)]
        assert_eq!(inode_of(&dest), inode_before, "inode must be unchanged");
        assert_no_temps(dir.path());
    }

    #[test]
    fn install_eexist_mismatch_quarantines_and_installs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, FIXTURE).unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(&dest, b"tampered content").unwrap();

        // Forced-false verifier stands in for a digest mismatch; the real
        // committer produces a genuine EEXIST against the existing dest.
        let out = install_model_with_hooks(&src, &dest, &|_| false, &hard_link_commit).unwrap();
        assert_eq!(out, InstallOutcome::ExistingReplaced);
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
        assert_eq!(
            std::fs::read(quarantine_path(&dest)).unwrap(),
            b"tampered content"
        );
        assert_no_temps(dir.path());
    }

    #[test]
    fn install_failure_cleans_up_temp() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.bin");
        std::fs::write(&src, FIXTURE).unwrap();
        let dest = dir.path().join("model.onnx");

        let always_fail = |_: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::other("injected commit failure"))
        };
        let err = install_model_with_hooks(&src, &dest, &|_| true, &always_fail).unwrap_err();
        assert!(
            format!("{err:#}").contains("injected commit failure"),
            "full error chain should carry the injected failure: {err:#}"
        );
        // No trusted destination ...
        assert!(!dest.exists());
        assert!(!quarantine_path(&dest).exists());
        // ... and no leftover temp.
        assert_no_temps(dir.path());

        // EEXIST path with a failing retry also cleans up.
        std::fs::write(&dest, b"existing").unwrap();
        let eexist_then_fail = |_: &Path, _: &Path| -> std::io::Result<()> {
            Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
        };
        assert!(install_model_with_hooks(&src, &dest, &|_| false, &eexist_then_fail).is_err());
        assert_no_temps(dir.path());
    }

    fn assert_no_temps(dir: &Path) {
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.as_encoded_bytes().windows(5).any(|w| w == b".tmp."))
            })
            .collect();
        assert!(leftovers.is_empty(), "leftover temps: {leftovers:?}");
    }

    // ------------------------------------------------------------------
    // Orchestrator (AC.5, async `ensure_one_model` + fake ModelSource)
    // ---------------------------------------------------------------------

    /// Serves `bad` until a quarantine marker exists next to the dest (or
    /// from the start, when `start_good`), then serves `good` — exercising
    /// the re-download that follows a quarantine.
    struct FakeSource {
        bad: PathBuf,
        good: PathBuf,
        marker: PathBuf,
        start_good: bool,
        calls: Mutex<Vec<&'static str>>,
    }

    impl FakeSource {
        fn new(dir: &Path, dest: &Path) -> (Self, PathBuf, PathBuf) {
            Self::with_mode(dir, dest, false)
        }

        fn with_mode(dir: &Path, dest: &Path, start_good: bool) -> (Self, PathBuf, PathBuf) {
            let bad = dir.join("fetch-bad.bin");
            let good = dir.join("fetch-good.bin");
            std::fs::write(&bad, b"bad bytes").unwrap();
            std::fs::write(&good, FIXTURE).unwrap();
            let source = FakeSource {
                bad: bad.clone(),
                good: good.clone(),
                marker: quarantine_path(dest),
                start_good,
                calls: Mutex::new(Vec::new()),
            };
            (source, bad, good)
        }
    }

    impl ModelSource for FakeSource {
        fn get<'a>(
            &'a self,
            _repo: &'a str,
            _file: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>> {
            Box::pin(async move {
                let serve_good = self.start_good || self.marker.exists();
                self.calls
                    .lock()
                    .unwrap()
                    .push(if serve_good { "good" } else { "bad" });
                if serve_good {
                    Ok(self.good.clone())
                } else {
                    Ok(self.bad.clone())
                }
            })
        }
    }

    const GOOD_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    const BAD_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[tokio::test]
    async fn ensure_quarantines_bad_digest_and_redownloads() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("nemotron").join("model.onnx");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        // Pre-planted wrong content.
        std::fs::write(&dest, b"pre-planted tampered bytes").unwrap();

        let (source, _bad, _good) = FakeSource::new(dir.path(), &dest);
        // Sequence: the wrong-content dest is quarantined FIRST (creating the
        // marker), and only then does the fetch run — so the source observes
        // the quarantine and serves the changed (good) content. That fetch
        // after quarantine is the re-download under test.
        let out = ensure_one_model(&source, "r", "f", &dest, GOOD_SHA256)
            .await
            .unwrap();
        assert_eq!(out, InstallOutcome::Installed);
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
        // The pre-planted bytes were quarantined.
        assert_eq!(
            std::fs::read(quarantine_path(&dest)).unwrap(),
            b"pre-planted tampered bytes"
        );
        // Exactly one fetch, and it happened after the quarantine (the
        // source flips to good content only when the marker exists).
        let calls = source.calls.lock().unwrap().clone();
        assert_eq!(calls, vec!["good"], "fetch must follow the quarantine");
        assert_no_temps(dest.parent().unwrap());
    }

    #[tokio::test]
    async fn ensure_quarantines_wrong_digest_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(&dest, b"stale wrong content").unwrap();

        let (source, _bad, _good) = FakeSource::new(dir.path(), &dest);
        let out = ensure_one_model(&source, "r", "f", &dest, GOOD_SHA256)
            .await
            .unwrap();
        assert_eq!(out, InstallOutcome::Installed);
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
        assert_eq!(
            std::fs::read(quarantine_path(&dest)).unwrap(),
            b"stale wrong content"
        );
    }

    #[tokio::test]
    async fn ensure_cleans_stale_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        // Simulate a crashed install: junk temp files for this dest.
        let t1 = dir.path().join("model.onnx.tmp.123.4");
        let t2 = dir.path().join("model.onnx.tmp.123.5");
        std::fs::write(&t1, b"torn").unwrap();
        std::fs::write(&t2, b"torn").unwrap();

        let (source, _bad, _good) = FakeSource::with_mode(dir.path(), &dest, true);
        let out = ensure_one_model(&source, "r", "f", &dest, GOOD_SHA256)
            .await
            .unwrap();
        assert_eq!(out, InstallOutcome::Installed);
        assert!(!t1.exists() && !t2.exists(), "stale temps must be swept");
        assert_eq!(std::fs::read(&dest).unwrap(), FIXTURE);
    }

    #[tokio::test]
    async fn ensure_refuses_fetch_that_never_matches() {
        // A source whose content can never satisfy the pin must surface an
        // error, never install, and never leave temps.
        struct NeverMatches(PathBuf);
        impl ModelSource for NeverMatches {
            fn get<'a>(
                &'a self,
                _repo: &'a str,
                _file: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>> {
                Box::pin(async move { Ok(self.0.clone()) })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("f.bin");
        std::fs::write(&fixture, FIXTURE).unwrap();
        let dest = dir.path().join("model.onnx");
        let err = ensure_one_model(&NeverMatches(fixture), "r", "f", &dest, BAD_SHA256)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("pinned digest"));
        assert!(!dest.exists());
        assert_no_temps(dir.path());
    }

    #[tokio::test]
    async fn ensure_verifies_existing_good_file_without_fetching() {
        struct ZeroFetch;
        impl ModelSource for ZeroFetch {
            fn get<'a>(
                &'a self,
                _repo: &'a str,
                _file: &'a str,
            ) -> Pin<Box<dyn Future<Output = Result<PathBuf>> + Send + 'a>> {
                Box::pin(async move {
                    Err(anyhow::anyhow!(
                        "network must not be touched when the local file verifies"
                    ))
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("model.onnx");
        std::fs::write(&dest, FIXTURE).unwrap();
        let out = ensure_one_model(&ZeroFetch, "r", "f", &dest, GOOD_SHA256)
            .await
            .unwrap();
        assert_eq!(out, InstallOutcome::ExistingVerified);
    }
}
