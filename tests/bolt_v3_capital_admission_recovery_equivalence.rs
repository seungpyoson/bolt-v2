use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3SubmitReservationRecoveryEvidence, read_submit_reservation_recovery_evidence,
};
use serde_json::{Value, json};

#[test]
fn migrated_v13_evidence_recovers_capital_admission_reservations_equivalently() {
    let repo = repo_root();
    let fixture_dir = repo.join("tests/fixtures/bolt_v3/capital_admission_recovery");
    let source_dir = fixture_dir.join("v13");
    let temp = tempfile::tempdir().expect("tempdir should create");
    let working_dir = temp.path().join("decision-evidence");
    copy_dir_recursive(&source_dir, &working_dir);

    let original_path = source_dir.join("decision-evidence.jsonl");
    let migrated_path = working_dir.join("decision-evidence.jsonl");
    let original_values = jsonl_values(&original_path);
    let original_lines = jsonl_lines(&original_path);

    run_evidence_migrator(&repo, &working_dir);

    let migrated_values = jsonl_values(&migrated_path);
    assert_migrated_records_preserve_reservation_payloads(&original_values, &migrated_values);

    let recovery = read_submit_reservation_recovery_evidence(&migrated_path, 100_000)
        .expect("migrated v14 reservation evidence should recover");
    let actual = recovered_snapshot_json(&recovery);
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(fixture_dir.join("recovered_v14_golden.json"))
            .expect("golden recovery snapshot should be readable"),
    )
    .expect("golden recovery snapshot should parse");
    assert_eq!(actual, expected);

    assert_unmigrated_v13_reservation_records_fail_closed(temp.path(), &original_lines);
    assert_legacy_v13_rebuild_audit_record_skips(temp.path(), &original_lines, &migrated_path);
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_evidence_migrator(repo: &Path, directory: &Path) {
    let output = Command::new("python3")
        .arg(repo.join("scripts/migrate_bolt_v3_decision_evidence_v13_to_v14.py"))
        .arg(directory)
        .output()
        .expect("python3 migrator process should start");
    assert!(
        output.status.success(),
        "migrator failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_dir_recursive(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination fixture dir should create");
    for entry in fs::read_dir(source).expect("source fixture dir should be readable") {
        let entry = entry.expect("source fixture entry should be readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should copy");
        }
    }
}

fn jsonl_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("jsonl fixture should be readable")
        .lines()
        .map(str::to_string)
        .collect()
}

fn jsonl_values(path: &Path) -> Vec<Value> {
    jsonl_lines(path)
        .into_iter()
        .map(|line| serde_json::from_str(&line).expect("jsonl fixture line should parse"))
        .collect()
}

fn assert_migrated_records_preserve_reservation_payloads(original: &[Value], migrated: &[Value]) {
    assert_eq!(
        original.len(),
        migrated.len(),
        "migration must not add or drop evidence records"
    );
    for (before, after) in original.iter().zip(migrated) {
        assert_eq!(after["schema_version"], json!(14));
        match before["kind"]
            .as_str()
            .expect("fixture record should have kind")
        {
            "position_sizer_rebuild" => {
                assert_eq!(after["kind"], "capital_admission_rebuild");
                assert_eq!(after["gate_id"], "bolt_v3.capital_admission_rebuild");
            }
            "submit_reservation_metadata" => {
                assert_eq!(after["kind"], "submit_reservation_metadata");
                assert_eq!(
                    before["metadata"], after["metadata"],
                    "reservation metadata payload must survive migration unchanged"
                );
            }
            "submit_reservation_fill" => {
                assert_eq!(after["kind"], "submit_reservation_fill");
                assert_eq!(
                    before["fill"], after["fill"],
                    "reservation fill payload must survive migration unchanged"
                );
            }
            "admission_decision" => {
                assert_eq!(after["kind"], "admission_decision");
                assert_eq!(after["decision"]["outcome"], "rejected_capital_admission");
                assert_eq!(
                    after["decision"]["snapshot_source"],
                    "nt_capital_admission_state"
                );
            }
            other => panic!("unexpected fixture record kind {other}"),
        }
    }
}

fn recovered_snapshot_json(recovery: &BoltV3SubmitReservationRecoveryEvidence) -> Value {
    let reservations = recovery
        .metadata_by_client_order_id
        .iter()
        .map(|(client_order_id, recovered)| {
            let metadata = &recovered.metadata;
            json!({
                "client_order_id": client_order_id,
                "submit_reservation_id": metadata.submit_reservation_id.as_str(),
                "capital_pool_id": metadata.capital_pool_id.as_str(),
                "collateral_group_id": metadata.collateral_group_id.as_str(),
                "instrument_id": metadata.instrument_id.as_str(),
                "side": metadata.side.as_str(),
                "submitted_quantity": metadata.submitted_quantity.as_str(),
                "reserved_liability": metadata.reserved_liability.as_str(),
                "metadata_source": metadata.source.as_str(),
                "fill_trade_ids": recovered.fill_trade_ids.iter().collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({ "reservations": reservations })
}

fn assert_unmigrated_v13_reservation_records_fail_closed(
    temp_root: &Path,
    original_lines: &[String],
) {
    let metadata_line = original_lines
        .iter()
        .find(|line| line.contains(r#""kind":"submit_reservation_metadata""#))
        .expect("fixture should include reservation metadata");
    let path = temp_root.join("unmigrated-reservation.jsonl");
    fs::write(&path, format!("{metadata_line}\n")).expect("unmigrated reservation fixture writes");

    let error = read_submit_reservation_recovery_evidence(&path, 100_000)
        .expect_err("unmigrated v13 reservation metadata must fail closed");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("schema_version mismatch"),
        "expected schema mismatch for unmigrated v13 reservation metadata, got: {rendered}"
    );
}

fn assert_legacy_v13_rebuild_audit_record_skips(
    temp_root: &Path,
    original_lines: &[String],
    migrated_path: &Path,
) {
    let legacy_audit_line = original_lines
        .iter()
        .find(|line| line.contains(r#""kind":"position_sizer_rebuild""#))
        .expect("fixture should include legacy audit line");
    let migrated_metadata_line = jsonl_lines(migrated_path)
        .into_iter()
        .find(|line| line.contains(r#""kind":"submit_reservation_metadata""#))
        .expect("migrated fixture should include current metadata");
    let path = temp_root.join("legacy-audit-plus-current-reservation.jsonl");
    fs::write(
        &path,
        format!("{legacy_audit_line}\n{migrated_metadata_line}\n"),
    )
    .expect("legacy audit skip fixture writes");

    let recovery = read_submit_reservation_recovery_evidence(&path, 100_000)
        .expect("legacy v13 rebuild audit line must skip");
    assert!(
        recovery
            .metadata_by_client_order_id
            .contains_key("client-order-pool-a-live"),
        "current reservation metadata should recover after legacy audit skip"
    );
}
