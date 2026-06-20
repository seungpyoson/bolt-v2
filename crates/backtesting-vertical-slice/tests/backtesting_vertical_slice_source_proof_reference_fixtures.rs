use std::{collections::BTreeSet, fs, path::PathBuf};

use backtesting_vertical_slice::source_proof::{
    CheckOutcome, FixtureType, NtMappingStatus, SourceCandidateClass, SourceProofFidelityClass,
    SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
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
    if report.usage_scope == SourceProofUsageScope::OneOffBackfillData {
        let acceptance_scope = report.acceptance_scope.as_ref().unwrap_or_else(|| {
            panic!("one-off fixture report {path:?} must carry bounded source scope")
        });
        assert_eq!(
            acceptance_scope.planned_objects, 1,
            "one-off fixture report {path:?} must bind exactly one planned source object"
        );
        assert_eq!(
            acceptance_scope.completed_objects, 1,
            "one-off fixture report {path:?} must bind exactly one completed source object"
        );
        assert_eq!(
            acceptance_scope.failed_objects, 0,
            "one-off fixture report {path:?} must not carry failed source objects"
        );
        assert_eq!(
            acceptance_scope.skipped_objects, 0,
            "one-off fixture report {path:?} must not carry skipped source objects"
        );
        assert!(
            acceptance_scope.accepted_bytes > 0,
            "one-off fixture report {path:?} must bind non-empty accepted source bytes"
        );
        assert_eq!(
            acceptance_scope.selector_scope_violations, 0,
            "one-off fixture report {path:?} must not carry selector scope violations"
        );
    } else {
        assert!(
            report.acceptance_scope.is_none(),
            "pending fixture report {path:?} must not carry accepted object scope outside one-off backfill evidence"
        );
    }
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

fn assert_sample_evidence_is_inspected(path: &PathBuf, report: &SourceProofReport) {
    for (field_name, value) in [
        ("raw_sample_uri", report.raw_sample_uri.as_str()),
        ("raw_sample_hash", report.raw_sample_hash.as_str()),
        ("schema_sample_uri", report.schema_sample_uri.as_str()),
        ("schema_sample_hash", report.schema_sample_hash.as_str()),
    ] {
        let trimmed = value.trim();
        assert!(
            !trimmed.is_empty() && !trimmed.starts_with("pending"),
            "fixture report {path:?} must bind inspected sample evidence in {field_name}"
        );
    }
    assert_eq!(
        report.required_checks.source_access.outcome,
        CheckOutcome::Passed,
        "fixture report {path:?} must pass source-access sample inspection before provider selection"
    );
    assert_eq!(
        report.required_checks.schema.outcome,
        CheckOutcome::Passed,
        "fixture report {path:?} must pass schema sample inspection before provider selection"
    );
}

fn assert_one_off_usage_scope_is_bounded(path: &PathBuf, report: &SourceProofReport) {
    if report.usage_scope != SourceProofUsageScope::OneOffBackfillData {
        return;
    }

    assert_eq!(
        report.status,
        SourceProofStatus::Pending,
        "one-off fixture report {path:?} must remain pending and outside canonical acceptance"
    );
    assert!(
        report.forbidden_claims.iter().any(|claim| {
            claim.contains("canonical") || claim.contains("broad") || claim.contains("production")
        }),
        "one-off fixture report {path:?} must forbid broad/canonical promotion"
    );
    assert!(
        report.claim_limits.iter().any(|limit| {
            limit.claim.contains("canonical")
                || limit.claim.contains("broad")
                || limit.claim.contains("production")
        }),
        "one-off fixture report {path:?} must bind broad/canonical limits to a claim-limit record"
    );
}

fn assert_one_off_l2_blocks_dynamic_tick_size_replay(path: &PathBuf, report: &SourceProofReport) {
    if report.usage_scope != SourceProofUsageScope::OneOffBackfillData
        || report.fidelity_class != SourceProofFidelityClass::L2Replay
    {
        return;
    }

    assert!(
        report.forbidden_claims.iter().any(|claim| {
            let claim = claim.to_ascii_lowercase();
            claim.contains("dynamic")
                && claim.contains("tick")
                && claim.contains("size")
                && claim.contains("replay")
        }),
        "one-off L2 fixture report {path:?} must forbid dynamic tick-size replay claims"
    );
    assert!(
        report.claim_limits.iter().any(|limit| {
            let claim = limit.claim.to_ascii_lowercase();
            claim.contains("dynamic")
                && claim.contains("tick")
                && claim.contains("size")
                && claim.contains("replay")
        }),
        "one-off L2 fixture report {path:?} must bind the dynamic tick-size replay ban to a claim-limit record"
    );
}

fn assert_nt_mapping_evidence_is_bounded(path: &PathBuf, report: &SourceProofReport) {
    match (report.table_family.as_str(), report.fidelity_class) {
        (_, SourceProofFidelityClass::L2Replay) => {
            assert!(
                report.l2_replay_evidence.order_book_delta_ref.is_some()
                    || report
                        .l2_replay_evidence
                        .sufficient_snapshot_cadence_ref
                        .is_some(),
                "L2 fixture report {path:?} must bind source replay evidence before provider selection"
            );
            let evidence = report.required_checks.nt_mapping.evidence_ref.as_str();
            match report.nt_mapping_status {
                NtMappingStatus::Accepted => {
                    assert_eq!(
                        report.required_checks.nt_mapping.outcome,
                        CheckOutcome::Passed,
                        "accepted L2 fixture report {path:?} must pass NT mapping"
                    );
                    assert!(
                        evidence.contains("OrderBookDelta")
                            || evidence.contains("OrderBookDepth10"),
                        "accepted L2 fixture report {path:?} must bind NT order-book replay mapping evidence"
                    );
                    assert!(
                        evidence.contains("ParquetDataCatalog"),
                        "accepted L2 fixture report {path:?} must bind NT catalog readback evidence"
                    );
                    let has_tick_size_policy_evidence = report
                        .l2_replay_evidence
                        .no_tick_size_change_universe_ref
                        .as_ref()
                        .is_some_and(|value| !value.trim().is_empty())
                        || report
                            .l2_replay_evidence
                            .timed_instrument_epoch_replay_ref
                            .as_ref()
                            .is_some_and(|value| !value.trim().is_empty());
                    assert!(
                        has_tick_size_policy_evidence,
                        "accepted L2 fixture report {path:?} must bind source-proof tick-size policy evidence"
                    );
                }
                NtMappingStatus::Pending => {
                    assert_eq!(
                        report.required_checks.nt_mapping.outcome,
                        CheckOutcome::Pending,
                        "pending L2 fixture report {path:?} must keep NT mapping pending"
                    );
                    assert!(
                        evidence.starts_with("repo://"),
                        "pending L2 fixture report {path:?} must bind a committed NT mapping inspection"
                    );
                    assert!(
                        report.claim_limits.iter().any(|limit| {
                            limit.reason.contains("source-backed")
                                && limit.reason.contains("BinaryOption")
                        }),
                        "pending L2 fixture report {path:?} must block placeholder instrument mappings"
                    );
                }
                other => panic!(
                    "L2 fixture report {path:?} must have accepted or pending NT mapping status, got {other:?}"
                ),
            }
        }
        ("trades", SourceProofFidelityClass::TradeReplay) => {
            assert_eq!(
                report.nt_mapping_status,
                NtMappingStatus::Accepted,
                "trade-replay fixture report {path:?} must carry accepted NT catalog mapping evidence"
            );
            assert_eq!(
                report.required_checks.nt_mapping.outcome,
                CheckOutcome::Passed,
                "trade-replay fixture report {path:?} must pass NT mapping before provider selection"
            );
            let evidence = report.required_checks.nt_mapping.evidence_ref.as_str();
            assert!(
                evidence.contains("TradeTick") && evidence.contains("ParquetDataCatalog"),
                "trade-replay fixture report {path:?} must bind mapping evidence to NT TradeTick catalog readback"
            );
        }
        ("funding_rates", SourceProofFidelityClass::FundingReplay) => {
            assert_eq!(
                report.nt_mapping_status,
                NtMappingStatus::Accepted,
                "funding-replay fixture report {path:?} must carry accepted NT catalog mapping evidence"
            );
            assert_eq!(
                report.required_checks.nt_mapping.outcome,
                CheckOutcome::Passed,
                "funding-replay fixture report {path:?} must pass NT mapping before provider selection"
            );
            let evidence = report.required_checks.nt_mapping.evidence_ref.as_str();
            assert!(
                evidence.contains("FundingRateUpdate") && evidence.contains("ParquetDataCatalog"),
                "funding-replay fixture report {path:?} must bind mapping evidence to NT FundingRateUpdate catalog readback"
            );
        }
        (_, SourceProofFidelityClass::TradeBarReplay) => {
            // Bar-replay sources (e.g. Kalshi official historical candlesticks) must carry
            // a committed NT mapping inspection reference, not a free-form "pending" string.
            // Accepted mappings must pass; pending mappings must point at a committed repo
            // artifact so the gap is tracked, not silently accepted.
            let evidence = report.required_checks.nt_mapping.evidence_ref.as_str();
            match report.nt_mapping_status {
                NtMappingStatus::Accepted => {
                    assert_eq!(
                        report.required_checks.nt_mapping.outcome,
                        CheckOutcome::Passed,
                        "accepted bar-replay fixture report {path:?} must pass NT mapping"
                    );
                    assert!(
                        evidence.contains("Bar") || evidence.contains("TradeTick"),
                        "accepted bar-replay fixture report {path:?} must bind NT Bar or TradeTick mapping evidence"
                    );
                    assert!(
                        evidence.contains("ParquetDataCatalog"),
                        "accepted bar-replay fixture report {path:?} must bind NT catalog readback evidence"
                    );
                }
                NtMappingStatus::Pending => {
                    assert_eq!(
                        report.required_checks.nt_mapping.outcome,
                        CheckOutcome::Pending,
                        "pending bar-replay fixture report {path:?} must keep NT mapping pending"
                    );
                    assert!(
                        evidence.starts_with("repo://"),
                        "pending bar-replay fixture report {path:?} must bind a committed NT mapping \
                         inspection (evidence_ref must start with \"repo://\"); \
                         got: {evidence:?}"
                    );
                }
                other => panic!(
                    "bar-replay fixture report {path:?} must have accepted or pending NT mapping \
                     status, got {other:?}"
                ),
            }
        }
        (_, SourceProofFidelityClass::MetadataOnly) => {
            assert_eq!(
                report.nt_mapping_status,
                NtMappingStatus::NotApplicable,
                "metadata-only fixture report {path:?} must not claim a replay catalog mapping"
            );
            assert_eq!(
                report.required_checks.nt_mapping.outcome,
                CheckOutcome::NotApplicable,
                "metadata-only fixture report {path:?} must mark NT replay mapping as not applicable"
            );
            assert!(
                report
                    .required_checks
                    .nt_mapping
                    .evidence_ref
                    .starts_with("repo://"),
                "metadata-only fixture report {path:?} must bind a committed NT mapping inspection"
            );
            assert!(
                report.forbidden_claims.iter().any(|claim| {
                    claim.contains("NT catalog")
                        || claim.contains("BinaryOption mapping")
                        || claim.contains("backtest")
                }),
                "metadata-only fixture report {path:?} must carry a no-overclaim mapping guard"
            );
        }
        (table_family, fidelity_class) => {
            panic!(
                "fixture report {path:?} has unhandled (table_family, fidelity_class) combination \
                 ({table_family:?}, {fidelity_class:?}); add an explicit arm to \
                 assert_nt_mapping_evidence_is_bounded"
            );
        }
    }
}

// Exercises the funding source-proof routing arm with a SYNTHETIC, in-memory-mutated report
// because NO committed `funding_rates` source-proof fixture exists yet (the four committed
// fixtures cover bars/prediction_market_outcomes/order_book_snapshot_deltas/trades). This
// validates the arm's LOGIC, not any real committed funding artifact; a real committed funding
// source-proof fixture is deferred to #836/#437 (funding raw acquisition). The companion
// negative test (`funding_replay_fixture_routing_arm_rejects_unbound_evidence`, just below) is
// the differential guard that proves this arm is load-bearing.
#[test]
fn funding_replay_fixture_routing_arm_is_exercised() {
    let (path, _, mut report) = reference_source_proof_reports()
        .into_iter()
        .find(|(_, _, report)| {
            report.table_family == "trades"
                && report.fidelity_class == SourceProofFidelityClass::TradeReplay
        })
        .expect("trade replay fixture available as a thin accepted report template");
    report.table_family = "funding_rates".to_string();
    report.fidelity_class = SourceProofFidelityClass::FundingReplay;
    report.nt_mapping_status = NtMappingStatus::Accepted;
    report.required_checks.nt_mapping.outcome = CheckOutcome::Passed;
    report.required_checks.nt_mapping.evidence_ref =
        "repo://synthetic FundingRateUpdate ParquetDataCatalog readback".to_string();

    assert_nt_mapping_evidence_is_bounded(&path, &report);
}

#[test]
#[should_panic(expected = "must bind mapping evidence to NT FundingRateUpdate catalog readback")]
fn funding_replay_fixture_routing_arm_rejects_unbound_evidence() {
    // Fail-closed proof for the funding arm of assert_nt_mapping_evidence_is_bounded: a
    // funding-replay report whose NT-mapping evidence does NOT bind the FundingRateUpdate catalog
    // readback must be REJECTED (panic). Without this negative arm the positive
    // funding_replay_fixture_routing_arm_is_exercised test alone would still pass even if the
    // binding assertion were silently weakened, so this pins the gate as load-bearing.
    let (path, _, mut report) = reference_source_proof_reports()
        .into_iter()
        .find(|(_, _, report)| {
            report.table_family == "trades"
                && report.fidelity_class == SourceProofFidelityClass::TradeReplay
        })
        .expect("trade replay fixture available as a thin accepted report template");
    report.table_family = "funding_rates".to_string();
    report.fidelity_class = SourceProofFidelityClass::FundingReplay;
    report.nt_mapping_status = NtMappingStatus::Accepted;
    report.required_checks.nt_mapping.outcome = CheckOutcome::Passed;
    // Evidence binds a TradeTick readback instead of the required FundingRateUpdate catalog,
    // so the funding arm's binding assertion must fire.
    report.required_checks.nt_mapping.evidence_ref =
        "repo://synthetic TradeTick ParquetDataCatalog readback".to_string();

    assert_nt_mapping_evidence_is_bounded(&path, &report);
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
        assert_one_off_usage_scope_is_bounded(path, report);
        assert_one_off_l2_blocks_dynamic_tick_size_replay(path, report);
        assert_sample_evidence_is_inspected(path, report);
        assert_nt_mapping_evidence_is_bounded(path, report);
        assert_no_heavy_payloads(path, value);
    }
}

