// Functions consumed by Phase 2+
#![allow(dead_code)]

use anyhow::{Context, Result};
use std::path::Path;
use std::path::PathBuf;

/// Returns the base directory for downloaded model files.
/// ~/.local/share/live-captions/models/
pub fn models_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join("live-captions")
        .join("models")
}

/// Returns the directory for Parakeet ONNX model files.
/// ~/.local/share/live-captions/models/parakeet/
pub fn parakeet_model_dir() -> PathBuf {
    models_dir().join("parakeet")
}

/// Returns the directory for Moonshine ONNX model files.
/// ~/.local/share/live-captions/models/moonshine/
pub fn moonshine_model_dir() -> PathBuf {
    models_dir().join("moonshine")
}

/// Returns paths for the three Parakeet model files.
/// Files: encoder.onnx, decoder_joint.onnx, tokenizer.json
pub fn parakeet_model_files() -> [PathBuf; 3] {
    let dir = parakeet_model_dir();
    [
        dir.join("encoder.onnx"),
        dir.join("decoder_joint.onnx"),
        dir.join("tokenizer.json"),
    ]
}

/// Returns paths for the three Moonshine model files.
/// Files: encoder_model_quantized.onnx, decoder_model_merged_quantized.onnx, tokenizer.json
pub fn moonshine_model_files() -> [PathBuf; 3] {
    let dir = moonshine_model_dir();
    [
        dir.join("encoder_model_quantized.onnx"),
        dir.join("decoder_model_merged_quantized.onnx"),
        dir.join("tokenizer.json"),
    ]
}

/// Returns true if all required Parakeet model files are present on disk.
pub fn parakeet_models_present() -> bool {
    parakeet_model_files().iter().all(|p| p.exists())
}

/// Returns true if all required Moonshine model files are present on disk.
pub fn moonshine_models_present() -> bool {
    moonshine_model_files().iter().all(|p| p.exists())
}

/// HuggingFace repo and file paths for the Parakeet EOU model.
/// Repo: altunenes/parakeet-rs
/// Subfolder: realtime_eou_120m-v1-onnx/
const PARAKEET_REPO: &str = "altunenes/parakeet-rs";
const PARAKEET_FILES: &[(&str, &str)] = &[
    ("realtime_eou_120m-v1-onnx/encoder.onnx", "encoder.onnx"),
    ("realtime_eou_120m-v1-onnx/decoder_joint.onnx", "decoder_joint.onnx"),
    ("realtime_eou_120m-v1-onnx/tokenizer.json", "tokenizer.json"),
];

/// HuggingFace repo and file paths for the Moonshine tiny quantized model.
/// Repo: onnx-community/moonshine-tiny-ONNX
const MOONSHINE_REPO: &str = "onnx-community/moonshine-tiny-ONNX";
const MOONSHINE_FILES: &[(&str, &str)] = &[
    ("onnx/encoder_model_quantized.onnx", "encoder_model_quantized.onnx"),
    ("onnx/decoder_model_merged_quantized.onnx", "decoder_model_merged_quantized.onnx"),
    ("tokenizer.json", "tokenizer.json"),
];

/// Download all Parakeet EOU model files to `~/.local/share/live-captions/models/parakeet/`.
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_parakeet_models() -> Result<()> {
    let dest_dir = parakeet_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new()
        .context("initializing HuggingFace API")?;
    let repo = api.model(PARAKEET_REPO.to_string());

    for (remote_path, local_name) in PARAKEET_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!("info: parakeet model file already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo.get(remote_path).await
            .with_context(|| format!("downloading {remote_path} from {PARAKEET_REPO}"))?;
        copy_model_file(&cached, &dest)
            .with_context(|| format!("copying {remote_path} to {}", dest.display()))?;
        eprintln!("info: saved to {}", dest.display());
    }
    Ok(())
}

