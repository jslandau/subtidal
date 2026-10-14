//! Static inference dependency and model-provenance migration contracts.
use std::path::Path;

#[test]
fn production_keeps_ort_without_burn() {
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    assert_eq!(manifest.matches("version = \"=0.4.1\"").count(), 2);
    assert_eq!(manifest.matches("version = \"=2.0.0-rc.14\"").count(), 2);
    assert!(!manifest.contains("burn-cuda"));
    assert!(!manifest.contains("burn-metal"));
    let lock =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).unwrap();
    assert!(!lock.contains("name = \"burn\""));
    assert!(!lock.contains("version = \"2.0.0-rc.12\""));
    assert!(!lock.contains("name = \"burn-cuda\""));
}