#[test]
fn reference_fixtures_exercise_one_off_l2_dynamic_tick_size_replay_ban() {
    // The per-report guard `assert_one_off_l2_blocks_dynamic_tick_size_replay`
    // early-returns unless a fixture is (OneOffBackfillData, L2Replay). If the
    // only matching fixture is ever flipped to another fidelity_class, that guard
    // silently becomes vacuous. This test fails loud when no committed fixture
    // exercises the (one-off, L2-replay) combination at all, and independently
    // re-asserts the dynamic-tick-size-replay ban for every such fixture so the
    // protection cannot decay into a no-op.
    let reports = reference_source_proof_reports();

    let one_off_l2_reports: Vec<_> = reports
        .iter()
        .filter(|(_, _, report)| {
            report.usage_scope == SourceProofUsageScope::OneOffBackfillData
                && report.fidelity_class == SourceProofFidelityClass::L2Replay
        })
        .collect();
    assert!(
        !one_off_l2_reports.is_empty(),
        "at least one committed reference fixture must be (OneOffBackfillData, L2Replay) so the \
         dynamic-tick-size-replay ban guard is exercised and cannot become vacuous"
    );

    for (path, _, report) in one_off_l2_reports {
        assert!(
            report.forbidden_claims.iter().any(|claim| {
                let claim = claim.to_ascii_lowercase();
                claim.contains("dynamic")
                    && claim.contains("tick")
                    && claim.contains("size")
                    && claim.contains("replay")
            }),
            "one-off L2 fixture report {path:?} must forbid dynamic tick-size replay claims \
             in forbidden_claims"
        );
        assert!(
            report.claim_limits.iter().any(|limit| {
                let claim = limit.claim.to_ascii_lowercase();
                claim.contains("dynamic")
                    && claim.contains("tick")
                    && claim.contains("size")
                    && claim.contains("replay")
            }),
            "one-off L2 fixture report {path:?} must bind the dynamic tick-size replay ban \
             to a claim-limit record"
        );
    }
}

#[test]
fn reference_fixtures_include_bounded_one_off_source_proof_scope() {
    let reports = reference_source_proof_reports();

    assert!(
        reports
            .iter()
            .any(|(_, _, report)| report.usage_scope == SourceProofUsageScope::OneOffBackfillData),
        "reference fixtures must include an explicit one-off backfill-data scope before any one-off source is used as evidence"
    );
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
        assert_sample_evidence_is_inspected(path, report);
        assert_nt_mapping_evidence_is_bounded(path, report);
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
