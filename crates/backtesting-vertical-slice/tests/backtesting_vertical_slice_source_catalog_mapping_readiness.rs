use backtesting_vertical_slice::source_catalog_mapping_readiness::{
    SOURCE_CATALOG_MAPPING_READINESS_REPORT_FILE, SourceCatalogMappingReadinessBlocker,
    SourceCatalogMappingReadinessInput, SourceCatalogMappingReadinessStatus,
    SourceCatalogMappingStatusEntry, evaluate_source_catalog_mapping_readiness,
    write_source_catalog_mapping_readiness_report_from_spec_file,
};

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
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
    });

    assert_eq!(report.status, SourceCatalogMappingReadinessStatus::Ready);
    assert!(report.blockers.is_empty());
    assert!(report.nt_catalog_mapping_proven);
    assert_eq!(
        report.observed_source_binding.as_deref(),
        Some("synthetic-native-trades")
    );
}

#[test]
fn source_catalog_mapping_readiness_blocks_unaccepted_mapping_statuses() {
    let entries = vec![SourceCatalogMappingStatusEntry {
        source_proof_id: Some("source-proof-synthetic-book-deltas".to_string()),
        source_proof_version: Some(1),
        source_binding: "synthetic-book-deltas".to_string(),
        table_family: "order_book_snapshot_deltas".to_string(),
        candidate_nt_data_classes: vec!["OrderBookDelta".to_string(), "TradeTick".to_string()],
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
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
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
        table_family: "trades".to_string(),
        candidate_nt_data_classes: vec!["TradeTick".to_string()],
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
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
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
      "fixture_type": "synthetic-fixture",
      "table_family": "trades",
      "source_sample_status": "sample_available",
      "candidate_nt_data_classes": ["TradeTick"],
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
allowed_current_bte_statuses = ["accepted"]
allowed_parquet_catalog_statuses = ["proven"]
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
        table_family: "trades".to_string(),
        candidate_nt_data_classes: vec!["TradeTick".to_string()],
        current_bte_status: "accepted".to_string(),
        parquet_catalog_status: "proven".to_string(),
    }
}
