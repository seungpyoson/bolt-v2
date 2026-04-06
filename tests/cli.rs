use std::process::Command;

#[test]
fn secrets_subcommand_reports_reference_detection_in_config() {
    let output = Command::new(env!("CARGO_BIN_EXE_bolt-v2"))
        .args([
            "secrets",
            "--config",
            "config/examples/polymarket-exec-tester.toml",
        ])
        .output()
        .expect("secrets subcommand should run");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("POLYMARKET: secret references found in config"));
}
