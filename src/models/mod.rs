use anyhow::{Context, Result};
use std::path::Path;
use std::path::PathBuf;

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

/// HuggingFace repo and file paths for the Nemotron streaming model.
/// Single source of truth: every other function that enumerates or checks for
/// Nemotron model files derives from this list.
const NEMOTRON_REPO: &str = "altunenes/parakeet-rs";
const NEMOTRON_FILES: &[(&str, &str)] = &[
    (
        "nemotron-speech-streaming-en-0.6b/encoder.onnx",
        "encoder.onnx",
    ),
    (
        "nemotron-speech-streaming-en-0.6b/encoder.onnx.data",
        "encoder.onnx.data",
    ),
    (
        "nemotron-speech-streaming-en-0.6b/decoder_joint.onnx",
        "decoder_joint.onnx",
    ),
    (
        "nemotron-speech-streaming-en-0.6b/tokenizer.model",
        "tokenizer.model",
    ),
];

/// HuggingFace repo and file path for the Sortformer diarization model.
const SORTFORMER_REPO: &str = "altunenes/parakeet-rs";
const SORTFORMER_FILE: (&str, &str) = (
    "diar_streaming_sortformer_4spk-v2.1.onnx",
    "diar_streaming_sortformer_4spk-v2.1.onnx",
);

/// Returns the local paths for every required Nemotron model file inside the
/// given base models directory (i.e. the parent of the `nemotron/` subdir).
pub fn nemotron_model_files_in(base_dir: &Path) -> Vec<PathBuf> {
    let model_dir = base_dir.join("nemotron");
    NEMOTRON_FILES
        .iter()
        .map(|(_, local)| model_dir.join(local))
        .collect()
}

/// Returns the local path for the Sortformer model file inside the given base directory.
pub fn diarization_model_file_in(base_dir: &Path) -> PathBuf {
    base_dir.join("diarization").join(SORTFORMER_FILE.1)
}

/// Returns true if all required Nemotron model files are present on disk in the given directory.
pub fn nemotron_models_present_in(dir: &Path) -> bool {
    nemotron_model_files_in(dir).iter().all(|p| p.exists())
}

/// Returns true if all required Nemotron model files are present on disk.
pub fn nemotron_models_present() -> bool {
    nemotron_models_present_in(&models_dir())
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

/// Returns true if the Sortformer model file is present on disk.
pub fn diarization_models_present() -> bool {
    diarization_model_dir().join(SORTFORMER_FILE.1).exists()
}

/// Download the Sortformer diarization model to `diarization_model_dir()` (per-OS data dir).
/// Skips download if the file already exists.
pub async fn ensure_diarization_models() -> Result<()> {
    let dest_dir = diarization_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let dest = dest_dir.join(SORTFORMER_FILE.1);
    if dest.exists() {
        eprintln!(
            "info: Sortformer model file already present: {}",
            dest.display()
        );
        return Ok(());
    }

    eprintln!("info: downloading Sortformer diarization model (~492 MB) ...");

    let api = hf_hub::api::tokio::Api::new().context("initializing HuggingFace API")?;
    let repo = api.model(SORTFORMER_REPO.to_string());

    let cached = repo
        .get(SORTFORMER_FILE.0)
        .await
        .with_context(|| format!("downloading {} from {SORTFORMER_REPO}", SORTFORMER_FILE.0))?;
    copy_model_file(&cached, &dest)
        .with_context(|| format!("copying Sortformer model to {}", dest.display()))?;
    eprintln!("info: Sortformer model saved to {}", dest.display());

    Ok(())
}

/// Download all Nemotron model files to `nemotron_model_dir()` (per-OS data dir).
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_nemotron_models() -> Result<()> {
    let dest_dir = nemotron_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new().context("initializing HuggingFace API")?;
    let repo = api.model(NEMOTRON_REPO.to_string());

    for (remote_path, local_name) in NEMOTRON_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!(
                "info: nemotron model file already present: {}",
                dest.display()
            );
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo
            .get(remote_path)
            .await
            .with_context(|| format!("downloading {remote_path} from {NEMOTRON_REPO}"))?;
        copy_model_file(&cached, &dest)
            .with_context(|| format!("copying {remote_path} to {}", dest.display()))?;
        eprintln!("info: saved to {}", dest.display());
    }
    Ok(())
}

fn copy_model_file(src: &Path, dest: &Path) -> Result<()> {
    // Resolve symlinks: hf-hub returns paths that are symlinks into its blob store.
    // We must resolve to the real file before hardlinking, otherwise we'd create a
    // hardlink to the symlink (which has a relative target that won't resolve from
    // our models directory).
    let real_src = std::fs::canonicalize(src)
        .with_context(|| format!("resolving symlink {}", src.display()))?;

    // Try hardlink first (free if on same filesystem as HF cache).
    // Fall back to copy if hardlink fails (different filesystem).
    if std::fs::hard_link(&real_src, dest).is_err() {
        std::fs::copy(&real_src, dest)
            .with_context(|| format!("copying {} to {}", real_src.display(), dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_nemotron_models_present_missing_file_returns_false() {
        // Check against a temp dir with no files — should return false.
        let tempdir = tempfile::tempdir().unwrap();
        assert!(!nemotron_models_present_in(tempdir.path()));
    }

    /// AC5.2: Skip download when models present.
    /// Test that nemotron_models_present returns true when all four required files exist.
    #[test]
    fn test_nemotron_models_present_when_files_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let model_dir = tempdir.path().join("nemotron");
        std::fs::create_dir_all(&model_dir).unwrap();

        // Create the four required files
        std::fs::write(model_dir.join("encoder.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("encoder.onnx.data"), b"dummy").unwrap();
        std::fs::write(model_dir.join("decoder_joint.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("tokenizer.model"), b"dummy").unwrap();

        // Test that nemotron_models_present_in returns true when all files exist
        assert!(
            nemotron_models_present_in(tempdir.path()),
            "nemotron_models_present_in should return true when all files exist"
        );
    }
}
