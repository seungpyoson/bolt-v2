use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    hashing::sha256_hex,
    source_catalog_mapping_readiness::{
        SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE, SourceCatalogMappingReadinessBlocker,
        SourceCatalogMappingReadinessInput, SourceCatalogMappingReadinessReport,
        SourceCatalogMappingReadinessStatus, SourceCatalogMappingStatusEntry,
        evaluate_source_catalog_mapping_readiness,
        write_source_catalog_mapping_readiness_report_from_spec_file,
    },
    source_proof::SourceProofUsageScope,
};
use serde::Deserialize;

const BLOCKED_CATALOG_MAPPING_EVALUATION: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const BLOCKED_CATALOG_MAPPING_EVALUATION_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-catalog-mapping-readiness/polymarket-parquet-archive-index-canonical/source-catalog-mapping-readiness-report.json"
);

#[derive(Debug, Deserialize)]
struct SourceCatalogMappingEvaluation {
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

#[test]
fn blocked_canonical_source_catalog_mapping_reference_artifact_matches_generic_evaluator() {
    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(BLOCKED_CATALOG_MAPPING_EVALUATION)
            .expect("mapping evaluation parses");
    let expected: SourceCatalogMappingReadinessReport =
        serde_json::from_str(BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("blocked source catalog-mapping readiness parses");

    let actual = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: &expected.readiness_id,
        catalog_mapping_evaluation_hash: &sha256_hex(BLOCKED_CATALOG_MAPPING_EVALUATION_BYTES),
        source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
        source_proof_id: &expected.source_proof_id,
        source_proof_version: expected.source_proof_version,
        source_binding: &expected.source_binding,
        required_table_family: &expected.required_table_family,
        required_nt_data_types: expected.required_nt_data_types.clone(),
        required_claim_evidence_refs: expected.required_claim_evidence_refs.clone(),
        allowed_current_bte_statuses: expected.allowed_current_bte_statuses.clone(),
        allowed_parquet_catalog_statuses: expected.allowed_parquet_catalog_statuses.clone(),
        allowed_usage_scopes: expected.allowed_usage_scopes.clone(),
    });

    assert_eq!(actual, expected);
    assert_eq!(actual.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(
        actual
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::RequiredClaimEvidenceMissing)
    );
    assert!(!actual.blockers.is_empty());
}

#[test]
fn source_catalog_mapping_readiness_accepts_configured_nt_catalog_statuses() {
    let entries = vec![accepted_mapping_entry()];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-synthetic-native-trades",
        source_proof_version: 1,
        source_binding: "synthetic-native-trades",
        required_table_family: "trades",
        required_nt_data_types: vec!["TradeTick".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Ready);
    assert!(report.blockers.is_empty());
    assert!(report.nt_catalog_mapping_proven);
    assert_eq!(
        report.observed_source_binding.as_deref(),
        Some("synthetic-native-trades")
    );
    assert_eq!(
        report.observed_nt_data_type_evidence_refs.get("TradeTick"),
        Some(&vec![
            "repo://synthetic/trade-tick-catalog-proof.json".to_string()
        ])
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_status_only_mapping_without_data_class_evidence() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-native-trades".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-native-trades".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "trades".to_string(),
        candidate_nt_data_classes: vec!["TradeTick".to_string()],
        nt_data_class_evidence_refs: BTreeMap::new(),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-synthetic-native-trades",
        source_proof_version: 1,
        source_binding: "synthetic-native-trades",
        required_table_family: "trades",
        required_nt_data_types: vec!["TradeTick".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(!report.nt_catalog_mapping_proven);
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::RequiredNtDataTypeEvidenceMissing)
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_unaccepted_mapping_statuses() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-book-deltas".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-book-deltas".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "order_book_snapshot_deltas".to_string(),
        candidate_nt_data_classes: vec!["OrderBookDelta".to_string(), "TradeTick".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "OrderBookDelta",
            "repo://synthetic/order-book-delta-catalog-proof.json",
        )]),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "pending".to_string(),
        parquet_catalog_status: "prototype_only_not_accepted".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-synthetic-book-deltas",
        source_proof_version: 1,
        source_binding: "synthetic-book-deltas",
        required_table_family: "order_book_snapshot_deltas",
        required_nt_data_types: vec!["OrderBookDelta".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(!report.nt_catalog_mapping_proven);
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::CurrentBteStatusNotAllowed)
    );
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::ParquetCatalogStatusNotAllowed)
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_source_proof_mismatches() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-old-version".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-native-trades".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "trades".to_string(),
        candidate_nt_data_classes: vec!["TradeTick".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "TradeTick",
            "repo://synthetic/trade-tick-catalog-proof.json",
        )]),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-current-version",
        source_proof_version: 2,
        source_binding: "synthetic-native-trades",
        required_table_family: "trades",
        required_nt_data_types: vec!["TradeTick".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(!report.nt_catalog_mapping_proven);
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::SourceProofMismatch)
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_one_off_usage_scope_for_canonical_gate() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-one-off-book-deltas".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-book-deltas".to_string(),
        usage_scope: Some(SourceProofUsageScope::OneOffBackfillData),
        table_family: "order_book_snapshot_deltas".to_string(),
        candidate_nt_data_classes: vec!["OrderBookDelta".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "OrderBookDelta",
            "repo://synthetic/order-book-delta-catalog-proof.json",
        )]),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-one-off-book-deltas",
        source_proof_version: 1,
        source_binding: "synthetic-book-deltas",
        required_table_family: "order_book_snapshot_deltas",
        required_nt_data_types: vec!["OrderBookDelta".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(!report.nt_catalog_mapping_proven);
    assert_eq!(
        report.observed_usage_scope,
        Some(SourceProofUsageScope::OneOffBackfillData)
    );
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::UsageScopeNotAllowed)
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_missing_required_claim_evidence() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-book-deltas".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-book-deltas".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "order_book_snapshot_deltas".to_string(),
        candidate_nt_data_classes: vec!["OrderBookDelta".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "OrderBookDelta",
            "repo://synthetic/order-book-delta-catalog-proof.json",
        )]),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-synthetic-book-deltas",
        source_proof_version: 1,
        source_binding: "synthetic-book-deltas",
        required_table_family: "order_book_snapshot_deltas",
        required_nt_data_types: vec!["OrderBookDelta".to_string()],
        required_claim_evidence_refs: vec![
            "repo://synthetic/dynamic-replay-policy.json".to_string(),
        ],
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Blocked);
    assert!(!report.nt_catalog_mapping_proven);
    assert!(
        report
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::RequiredClaimEvidenceMissing)
    );
}

