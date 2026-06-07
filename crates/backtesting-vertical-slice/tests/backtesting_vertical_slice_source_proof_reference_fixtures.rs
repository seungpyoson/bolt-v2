use std::{fs, path::PathBuf};

use backtesting_vertical_slice::source_proof::{
    FixtureType, SourceCandidateClass, SourceProofReport, SourceProofStatus, SourceSelectionStatus,
};
use serde_json::Value;

fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference")
}

fn reference_source_proof_reports() -> Vec<(PathBuf, Value, SourceProofReport)> {
    fs::read_dir(reference_dir())
        .expect("read reference directory")
        .filter_map(|entry| {
            let path = entry.expect("reference entry").path();
            let name = path.file_name()?.to_str()?;
            (name.starts_with("source-proof-fixture.") && name.ends_with(".json")).then_some(path)
        })
        .map(|path| {
            let bytes = fs::read(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
            let value: Value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("parse JSON {path:?}: {error}"));
            let report: SourceProofReport = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("parse source proof {path:?}: {error}"));
            (path, value, report)
        })
        .collect()
}

fn value_has_any_key(value: &Value, forbidden_keys: &[&str]) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            forbidden_keys.contains(&key.as_str()) || value_has_any_key(value, forbidden_keys)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| value_has_any_key(value, forbidden_keys)),
        _ => false,
    }
}

#[test]
fn reference_fixtures_include_unselected_binary_option_source_proof() {
    let reports = reference_source_proof_reports();
    assert!(
        !reports.is_empty(),
        "reference source-proof fixture reports must be committed before provider selection"
    );

    let binary_option_reports: Vec<_> = reports
        .iter()
        .filter(|(_, _, report)| report.fixture_type == FixtureType::BinaryOption)
        .collect();
    assert!(
        !binary_option_reports.is_empty(),
        "binary-option fixture needs a SourceProofReport before provider selection"
    );

    let has_official_free_pending_candidate = binary_option_reports.iter().any(|(_, _, report)| {
        report.status == SourceProofStatus::Pending
            && report.source_candidate_class == SourceCandidateClass::OfficialFree
            && report.source_selection_status == SourceSelectionStatus::PendingMoreProof
            && report.acceptance_mode.is_none()
            && report.accepted_by.is_none()
            && report.accepted_at.is_none()
    });
    assert!(
        has_official_free_pending_candidate,
        "binary-option fixture must record an official/free pending candidate before paid/vendor or forward-capture paths"
    );

    for (path, value, report) in binary_option_reports {
        assert_ne!(
            report.status,
            SourceProofStatus::Accepted,
            "binary-option fixture report {path:?} must not select a provider before required checks pass"
        );
        assert!(
            report.acceptance_scope.is_none(),
            "non-accepted binary-option fixture report {path:?} must not carry accepted object scope"
        );
        assert!(
            report
                .forbidden_claims
                .iter()
                .any(|claim| claim.contains("NT catalog") && claim.contains("backtest")),
            "binary-option fixture report {path:?} must forbid canonical NT catalog/backtest use"
        );
        assert!(
            report.claim_limits.iter().any(|limit| {
                limit.claim.contains("NT catalog") && limit.claim.contains("backtest")
            }),
            "binary-option fixture report {path:?} must bind the catalog/backtest ban to a claim-limit record"
        );
        assert!(
            !value_has_any_key(
                value,
                &[
                    "raw_payload",
                    "raw_payload_records",
                    "canonical_rows",
                    "catalog_data",
                    "backtest_result",
                    "result_payload"
                ]
            ),
            "source-proof fixture report {path:?} must stay thin and avoid heavy raw/catalog/result payloads"
        );
    }
}
