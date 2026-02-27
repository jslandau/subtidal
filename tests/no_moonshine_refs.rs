/// AC1.3: Verify that no references to "moonshine" exist in source code implementation.
/// This test uses grep to search for "moonshine" in the actual implementation code.
///
/// Since Moonshine is intentionally removed from the codebase, no references should exist
/// in the actual implementation. Test data and comments that reference it as unsupported are OK.

#[test]
fn no_moonshine_references_in_implementation() {
    use std::process::Command;

    // Check specific source files where moonshine would be referenced if it were implemented:
    // - stt/moonshine.rs would be the implementation file
    // - stt/mod.rs would reference the Moonshine variant in the engine enum
    // - The Engine enum in config.rs should not have Moonshine variant

    // First check: stt/moonshine.rs should not exist
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let moonshine_rs = std::path::Path::new(manifest_dir).join("src/stt/moonshine.rs");
    assert!(
        !moonshine_rs.exists(),
        "Found stt/moonshine.rs - Moonshine implementation should be removed"
    );

    // Second check: stt/mod.rs should not reference Moonshine engine variant
    let output = Command::new("grep")
        .args(&["-n", "Moonshine", "src/stt/mod.rs"])
        .current_dir(manifest_dir)
        .output()
        .expect("Failed to run grep on stt/mod.rs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.trim().is_empty(),
        "Found 'Moonshine' in stt/mod.rs (should be removed): {}",
        stdout
    );
}

#[test]
fn no_moonshine_engine_variant_in_engine_enum() {
    // This is a stricter check: ensure the Engine enum in config.rs doesn't have a Moonshine variant.
    let output = std::process::Command::new("grep")
        .args(&["-n", "\\bMoonshine\\b", "src/config.rs"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run grep on config.rs");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The grep search uses word boundaries to exclude comments about moonshine as an unsupported value
    assert!(
        stdout.trim().is_empty(),
        "Found 'Moonshine' enum variant in Engine enum (should only have Nemotron): {}",
        stdout
    );
}
