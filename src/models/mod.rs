// Functions consumed by Phase 2+
#![allow(dead_code)]

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
}
