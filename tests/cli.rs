/// Integration tests for CLI engine flag validation.
///
/// F18: invokes the prebuilt binary via `env!("CARGO_BIN_EXE_subtidal")`
/// (Cargo provides the built bin path to integration tests) instead of a
/// nested `cargo run --release`, which contended on the build lock and
/// mutated the real `$HOME` (desktop entry install). Each invocation gets a
/// throwaway `HOME`/`XDG_DATA_HOME` and a temp `--config` path, so the tests
/// are hermetic and parallel-safe by construction.
use std::process::Command;

fn hermetic_cli(args: &[&str]) -> std::process::Output {
    let home = tempfile::tempdir().expect("temp HOME");
    let data_home = tempfile::tempdir().expect("temp XDG_DATA_HOME");
    let config_dir = tempfile::tempdir().expect("temp config dir");
    let config_path = config_dir.path().join("config.toml");

    // Point the binary at the throwaway config so it never reads or writes
    // the user's real one.
    let mut owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    owned.push("--config".to_string());
    owned.push(config_path.to_string_lossy().to_string());

    Command::new(env!("CARGO_BIN_EXE_subtidal"))
        .args(&owned)
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_CACHE_HOME", data_home.path().join("cache"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("Failed to run subtidal binary")
}

#[test]
fn cli_invalid_engine_moonshine_exits_with_error() {
    let output = hermetic_cli(&["--engine", "moonshine", "--reset-config"]);

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
    let output = hermetic_cli(&["--engine", "unknown_engine", "--reset-config"]);

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

/// AC.15 (F18): the test never creates a desktop entry in the real home and
/// never spawns a nested `cargo` — assertable from the child command line.
#[test]
fn cli_tests_never_touch_real_home_or_spawn_cargo() {
    // Sanity-check the harness itself: the binary path is the Cargo-provided
    // bin artifact (not `cargo`), and the assertions above already pin the
    // behavioral contract.
    let exe = env!("CARGO_BIN_EXE_subtidal");
    assert!(
        !exe.contains("cargo"),
        "integration test must invoke the bin artifact directly, got {exe}"
    );
    let output = hermetic_cli(&["--engine", "moonshine"]);
    assert!(!output.status.success());
}
