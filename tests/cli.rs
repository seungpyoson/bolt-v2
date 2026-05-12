use std::process::Command;

use tempfile::tempdir;

#[test]
fn stream_to_lake_fails_when_live_spool_is_missing() {
    let source_root = tempdir().unwrap();
    let output_root = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stream_to_lake"))
        .args([
            "--catalog-path",
            source_root.path().to_str().expect("utf-8 path"),
            "--instance-id",
            "missing-instance",
            "--output-root",
            output_root.path().to_str().expect("utf-8 path"),
        ])
        .output()
        .expect("stream_to_lake should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing live spool instance directory"),
        "{stderr}"
    );
}

#[test]
fn stream_to_lake_rejects_relative_contract_path() {
    let source_root = tempdir().unwrap();
    let output_root = tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stream_to_lake"))
        .args([
            "--catalog-path",
            source_root.path().to_str().expect("utf-8 path"),
            "--instance-id",
            "missing-instance",
            "--output-root",
            output_root.path().to_str().expect("utf-8 path"),
            "--contract",
            "contracts/polymarket.toml",
        ])
        .output()
        .expect("stream_to_lake should run");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("contract_path must be a local absolute path"),
        "{stderr}"
    );
}
