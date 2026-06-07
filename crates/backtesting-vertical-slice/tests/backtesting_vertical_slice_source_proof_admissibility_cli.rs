use std::{fs, process::Command};

use backtesting_vertical_slice::{
    source_proof::CONTRACT_VERSION,
    source_proof_admissibility::{
        SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE, SourceProofAdmissibilityReport,
    },
};

#[test]
fn source_proof_admissibility_cli_writes_report_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let proof_path = dir.path().join("source-proof.json");
    fs::write(
        &proof_path,
        serde_json::to_vec(&serde_json::json!({
            "source_proof_id": "source-proof-synthetic-cli-legacy",
            "source_proof_version": 1,
            "contract_version": CONTRACT_VERSION,
            "schema_version": "source-proof-v3.legacy",
            "status": "pending",
            "source_binding_key": "synthetic-native-trades",
            "venue": "synthetic",
            "product_family": "native_trades",
            "table_families": ["trades"],
            "raw_payload_records": [],
            "required_checks": {
                "source_access": "pending"
            }
        }))
        .expect("serialize proof"),
    )
    .expect("write proof");

    let output_dir = dir.path().join("source-proof-admissibility");
    let spec_path = dir.path().join("source-proof-admissibility.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
report_id = "source-proof-admissibility-cli"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[source_proof]]
proof_uri = "proof://synthetic/cli-source-proof.json"
path = "{}"
"#,
            output_dir.display(),
            proof_path.display()
        ),
    )
    .expect("write spec");

    let binary = std::env::var("CARGO_BIN_EXE_source_proof_admissibility")
        .expect("source_proof_admissibility binary path");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run source proof admissibility CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("source_proof_admissibility_report = "),
        "{stdout}"
    );
    assert!(stdout.contains("records = 1"), "{stdout}");
    assert!(
        stdout.contains("non_current_contract_records = 1"),
        "{stdout}"
    );

    let report_path = output_dir.join(SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE);
    let report: SourceProofAdmissibilityReport =
        serde_json::from_slice(&fs::read(report_path).expect("read report")).expect("parse report");
    assert_eq!(report.report_id, "source-proof-admissibility-cli");
    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.non_current_contract_records, 1);
}
