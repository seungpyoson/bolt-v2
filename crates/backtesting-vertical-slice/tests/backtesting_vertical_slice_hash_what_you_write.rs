use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use arrow::{
    array::{ArrayRef, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use backtesting_vertical_slice::{
    backfill_accepted_tranche as accepted_tranche, backfill_binding_coverage as binding_coverage,
    backfill_conversion_batch as conversion_batch,
    backfill_conversion_completion as conversion_completion, backfill_coverage as coverage,
    backfill_execution_plan as execution_plan, backfill_execution_readiness as execution_readiness,
    backfill_object_staging as object_staging, backfill_preflight as preflight,
    backfill_readiness as readiness, backfill_source_proof_scope as source_scope,
    conversion_boundary, first_proof_selector as first_proof, nt_catalog_proof,
    run_manifest::ManifestArtifactStore,
    selected_source_slice, source_catalog_mapping_readiness as mapping_readiness,
    source_proof::{
        FixtureType, SourceCandidateClass, SourceProofFidelityClass, SourceProofUsageScope,
        SourceSelectionStatus,
    },
    source_proof_evidence_staging as evidence_staging,
    source_proof_legacy_derivability as legacy_derivability,
    source_proof_migration_preflight as migration_preflight,
    source_proof_shortlist as proof_shortlist, source_selection_readiness,
};
use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};
use sha2::{Digest, Sha256};

const HASH_WHAT_YOU_WRITE_MODULES: &[&str] = &[
    "backfill_accepted_tranche",
    "backfill_binding_coverage",
    "backfill_conversion_batch",
    "backfill_conversion_completion",
    "backfill_coverage",
    "backfill_execution_plan",
    "backfill_execution_readiness",
    "backfill_object_staging",
    "backfill_preflight",
    "backfill_readiness",
    "backfill_source_proof_scope",
    "conversion_boundary",
    "first_proof_selector",
    "nt_catalog_proof",
    "selected_source_slice",
    "source_catalog_mapping_readiness",
    "source_proof_evidence_staging",
    "source_proof_legacy_derivability",
    "source_proof_migration_preflight",
    "source_proof_shortlist",
    "source_selection_readiness",
];

// This allowlist is intentionally narrow: these compact serializations are
// semantic hashes, not hashes recorded for pretty JSON artifact bytes. Any new
// serde_json::to_vec hash outside this list is treated as a regression until it
// either joins the writer table above or proves it is a distinct semantic hash.
const COMPACT_SERIALIZATION_HASH_ALLOWLIST: &[&str] = &[
    "artifact_index",
    "artifact_store",
    "canonical_trades",
    "research_analytics",
    "result_contract",
    "run_manifest",
];

struct WriterCase {
    module: &'static str,
    write: fn(&Path) -> Result<Vec<HashClaim>>,
}

struct HashClaim {
    module: &'static str,
    label: &'static str,
    path: PathBuf,
    recorded_hash: String,
    recorded_bytes: Option<u64>,
}

#[test]
fn artifact_writers_record_hashes_for_the_bytes_written_to_disk() -> Result<()> {
    let cases = writer_cases();
    let case_modules = cases
        .iter()
        .map(|case| case.module)
        .collect::<BTreeSet<_>>();
    let expected_modules = HASH_WHAT_YOU_WRITE_MODULES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        case_modules, expected_modules,
        "hash-what-you-write regression table must enumerate every covered writer module"
    );

    let mut failures = Vec::new();
    for case in cases {
        let dir = tempfile::tempdir().with_context(|| format!("tempdir for {}", case.module))?;
        match (case.write)(dir.path()) {
            Ok(claims) if claims.is_empty() => {
                failures.push(format!("{} did not return any hash claims", case.module));
            }
            Ok(claims) => {
                for claim in claims {
                    if let Err(error) = verify_hash_claim(&claim) {
                        failures.push(format!("{} {}: {error}", claim.module, claim.label));
                    }
                }
            }
            Err(error) => {
                failures.push(format!("{} writer failed: {error:#}", case.module));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "hash-what-you-write failures:\n{}",
        failures.join("\n")
    );

    Ok(())
}

#[test]
fn pretty_json_artifact_writers_do_not_use_compact_serialization_for_hash_claims() -> Result<()> {
    let src_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowlist = COMPACT_SERIALIZATION_HASH_ALLOWLIST
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut unexpected = Vec::new();

    for entry in fs::read_dir(&src_root).context("read crate src directory")? {
        let entry = entry.context("read src entry")?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let module = path
            .file_stem()
            .and_then(|value| value.to_str())
            .context("source file stem")?;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read source file {}", path.display()))?;
        if source.contains("serde_json::to_vec(") && !allowlist.contains(module) {
            unexpected.push(module.to_string());
        }
    }

    unexpected.sort();
    assert!(
        unexpected.is_empty(),
        "compact serde_json::to_vec hashing remains outside the semantic allowlist: {unexpected:?}"
    );
    Ok(())
}

fn writer_cases() -> Vec<WriterCase> {
    vec![
        WriterCase {
            module: "backfill_accepted_tranche",
            write: write_backfill_accepted_tranche_claims,
        },
        WriterCase {
            module: "backfill_binding_coverage",
            write: write_backfill_binding_coverage_claims,
        },
        WriterCase {
            module: "backfill_conversion_batch",
            write: write_backfill_conversion_batch_claims,
        },
        WriterCase {
            module: "backfill_conversion_completion",
            write: write_backfill_conversion_completion_claims,
        },
        WriterCase {
            module: "backfill_coverage",
            write: write_backfill_coverage_claims,
        },
        WriterCase {
            module: "backfill_execution_plan",
            write: write_backfill_execution_plan_claims,
        },
        WriterCase {
            module: "backfill_execution_readiness",
            write: write_backfill_execution_readiness_claims,
        },
        WriterCase {
            module: "backfill_object_staging",
            write: write_backfill_object_staging_claims,
        },
        WriterCase {
            module: "backfill_preflight",
            write: write_backfill_preflight_claims,
        },
        WriterCase {
            module: "backfill_readiness",
            write: write_backfill_readiness_claims,
        },
        WriterCase {
            module: "backfill_source_proof_scope",
            write: write_backfill_source_proof_scope_claims,
        },
        WriterCase {
            module: "conversion_boundary",
            write: write_conversion_boundary_claims,
        },
        WriterCase {
            module: "first_proof_selector",
            write: write_first_proof_selector_claims,
        },
        WriterCase {
            module: "nt_catalog_proof",
            write: write_nt_catalog_proof_claims,
        },
        WriterCase {
            module: "selected_source_slice",
            write: write_selected_source_slice_claims,
        },
        WriterCase {
            module: "source_catalog_mapping_readiness",
            write: write_source_catalog_mapping_readiness_claims,
        },
        WriterCase {
            module: "source_proof_evidence_staging",
            write: write_source_proof_evidence_staging_claims,
        },
        WriterCase {
            module: "source_proof_legacy_derivability",
            write: write_source_proof_legacy_derivability_claims,
        },
        WriterCase {
            module: "source_proof_migration_preflight",
            write: write_source_proof_migration_preflight_claims,
        },
        WriterCase {
            module: "source_proof_shortlist",
            write: write_source_proof_shortlist_claims,
        },
        WriterCase {
            module: "source_selection_readiness",
            write: write_source_selection_readiness_claims,
        },
    ]
}

fn verify_hash_claim(claim: &HashClaim) -> Result<()> {
    let bytes = fs::read(&claim.path).with_context(|| format!("read {}", claim.path.display()))?;
    let actual_hash = sha256_hex(&bytes);
    if claim.recorded_hash != actual_hash {
        bail!(
            "recorded hash {} did not equal sha256(on-disk bytes) {} for {}",
            claim.recorded_hash,
            actual_hash,
            claim.path.display()
        );
    }
    if let Some(recorded_bytes) = claim.recorded_bytes {
        let actual_bytes = bytes.len() as u64;
        if recorded_bytes != actual_bytes {
            bail!(
                "recorded bytes {recorded_bytes} did not equal on-disk byte length {actual_bytes} for {}",
                claim.path.display()
            );
        }
    }
    Ok(())
}

fn claim(
    module: &'static str,
    label: &'static str,
    path: PathBuf,
    recorded_hash: String,
    recorded_bytes: Option<u64>,
) -> HashClaim {
    HashClaim {
        module,
        label,
        path,
        recorded_hash,
        recorded_bytes,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn empty_artifact_store() -> ManifestArtifactStore {
    ManifestArtifactStore {
        storage_options: BTreeMap::new(),
        rust_storage_options: BTreeMap::new(),
        ssm_parameters: None,
    }
}

fn write_backfill_accepted_tranche_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = source_scope_report();
    let report_path = dir.join("scope.json");
    let report_bytes = serde_json::to_vec_pretty(&report).context("serialize scope report")?;
    fs::write(&report_path, &report_bytes).context("write scope report")?;
    let output_dir = dir.join("accepted");
    let spec_path = dir.join("accepted.toml");
    fs::write(
        &spec_path,
        format!(
            r#"tranche_id = "tranche-test"
source_proof_scope_report_path = "{}"
output_dir = "{}"
"#,
            report_path.display(),
            output_dir.display()
        ),
    )
    .context("write accepted-tranche spec")?;

    let artifact =
        accepted_tranche::write_backfill_accepted_tranche_manifest_from_spec_file(&spec_path)
            .context("write accepted tranche")?;
    let manifest: accepted_tranche::BackfillAcceptedTrancheManifest =
        serde_json::from_slice(&fs::read(&artifact.path).context("read accepted manifest")?)
            .context("parse accepted manifest")?;

    Ok(vec![
        claim(
            "backfill_accepted_tranche",
            "content_hash",
            artifact.path,
            artifact.content_hash,
            Some(artifact.bytes),
        ),
        claim(
            "backfill_accepted_tranche",
            "source_proof_scope_report_hash",
            report_path,
            manifest.source_proof_scope_report_hash,
            None,
        ),
    ])
}

fn write_backfill_binding_coverage_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = binding_coverage::BackfillBindingCoverageReport {
        schema_version: binding_coverage::BACKFILL_BINDING_COVERAGE_SCHEMA_VERSION.to_string(),
        report_id: "binding-coverage-test".to_string(),
        status: binding_coverage::BackfillBindingCoverageStatus::Blocked,
        required_table_families: vec!["trades".to_string()],
        configured_required_binding_count: 0,
        ledger_records_for_required_bindings: 0,
        empty_source_binding_record_count: 0,
        missing_table_family_record_count: 0,
        unconfigured_source_bindings: Vec::new(),
        bindings: Vec::new(),
        blocking_issues: vec![
            binding_coverage::BackfillBindingCoverageIssue::NoConfiguredBindingForRequiredTableFamily,
        ],
    };
    let artifact =
        binding_coverage::write_backfill_binding_coverage_report(&dir.join("out"), &report)
            .context("write binding coverage")?;
    Ok(vec![claim(
        "backfill_binding_coverage",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_conversion_batch_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let plan = conversion_batch::BackfillConversionBatchPlan {
        schema_version: conversion_batch::BACKFILL_CONVERSION_BATCH_PLAN_SCHEMA_VERSION.to_string(),
        batch_id: "conversion-batch-test".to_string(),
        coverage_ledger_id: "coverage-ledger-test".to_string(),
        status: conversion_batch::BackfillConversionBatchStatus::Blocked,
        selection: conversion_batch::BackfillConversionBatchSelection {
            max_records: 1,
            max_accepted_objects: 1,
            max_accepted_bytes: 1,
            require_uniform_source_binding: true,
            allow_gaps: false,
        },
        record_count: 0,
        total_accepted_objects: 0,
        total_accepted_bytes: 0,
        canonical_ready_records: 0,
        records: Vec::new(),
        blocking_issues: vec![
            conversion_batch::BackfillConversionBatchBlockingIssue::EmptyInputSet,
        ],
    };
    let artifact = conversion_batch::write_backfill_conversion_batch_plan(&dir.join("out"), &plan)
        .context("write conversion batch")?;
    Ok(vec![claim(
        "backfill_conversion_batch",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_conversion_completion_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let ledger = conversion_completion::BackfillConversionCompletionLedger {
        schema_version: conversion_completion::BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION
            .to_string(),
        ledger_id: "conversion-completion-test".to_string(),
        batch_id: "batch-test".to_string(),
        status: conversion_completion::BackfillConversionCompletionStatus::Blocked,
        requirements: conversion_completion::BackfillConversionCompletionRequirements {
            scope_status: "published".to_string(),
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            current_bte_status: "ready".to_string(),
            parquet_catalog_status: "ready".to_string(),
            nt_data_type: "TradeTick".to_string(),
            fidelity_class: "TRADE_REPLAY".to_string(),
            require_direct_s3_catalog_access: false,
            require_publication_verification: false,
        },
        record_count: 0,
        published_records: 0,
        mapping_proven_records: 0,
        total_accepted_bytes: 0,
        total_canonical_rows: 0,
        total_nt_iterations: 0,
        records: Vec::new(),
        blocking_issues: vec![
            conversion_completion::BackfillConversionCompletionBlockingIssue::EmptyRecordSet,
        ],
    };
    let artifact = conversion_completion::write_backfill_conversion_completion_ledger(
        &dir.join("out"),
        &ledger,
    )
    .context("write conversion completion")?;
    Ok(vec![claim(
        "backfill_conversion_completion",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_coverage_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let ledger = empty_coverage_ledger("coverage-ledger-test");
    let artifact = coverage::write_coverage_ledger_artifact(&dir.join("out"), &ledger)
        .context("write coverage ledger")?;
    Ok(vec![claim(
        "backfill_coverage",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_execution_plan_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let plan = execution_plan_report();
    let artifact = execution_plan::write_backfill_execution_plan(&dir.join("out"), &plan)
        .context("write execution plan")?;
    Ok(vec![claim(
        "backfill_execution_plan",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_execution_readiness_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = execution_readiness::BackfillExecutionReadinessReport {
        schema_version: execution_readiness::BACKFILL_EXECUTION_READINESS_SCHEMA_VERSION
            .to_string(),
        readiness_id: "execution-readiness-test".to_string(),
        status: execution_readiness::BackfillExecutionReadinessStatus::Blocked,
        required_table_family: "trades".to_string(),
        required_nt_data_type: "TradeTick".to_string(),
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: vec![
            execution_readiness::BackfillExecutionReadinessSupportedDataPath {
                table_family: "trades".to_string(),
                nt_data_type: "TradeTick".to_string(),
            },
        ],
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_id: None,
        artifact_index_commit_proof_hash: None,
        artifact_index_direct_s3_commit_proven: None,
        artifact_index_producer_iam_scope_proven: None,
        source_selection_readiness_required: false,
        source_selection_readiness_id: None,
        source_selection_readiness_hash: None,
        source_selection_readiness_status: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_id: None,
        source_catalog_mapping_readiness_hash: None,
        source_catalog_mapping_readiness_status: None,
        accepted_tranche_id: "tranche-test".to_string(),
        accepted_tranche_manifest_hash: "tranche-hash".to_string(),
        execution_plan_id: "plan-test".to_string(),
        execution_plan_hash: "plan-hash".to_string(),
        operator_run_id: "operator-run-test".to_string(),
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        source_binding: "binance-spot-trades".to_string(),
        table_family: "trades".to_string(),
        object_count: 0,
        accepted_bytes: 0,
        blockers: vec![
            execution_readiness::BackfillExecutionReadinessBlocker::ExecutionPlanNotReady,
        ],
    };
    let artifact =
        execution_readiness::write_backfill_execution_readiness_report(&dir.join("out"), &report)
            .context("write execution readiness")?;
    Ok(vec![claim(
        "backfill_execution_readiness",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_object_staging_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let local_object = dir.join("source.csv");
    let object_bytes = b"trade_id,price\n1,10.00\n";
    fs::write(&local_object, object_bytes).context("write local object")?;
    let object_hash = sha256_hex(object_bytes);
    let artifact_root = dir.join("artifact-root");
    let output_object = artifact_root.join("raw").join("object.csv");
    let spec = object_staging::BackfillObjectStagingSpec {
        staging_id: "object-staging-test".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        local_object_path: local_object,
        output_object_uri: format!("file://{}", output_object.display()),
        source_url: "https://data.example.test/object.csv".to_string(),
        expected_sha256: object_hash,
        expected_bytes: object_bytes.len() as u64,
        archive_date: "2026-03-01".to_string(),
        schema_columns: vec!["trade_id".to_string(), "price".to_string()],
        output_dir: dir.join("out"),
    };
    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let artifact = object_staging::stage_backfill_object_with_resolver(&spec, &mut resolver)
        .context("stage backfill object")?;
    Ok(vec![claim(
        "backfill_object_staging",
        "manifest_hash",
        artifact.manifest_path,
        artifact.manifest_hash,
        Some(artifact.manifest_bytes),
    )])
}

fn write_backfill_preflight_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = preflight::BackfillPreflightReport {
        schema_version: preflight::BACKFILL_PREFLIGHT_REPORT_SCHEMA_VERSION.to_string(),
        preflight_id: "preflight-test".to_string(),
        coverage_ledger_id: "coverage-ledger-test".to_string(),
        status: preflight::BackfillPreflightStatus::Blocked,
        selection: preflight::BackfillPreflightSelection {
            max_accepted_objects: 1,
            max_accepted_bytes: 1,
            require_canonical_ready: true,
            allow_gaps: false,
        },
        total_records: 0,
        accepted_records: 0,
        accepted_with_gaps_records: 0,
        canonical_ready_records: 0,
        eligible_record_count: 0,
        selected_record: None,
        blocking_reasons: vec![preflight::BackfillPreflightBlockingReason::EmptyLedger],
    };
    let artifact = preflight::write_backfill_preflight_report(&dir.join("out"), &report)
        .context("write preflight")?;
    Ok(vec![claim(
        "backfill_preflight",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_readiness_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = readiness::BackfillReadinessReport {
        schema_version: readiness::BACKFILL_READINESS_SCHEMA_VERSION.to_string(),
        readiness_id: "backfill-readiness-test".to_string(),
        status: readiness::BackfillReadinessStatus::Blocked,
        required_table_family: "trades".to_string(),
        required_nt_data_type: "TradeTick".to_string(),
        supported_data_paths: vec![readiness::BackfillReadinessSupportedDataPath {
            table_family: "trades".to_string(),
            nt_data_type: "TradeTick".to_string(),
        }],
        backfill_preflight_id: "preflight-test".to_string(),
        backfill_preflight_status: preflight::BackfillPreflightStatus::Blocked,
        source_proof_migration_preflight_id: "migration-test".to_string(),
        source_proof_migration_preflight_status:
            migration_preflight::SourceProofMigrationPreflightStatus::Blocked,
        backfill_binding_coverage_id: "binding-coverage-test".to_string(),
        backfill_binding_coverage_status: binding_coverage::BackfillBindingCoverageStatus::Blocked,
        selected_backfill_record: None,
        selected_source_proof_candidate: None,
        blockers: vec![readiness::BackfillReadinessBlocker::BackfillPreflightBlocked],
    };
    let artifact = readiness::write_backfill_readiness_report(&dir.join("out"), &report)
        .context("write readiness")?;
    Ok(vec![claim(
        "backfill_readiness",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_backfill_source_proof_scope_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = source_scope_report();
    let artifact =
        source_scope::write_backfill_source_proof_scope_report(&dir.join("out"), &report)
            .context("write source-proof scope")?;
    Ok(vec![claim(
        "backfill_source_proof_scope",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_conversion_boundary_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let fingerprint = conversion_fingerprint();
    let checkpoint = conversion_boundary::ConversionCheckpoint::completed(
        fingerprint.clone(),
        2,
        "catalog-hash",
        "2026-03-01T00:00:00Z",
    );
    let checkpoint_hash = checkpoint.content_hash().context("checkpoint hash")?;
    let manifest = conversion_boundary::ConversionManifest::completed(
        fingerprint,
        "canonical-trades.v1",
        "TradeTick",
        "BTCUSDT.BINANCE",
        2,
        "file:///tmp/catalog",
        "catalog-hash",
        checkpoint_hash.clone(),
        "2026-03-01T00:00:01Z",
    );
    let manifest_hash = manifest.content_hash().context("manifest hash")?;
    let metadata = conversion_boundary::ConversionCatalogMetadata::from_manifest(
        &manifest,
        manifest_hash.clone(),
        checkpoint_hash.clone(),
    );
    let metadata_hash = metadata.content_hash().context("metadata hash")?;
    conversion_boundary::write_completed_conversion_artifacts(
        &dir.join("out"),
        &manifest,
        &checkpoint,
        &metadata,
    )
    .context("write conversion boundary artifacts")?;

    Ok(vec![
        claim(
            "conversion_boundary",
            "checkpoint_hash",
            dir.join("out")
                .join(conversion_boundary::CONVERSION_CHECKPOINT_FILE),
            checkpoint_hash,
            None,
        ),
        claim(
            "conversion_boundary",
            "manifest_hash",
            dir.join("out")
                .join(conversion_boundary::CONVERSION_MANIFEST_FILE),
            manifest_hash,
            None,
        ),
        claim(
            "conversion_boundary",
            "metadata_hash",
            dir.join("out")
                .join(conversion_boundary::CATALOG_METADATA_FILE),
            metadata_hash,
            None,
        ),
    ])
}

fn write_first_proof_selector_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let ledger_report = first_proof::FirstProofEventCountLedgerReport {
        schema_version: first_proof::FIRST_PROOF_EVENT_COUNT_LEDGER_SCHEMA_VERSION.to_string(),
        source_rows: 2,
        event_counts: vec![first_proof::AssetEventCount {
            asset_id: "asset-a".to_string(),
            event_family: "book".to_string(),
            rows: 2,
            source_row_groups: vec![0],
        }],
    };
    let ledger_artifact = first_proof::write_first_proof_event_count_ledger(
        &dir.join("event-counts.json"),
        &ledger_report,
    )
    .context("write event-count ledger")?;
    let selector_report = first_proof::FirstProofSelectorReport {
        schema_version: first_proof::FIRST_PROOF_SELECTOR_SCHEMA_VERSION.to_string(),
        selector_id: "selector-test".to_string(),
        status: first_proof::FirstProofSelectorStatus::Selected,
        selection: first_proof::FirstProofSelection {
            required_event_families: vec!["book".to_string()],
            excluded_event_families: vec!["tick_size_change".to_string()],
            candidate_asset_ids: Vec::new(),
            row_budget: 10,
            max_selected_assets: 1,
        },
        event_count_ledger_hash: ledger_artifact.content_hash.clone(),
        total_assets: 1,
        eligible_assets: 1,
        selected_assets: vec![first_proof::SelectedFirstProofAsset {
            asset_id: "asset-a".to_string(),
            replay_rows: 2,
            source_row_groups: vec![0],
        }],
        selected_asset_ids_hash: "selected-asset-ids-hash".to_string(),
        excluded_event_asset_count: 0,
        excluded_event_row_count: 0,
        blocking_issues: Vec::new(),
    };
    let selector_artifact =
        first_proof::write_first_proof_selector_report(&dir.join("selector"), &selector_report)
            .context("write selector report")?;
    let selector: first_proof::FirstProofSelectorReport =
        serde_json::from_slice(&fs::read(&selector_artifact.path).context("read selector")?)
            .context("parse selector")?;

    Ok(vec![
        claim(
            "first_proof_selector",
            "event_count_ledger_content_hash",
            ledger_artifact.path.clone(),
            ledger_artifact.content_hash,
            Some(ledger_artifact.bytes),
        ),
        claim(
            "first_proof_selector",
            "selector_content_hash",
            selector_artifact.path,
            selector_artifact.content_hash,
            Some(selector_artifact.bytes),
        ),
        claim(
            "first_proof_selector",
            "selector_event_count_ledger_hash",
            ledger_artifact.path,
            selector.event_count_ledger_hash,
            None,
        ),
    ])
}

fn write_nt_catalog_proof_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let catalog_root = dir.join("catalog");
    let output_dir = dir.join("proof-output");
    let spec_path = dir.join("proof.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
proof_id = "nt-catalog-proof-hash-test"
catalog_uri = "file://{catalog_root}"
output_dir = "{output_dir}"
ticks_per_instrument = 1
base_timestamp_nanos = 1740787200000000000
trade_interval_nanos = 1000000000

[artifact_store]
storage_options = {{}}
rust_storage_options = {{ region = "local-test" }}

[[instruments]]
symbol = "BTCUSDT"
venue = "SIM"
base_currency = "BTC"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "0.500"
price_start = "50000.00"

[[instruments]]
symbol = "ETHUSDT"
venue = "SIM"
base_currency = "ETH"
quote_currency = "USDT"
price_precision = 2
size_precision = 3
price_increment = "0.01"
size_increment = "0.001"
quantity = "1.500"
price_start = "3000.00"
"#,
            catalog_root = catalog_root.display(),
            output_dir = output_dir.display(),
        ),
    )
    .context("write nt catalog proof spec")?;
    let artifact = nt_catalog_proof::run_nt_catalog_proof_from_spec_file_with_resolver(
        &spec_path,
        &mut |_, _| Ok("unused-secret".to_string()),
    )
    .context("run nt catalog proof")?;
    Ok(vec![claim(
        "nt_catalog_proof",
        "content_hash",
        artifact.report_path,
        artifact.content_hash,
        Some(artifact.report_bytes),
    )])
}

fn write_selected_source_slice_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let source_path = dir.join("source.parquet");
    let selector_path = dir.join("selector.json");
    let output_path = dir.join("selected.parquet");
    let report_path = dir.join("selected-report.json");
    let spec_path = dir.join("selected.toml");
    write_source_parquet(&source_path).context("write source parquet")?;
    write_selector_report(&selector_path).context("write selector report")?;
    let max_source_parquet_bytes = fs::metadata(&source_path)
        .context("stat source parquet")?
        .len();
    fs::write(
        &spec_path,
        format!(
            r#"source_parquet_path = "{}"
selector_report_path = "{}"
output_parquet_path = "{}"
report_path = "{}"
asset_id_column = "asset"
usage_scope = "one_off_backfill_data"
max_source_parquet_bytes = {max_source_parquet_bytes}
projected_columns = ["asset", "event_type", "payload"]
"#,
            source_path.display(),
            selector_path.display(),
            output_path.display(),
            report_path.display()
        ),
    )
    .context("write selected-source-slice spec")?;
    let artifact = selected_source_slice::write_selected_source_slice_from_spec_file(&spec_path)
        .context("write selected source slice")?;
    Ok(vec![claim(
        "selected_source_slice",
        "report_hash",
        artifact.report_path,
        artifact.report_hash,
        Some(artifact.report_bytes),
    )])
}

fn write_source_catalog_mapping_readiness_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = mapping_readiness::SourceCatalogMappingReadinessReport {
        schema_version: mapping_readiness::SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION
            .to_string(),
        readiness_id: "mapping-readiness-test".to_string(),
        status: mapping_readiness::SourceCatalogMappingReadinessStatus::Blocked,
        catalog_mapping_evaluation_hash: "mapping-evaluation-hash".to_string(),
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        source_binding: "binance-spot-trades".to_string(),
        required_table_family: "trades".to_string(),
        required_nt_data_types: vec!["TradeTick".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["ready".to_string()],
        allowed_parquet_catalog_statuses: vec!["ready".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
        observed_source_proof_id: None,
        observed_source_proof_version: None,
        observed_source_binding: None,
        observed_table_family: None,
        observed_usage_scope: None,
        observed_nt_data_types: Vec::new(),
        observed_nt_data_type_evidence_refs: BTreeMap::new(),
        observed_claim_evidence_refs: BTreeMap::new(),
        observed_current_bte_status: None,
        observed_parquet_catalog_status: None,
        nt_catalog_mapping_proven: false,
        blockers: vec![
            mapping_readiness::SourceCatalogMappingReadinessBlocker::MappingEntryNotFound,
        ],
    };
    let artifact =
        mapping_readiness::write_source_catalog_mapping_readiness_report(&dir.join("out"), &report)
            .context("write mapping readiness")?;
    Ok(vec![claim(
        "source_catalog_mapping_readiness",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_source_proof_evidence_staging_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let local_evidence = dir.join("evidence.txt");
    let evidence_bytes = b"schema columns: trade_id,price\n";
    fs::write(&local_evidence, evidence_bytes).context("write local evidence")?;
    let artifact_root = dir.join("artifact-root");
    let output_evidence = artifact_root.join("source-proofs").join("evidence.txt");
    let spec = evidence_staging::SourceProofEvidenceStagingSpec {
        staging_id: "evidence-staging-test".to_string(),
        artifact_root: format!("file://{}", artifact_root.display()),
        artifact_store: empty_artifact_store(),
        evidence_files: vec![evidence_staging::SourceProofEvidenceStagingFile {
            evidence_kind: "schema_sample".to_string(),
            local_path: local_evidence,
            output_uri: format!("file://{}", output_evidence.display()),
            expected_sha256: sha256_hex(evidence_bytes),
            expected_bytes: evidence_bytes.len() as u64,
        }],
        output_dir: dir.join("out"),
    };
    let mut resolver = |_region: &str, _path: &str| {
        Err::<String, String>("no SSM resolution expected".to_string())
    };
    let artifact =
        evidence_staging::stage_source_proof_evidence_with_resolver(&spec, &mut resolver)
            .context("stage source-proof evidence")?;
    Ok(vec![claim(
        "source_proof_evidence_staging",
        "manifest_hash",
        artifact.manifest_path,
        artifact.manifest_hash,
        Some(artifact.manifest_bytes),
    )])
}

fn write_source_proof_legacy_derivability_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = legacy_derivability::SourceProofLegacyDerivabilityReport {
        schema_version: legacy_derivability::SOURCE_PROOF_LEGACY_DERIVABILITY_SCHEMA_VERSION
            .to_string(),
        report_id: "legacy-derivability-test".to_string(),
        records: Vec::new(),
        summary: legacy_derivability::SourceProofLegacyDerivabilitySummary {
            total_records: 0,
            s3_bound_records: 0,
            single_table_family_records: 0,
            acceptance_blocked_records: 0,
            blocking_issue_count: 0,
            blocking_issue_counts: Vec::new(),
            table_family_counts: Vec::new(),
        },
    };
    let artifact = legacy_derivability::write_source_proof_legacy_derivability_report(
        &dir.join("out"),
        &report,
    )
    .context("write legacy derivability")?;
    Ok(vec![claim(
        "source_proof_legacy_derivability",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_source_proof_migration_preflight_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = migration_preflight::SourceProofMigrationPreflightReport {
        schema_version: migration_preflight::SOURCE_PROOF_MIGRATION_PREFLIGHT_SCHEMA_VERSION
            .to_string(),
        preflight_id: "migration-preflight-test".to_string(),
        derivability_report_id: "legacy-derivability-test".to_string(),
        status: migration_preflight::SourceProofMigrationPreflightStatus::Blocked,
        selection: migration_preflight::SourceProofMigrationPreflightSelection {
            allowed_table_families: vec!["trades".to_string()],
            required_derivable_fields: vec![
                legacy_derivability::SourceProofLegacyDerivableField::SourceBinding,
            ],
            max_raw_payload_records: 1,
            max_accepted_bytes_from_s3: 1,
            require_single_table_family: true,
            require_s3_bound_payloads: true,
        },
        total_records: 0,
        eligible_candidate_count: 0,
        selected_candidate: None,
        blocking_reasons: vec![
            migration_preflight::SourceProofMigrationPreflightReason::EmptyDerivabilityReport,
        ],
    };
    let artifact = migration_preflight::write_source_proof_migration_preflight_report(
        &dir.join("out"),
        &report,
    )
    .context("write migration preflight")?;
    Ok(vec![claim(
        "source_proof_migration_preflight",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_source_proof_shortlist_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = proof_shortlist::SourceProofShortlistReport {
        schema_version: proof_shortlist::SOURCE_PROOF_SHORTLIST_SCHEMA_VERSION.to_string(),
        shortlist_id: "proof-shortlist-test".to_string(),
        status: proof_shortlist::SourceProofShortlistStatus::Blocked,
        selection: proof_shortlist::SourceProofShortlistSelection {
            allowed_fixture_types: vec![FixtureType::PerpsSpot],
            allowed_table_families: vec!["trades".to_string()],
            allowed_candidate_classes: vec![SourceCandidateClass::OfficialFree],
            max_candidates: 1,
        },
        total_reports: 0,
        eligible_candidate_count: 0,
        candidates: Vec::new(),
        blocking_reasons: vec![
            proof_shortlist::SourceProofShortlistReason::EmptySourceProofReports,
        ],
    };
    let artifact = proof_shortlist::write_source_proof_shortlist_report(&dir.join("out"), &report)
        .context("write source-proof shortlist")?;
    Ok(vec![claim(
        "source_proof_shortlist",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn write_source_selection_readiness_claims(dir: &Path) -> Result<Vec<HashClaim>> {
    let report = source_selection_readiness::SourceSelectionReadinessReport {
        schema_version: source_selection_readiness::SOURCE_SELECTION_READINESS_SCHEMA_VERSION
            .to_string(),
        selection_id: "source-selection-readiness-test".to_string(),
        status: source_selection_readiness::SourceSelectionReadinessStatus::Blocked,
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        source_proof_hash: "source-proof-hash".to_string(),
        source_binding: "binance-spot-trades".to_string(),
        venue: "binance".to_string(),
        fixture_type: FixtureType::PerpsSpot,
        table_family: "trades".to_string(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        source_selection_status: SourceSelectionStatus::PendingMoreProof,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        required_fixture_type: FixtureType::PerpsSpot,
        required_table_family: "trades".to_string(),
        allowed_fidelity_classes: vec![SourceProofFidelityClass::TradeReplay],
        allow_lower_fidelity: false,
        source_proof_accepted: false,
        canonical_usage_scope_proven: false,
        source_access_proven: false,
        license_proven: false,
        sample_schema_proven: false,
        time_semantics_proven: false,
        instrument_universe_proven: false,
        coverage_proven: false,
        retention_freshness_proven: false,
        granularity_proven: false,
        completeness_proven: false,
        nt_mapping_proven: false,
        cost_proven: false,
        storage_proven: false,
        claim_limits_recorded: false,
        source_proof_acceptance_error: Some("pending".to_string()),
        unmet_required_checks: vec!["source_access".to_string()],
        blockers: vec![
            source_selection_readiness::SourceSelectionReadinessBlocker::SourceProofNotAccepted,
        ],
    };
    let artifact = source_selection_readiness::write_source_selection_readiness_report(
        &dir.join("out"),
        &report,
    )
    .context("write source-selection readiness")?;
    Ok(vec![claim(
        "source_selection_readiness",
        "content_hash",
        artifact.path,
        artifact.content_hash,
        Some(artifact.bytes),
    )])
}

fn source_scope_report() -> source_scope::BackfillSourceProofScopeReport {
    source_scope::BackfillSourceProofScopeReport {
        schema_version: source_scope::BACKFILL_SOURCE_PROOF_SCOPE_SCHEMA_VERSION.to_string(),
        report_id: "scope-report-test".to_string(),
        status: source_scope::BackfillSourceProofScopeStatus::CandidateFound,
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        source_binding: "binance-spot-trades".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        manifest_id: "manifest-test".to_string(),
        accepted_scope_completed_objects: 1,
        accepted_scope_accepted_bytes: 23,
        manifest_payload_object_count: 1,
        matching_object_count: 1,
        object_level_tranche_required: true,
        selected_object: Some(source_scope::BackfillSourceProofScopeObject {
            s3_uri: "s3://bucket/raw/object.csv".to_string(),
            source_url: "https://data.example.test/object.csv".to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            bytes: 23,
            archive_date: "2026-03-01".to_string(),
            source_row_groups: vec![0],
            predicate_ref: Some("asset=BTCUSDT".to_string()),
        }),
        source_proof_acceptance_error: None,
        blocking_issues: Vec::new(),
    }
}

fn execution_plan_report() -> execution_plan::BackfillExecutionPlan {
    execution_plan::BackfillExecutionPlan {
        schema_version: execution_plan::BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION.to_string(),
        plan_id: "execution-plan-test".to_string(),
        status: execution_plan::BackfillExecutionPlanStatus::Blocked,
        accepted_tranche_id: "tranche-test".to_string(),
        accepted_tranche_manifest_hash: "tranche-hash".to_string(),
        run_spec_hash: "run-spec-hash".to_string(),
        operator_run_id: "operator-run-test".to_string(),
        output_prefix: "s3://bucket/output".to_string(),
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        source_binding: "binance-spot-trades".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        object_count: 0,
        accepted_bytes: 0,
        max_object_bytes: 0,
        max_decoded_bytes: 0,
        max_source_rows: 0,
        max_projected_row_groups: 0,
        max_wall_seconds: 0,
        require_object_selection_metadata: false,
        objects: Vec::new(),
        blocking_issues: vec![
            execution_plan::BackfillExecutionPlanIssue::ExecutionPlanSourceRowBudgetMissing,
        ],
    }
}

fn empty_coverage_ledger(ledger_id: &str) -> coverage::BackfillCoverageLedger {
    coverage::BackfillCoverageLedger {
        schema_version: coverage::BACKFILL_COVERAGE_LEDGER_SCHEMA_VERSION.to_string(),
        ledger_id: ledger_id.to_string(),
        records: Vec::new(),
        summary: coverage::BackfillCoverageSummary {
            total_records: 0,
            accepted_records: 0,
            accepted_with_gaps_records: 0,
            rejected_records: 0,
            physical_only_records: 0,
            canonical_ready_records: 0,
            accepted_objects: 0,
            accepted_bytes: 0,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        },
    }
}

fn conversion_fingerprint() -> conversion_boundary::ConversionFingerprint {
    conversion_boundary::ConversionFingerprint {
        source_proof_id: "proof-test".to_string(),
        source_proof_version: 1,
        accepted_object_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        converter_identity: "converter-test".to_string(),
        converter_version: "1".to_string(),
        converter_config_hash: "converter-config-hash".to_string(),
    }
}

fn write_selector_report(path: &Path) -> Result<()> {
    let selector_bytes = serde_json::to_vec_pretty(&first_proof::FirstProofSelectorReport {
        schema_version: first_proof::FIRST_PROOF_SELECTOR_SCHEMA_VERSION.to_string(),
        selector_id: "selector-synthetic".to_string(),
        status: first_proof::FirstProofSelectorStatus::Selected,
        selection: first_proof::FirstProofSelection {
            required_event_families: vec!["book".to_string()],
            excluded_event_families: vec!["tick_size_change".to_string()],
            candidate_asset_ids: Vec::new(),
            row_budget: 10,
            max_selected_assets: 1,
        },
        event_count_ledger_hash: "event-ledger-hash".to_string(),
        total_assets: 2,
        eligible_assets: 1,
        selected_assets: vec![first_proof::SelectedFirstProofAsset {
            asset_id: "asset-a".to_string(),
            replay_rows: 2,
            source_row_groups: vec![1],
        }],
        selected_asset_ids_hash: "selected-assets-hash".to_string(),
        excluded_event_asset_count: 0,
        excluded_event_row_count: 0,
        blocking_issues: Vec::new(),
    })
    .context("serialize selector report")?;
    fs::write(path, selector_bytes).context("write selector report")
}

fn write_source_parquet(path: &Path) -> Result<()> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("asset", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, false),
        Field::new("ignored", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![
                "asset-b", "asset-b", "asset-a", "asset-a",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "book",
                "price_change",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "payload-b-book",
                "payload-b-price",
                "payload-a-book",
                "payload-a-price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "ignored-b1",
                "ignored-b2",
                "ignored-a1",
                "ignored-a2",
            ])) as ArrayRef,
        ],
    )
    .context("record batch")?;
    let file = File::create(path).context("create source parquet")?;
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(2))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).context("parquet writer")?;
    writer.write(&batch).context("write batch")?;
    writer.close().context("close parquet")?;
    Ok(())
}
