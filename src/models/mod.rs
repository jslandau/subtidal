// Functions consumed by Phase 2+
#![allow(dead_code)]

use anyhow::{Context, Result};
use std::path::Path;
use std::path::PathBuf;

/// Returns the base directory for downloaded model files.
/// ~/.local/share/subtidal/models/
pub fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("subtidal")
        .join("models")
}

/// Returns the directory for Nemotron ONNX model files.
/// ~/.local/share/subtidal/models/nemotron/
pub fn nemotron_model_dir() -> PathBuf {
    models_dir().join("nemotron")
}

/// Returns paths for the four Nemotron model files.
/// Files: encoder.onnx, encoder.onnx.data, decoder_joint.onnx, tokenizer.model
pub fn nemotron_model_files() -> [PathBuf; 4] {
    let dir = nemotron_model_dir();
    [
        dir.join("encoder.onnx"),
        dir.join("encoder.onnx.data"),
        dir.join("decoder_joint.onnx"),
        dir.join("tokenizer.model"),
    ]
}

/// Returns true if all required Nemotron model files are present on disk in the given directory.
pub fn nemotron_models_present_in(dir: &Path) -> bool {
    let model_dir = dir.join("nemotron");
    [
        model_dir.join("encoder.onnx"),
        model_dir.join("encoder.onnx.data"),
        model_dir.join("decoder_joint.onnx"),
        model_dir.join("tokenizer.model"),
    ]
    .iter()
    .all(|p| p.exists())
}

/// Returns true if all required Nemotron model files are present on disk.
pub fn nemotron_models_present() -> bool {
    nemotron_models_present_in(&models_dir())
}

/// HuggingFace repo and file paths for the Nemotron streaming model.
/// Repo: altunenes/parakeet-rs
/// Subfolder: nemotron-speech-streaming-en-0.6b/
const NEMOTRON_REPO: &str = "altunenes/parakeet-rs";
const NEMOTRON_FILES: &[(&str, &str)] = &[
    ("nemotron-speech-streaming-en-0.6b/encoder.onnx", "encoder.onnx"),
    ("nemotron-speech-streaming-en-0.6b/encoder.onnx.data", "encoder.onnx.data"),
    ("nemotron-speech-streaming-en-0.6b/decoder_joint.onnx", "decoder_joint.onnx"),
    ("nemotron-speech-streaming-en-0.6b/tokenizer.model", "tokenizer.model"),
];

/// Download all Nemotron model files to `~/.local/share/subtidal/models/nemotron/`.
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_nemotron_models() -> Result<()> {
    let dest_dir = nemotron_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new()
        .context("initializing HuggingFace API")?;
    let repo = api.model(NEMOTRON_REPO.to_string());

    for (remote_path, local_name) in NEMOTRON_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!("info: nemotron model file already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo.get(remote_path).await
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
    fn test_nemotron_model_files_have_correct_names() {
        let files = nemotron_model_files();
        assert_eq!(files.len(), 4);
        assert!(files[0].ends_with("encoder.onnx"));
        assert!(files[1].ends_with("encoder.onnx.data"));
        assert!(files[2].ends_with("decoder_joint.onnx"));
        assert!(files[3].ends_with("tokenizer.model"));
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
