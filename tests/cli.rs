/// AC2.3: Integration tests for CLI engine flag validation.
/// Tests that invalid engine names produce proper error messages and non-zero exit codes.

#[test]
fn cli_invalid_engine_moonshine_exits_with_error() {
    let output = std::process::Command::new("cargo")
        .args(&["run", "--release", "--", "--engine", "moonshine", "--reset-config"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run subtidal binary");

    // Should exit with non-zero code when given an invalid engine
    assert!(
        !output.status.success(),
        "Expected non-zero exit code for invalid engine 'moonshine'"
    );

    // Should print error message about unknown engine
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown engine") || stderr.contains("moonshine"),
        "Expected error message about unknown engine in stderr: {}",
        stderr
    );
}

#[test]
fn cli_invalid_engine_unknown_exits_with_error() {
    let output = std::process::Command::new("cargo")
        .args(&["run", "--release", "--", "--engine", "unknown_engine", "--reset-config"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run subtidal binary");

    // Should exit with non-zero code
    assert!(
        !output.status.success(),
        "Expected non-zero exit code for invalid engine 'unknown_engine'"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown engine"),
        "Expected 'Unknown engine' in error message: {}",
        stderr
    );
}