#[test]
fn source_catalog_mapping_readiness_accepts_required_claim_evidence_from_mapping() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-book-deltas".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-book-deltas".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "order_book_snapshot_deltas".to_string(),
        candidate_nt_data_classes: vec!["OrderBookDelta".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "OrderBookDelta",
            "repo://synthetic/order-book-delta-catalog-proof.json",
        )]),
        claim_evidence_refs: claim_evidence_refs([(
            "dynamic_replay_policy",
            "repo://synthetic/dynamic-replay-policy.json",
        )]),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }];

    let report = evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
        readiness_id: "synthetic-catalog-mapping",
        catalog_mapping_evaluation_hash: "synthetic-evaluation-hash",
        source_sample_mapping_status: &entries,
        source_proof_id: "source-proof-synthetic-book-deltas",
        source_proof_version: 1,
        source_binding: "synthetic-book-deltas",
        required_table_family: "order_book_snapshot_deltas",
        required_nt_data_types: vec!["OrderBookDelta".to_string()],
        required_claim_evidence_refs: vec![
            "repo://synthetic/dynamic-replay-policy.json".to_string(),
        ],
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Ready);
    assert!(report.nt_catalog_mapping_proven);
    assert_eq!(
        report
            .observed_claim_evidence_refs
            .get("dynamic_replay_policy"),
        Some(&vec![
            "repo://synthetic/dynamic-replay-policy.json".to_string()
        ])
    );
}

#[test]
fn source_catalog_mapping_readiness_writer_reads_evaluation_and_writes_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let evaluation_path = dir.path().join("evaluation.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("readiness.toml");

    std::fs::write(
        &evaluation_path,
        r#"{
  "source_sample_mapping_status": [
    {
      "source_binding": "synthetic-native-trades",
      "source_proof_id": "source-proof-synthetic-native-trades",
      "source_proof_version": 1,
      "usage_scope": "canonical_backfill_input",
      "fixture_type": "synthetic-fixture",
      "table_family": "trades",
      "source_sample_status": "sample_available",
      "candidate_nt_data_classes": ["TradeTick"],
      "nt_data_class_evidence_refs": {
        "TradeTick": ["repo://synthetic/trade-tick-catalog-proof.json"]
      },
      "claim_evidence_refs": {
        "no_dynamic_replay_required": ["repo://synthetic/no-dynamic-replay-required.json"]
      },
      "current_bte_status": "accepted",
      "parquet_catalog_status": "proven",
      "decision": "synthetic evidence"
    }
  ]
}"#,
    )
    .expect("write evaluation");
    std::fs::write(
        &spec_path,
        format!(
            r#"readiness_id = "synthetic-catalog-mapping"
catalog_mapping_evaluation_path = "{}"
output_dir = "{}"
source_proof_id = "source-proof-synthetic-native-trades"
source_proof_version = 1
source_binding = "synthetic-native-trades"
required_table_family = "trades"
required_nt_data_types = ["TradeTick"]
required_claim_evidence_refs = ["repo://synthetic/no-dynamic-replay-required.json"]
allowed_current_bte_statuses = ["accepted"]
allowed_parquet_catalog_statuses = ["proven"]
allowed_usage_scopes = ["canonical_backfill_input"]
"#,
            evaluation_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first =
        write_source_catalog_mapping_readiness_report_from_spec_file(&spec_path).expect("first");
    let second =
        write_source_catalog_mapping_readiness_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE)
    );
}

fn accepted_mapping_entry() -> SourceCatalogMappingStatusEntry {
    SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-native-trades".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-native-trades".to_string(),
        usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        table_family: "trades".to_string(),
        candidate_nt_data_classes: vec!["TradeTick".to_string()],
        nt_data_class_evidence_refs: nt_data_class_evidence_refs([(
            "TradeTick",
            "repo://synthetic/trade-tick-catalog-proof.json",
        )]),
        claim_evidence_refs: BTreeMap::new(),
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }
}

fn nt_data_class_evidence_refs<const N: usize>(
    entries: [(&str, &str); N],
) -> BTreeMap<String, Vec<String>> {
    entries
        .into_iter()
        .map(|(data_type, evidence_ref)| (data_type.to_string(), vec![evidence_ref.to_string()]))
        .collect()
}

fn claim_evidence_refs<const N: usize>(
    entries: [(&str, &str); N],
) -> BTreeMap<String, Vec<String>> {
    entries
        .into_iter()
        .map(|(claim_key, evidence_ref)| (claim_key.to_string(), vec![evidence_ref.to_string()]))
        .collect()
}
