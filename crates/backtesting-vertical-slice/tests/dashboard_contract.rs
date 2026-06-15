use backtesting_vertical_slice::dashboard_contract::{
    ArtifactIndexBulkListSource, DashboardAction, DashboardArtifactLink,
    DashboardArtifactLinkScope, DashboardFieldGroup, DashboardFieldSource,
    DashboardFreshnessEvidence, DashboardMutationKind, DashboardProductGate,
    DashboardProductGateOutcome, DashboardReadModelSpec, DashboardSourceBinding,
    DashboardSourceRef, DataStatus, FidelityClass, GapReason, RaVerdictSource, RunPurpose,
    SourceRole, validate_dashboard_read_model,
};

fn binding(key: &str, venue_key: &str, provider_key: &str) -> DashboardSourceBinding {
    DashboardSourceBinding {
        source_binding_key: key.to_string(),
        venue_key: venue_key.to_string(),
        provider_key: provider_key.to_string(),
    }
}

fn current_freshness() -> DashboardFreshnessEvidence {
    DashboardFreshnessEvidence {
        source_timestamp_epoch_seconds: 100,
        observed_at_epoch_seconds: 110,
        stale_after_seconds: 30,
    }
}

fn field(
    field_key: &str,
    group: DashboardFieldGroup,
    source_ref: DashboardSourceRef,
) -> DashboardFieldSource {
    DashboardFieldSource {
        field_key: field_key.to_string(),
        group,
        source_ref,
        source_binding_key: Some("primary-source-binding".to_string()),
        source_role: SourceRole::Authoritative,
        data_status: DataStatus::Current,
        gap_reason: None,
        source_proof_id: Some("source-proof-primary-v1".to_string()),
        run_purpose: RunPurpose::Normal,
        fidelity_class: Some(FidelityClass::L2Replay),
        claim_limits: vec!["queue-position-unproven".to_string()],
        warning_fields: Vec::new(),
        freshness: Some(current_freshness()),
        accepted_source_contract_ref: Some("accepted-source-contract:primary:v1".to_string()),
        upgrades_proof_strength: false,
        weakens_forbidden_claims: false,
        relabels_historical_result_after_supersession: false,
        calculates_strategy_truth: false,
    }
}

fn valid_spec() -> DashboardReadModelSpec {
    DashboardReadModelSpec {
        artifact_root: "s3://example-bucket/nt-research-analytics".to_string(),
        source_bindings: vec![
            binding(
                "primary-source-binding",
                "venue-config-key-a",
                "provider-config-key-a",
            ),
            binding(
                "reference-source-binding",
                "venue-config-key-b",
                "provider-config-key-b",
            ),
        ],
        required_field_keys: vec![
            "account_equity".to_string(),
            "historical_pnl".to_string(),
            "strategy_outlook".to_string(),
            "data_health".to_string(),
        ],
        fields: vec![
            field(
                "account_equity",
                DashboardFieldGroup::AccountStateAndPortfolioEquity,
                DashboardSourceRef::PortfolioSnapshot {
                    snapshot_id: "portfolio-snapshot-primary".to_string(),
                },
            ),
            field(
                "historical_pnl",
                DashboardFieldGroup::HistoricalPnl,
                DashboardSourceRef::DurableTradeHistoryPnl {
                    artifact_uri: "s3://example-bucket/nt-research-analytics/backtests/v1/run=run-1/history-pnl.parquet"
                        .to_string(),
                },
            ),
            {
                let mut outlook = field(
                    "strategy_outlook",
                    DashboardFieldGroup::StrategyStateOutlook,
                    DashboardSourceRef::AcceptedAnalytics {
                        artifact_uri:
                            "s3://example-bucket/nt-research-analytics/research-analytics/v1/experiment-results/outlook.parquet"
                                .to_string(),
                    },
                );
                outlook.source_role = SourceRole::Exploratory;
                outlook.fidelity_class = Some(FidelityClass::SignalOnly);
                outlook
            },
            field(
                "data_health",
                DashboardFieldGroup::DataHealthFreshness,
                DashboardSourceRef::NtSnapshot {
                    snapshot_uri:
                        "s3://example-bucket/nt-research-analytics/backtests/v1/run=run-1/nt-snapshot.json"
                            .to_string(),
                },
            ),
        ],
        artifact_links: vec![DashboardArtifactLink {
            link_key: "result-contract".to_string(),
            uri: "s3://example-bucket/nt-research-analytics/backtests/v1/run=run-1/result-contract.json"
                .to_string(),
            scope: DashboardArtifactLinkScope::ArtifactRootUri,
        }],
        artifact_index_bulk_list_source: ArtifactIndexBulkListSource::CommittedSnapshot {
            snapshot_uri:
                "s3://example-bucket/nt-research-analytics/artifact-index/v1/snapshots/kind=backtests/snapshot.json"
                    .to_string(),
            manifest_lineage_ids: vec!["run-1".to_string()],
        },
        actions: vec![DashboardAction::ReadOnlyView {
            action_key: "open-result-contract".to_string(),
        }],
        product_gate: DashboardProductGate {
            outcome: DashboardProductGateOutcome::SelectedExistingProduct,
            selected_product_ref: "dashboard-product-gate:metabase:v1".to_string(),
            rejected_product_refs: Vec::new(),
            no_mutation_controls_ref: "dashboard-no-mutation-controls:metabase:v1".to_string(),
            annotation_writes_enabled: false,
            annotation_owner_schema_audit_ref: None,
        },
        ra_verdict_source: RaVerdictSource::DisplayOnly {
            verdict_artifact_ref: "ra-verdict:experiment-result:v1".to_string(),
        },
        redemption_realized_pnl_included: false,
    }
}