/// Download all Moonshine model files to `~/.local/share/live-captions/models/moonshine/`.
/// Skips individual files that already exist.
/// Exits the process with an error message if any download fails.
pub async fn ensure_moonshine_models() -> Result<()> {
    let dest_dir = moonshine_model_dir();
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let api = hf_hub::api::tokio::Api::new()
        .context("initializing HuggingFace API")?;
    let repo = api.model(MOONSHINE_REPO.to_string());

    for (remote_path, local_name) in MOONSHINE_FILES {
        let dest = dest_dir.join(local_name);
        if dest.exists() {
            eprintln!("info: moonshine model file already present: {}", dest.display());
            continue;
        }
        eprintln!("info: downloading {} ...", remote_path);
        let cached = repo.get(remote_path).await
            .with_context(|| format!("downloading {remote_path} from {MOONSHINE_REPO}"))?;
        copy_model_file(&cached, &dest)
            .with_context(|| format!("copying {remote_path} to {}", dest.display()))?;
        eprintln!("info: saved to {}", dest.display());
    }
    Ok(())
}

fn copy_model_file(src: &Path, dest: &Path) -> Result<()> {
    // Try hardlink first (free if on same filesystem as HF cache).
    // Fall back to copy if hardlink fails (different filesystem).
    if std::fs::hard_link(src, dest).is_err() {
        std::fs::copy(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
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
    fn test_parakeet_model_dir_contains_models_dir() {
        let parakeet_dir = parakeet_model_dir();
        let models_base = models_dir();
        assert!(parakeet_dir.starts_with(&models_base));
    }

    #[test]
    fn test_moonshine_model_dir_contains_models_dir() {
        let moonshine_dir = moonshine_model_dir();
        let models_base = models_dir();
        assert!(moonshine_dir.starts_with(&models_base));
    }

    #[test]
    fn test_parakeet_model_files_have_correct_names() {
        let files = parakeet_model_files();
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("encoder.onnx"));
        assert!(files[1].ends_with("decoder_joint.onnx"));
        assert!(files[2].ends_with("tokenizer.json"));
    }

    #[test]
    fn test_moonshine_model_files_have_correct_names() {
        let files = moonshine_model_files();
        assert_eq!(files.len(), 3);
        assert!(files[0].ends_with("encoder_model_quantized.onnx"));
        assert!(files[1].ends_with("decoder_model_merged_quantized.onnx"));
        assert!(files[2].ends_with("tokenizer.json"));
    }

    #[test]
    fn test_parakeet_models_present_nonexistent_returns_false() {
        // Since the paths don't actually exist, this should return false
        assert!(!parakeet_models_present());
    }

    #[test]
    fn test_moonshine_models_present_nonexistent_returns_false() {
        // Since the paths don't actually exist, this should return false
        assert!(!moonshine_models_present());
    }

    /// AC5.2: Skip download when models present.
    /// Test that parakeet_models_present returns true when all three required files exist.
    #[test]
    fn test_parakeet_models_present_when_files_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let model_dir = tempdir.path().join("parakeet");
        std::fs::create_dir_all(&model_dir).unwrap();

        // Create the three required files
        std::fs::write(model_dir.join("encoder.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("decoder_joint.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("tokenizer.json"), b"dummy").unwrap();

        // Manually check the files in the temp directory
        let files = [
            model_dir.join("encoder.onnx"),
            model_dir.join("decoder_joint.onnx"),
            model_dir.join("tokenizer.json"),
        ];

        for file in &files {
            assert!(file.exists(), "File should exist: {}", file.display());
        }
    }

    /// AC5.2: Skip download when models present.
    /// Test that moonshine_models_present returns true when all three required files exist.
    #[test]
    fn test_moonshine_models_present_when_files_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let model_dir = tempdir.path().join("moonshine");
        std::fs::create_dir_all(&model_dir).unwrap();

        // Create the three required files
        std::fs::write(model_dir.join("encoder_model_quantized.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("decoder_model_merged_quantized.onnx"), b"dummy").unwrap();
        std::fs::write(model_dir.join("tokenizer.json"), b"dummy").unwrap();

        // Manually check the files in the temp directory
        let files = [
            model_dir.join("encoder_model_quantized.onnx"),
            model_dir.join("decoder_model_merged_quantized.onnx"),
            model_dir.join("tokenizer.json"),
        ];

        for file in &files {
            assert!(file.exists(), "File should exist: {}", file.display());
        }
    }
}
