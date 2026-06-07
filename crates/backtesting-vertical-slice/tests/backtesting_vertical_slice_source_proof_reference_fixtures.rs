use std::{collections::BTreeSet, fs, path::PathBuf};

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

fn assert_no_heavy_payloads(path: &PathBuf, value: &Value) {
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

fn assert_unselected_official_free_candidate(path: &PathBuf, report: &SourceProofReport) {
    assert_eq!(
        report.status,
        SourceProofStatus::Pending,
        "fixture report {path:?} must remain pending before provider selection"
    );
    assert_eq!(
        report.source_candidate_class,
        SourceCandidateClass::OfficialFree,
        "fixture report {path:?} must record official/free candidates before paid/vendor or forward-capture paths"
    );
    assert_eq!(
        report.source_selection_status,
        SourceSelectionStatus::PendingMoreProof,
        "fixture report {path:?} must require more proof before provider selection"
    );
    assert!(
        report.acceptance_mode.is_none()
            && report.accepted_by.is_none()
            && report.accepted_at.is_none(),
        "pending fixture report {path:?} must not carry acceptance provenance"
    );
    assert!(
        report.acceptance_scope.is_none(),
        "pending fixture report {path:?} must not carry accepted object scope"
    );
    assert!(
        report
            .forbidden_claims
            .iter()
            .any(|claim| claim.contains("NT catalog") && claim.contains("backtest")),
        "fixture report {path:?} must forbid canonical NT catalog/backtest use"
    );
    assert!(
        report.claim_limits.iter().any(|limit| {
            limit.claim.contains("NT catalog") && limit.claim.contains("backtest")
        }),
        "fixture report {path:?} must bind the catalog/backtest ban to a claim-limit record"
    );
}

fn assert_kimchi_premium_component_shape(path: &PathBuf, report: &SourceProofReport) {
    let roles = report
        .cross_market_components
        .iter()
        .map(|component| component.role.as_str())
        .collect::<BTreeSet<_>>();
    let expected_roles = BTreeSet::from([
        "korean_spot",
        "reference_price",
        "fx_quote",
        "token_mapping",
    ]);
    assert_eq!(
        roles, expected_roles,
        "kimchi-premium fixture report {path:?} must carry point-in-time component source-proof roles"
    );
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
        assert_unselected_official_free_candidate(path, report);
        assert_no_heavy_payloads(path, value);
    }
}

#[test]
fn reference_fixtures_include_unselected_perps_spot_source_proof() {
    let reports = reference_source_proof_reports();
    let perps_spot_reports: Vec<_> = reports
        .iter()
        .filter(|(_, _, report)| report.fixture_type == FixtureType::PerpsSpot)
        .collect();
    assert!(
        !perps_spot_reports.is_empty(),
        "perps/spot fixture needs a SourceProofReport before provider selection"
    );

    for (path, value, report) in perps_spot_reports {
        assert_unselected_official_free_candidate(path, report);
        assert_no_heavy_payloads(path, value);
        if report.product_category == "kimchi-premium" {
            assert_kimchi_premium_component_shape(path, report);
        } else {
            assert!(
                report.cross_market_components.is_empty(),
                "non-kimchi perps/spot fixture report {path:?} must not carry cross-market component joins"
            );
        }
    }
}
