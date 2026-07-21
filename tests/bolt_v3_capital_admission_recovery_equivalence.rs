use std::{fs, path::PathBuf};

use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3SubmitReservationRecoveryEvidence, read_submit_reservation_recovery_evidence,
};
use serde_json::{Value, json};

#[test]
fn native_v13_identity_decoders_recover_capital_admission_reservations() {
    let repo = repo_root();
    let fixture_dir = repo.join("tests/fixtures/bolt_v3/capital_admission_recovery");
    let original_path = fixture_dir.join("v13/decision-evidence.jsonl");
    let recovery = read_submit_reservation_recovery_evidence(&original_path, 100_000)
        .expect("registered v13 reservation identities should recover without rewriting history");
    let actual = recovered_snapshot_json(&recovery);
    let expected: Value = serde_json::from_str(
        &fs::read_to_string(fixture_dir.join("recovered_v15_golden.json"))
            .expect("golden recovery snapshot should be readable"),
    )
    .expect("golden recovery snapshot should parse");
    assert_eq!(actual, expected);
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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
