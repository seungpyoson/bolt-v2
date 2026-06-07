use std::{fs, process::Command};

use backtesting_vertical_slice::backfill_coverage::{
    BACKFILL_COVERAGE_LEDGER_FILE, BackfillCoverageLedger,
};

#[test]
fn coverage_ledger_cli_writes_artifact_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("summary.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-cli-a",
            "source_binding": "synthetic-native-trades",
            "source_proof_id": "source-proof-synthetic-native-trades",
            "source_proof_version": 1,
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "completed_objects": 13,
            "completed_bytes": 3_900,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");

    let output_dir = dir.path().join("coverage-ledger");
    let spec_path = dir.path().join("coverage.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-cli"
output_dir = "{}"

[[manifest]]
manifest_uri = "manifest://synthetic/cli-a.json"
path = "{}"
source_proof_status = "accepted"
"#,
            output_dir.display(),
            manifest_path.display()
        ),
    )
    .expect("write spec");

    let binary = std::env::var("CARGO_BIN_EXE_backfill_coverage_ledger")
        .expect("backfill_coverage_ledger binary path");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run backfill coverage ledger CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("coverage_ledger = "), "{stdout}");
    assert!(stdout.contains("accepted_objects = 13"), "{stdout}");

    let ledger_path = output_dir.join(BACKFILL_COVERAGE_LEDGER_FILE);
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(ledger_path).expect("read ledger")).expect("parse ledger");
    assert_eq!(ledger.ledger_id, "ledger-synthetic-cli");
    assert_eq!(ledger.summary.accepted_records, 1);
    assert_eq!(ledger.summary.accepted_objects, 13);
    assert_eq!(ledger.summary.accepted_bytes, 3_900);
}
