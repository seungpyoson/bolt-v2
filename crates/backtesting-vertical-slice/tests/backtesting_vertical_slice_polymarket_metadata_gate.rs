use backtesting_vertical_slice::polymarket_metadata_gate::{
    PolymarketMetadataGateSpec, PolymarketMetadataGateStatus, evaluate_polymarket_metadata_gate,
};
use std::process::Command;

#[test]
fn polymarket_metadata_gate_blocks_when_gamma_response_lacks_selected_token() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let gamma_path = dir.path().join("gamma.json");
    std::fs::write(&gamma_path, b"[]").expect("write gamma");

    let report = evaluate_polymarket_metadata_gate(&PolymarketMetadataGateSpec {
        source_binding: "synthetic-polymarket-source".to_string(),
        selected_token_id: "token-a".to_string(),
        selected_condition_id: "0xcondition".to_string(),
        gamma_markets_path: gamma_path,
    })
    .expect("metadata gate");

    assert_eq!(
        report.status,
        PolymarketMetadataGateStatus::BlockedMissingGammaMarket
    );
    assert_eq!(report.gamma_market_count, 0);
    assert_eq!(report.matching_gamma_market_count, 0);
    assert_eq!(report.nt_instrument_def_count, 0);
    assert!(
        report.blocking_issues.iter().any(|issue| {
            issue.contains("selected token") && issue.contains("selected condition")
        })
    );
}

#[test]
fn polymarket_metadata_gate_accepts_source_backed_gamma_market_through_nt_parser() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let gamma_path = dir.path().join("gamma.json");
    std::fs::write(
        &gamma_path,
        r#"[{
  "id": "market-1",
  "conditionId": "0xcondition",
  "questionID": "0xquestion",
  "clobTokenIds": "[\"token-a\", \"token-b\"]",
  "outcomes": "[\"Yes\", \"No\"]",
  "question": "Synthetic source-backed question?",
  "description": "Synthetic description",
  "startDate": "2026-05-20T20:00:00Z",
  "endDate": "2026-05-21T20:00:00Z",
  "active": true,
  "closed": false,
  "acceptingOrders": true,
  "enableOrderBook": true,
  "negRisk": false,
  "orderPriceMinTickSize": 0.01,
  "orderMinSize": 5,
  "feeSchedule": {
    "exponent": 1,
    "rate": 0.03,
    "takerOnly": true,
    "rebateRate": 0
  }
}]"#,
    )
    .expect("write gamma");

    let report = evaluate_polymarket_metadata_gate(&PolymarketMetadataGateSpec {
        source_binding: "synthetic-polymarket-source".to_string(),
        selected_token_id: "token-a".to_string(),
        selected_condition_id: "0xcondition".to_string(),
        gamma_markets_path: gamma_path,
    })
    .expect("metadata gate");

    assert_eq!(report.status, PolymarketMetadataGateStatus::Accepted);
    assert_eq!(report.gamma_market_count, 1);
    assert_eq!(report.matching_gamma_market_count, 1);
    assert_eq!(report.nt_instrument_def_count, 2);
    assert_eq!(report.selected_token_nt_def_count, 1);
    assert!(report.blocking_issues.is_empty());
}

#[test]
fn polymarket_metadata_gate_blocks_when_matching_gamma_market_omits_neg_risk() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let gamma_path = dir.path().join("gamma.json");
    std::fs::write(
        &gamma_path,
        r#"[{
  "id": "market-1",
  "conditionId": "0xcondition",
  "questionID": "0xquestion",
  "clobTokenIds": "[\"token-a\", \"token-b\"]",
  "outcomes": "[\"Yes\", \"No\"]",
  "question": "Missing provenance?",
  "startDate": "2026-05-20T20:00:00Z",
  "endDate": "2026-05-21T20:00:00Z",
  "active": true,
  "closed": false,
  "acceptingOrders": true,
  "enableOrderBook": true,
  "orderPriceMinTickSize": 0.01,
  "orderMinSize": 5
}]"#,
    )
    .expect("write gamma");

    let report = evaluate_polymarket_metadata_gate(&PolymarketMetadataGateSpec {
        source_binding: "synthetic-polymarket-source".to_string(),
        selected_token_id: "token-a".to_string(),
        selected_condition_id: "0xcondition".to_string(),
        gamma_markets_path: gamma_path,
    })
    .expect("metadata gate");

    assert_eq!(
        report.status,
        PolymarketMetadataGateStatus::BlockedInvalidGammaMarket
    );
    assert_eq!(report.nt_instrument_def_count, 0);
    assert!(
        report
            .blocking_issues
            .iter()
            .any(|issue| issue.contains("missing required negRisk metadata"))
    );
}

#[test]
fn polymarket_metadata_gate_cli_writes_report_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let gamma_path = dir.path().join("gamma.json");
    let report_path = dir.path().join("metadata-gate-report.json");
    let spec_path = dir.path().join("metadata-gate.toml");
    std::fs::write(
        &gamma_path,
        r#"[{
  "id": "market-1",
  "conditionId": "0xcondition",
  "questionID": "0xquestion",
  "clobTokenIds": "[\"token-a\", \"token-b\"]",
  "outcomes": "[\"Yes\", \"No\"]",
  "question": "Synthetic source-backed question?",
  "description": "Synthetic description",
  "startDate": "2026-05-20T20:00:00Z",
  "endDate": "2026-05-21T20:00:00Z",
  "active": true,
  "closed": false,
  "acceptingOrders": true,
  "enableOrderBook": true,
  "negRisk": false,
  "orderPriceMinTickSize": 0.01,
  "orderMinSize": 5,
  "feeSchedule": {
    "exponent": 1,
    "rate": 0.03,
    "takerOnly": true,
    "rebateRate": 0
  }
}]"#,
    )
    .expect("write gamma");
    std::fs::write(
        &spec_path,
        format!(
            r#"source_binding = "synthetic-polymarket-source"
selected_token_id = "token-a"
selected_condition_id = "0xcondition"
gamma_markets_path = "{}"
output_path = "{}"
"#,
            gamma_path.display(),
            report_path.display()
        ),
    )
    .expect("write metadata gate spec");

    let binary = std::env::var("CARGO_BIN_EXE_polymarket_metadata_gate")
        .expect("polymarket_metadata_gate binary");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run polymarket metadata gate CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("polymarket_metadata_gate_report = "));
    assert!(stdout.contains("status = accepted"));

    let report = std::fs::read_to_string(report_path).expect("read report");
    assert!(report.contains("\"status\": \"accepted\""));
}
