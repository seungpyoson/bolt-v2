use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::tempdir_in_repo_target;

use backtesting_vertical_slice::source_proof::{
    AcceptanceMode, AcceptanceScope, CONTRACT_VERSION, EvidenceState, FixtureType,
    L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks,
    SOURCE_PROOF_SCHEMA_VERSION, SourceCandidateClass, SourceProofClaimLimit,
    SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
    SourceSelectionStatus, TimeRange,
};
use backtesting_vertical_slice::source_universe_object_gates::{
    SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateStatus,
    write_source_universe_object_gate_materialization_from_spec_file,
};
use serde_json::json;

#[test]
fn source_universe_object_gates_cover_every_bybit_queue_item() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("object-gates");
    let spec_path = temp_dir.path().join("source-universe-object-gates.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
gate_id = "source-universe-object-gates-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
queue_path = "{queue_path}"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"

[[source_binding]]
source_binding = "bybit-spot-tick-trades"
source_proof_path = "{spot_proof}"
category_manifest_path = "{spot_manifest}"

[[source_binding]]
source_binding = "bybit-linear-tick-trades"
source_proof_path = "{linear_proof}"
category_manifest_path = "{linear_manifest}"

[[source_binding]]
source_binding = "bybit-inverse-tick-trades"
source_proof_path = "{inverse_proof}"
category_manifest_path = "{inverse_manifest}"
"#,
            output_dir = output_dir.display(),
            source_bindings_path = reference_root
                .join("backfill-source-bindings.v1.toml")
                .display(),
            queue_path = reference_root
                .join("source-universe-conversion-queues/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/queue/source-universe-conversion-queue.json")
                .display(),
            spot_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-spot-public-archive-tick-trades.json")
                .display(),
            spot_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-spot.json")
                .display(),
            linear_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-linear-public-archive-tick-trades.json")
                .display(),
            linear_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-linear.json")
                .display(),
            inverse_proof = reference_root
                .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-proof-bybit-inverse-public-archive-tick-trades.json")
                .display(),
            inverse_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests/bybit-public-archive-tick-trades-object-manifest-inverse.json")
                .display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_object_gate_materialization_from_spec_file(&spec_path)
        .expect("object gates generate");
    let gates: SourceUniverseObjectGateMaterialization =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read gates"))
            .expect("parse gates");

    assert_eq!(gates.status, SourceUniverseObjectGateStatus::Ready);
    assert_eq!(
        gates.queue_id,
        "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(gates.work_item_count, 5_857);
    assert_eq!(gates.accepted_gate_count, 5_857);
    assert_eq!(gates.total_accepted_bytes, 20_309_079_098);
    assert_eq!(gates.source_binding_count, 3);
    assert_eq!(gates.source_binding_summaries.len(), 3);

    let inverse = gates
        .source_binding_summaries
        .iter()
        .find(|summary| summary.source_binding == "bybit-inverse-tick-trades")
        .expect("inverse summary");
    assert_eq!(inverse.work_item_count, 702);
    assert_eq!(inverse.accepted_bytes, 624_992_483);
    assert_eq!(
        inverse.source_proof_id,
        "source-proof-bybit-inverse-public-archive-tick-trades-2025-06-01-2026-06-01"
    );

    let first = gates.records.first().expect("first gate record");
    assert_eq!(first.source_binding, "bybit-inverse-tick-trades");
    assert_eq!(first.category, "inverse");
    assert_eq!(first.symbol, "AAVEUSD");
    assert_eq!(first.archive_date, "2025-06-01");
    assert_eq!(
        first.selected_object_sha256,
        "0c92b646ffca8f0621eb36741b3d7382c9212905d781905ff066bfc0b5d72516"
    );
    assert_eq!(first.selected_object_bytes, 124_717);
    assert!(
        first
            .source_proof_scope_report_id
            .contains(first.work_item_id.as_str())
    );
    assert!(
        first
            .accepted_tranche_id
            .contains(first.work_item_id.as_str())
    );
}

#[test]
fn committed_binance_source_universe_object_gates_use_canonical_trade_table_family() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("object-gates");
    let spec_path = temp_dir.path().join("source-universe-object-gates.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
gate_id = "source-universe-object-gates-binance-data-vision-trades-2026-03-01-all-instruments"
queue_path = "{queue_path}"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"

[[source_binding]]
source_binding = "binance-spot-native-trades"
source_proof_path = "{spot_proof}"
category_manifest_path = "{spot_manifest}"

[[source_binding]]
source_binding = "binance-usd-m-perpetual-native-trades"
source_proof_path = "{usd_m_perpetual_proof}"
category_manifest_path = "{usd_m_perpetual_manifest}"

[[source_binding]]
source_binding = "binance-usd-m-delivery-native-trades"
source_proof_path = "{usd_m_delivery_proof}"
category_manifest_path = "{usd_m_delivery_manifest}"

[[source_binding]]
source_binding = "binance-coin-m-perpetual-native-trades"
source_proof_path = "{coin_m_perpetual_proof}"
category_manifest_path = "{coin_m_perpetual_manifest}"

[[source_binding]]
source_binding = "binance-coin-m-delivery-native-trades"
source_proof_path = "{coin_m_delivery_proof}"
category_manifest_path = "{coin_m_delivery_manifest}"
"#,
            output_dir = output_dir.display(),
            source_bindings_path = reference_root
                .join("backfill-source-bindings.v1.toml")
                .display(),
            queue_path = reference_root
                .join("source-universe-conversion-queues/binance-data-vision-trades-2026-03-01-all-instruments/queue/source-universe-conversion-queue.json")
                .display(),
            spot_proof = reference_root
                .join("backfill-source-proofs/binance-data-vision-trades-2026-03-01-all-instruments/source-proof-binance-spot-native-trades-2026-03-01-all-instruments.json")
                .display(),
            spot_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-spot.json")
                .display(),
            usd_m_perpetual_proof = reference_root
                .join("backfill-source-proofs/binance-data-vision-trades-2026-03-01-all-instruments/source-proof-binance-usd-m-perpetual-native-trades-2026-03-01-all-instruments.json")
                .display(),
            usd_m_perpetual_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-usd_m_perpetual.json")
                .display(),
            usd_m_delivery_proof = reference_root
                .join("backfill-source-proofs/binance-data-vision-trades-2026-03-01-all-instruments/source-proof-binance-usd-m-delivery-native-trades-2026-03-01-all-instruments.json")
                .display(),
            usd_m_delivery_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-usd_m_delivery.json")
                .display(),
            coin_m_perpetual_proof = reference_root
                .join("backfill-source-proofs/binance-data-vision-trades-2026-03-01-all-instruments/source-proof-binance-coin-m-perpetual-native-trades-2026-03-01-all-instruments.json")
                .display(),
            coin_m_perpetual_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-coin_m_perpetual.json")
                .display(),
            coin_m_delivery_proof = reference_root
                .join("backfill-source-proofs/binance-data-vision-trades-2026-03-01-all-instruments/source-proof-binance-coin-m-delivery-native-trades-2026-03-01-all-instruments.json")
                .display(),
            coin_m_delivery_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/category-manifests/binance-data-vision-trades-object-manifest-coin_m_delivery.json")
                .display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_object_gate_materialization_from_spec_file(&spec_path)
        .expect("Binance object gates are reproducible");
    let gates: SourceUniverseObjectGateMaterialization =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read Binance object gates"))
            .expect("Binance object gates parse");

    assert_eq!(gates.table_family, "trades");
    assert!(
        gates
            .records
            .iter()
            .all(|record| record.table_family == "trades")
    );
}

#[test]
fn source_universe_object_gates_accept_non_sha_source_hashes_without_faking_payload_sha256() {
    let temp_dir = tempdir_in_repo_target();
    let proof_path = temp_dir.path().join("source-proof.json");
    let category_manifest_path = temp_dir.path().join("category-manifest.json");
    let queue_path = temp_dir
        .path()
        .join("source-universe-conversion-queue.json");
    let output_dir = temp_dir.path().join("object-gates");
    let spec_path = temp_dir.path().join("source-universe-object-gates.toml");

    let source_uri = "s3://bolt-parquet/backfill-staging/pmxt/raw/v1/source=polymarket-v2-archive/family=order_book_snapshot_deltas/category=orderbook/dt=2026-06-10T15:00:00Z/object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70.parquet";
    let source_url = "https://r2v2.pmxt.dev/polymarket_orderbook_2026-06-10T15.parquet";
    let source_hash_algorithm = "r2_multipart_etag";
    let source_hash = "\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"";
    let source_bytes = 586_780_173_u64;

    fs::write(
        &proof_path,
        serde_json::to_vec_pretty(&accepted_non_sha_source_proof(
            source_uri,
            source_hash,
            source_bytes,
        ))
        .expect("serialize proof"),
    )
    .expect("write proof");
    fs::write(
        &category_manifest_path,
        serde_json::to_vec_pretty(&json!({
            "manifest_id": "category-manifest-non-sha-source-hashes",
            "source_binding": "bybit-spot-tick-trades",
            "object_count": 1,
            "accepted_bytes": source_bytes,
            "payload_records": [{
                "s3_uri": source_uri,
                "source_url": source_url,
                "source_hash_algorithm": source_hash_algorithm,
                "source_hash": source_hash,
                "bytes": source_bytes,
                "archive_date": "2026-03-01",
                "category": "spot",
                "symbol": "BNBUSDC",
                "source_binding": "bybit-spot-tick-trades"
            }]
        }))
        .expect("serialize category manifest"),
    )
    .expect("write category manifest");
    fs::write(
        &queue_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "source-universe-conversion-queue.v1",
            "queue_id": "source-universe-conversion-queue-non-sha-source-hashes",
            "status": "ready",
            "manifest_id": "source-universe-manifest-non-sha-source-hashes",
            "universe_id": "source-universe-non-sha-source-hashes",
            "venue": "pmxt",
            "source": "polymarket-v2-archive",
            "family": "order_book_snapshot_deltas",
            "table_family": "trades",
            "source_manifest_path": "manifest.json",
            "source_manifest_hash": "manifest-hash",
            "output_prefix_template": "source-universe={universe_id}/category={category}/object={source_hash}",
            "work_item_count": 1,
            "pending_conversion_items": 1,
            "total_source_bytes": source_bytes,
            "category_summaries": [{
                "category": "spot",
                "source_binding": "bybit-spot-tick-trades",
                "instrument_count": 1,
                "work_item_count": 1,
                "source_bytes": source_bytes,
                "first_archive_date": "2026-03-01",
                "last_archive_date": "2026-03-01"
            }],
            "artifact_refs": [],
            "work_items": [{
                "work_item_id": "bybit-spot-tick-trades:BNBUSDC:2026-03-01:\"9b8839adc79af4b1c8fd607cf5cc8f97-70\"",
                "work_state": "pending_conversion",
                "source_binding": "bybit-spot-tick-trades",
                "table_family": "trades",
                "category": "spot",
                "symbol": "BNBUSDC",
                "archive_date": "2026-03-01",
                "source_uri": source_uri,
                "source_url": source_url,
                "source_hash_algorithm": source_hash_algorithm,
                "source_hash": source_hash,
                "source_bytes": source_bytes,
                "schema_columns": ["timestamp", "market", "asset_id", "bids", "asks"],
                "output_prefix": "source-universe=source-universe-non-sha-source-hashes/category=spot/object=etag-9b8839adc79af4b1c8fd607cf5cc8f97-70"
            }]
        }))
        .expect("serialize queue"),
    )
    .expect("write queue");
    fs::write(
        &spec_path,
        format!(
            r#"
gate_id = "source-universe-object-gates-non-sha-source-hashes"
queue_path = "{queue_path}"
output_dir = "{output_dir}"
source_bindings_path = "{source_bindings_path}"

[[source_binding]]
source_binding = "bybit-spot-tick-trades"
source_proof_path = "{proof_path}"
category_manifest_path = "{category_manifest_path}"
"#,
            queue_path = queue_path.display(),
            output_dir = output_dir.display(),
            source_bindings_path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml")
                .display(),
            proof_path = proof_path.display(),
            category_manifest_path = category_manifest_path.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_object_gate_materialization_from_spec_file(&spec_path)
        .expect("object gates generate for non-SHA source hash");
    let gates: SourceUniverseObjectGateMaterialization =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read gates"))
            .expect("parse gates");

    let record = gates.records.first().expect("gate record");
    assert_eq!(record.selected_object_hash_algorithm, source_hash_algorithm);
    assert_eq!(record.selected_object_hash, source_hash);
    assert_eq!(record.selected_object_sha256, "");
    assert_eq!(record.selected_object_bytes, source_bytes);
    assert_eq!(gates.total_accepted_bytes, source_bytes);
}

fn accepted_non_sha_source_proof(
    raw_sample_uri: &str,
    raw_sample_hash: &str,
    accepted_bytes: u64,
) -> SourceProofReport {
    let forbidden_claims =
        vec!["No execution-quality, queue-position, or order-book-liquidity claims.".to_string()];
    SourceProofReport {
        source_proof_id: "source-proof-non-sha-source-hashes".to_string(),
        source_proof_version: 1,
        contract_version: CONTRACT_VERSION.to_string(),
        schema_version: SOURCE_PROOF_SCHEMA_VERSION.to_string(),
        status: SourceProofStatus::Accepted,
        source_binding: "bybit-spot-tick-trades".to_string(),
        venue: "bybit".to_string(),
        product_family: "spot".to_string(),
        product_category: "spot".to_string(),
        table_family: "trades".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        source_candidate_class: SourceCandidateClass::OfficialFree,
        source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        official_free_gap_ref: None,
        paid_vendor_gap_ref: None,
        fixture_type: FixtureType::PerpsSpot,
        requested_time_range: TimeRange {
            start_utc: "2026-03-01T00:00:00Z".to_string(),
            end_utc: "2026-03-02T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-03-01T00:00:00Z".to_string(),
            end_utc: "2026-03-02T00:00:00Z".to_string(),
        },
        instrument_universe_id: "bybit-spot-instruments-non-sha-source-hashes".to_string(),
        raw_sample_uri: raw_sample_uri.to_string(),
        raw_sample_hash: raw_sample_hash.to_string(),
        schema_sample_uri:
            "s3://bolt-parquet/backfill-staging/pmxt/source-proofs/non-sha-schema.json".to_string(),
        schema_sample_hash: "schema-sample-hash".to_string(),
        license_ref: "https://public.bybit.com/ (attestation)".to_string(),
        license_scope: LicenseScope::Public,
        retention_ref: "https://public.bybit.com/ (archive retention reviewed)".to_string(),
        cost_ref: "cost://free-public-archive".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        l2_replay_evidence: L2ReplayEvidence {
            order_book_delta_ref: None,
            sufficient_snapshot_cadence_ref: None,
            no_tick_size_change_universe_ref: None,
            timed_instrument_epoch_replay_ref: None,
        },
        forbidden_claims: forbidden_claims.clone(),
        claim_limits: claim_limits_for(&forbidden_claims),
        cross_market_components: Vec::new(),
        acceptance_scope: Some(AcceptanceScope {
            planned_objects: 1,
            completed_objects: 1,
            failed_objects: 0,
            skipped_objects: 0,
            accepted_bytes,
            selector_scope_violations: 0,
        }),
        gap_policy_id: String::new(),
        required_checks: passing_checks(),
        acceptance_mode: Some(AcceptanceMode::Manual),
        accepted_by: Some("venue-scale-conversion-test".to_string()),
        accepted_at: Some("2026-06-10T00:00:00Z".to_string()),
        supersedes_source_proof_id: None,
    }
}

fn passing_checks() -> RequiredChecks {
    let evidence = "source-proof://non-sha-source-hashes";
    RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed(evidence),
        schema: RequiredCheck::passed(evidence),
        time_semantics: RequiredCheck::passed(evidence),
        instrument_universe: RequiredCheck::passed(evidence),
        coverage: RequiredCheck::passed(evidence),
        retention_freshness: RequiredCheck::passed(evidence),
        granularity: RequiredCheck::passed(evidence),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed(evidence),
        cost: RequiredCheck::passed(evidence),
        storage: RequiredCheck::passed(evidence),
    }
}

fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
    claims
        .iter()
        .enumerate()
        .map(|(index, claim)| SourceProofClaimLimit {
            id: format!("claim-limit-{}", index + 1),
            severity: "blocking".to_string(),
            claim: claim.clone(),
            reason: "source fidelity does not prove this claim".to_string(),
            evidence_ref: "source-proof://non-sha-source-hashes".to_string(),
        })
        .collect()
}