#[test]
fn dashboard_read_model_accepts_read_only_sources_with_config_binding_keys() {
    validate_dashboard_read_model(&valid_spec()).expect("dashboard read model should validate");
}

#[test]
fn dashboard_rejects_source_reclassification_and_ra_verdict_derivation() {
    let mut spec = valid_spec();
    spec.fields[0].upgrades_proof_strength = true;
    let err = validate_dashboard_read_model(&spec).expect_err("dashboard cannot upgrade proof");
    assert!(err.to_string().contains("proof-strength"), "{err}");

    let mut spec = valid_spec();
    spec.fields[0].weakens_forbidden_claims = true;
    let err =
        validate_dashboard_read_model(&spec).expect_err("dashboard cannot weaken claim limits");
    assert!(err.to_string().contains("forbidden"), "{err}");

    let mut spec = valid_spec();
    spec.fields[1].relabels_historical_result_after_supersession = true;
    let err = validate_dashboard_read_model(&spec).expect_err("dashboard cannot relabel history");
    assert!(err.to_string().contains("historical"), "{err}");

    let mut spec = valid_spec();
    spec.ra_verdict_source = RaVerdictSource::DerivedFromBteMetrics;
    let err = validate_dashboard_read_model(&spec).expect_err("dashboard cannot derive RA verdict");
    assert!(err.to_string().contains("RA verdict"), "{err}");

    let mut spec = valid_spec();
    spec.ra_verdict_source = RaVerdictSource::MutatesFindingReviewArtifact;
    let err = validate_dashboard_read_model(&spec).expect_err("dashboard cannot mutate RA verdict");
    assert!(err.to_string().contains("RA finding"), "{err}");
}

#[test]
fn dashboard_field_source_resolution_uses_source_binding_key_not_venue_or_provider() {
    let mut venue_literal = valid_spec();
    venue_literal.fields[0].source_binding_key = Some("venue-config-key-a".to_string());
    let err = validate_dashboard_read_model(&venue_literal)
        .expect_err("field must not resolve through venue key");
    assert!(err.to_string().contains("source_binding_key"), "{err}");

    let mut provider_literal = valid_spec();
    provider_literal.fields[0].source_binding_key = Some("provider-config-key-a".to_string());
    let err = validate_dashboard_read_model(&provider_literal)
        .expect_err("field must not resolve through provider key");
    assert!(err.to_string().contains("source_binding_key"), "{err}");
}

#[test]
fn dashboard_artifact_links_and_index_reads_stay_under_artifact_root() {
    let mut outside_root = valid_spec();
    outside_root.artifact_links[0].uri =
        "s3://other-bucket/nt-research-analytics/backtests/v1/run=run-1/result-contract.json"
            .to_string();
    let err = validate_dashboard_read_model(&outside_root)
        .expect_err("dashboard links must stay under artifact root");
    assert!(err.to_string().contains("artifact_root"), "{err}");

    let mut direct_handle = valid_spec();
    direct_handle.artifact_links[0] = DashboardArtifactLink {
        link_key: "upstream-direct".to_string(),
        uri: "artifact://upstream-handoff/backtest-result-contract/run-1".to_string(),
        scope: DashboardArtifactLinkScope::DirectUpstreamHandle,
    };
    validate_dashboard_read_model(&direct_handle).expect("explicit upstream handle is allowed");

    let mut latest_pointer_join = valid_spec();
    latest_pointer_join.artifact_index_bulk_list_source =
        ArtifactIndexBulkListSource::IndependentLatestPointers {
            pointer_uris: vec![
                "s3://example-bucket/nt-research-analytics/artifact-index/v1/pointers/kind=backtests/latest.json"
                    .to_string(),
                "s3://example-bucket/nt-research-analytics/artifact-index/v1/pointers/kind=source-proofs/latest.json"
                    .to_string(),
            ],
        };
    let err = validate_dashboard_read_model(&latest_pointer_join)
        .expect_err("dashboard bulk lists must use committed snapshots");
    assert!(err.to_string().contains("committed snapshot"), "{err}");
}

#[test]
fn dashboard_rejects_mutation_actions_and_canonical_artifact_writes() {
    for mutation in [
        DashboardMutationKind::SubmitOrder,
        DashboardMutationKind::CancelOrder,
        DashboardMutationKind::TransferFunds,
        DashboardMutationKind::MutateCredential,
        DashboardMutationKind::MutateRuntimeConfig,
        DashboardMutationKind::DeleteCanonicalArtifact,
        DashboardMutationKind::ExpireCanonicalArtifact,
        DashboardMutationKind::PublishArtifactIndexRecord,
        DashboardMutationKind::RepairArtifactIndexRecord,
        DashboardMutationKind::MutateAcceptedSourceProof,
        DashboardMutationKind::MutateRaFindingVerdict,
    ] {
        let mut spec = valid_spec();
        spec.actions.push(DashboardAction::Mutation {
            action_key: format!("mutation-{mutation:?}"),
            mutation,
        });
        let err = validate_dashboard_read_model(&spec).expect_err("mutation action rejected");
        assert!(err.to_string().contains("read-only"), "{err}");
    }
}

#[test]
fn dashboard_rejects_unmapped_stale_missing_pnl_and_strategy_truth_fields() {
    let mut missing_required = valid_spec();
    missing_required
        .fields
        .retain(|field| field.field_key != "historical_pnl");
    let err =
        validate_dashboard_read_model(&missing_required).expect_err("required field is unmapped");
    assert!(err.to_string().contains("unmapped"), "{err}");

    let mut stale_as_current = valid_spec();
    stale_as_current.fields[0].freshness = Some(DashboardFreshnessEvidence {
        source_timestamp_epoch_seconds: 100,
        observed_at_epoch_seconds: 200,
        stale_after_seconds: 30,
    });
    let err =
        validate_dashboard_read_model(&stale_as_current).expect_err("stale data cannot be current");
    assert!(err.to_string().contains("stale"), "{err}");

    let mut missing_pnl_gap = valid_spec();
    missing_pnl_gap.fields[0].source_ref = DashboardSourceRef::AcceptedAnalytics {
        artifact_uri:
            "s3://example-bucket/nt-research-analytics/research-analytics/accepted-equity.json"
                .to_string(),
    };
    missing_pnl_gap.fields[0].data_status = DataStatus::Current;
    missing_pnl_gap.fields[0].gap_reason = None;
    let err = validate_dashboard_read_model(&missing_pnl_gap)
        .expect_err("missing PnL source needs explicit gap label");
    assert!(err.to_string().contains("PortfolioSnapshot"), "{err}");

    let mut accepted_gap = valid_spec();
    accepted_gap.fields[0].source_ref = DashboardSourceRef::GapLabel {
        gap_key: "portfolio-snapshot-missing".to_string(),
    };
    accepted_gap.fields[0].data_status = DataStatus::Unavailable;
    accepted_gap.fields[0].gap_reason = Some(GapReason::UpstreamBlocked);
    accepted_gap.fields[0].source_role = SourceRole::Derived;
    validate_dashboard_read_model(&accepted_gap).expect("missing PnL with gap label is allowed");

    let mut strategy_truth = valid_spec();
    strategy_truth.fields[2].source_role = SourceRole::Authoritative;
    strategy_truth.fields[2].accepted_source_contract_ref = None;
    strategy_truth.fields[2].calculates_strategy_truth = true;
    let err = validate_dashboard_read_model(&strategy_truth)
        .expect_err("dashboard cannot calculate strategy truth");
    assert!(err.to_string().contains("strategy"), "{err}");
}
