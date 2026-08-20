use crate::backtesting_vertical_slice_test_support::{
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH, generated_evicted_conversion_batch_plan,
};
use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::{
        ArtifactIndexCommitProofEvidence, ArtifactIndexCommitProofReportV1,
    },
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheManifest, BackfillAcceptedTrancheStatus,
        evaluate_backfill_accepted_tranche,
    },
    backfill_conversion_batch::{BackfillConversionBatchPlan, BackfillConversionBatchStatus},
    backfill_coverage::{BackfillCoverageLedger, BackfillCoverageStatus},
    backfill_execution_plan::{
        BackfillExecutionPlan, BackfillExecutionRunBinding, BackfillExecutionWorkBudget,
        evaluate_backfill_execution_plan,
    },
    backfill_execution_readiness::{
        BackfillExecutionReadinessBlocker, BackfillExecutionReadinessInput,
        BackfillExecutionReadinessReport, BackfillExecutionReadinessStatus,
        BackfillExecutionReadinessSupportedDataPath, evaluate_backfill_execution_readiness,
    },
    backfill_source_proof_scope::{
        BackfillSourceProofScopeReport, BackfillSourceProofScopeStatus,
        evaluate_backfill_source_proof_scope,
        evaluate_backfill_source_proof_scope_for_selected_object,
    },
    operator::RunSpec,
    source_catalog_mapping_readiness::{
        SourceCatalogMappingReadinessBlocker, SourceCatalogMappingReadinessInput,
        SourceCatalogMappingReadinessReport, SourceCatalogMappingReadinessSpec,
        SourceCatalogMappingReadinessStatus, SourceCatalogMappingStatusEntry,
        evaluate_source_catalog_mapping_readiness,
    },
    source_proof::{SourceProofReport, SourceProofStatus, SourceProofUsageScope},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const SOURCE_PROOF: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-01.json"
);
const RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
);
const RUN_SPEC_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
);
const OBJECT_STAGING_MANIFEST: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/object-staging/backfill-object-staging-manifest.json"
);
const SOURCE_PROOF_SCOPE_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-proof-scope/backfill-source-proof-scope-report.json"
);
const ACCEPTED_TRANCHE_MANIFEST: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/accepted-tranche/backfill-accepted-tranche-manifest.json"
);
const ACCEPTED_TRANCHE_MANIFEST_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/accepted-tranche/backfill-accepted-tranche-manifest.json"
);
const EXECUTION_PLAN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/backfill-execution-plan.toml"
);
const EXECUTION_PLAN: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json"
);
const EXECUTION_PLAN_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json"
);
const CATALOG_MAPPING_EVALUATION: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.binance-bnbusdc-2026-03-01.2026-06-10.json"
);
const CATALOG_MAPPING_EVALUATION_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.binance-bnbusdc-2026-03-01.2026-06-10.json"
);
const BLOCKED_CATALOG_MAPPING_EVALUATION: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const BLOCKED_CATALOG_MAPPING_EVALUATION_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const SOURCE_CATALOG_MAPPING_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"
);
const SOURCE_CATALOG_MAPPING_READINESS_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"
);
const BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-catalog-mapping-readiness/polymarket-parquet-archive-index-canonical/source-catalog-mapping-readiness-report.json"
);
const EXECUTION_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-readiness/backfill-execution-readiness-report.json"
);
const ARTIFACT_INDEX_COMMIT_STATUS: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof-status.backtesting-engine-006.2026-06-08.json"
);
const ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-direct-s3.2026-06-08.json"
);
const ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-direct-s3.2026-06-08.json"
);
const ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-iam-scope.2026-06-08.json"
);
const ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-iam-scope.2026-06-08.json"
);
const ARTIFACT_INDEX_BACKTESTS_COMPLETE_PROOF_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-producer-iam-scope-proof.backtesting-engine-006.backtests-complete.2026-06-15.json"
);
const ARTIFACT_INDEX_ALL_PRODUCER_SCOPE_PROOF_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-all-producer-iam-scope-proof.backtesting-engine-006.2026-06-15.json"
);
const ARTIFACT_INDEX_REQUIRED_EXECUTION_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-readiness-artifact-index-required/backfill-execution-readiness-report.json"
);
const BINANCE_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01"
);
const BINANCE_2026_03_02_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-02"
);
const BINANCE_2026_03_02_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-02.json"
);
const BINANCE_2026_03_03_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-03"
);
const BINANCE_2026_03_03_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-03.json"
);
const BINANCE_2026_03_04_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-04"
);
const BINANCE_2026_03_04_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-04.json"
);
const BINANCE_2026_03_05_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-05"
);
const BINANCE_2026_03_05_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-05.json"
);
const BINANCE_2026_03_06_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-06"
);
const BINANCE_2026_03_06_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-06.json"
);
const BINANCE_2026_03_07_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-07"
);
const BINANCE_2026_03_07_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-07.json"
);
const BINANCE_2026_03_08_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-08"
);
const BINANCE_2026_03_08_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-08.json"
);
const BINANCE_2026_03_09_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-09"
);
const BINANCE_2026_03_09_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-09.json"
);
const BINANCE_2026_03_10_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-10"
);
const BINANCE_2026_03_10_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-10.json"
);
const BINANCE_2026_03_11_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-11"
);
const BINANCE_2026_03_11_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-11.json"
);
const BINANCE_2026_03_12_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-12"
);
const BINANCE_2026_03_12_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-12.json"
);
const BINANCE_2026_03_13_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-13"
);
const BINANCE_2026_03_13_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-13.json"
);
const BINANCE_2026_03_14_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-14"
);
const BINANCE_2026_03_14_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-14.json"
);
const BINANCE_2026_03_15_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-15"
);
const BINANCE_2026_03_15_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-15.json"
);
const BINANCE_2026_03_16_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-16"
);
const BINANCE_2026_03_16_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-16.json"
);
const BINANCE_2026_03_17_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-17"
);
const BINANCE_2026_03_17_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-17.json"
);
const BINANCE_2026_03_18_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-18"
);
const BINANCE_2026_03_18_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-18.json"
);
const BINANCE_2026_03_19_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-19"
);
const BINANCE_2026_03_19_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-19.json"
);
const BINANCE_2026_03_20_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-20"
);
const BINANCE_2026_03_20_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-20.json"
);
const BINANCE_2026_03_21_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-21"
);
const BINANCE_2026_03_21_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-21.json"
);
const BINANCE_2026_03_22_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-22"
);
const BINANCE_2026_03_22_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-22.json"
);
const BINANCE_2026_03_23_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-23"
);
const BINANCE_2026_03_23_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-23.json"
);
const BINANCE_2026_03_24_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-24"
);
const BINANCE_2026_03_24_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-24.json"
);
const BINANCE_2026_03_25_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-25"
);
const BINANCE_2026_03_25_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-25.json"
);
const BINANCE_2026_03_26_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-26"
);
const BINANCE_2026_03_26_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-26.json"
);
const BINANCE_2026_03_27_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-27"
);
const BINANCE_2026_03_27_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-27.json"
);
const BINANCE_2026_03_28_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-28"
);
const BINANCE_2026_03_28_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-28.json"
);
const BINANCE_2026_03_29_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-29"
);
const BINANCE_2026_03_29_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-29.json"
);
const BINANCE_2026_03_30_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-30"
);
const BINANCE_2026_03_30_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-30.json"
);
const BINANCE_2026_03_31_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-31"
);
const BINANCE_2026_03_31_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-31.json"
);
const BINANCE_2026_04_01_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-01"
);
const BINANCE_2026_04_01_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-01.json"
);
const BINANCE_2026_04_02_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-02"
);
const BINANCE_2026_04_02_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-02.json"
);
const BINANCE_2026_04_03_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-03"
);
const BINANCE_2026_04_03_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-03.json"
);
const BINANCE_2026_04_04_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-04"
);
const BINANCE_2026_04_04_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-04.json"
);
const BINANCE_2026_04_05_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-05"
);
const BINANCE_2026_04_05_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-05.json"
);
const BINANCE_2026_04_06_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-06"
);
const BINANCE_2026_04_06_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-06.json"
);
const BINANCE_2026_04_07_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-07"
);
const BINANCE_2026_04_07_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-07.json"
);
const BINANCE_2026_04_08_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-08"
);
const BINANCE_2026_04_08_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-08.json"
);
const BINANCE_2026_04_09_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-09"
);
const BINANCE_2026_04_09_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-09.json"
);
const BINANCE_2026_04_10_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-10"
);
const BINANCE_2026_04_10_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-10.json"
);
const BINANCE_2026_04_11_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-11"
);
const BINANCE_2026_04_11_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-11.json"
);
const BINANCE_2026_04_12_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-12"
);
const BINANCE_2026_04_12_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-12.json"
);
const BINANCE_2026_04_13_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-13"
);
const BINANCE_2026_04_13_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-13.json"
);
const BINANCE_2026_04_14_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-14"
);
const BINANCE_2026_04_14_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-14.json"
);
const BINANCE_2026_04_15_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-15"
);
const BINANCE_2026_04_15_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-15.json"
);
const BINANCE_2026_04_16_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-16"
);
const BINANCE_2026_04_16_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-16.json"
);
const BINANCE_2026_04_17_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-17"
);
const BINANCE_2026_04_17_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-17.json"
);
const BINANCE_2026_04_18_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-18"
);
const BINANCE_2026_04_18_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-18.json"
);
const BINANCE_2026_04_19_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-19"
);
const BINANCE_2026_04_19_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-19.json"
);
const BINANCE_2026_04_20_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-20"
);
const BINANCE_2026_04_20_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-20.json"
);
const BINANCE_2026_04_21_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-21"
);
const BINANCE_2026_04_21_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-21.json"
);
const BINANCE_2026_04_22_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-22"
);
const BINANCE_2026_04_22_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-22.json"
);
const BINANCE_2026_04_23_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-23"
);
const BINANCE_2026_04_23_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-23.json"
);
const BINANCE_2026_04_24_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-24"
);
const BINANCE_2026_04_24_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-24.json"
);
const BINANCE_2026_04_25_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-25"
);
const BINANCE_2026_04_25_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-25.json"
);
const BINANCE_2026_04_26_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-26"
);
const BINANCE_2026_04_26_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-26.json"
);
const BINANCE_2026_04_27_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-27"
);
const BINANCE_2026_04_27_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-27.json"
);
const BINANCE_2026_04_28_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-28"
);
const BINANCE_2026_04_28_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-28.json"
);
const BINANCE_2026_04_29_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-29"
);
const BINANCE_2026_04_29_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-29.json"
);
const BINANCE_2026_04_30_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-04-30"
);
const BINANCE_2026_04_30_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-04-30.json"
);
const BINANCE_2026_05_01_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-01"
);
const BINANCE_2026_05_01_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-01.json"
);
const BINANCE_2026_05_02_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-02"
);
const BINANCE_2026_05_02_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-02.json"
);
const BINANCE_2026_05_03_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-03"
);
const BINANCE_2026_05_03_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-03.json"
);
const BINANCE_2026_05_04_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-04"
);
const BINANCE_2026_05_04_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-04.json"
);
const BINANCE_2026_05_05_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-05"
);
const BINANCE_2026_05_05_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-05.json"
);
const BINANCE_2026_05_06_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-06"
);
const BINANCE_2026_05_06_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-06.json"
);
const BINANCE_2026_05_07_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-07"
);
const BINANCE_2026_05_07_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-07.json"
);
const BINANCE_2026_05_08_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-08"
);
const BINANCE_2026_05_08_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-08.json"
);
const BINANCE_2026_05_09_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-09"
);
const BINANCE_2026_05_09_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-09.json"
);
const BINANCE_2026_05_10_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-10"
);
const BINANCE_2026_05_10_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-10.json"
);
const BINANCE_2026_05_11_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-11"
);
const BINANCE_2026_05_11_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-11.json"
);
const BINANCE_2026_05_12_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-12"
);
const BINANCE_2026_05_12_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-12.json"
);
const BINANCE_2026_05_13_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-13"
);
const BINANCE_2026_05_13_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-13.json"
);
const BINANCE_2026_05_14_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-14"
);
const BINANCE_2026_05_14_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-14.json"
);
const BINANCE_2026_05_15_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-15"
);
const BINANCE_2026_05_15_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-15.json"
);
const BINANCE_2026_05_16_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-16"
);
const BINANCE_2026_05_16_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-16.json"
);
const BINANCE_2026_05_17_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-17"
);
const BINANCE_2026_05_17_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-17.json"
);
const BINANCE_2026_05_18_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-18"
);
const BINANCE_2026_05_18_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-18.json"
);
const BINANCE_2026_05_19_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-19"
);
const BINANCE_2026_05_19_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-19.json"
);
const BINANCE_2026_05_20_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-20"
);
const BINANCE_2026_05_20_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-20.json"
);
const BINANCE_2026_05_21_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-21"
);
const BINANCE_2026_05_21_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-21.json"
);
const BINANCE_2026_05_22_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-22"
);
const BINANCE_2026_05_22_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-22.json"
);
const BINANCE_2026_05_23_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-23"
);
const BINANCE_2026_05_23_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-23.json"
);
const BINANCE_2026_05_24_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-24"
);
const BINANCE_2026_05_24_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-24.json"
);
const BINANCE_2026_05_25_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-25"
);
const BINANCE_2026_05_25_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-25.json"
);
const BINANCE_2026_05_26_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-26"
);
const BINANCE_2026_05_26_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-26.json"
);
const BINANCE_2026_05_27_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-27"
);
const BINANCE_2026_05_27_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-27.json"
);
const BINANCE_2026_05_28_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-28"
);
const BINANCE_2026_05_28_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-28.json"
);
const BINANCE_2026_05_29_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-29"
);
const BINANCE_2026_05_29_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-29.json"
);
const BINANCE_2026_05_30_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-30"
);
const BINANCE_2026_05_30_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-30.json"
);
const BINANCE_2026_05_31_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-05-31"
);
const BINANCE_2026_05_31_SOURCE_PROOF_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-05-31.json"
);

#[test]
fn binance_backfill_gate_reference_artifacts_match_generic_evaluators() {
    let expected_scope: BackfillSourceProofScopeReport =
        serde_json::from_str(SOURCE_PROOF_SCOPE_REPORT).expect("scope report parses");
    let actual_scope = evaluate_backfill_source_proof_scope(
        expected_scope.report_id.clone(),
        SOURCE_PROOF,
        OBJECT_STAGING_MANIFEST,
    )
    .expect("source-proof scope evaluates");

    assert_eq!(actual_scope, expected_scope);

    let expected_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_str(ACCEPTED_TRANCHE_MANIFEST).expect("accepted tranche parses");
    let actual_tranche = evaluate_backfill_accepted_tranche(
        expected_tranche.tranche_id.clone(),
        &actual_scope,
        &source_proof_scope_hash(&actual_scope),
    )
    .expect("accepted tranche evaluates");

    assert_eq!(actual_tranche, expected_tranche);

    let expected_plan: BackfillExecutionPlan =
        serde_json::from_str(EXECUTION_PLAN).expect("execution plan parses");
    let run_spec: RunSpec = toml::from_str(RUN_SPEC).expect("run spec parses");
    let actual_plan = evaluate_backfill_execution_plan(
        expected_plan.plan_id.clone(),
        sha256_hex(ACCEPTED_TRANCHE_MANIFEST_BYTES),
        &actual_tranche,
        sha256_hex(RUN_SPEC_BYTES),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: expected_plan.max_source_rows,
            max_projected_row_groups: expected_plan.max_projected_row_groups,
            max_wall_seconds: expected_plan.max_wall_seconds,
            require_object_selection_metadata: expected_plan.require_object_selection_metadata,
        },
    );

    assert_eq!(actual_plan, expected_plan);
    assert!(actual_plan.blocking_issues.is_empty());

    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(CATALOG_MAPPING_EVALUATION).expect("mapping evaluation parses");
    let manifest_exposure = &mapping_evaluation
        .nt_surface_evidence
        .current_bte_manifest_exposure;
    assert!(
        manifest_exposure
            .accepted_data_classes
            .iter()
            .any(|data_class| data_class == "TradeTick")
    );
    assert!(
        manifest_exposure
            .accepted_data_classes
            .iter()
            .any(|data_class| data_class == "OrderBookDelta")
    );
    assert!(
        !manifest_exposure
            .rejected_for_now
            .iter()
            .any(|data_class| data_class == "OrderBookDelta")
    );
    assert!(
        manifest_exposure
            .rejected_for_now
            .iter()
            .any(|data_class| data_class == "QuoteTick")
    );
    let expected_catalog_mapping_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("source catalog-mapping readiness parses");
    let actual_catalog_mapping_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_catalog_mapping_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(CATALOG_MAPPING_EVALUATION_BYTES),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_catalog_mapping_readiness.source_proof_id,
            source_proof_version: expected_catalog_mapping_readiness.source_proof_version,
            source_binding: &expected_catalog_mapping_readiness.source_binding,
            required_table_family: &expected_catalog_mapping_readiness.required_table_family,
            required_nt_data_types: expected_catalog_mapping_readiness
                .required_nt_data_types
                .clone(),
            required_claim_evidence_refs: expected_catalog_mapping_readiness
                .required_claim_evidence_refs
                .clone(),
            allowed_current_bte_statuses: expected_catalog_mapping_readiness
                .allowed_current_bte_statuses
                .clone(),
            allowed_parquet_catalog_statuses: expected_catalog_mapping_readiness
                .allowed_parquet_catalog_statuses
                .clone(),
            allowed_usage_scopes: expected_catalog_mapping_readiness
                .allowed_usage_scopes
                .clone(),
        });

    assert_eq!(
        actual_catalog_mapping_readiness,
        expected_catalog_mapping_readiness
    );

    let expected_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(EXECUTION_READINESS_REPORT).expect("execution readiness parses");
    let accepted_tranche_manifest_hash = sha256_hex(ACCEPTED_TRANCHE_MANIFEST_BYTES);
    let execution_plan_hash = sha256_hex(EXECUTION_PLAN_BYTES);
    let source_catalog_mapping_readiness_hash =
        sha256_hex(SOURCE_CATALOG_MAPPING_READINESS_REPORT_BYTES);
    let readiness = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "binance-bnbusdc-2026-03-01-reference-readiness",
        accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
        tranche: &actual_tranche,
        execution_plan_hash: &execution_plan_hash,
        plan: &actual_plan,
        required_table_family: &actual_plan.table_family,
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
            table_family: actual_plan.table_family.clone(),
            nt_data_type: "TradeTick".to_string(),
        }],
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some(&source_catalog_mapping_readiness_hash),
        source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
    });

    assert_eq!(readiness, expected_readiness);
    assert_eq!(readiness.status, BackfillExecutionReadinessStatus::Ready);
    assert!(readiness.blockers.is_empty());

    let artifact_index_proof: ArtifactIndexCommitProofEvidence =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("Artifact Index IAM-scope proof report parses");
    let mut expected_artifact_index_required_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(ARTIFACT_INDEX_REQUIRED_EXECUTION_READINESS_REPORT)
            .expect("Artifact Index-required execution readiness parses");
    expected_artifact_index_required_readiness.blockers.insert(
        0,
        BackfillExecutionReadinessBlocker::ArtifactIndexCommitMechanicsUnproven,
    );
    let artifact_index_required_readiness =
        evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
            readiness_id: "binance-bnbusdc-2026-03-01-artifact-index-required-readiness",
            accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
            tranche: &actual_tranche,
            execution_plan_hash: &execution_plan_hash,
            plan: &actual_plan,
            required_table_family: &actual_plan.table_family,
            required_nt_data_type: "TradeTick",
            required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
                table_family: actual_plan.table_family.clone(),
                nt_data_type: "TradeTick".to_string(),
            }],
            artifact_index_commit_required: true,
            required_artifact_index_kind: Some(ArtifactKind::Backtests),
            artifact_index_commit_proof_report_hash: Some(&sha256_hex(
                ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES,
            )),
            artifact_index_commit_proof_report: Some(&artifact_index_proof),
            source_selection_readiness_required: false,
            source_selection_readiness_report_hash: None,
            source_selection_readiness_report: None,
            source_catalog_mapping_readiness_required: true,
            source_catalog_mapping_readiness_report_hash: Some(
                &source_catalog_mapping_readiness_hash,
            ),
            source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
        });

    assert_eq!(
        artifact_index_required_readiness,
        expected_artifact_index_required_readiness
    );
    assert_eq!(
        artifact_index_required_readiness.status,
        BackfillExecutionReadinessStatus::Blocked
    );
    assert_eq!(
        artifact_index_required_readiness.blockers,
        vec![
            BackfillExecutionReadinessBlocker::ArtifactIndexCommitMechanicsUnproven,
            BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven,
        ]
    );
}

#[test]
fn binance_backfill_gate_commits_materialized_run_spec_before_execution_plan() {
    let materialization_spec_path =
        Path::new(BINANCE_GATE_ROOT).join("backfill-run-spec-materialization.toml");
    let materialized_run_spec_path = Path::new(BINANCE_GATE_ROOT)
        .join("materialized-run-spec")
        .join("backfill-run-spec.toml");

    assert!(
        materialization_spec_path.exists(),
        "Binance reference gate must commit the run-spec materialization spec"
    );
    assert!(
        materialized_run_spec_path.exists(),
        "Binance reference gate must commit the materialized run spec used by the execution plan"
    );
    assert!(
        RUN_SPEC.contains(r#"usage_scope = "canonical_backfill_input""#),
        "Binance materialized run spec must explicitly bind canonical source usage scope"
    );
    assert!(
        EXECUTION_PLAN_SPEC.contains(
            "backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
        ),
        "Binance reference execution plan must consume the materialized run spec"
    );
}

#[test]
fn binance_2026_03_02_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_02_GATE_ROOT),
        Path::new(BINANCE_2026_03_02_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_03_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_03_GATE_ROOT),
        Path::new(BINANCE_2026_03_03_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_04_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_04_GATE_ROOT),
        Path::new(BINANCE_2026_03_04_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_05_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_05_GATE_ROOT),
        Path::new(BINANCE_2026_03_05_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_06_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_06_GATE_ROOT),
        Path::new(BINANCE_2026_03_06_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_07_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_07_GATE_ROOT),
        Path::new(BINANCE_2026_03_07_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_08_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_08_GATE_ROOT),
        Path::new(BINANCE_2026_03_08_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_09_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_09_GATE_ROOT),
        Path::new(BINANCE_2026_03_09_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_10_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_10_GATE_ROOT),
        Path::new(BINANCE_2026_03_10_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_11_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_11_GATE_ROOT),
        Path::new(BINANCE_2026_03_11_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_12_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_12_GATE_ROOT),
        Path::new(BINANCE_2026_03_12_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_13_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_13_GATE_ROOT),
        Path::new(BINANCE_2026_03_13_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_14_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_14_GATE_ROOT),
        Path::new(BINANCE_2026_03_14_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_15_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_15_GATE_ROOT),
        Path::new(BINANCE_2026_03_15_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_16_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_16_GATE_ROOT),
        Path::new(BINANCE_2026_03_16_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_17_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_17_GATE_ROOT),
        Path::new(BINANCE_2026_03_17_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_18_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_18_GATE_ROOT),
        Path::new(BINANCE_2026_03_18_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_19_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_19_GATE_ROOT),
        Path::new(BINANCE_2026_03_19_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_20_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_20_GATE_ROOT),
        Path::new(BINANCE_2026_03_20_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_21_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_21_GATE_ROOT),
        Path::new(BINANCE_2026_03_21_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_22_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_22_GATE_ROOT),
        Path::new(BINANCE_2026_03_22_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_23_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_23_GATE_ROOT),
        Path::new(BINANCE_2026_03_23_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_24_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_24_GATE_ROOT),
        Path::new(BINANCE_2026_03_24_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_25_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_25_GATE_ROOT),
        Path::new(BINANCE_2026_03_25_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_26_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_26_GATE_ROOT),
        Path::new(BINANCE_2026_03_26_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_27_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_27_GATE_ROOT),
        Path::new(BINANCE_2026_03_27_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_28_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_28_GATE_ROOT),
        Path::new(BINANCE_2026_03_28_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_29_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_29_GATE_ROOT),
        Path::new(BINANCE_2026_03_29_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_30_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_30_GATE_ROOT),
        Path::new(BINANCE_2026_03_30_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_03_31_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_03_31_GATE_ROOT),
        Path::new(BINANCE_2026_03_31_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_01_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_01_GATE_ROOT),
        Path::new(BINANCE_2026_04_01_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_02_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_02_GATE_ROOT),
        Path::new(BINANCE_2026_04_02_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_03_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_03_GATE_ROOT),
        Path::new(BINANCE_2026_04_03_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_04_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_04_GATE_ROOT),
        Path::new(BINANCE_2026_04_04_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_05_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_05_GATE_ROOT),
        Path::new(BINANCE_2026_04_05_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_06_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_06_GATE_ROOT),
        Path::new(BINANCE_2026_04_06_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_07_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_07_GATE_ROOT),
        Path::new(BINANCE_2026_04_07_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_08_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_08_GATE_ROOT),
        Path::new(BINANCE_2026_04_08_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_09_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_09_GATE_ROOT),
        Path::new(BINANCE_2026_04_09_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_10_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_10_GATE_ROOT),
        Path::new(BINANCE_2026_04_10_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_11_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_11_GATE_ROOT),
        Path::new(BINANCE_2026_04_11_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_12_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_12_GATE_ROOT),
        Path::new(BINANCE_2026_04_12_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_13_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_13_GATE_ROOT),
        Path::new(BINANCE_2026_04_13_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_14_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_14_GATE_ROOT),
        Path::new(BINANCE_2026_04_14_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_15_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_15_GATE_ROOT),
        Path::new(BINANCE_2026_04_15_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_16_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_16_GATE_ROOT),
        Path::new(BINANCE_2026_04_16_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_17_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_17_GATE_ROOT),
        Path::new(BINANCE_2026_04_17_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_18_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_18_GATE_ROOT),
        Path::new(BINANCE_2026_04_18_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_19_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_19_GATE_ROOT),
        Path::new(BINANCE_2026_04_19_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_20_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_20_GATE_ROOT),
        Path::new(BINANCE_2026_04_20_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_21_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_21_GATE_ROOT),
        Path::new(BINANCE_2026_04_21_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_22_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_22_GATE_ROOT),
        Path::new(BINANCE_2026_04_22_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_23_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_23_GATE_ROOT),
        Path::new(BINANCE_2026_04_23_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_24_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_24_GATE_ROOT),
        Path::new(BINANCE_2026_04_24_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_25_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_25_GATE_ROOT),
        Path::new(BINANCE_2026_04_25_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_26_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_26_GATE_ROOT),
        Path::new(BINANCE_2026_04_26_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_27_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_27_GATE_ROOT),
        Path::new(BINANCE_2026_04_27_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_28_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_28_GATE_ROOT),
        Path::new(BINANCE_2026_04_28_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_29_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_29_GATE_ROOT),
        Path::new(BINANCE_2026_04_29_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_04_30_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_04_30_GATE_ROOT),
        Path::new(BINANCE_2026_04_30_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_01_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_01_GATE_ROOT),
        Path::new(BINANCE_2026_05_01_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_02_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_02_GATE_ROOT),
        Path::new(BINANCE_2026_05_02_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_03_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_03_GATE_ROOT),
        Path::new(BINANCE_2026_05_03_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_04_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_04_GATE_ROOT),
        Path::new(BINANCE_2026_05_04_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_05_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_05_GATE_ROOT),
        Path::new(BINANCE_2026_05_05_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_06_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_06_GATE_ROOT),
        Path::new(BINANCE_2026_05_06_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_07_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_07_GATE_ROOT),
        Path::new(BINANCE_2026_05_07_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_08_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_08_GATE_ROOT),
        Path::new(BINANCE_2026_05_08_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_09_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_09_GATE_ROOT),
        Path::new(BINANCE_2026_05_09_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_10_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_10_GATE_ROOT),
        Path::new(BINANCE_2026_05_10_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_11_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_11_GATE_ROOT),
        Path::new(BINANCE_2026_05_11_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_12_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_12_GATE_ROOT),
        Path::new(BINANCE_2026_05_12_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_13_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_13_GATE_ROOT),
        Path::new(BINANCE_2026_05_13_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_14_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_14_GATE_ROOT),
        Path::new(BINANCE_2026_05_14_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_15_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_15_GATE_ROOT),
        Path::new(BINANCE_2026_05_15_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_16_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_16_GATE_ROOT),
        Path::new(BINANCE_2026_05_16_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_17_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_17_GATE_ROOT),
        Path::new(BINANCE_2026_05_17_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_18_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_18_GATE_ROOT),
        Path::new(BINANCE_2026_05_18_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_19_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_19_GATE_ROOT),
        Path::new(BINANCE_2026_05_19_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_20_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_20_GATE_ROOT),
        Path::new(BINANCE_2026_05_20_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_21_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_21_GATE_ROOT),
        Path::new(BINANCE_2026_05_21_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_22_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_22_GATE_ROOT),
        Path::new(BINANCE_2026_05_22_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_23_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_23_GATE_ROOT),
        Path::new(BINANCE_2026_05_23_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_24_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_24_GATE_ROOT),
        Path::new(BINANCE_2026_05_24_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_25_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_25_GATE_ROOT),
        Path::new(BINANCE_2026_05_25_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_26_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_26_GATE_ROOT),
        Path::new(BINANCE_2026_05_26_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_27_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_27_GATE_ROOT),
        Path::new(BINANCE_2026_05_27_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_28_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_28_GATE_ROOT),
        Path::new(BINANCE_2026_05_28_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_29_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_29_GATE_ROOT),
        Path::new(BINANCE_2026_05_29_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_30_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_30_GATE_ROOT),
        Path::new(BINANCE_2026_05_30_SOURCE_PROOF_PATH),
    );
}

#[test]
fn binance_2026_05_31_backfill_gate_reference_artifacts_match_generic_evaluators() {
    assert_binance_gate_matches_generic_evaluators(
        Path::new(BINANCE_2026_05_31_GATE_ROOT),
        Path::new(BINANCE_2026_05_31_SOURCE_PROOF_PATH),
    );
}

#[test]
fn blocked_canonical_source_catalog_mapping_reference_artifact_matches_generic_evaluator() {
    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(BLOCKED_CATALOG_MAPPING_EVALUATION)
            .expect("mapping evaluation parses");
    let expected_blocked_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("blocked source catalog-mapping readiness parses");

    let actual_blocked_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_blocked_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(BLOCKED_CATALOG_MAPPING_EVALUATION_BYTES),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_blocked_readiness.source_proof_id,
            source_proof_version: expected_blocked_readiness.source_proof_version,
            source_binding: &expected_blocked_readiness.source_binding,
            required_table_family: &expected_blocked_readiness.required_table_family,
            required_nt_data_types: expected_blocked_readiness.required_nt_data_types.clone(),
            required_claim_evidence_refs: expected_blocked_readiness
                .required_claim_evidence_refs
                .clone(),
            allowed_current_bte_statuses: expected_blocked_readiness
                .allowed_current_bte_statuses
                .clone(),
            allowed_parquet_catalog_statuses: expected_blocked_readiness
                .allowed_parquet_catalog_statuses
                .clone(),
            allowed_usage_scopes: expected_blocked_readiness.allowed_usage_scopes.clone(),
        });

    assert_eq!(actual_blocked_readiness, expected_blocked_readiness);
    assert_eq!(
        actual_blocked_readiness.status,
        SourceCatalogMappingReadinessStatus::Blocked
    );
    assert!(
        actual_blocked_readiness
            .blockers
            .contains(&SourceCatalogMappingReadinessBlocker::RequiredClaimEvidenceMissing)
    );
    assert!(!actual_blocked_readiness.blockers.is_empty());
}

#[test]
fn artifact_index_commit_status_references_committed_proof_reports() {
    let status: serde_json::Value =
        serde_json::from_str(ARTIFACT_INDEX_COMMIT_STATUS).expect("Artifact Index status parses");
    let direct_s3_proof: ArtifactIndexCommitProofReportV1 =
        serde_json::from_str(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT)
            .expect("direct S3 proof report parses");
    let iam_scope_proof: ArtifactIndexCommitProofReportV1 =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("IAM-scope proof report parses");

    assert!(direct_s3_proof.direct_s3_commit_proven);
    assert!(direct_s3_proof.event_create_only_proven);
    assert!(direct_s3_proof.latest_pointer_update_if_match_proven);
    assert!(!iam_scope_proof.producer_iam_scope_proven);
    assert_eq!(iam_scope_proof.producer_iam_scope_denied_write_attempts, 3);
    assert_eq!(
        iam_scope_proof.producer_iam_scope_denied_write_rejections,
        0
    );
    assert_eq!(iam_scope_proof.producer_iam_scope_violation_count, 3);

    assert_eq!(
        status["store_commit_mechanics"]["report_file_sha256"]
            .as_str()
            .expect("store commit proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["historical_generic_credential_failure"]["report_file_sha256"]
            .as_str()
            .expect("historical producer IAM proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["current_backtests_producer_proof"]["proof_file_sha256"]
            .as_str()
            .expect("backtests producer proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_BACKTESTS_COMPLETE_PROOF_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["all_current_producer_proof"]["proof_file_sha256"]
            .as_str()
            .expect("all-producer proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_ALL_PRODUCER_SCOPE_PROOF_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["all_current_producer_proof"]
            ["combined_denied_write_attempts"]
            .as_u64(),
        Some(90)
    );
    assert_eq!(
        status["producer_iam_scope"]["all_current_producer_proof"]
            ["combined_denied_write_rejections"]
            .as_u64(),
        Some(90)
    );
    assert_eq!(
        status["producer_iam_scope"]["all_current_producer_proof"]["combined_violation_count"]
            .as_u64(),
        Some(0)
    );
    assert_eq!(status["bte_006_can_close"].as_bool(), Some(true));
}

#[test]
fn binance_bnbusdc_venue_coverage_ledger_binds_all_accepted_tranches() {
    let ledger_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/023-nt-research-analytics-platform/reference/backfill-coverage-ledgers/binance-bnbusdc-2026-03-01-2026-05-31",
    );
    let spec_path = ledger_root.join("backfill-coverage-ledger.toml");
    let ledger_path = ledger_root.join("ledger/backfill-coverage-ledger.json");
    let spec: toml::Value =
        toml::from_str(&read_required_string(&spec_path)).expect("coverage ledger spec parses");
    let manifests = spec["manifest"]
        .as_array()
        .expect("coverage ledger spec has manifest entries");
    assert_eq!(manifests.len(), 92);
    for manifest in manifests {
        assert_eq!(manifest["coverage_axis"].as_str(), Some("archive_date"));
        assert_eq!(manifest["source_proof_status"].as_str(), Some("accepted"));
        assert_eq!(manifest["write_mode"].as_str(), Some("s3_staging"));
        assert_eq!(manifest["canonical_s3_write"].as_bool(), Some(false));
    }

    let ledger: BackfillCoverageLedger =
        serde_json::from_str(&read_required_string(&ledger_path)).expect("coverage ledger parses");
    assert_eq!(
        ledger.ledger_id,
        "backfill-coverage-ledger-binance-bnbusdc-2026-03-01-2026-05-31"
    );
    assert_eq!(ledger.summary.total_records, 92);
    assert_eq!(ledger.summary.accepted_records, 92);
    assert_eq!(ledger.summary.accepted_objects, 92);
    assert_eq!(ledger.summary.accepted_bytes, 66_451_476);
    assert_eq!(ledger.summary.rejected_records, 0);
    assert_eq!(ledger.summary.physical_only_records, 0);
    assert_eq!(ledger.summary.canonical_ready_records, 0);
    assert!(
        ledger
            .records
            .iter()
            .all(|record| record.status == BackfillCoverageStatus::Accepted),
        "all Binance BNBUSDC venue coverage records must be accepted"
    );
    assert_eq!(
        ledger
            .records
            .first()
            .map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-binance-bnbusdc-2026-03-01")
    );
    assert_eq!(
        ledger
            .records
            .last()
            .map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-binance-bnbusdc-2026-05-31")
    );
}

#[test]
fn binance_bnbusdc_venue_conversion_batch_binds_accepted_coverage_to_operator_inputs() {
    let batch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31",
    );
    let spec_path = batch_root.join("backfill-conversion-batch-plan.toml");
    let spec: toml::Value =
        toml::from_str(&read_required_string(&spec_path)).expect("conversion batch spec parses");
    let inputs = spec["input"]
        .as_array()
        .expect("conversion batch spec has input entries");
    assert_eq!(inputs.len(), 92);
    assert_eq!(spec["selection"]["max_records"].as_integer(), Some(92));
    assert_eq!(
        spec["selection"]["max_accepted_objects"].as_integer(),
        Some(92)
    );
    assert_eq!(
        spec["selection"]["max_accepted_bytes"].as_integer(),
        Some(66_451_476)
    );
    assert_eq!(
        spec["selection"]["require_uniform_source_binding"].as_bool(),
        Some(true)
    );
    assert_eq!(spec["selection"]["allow_gaps"].as_bool(), Some(false));

    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let plan = generated_binance_bnbusdc_conversion_batch_plan(&reference_root);
    assert_eq!(
        plan.batch_id,
        "backfill-conversion-batch-binance-bnbusdc-2026-03-01-2026-05-31"
    );
    assert_eq!(
        plan.coverage_ledger_id,
        "backfill-coverage-ledger-binance-bnbusdc-2026-03-01-2026-05-31"
    );
    assert_eq!(plan.status, BackfillConversionBatchStatus::Ready);
    assert_eq!(plan.record_count, 92);
    assert_eq!(plan.total_accepted_objects, 92);
    assert_eq!(plan.total_accepted_bytes, 66_451_476);
    assert_eq!(plan.canonical_ready_records, 0);
    assert!(plan.blocking_issues.is_empty());
    assert!(
        plan.records.iter().all(|record| {
            record.source_binding == "binance-spot-native-trades"
                && record.table_family == "trades"
                && record.coverage_axis == "archive_date"
                && !record.canonical_ready
                && record.accepted_objects == 1
        }),
        "all Binance BNBUSDC venue conversion records must bind one accepted staging object"
    );
    assert_eq!(
        plan.records.first().map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-binance-bnbusdc-2026-03-01")
    );
    assert_eq!(
        plan.records.last().map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-binance-bnbusdc-2026-05-31")
    );
}

#[test]
fn binance_bnbusdc_venue_publication_and_mapping_evidence_cover_all_accepted_tranches() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let plan = generated_binance_bnbusdc_conversion_batch_plan(&reference_root);
    assert_eq!(plan.records.len(), 92);

    for record in &plan.records {
        let archive_date = record
            .record_id
            .strip_prefix("backfill-accepted-tranche-binance-bnbusdc-")
            .expect("Binance BNBUSDC record id carries archive date");
        let publication_path = single_reference_file_with_prefix_suffix(
            &reference_root,
            &format!("binance-bnbusdc-{archive_date}-accepted-publication-evidence."),
            ".json",
        );
        let mapping_path = single_reference_file_with_prefix_suffix(
            &reference_root,
            &format!(
                "source-proof-nt-catalog-mapping-evaluation.backtesting-engine.binance-bnbusdc-{archive_date}."
            ),
            ".json",
        );

        let publication: serde_json::Value =
            serde_json::from_str(&read_required_string(&publication_path))
                .expect("accepted publication evidence parses");
        assert_eq!(
            publication["scope"]["archive_date"].as_str(),
            Some(archive_date)
        );
        assert_eq!(
            publication["scope"]["status"].as_str(),
            Some("accepted_gate_committed_and_s3_published")
        );
        assert_eq!(
            publication["accepted_conversion_and_publication"]["published_catalog_direct_s3"]
                .as_bool(),
            Some(true)
        );

        let gate_root =
            reference_root.join(format!("backfill-gates/binance-bnbusdc-{archive_date}"));
        let readiness_spec_path = gate_root.join("source-catalog-mapping-readiness.toml");
        let readiness_spec: SourceCatalogMappingReadinessSpec =
            toml::from_str(&read_required_string(&readiness_spec_path))
                .expect("source catalog-mapping readiness spec parses");
        assert_eq!(
            repo_relative_path(&readiness_spec.catalog_mapping_evaluation_path),
            mapping_path
        );
        let readiness_report_path = gate_root
            .join("source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json");
        let readiness_report: SourceCatalogMappingReadinessReport =
            serde_json::from_str(&read_required_string(&readiness_report_path))
                .expect("source catalog-mapping readiness report parses");
        assert_eq!(
            readiness_report.status,
            SourceCatalogMappingReadinessStatus::Ready
        );
        assert_eq!(
            readiness_report.catalog_mapping_evaluation_hash,
            sha256_hex(&read_required_bytes(&mapping_path))
        );
    }
}

#[test]
fn bybit_bnbusdc_venue_backfill_gate_reference_artifacts_match_generic_evaluators() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let plan = generated_bybit_bnbusdc_conversion_batch_plan(&reference_root);
    assert_eq!(plan.records.len(), 93);

    for record in &plan.records {
        let archive_date = record
            .record_id
            .strip_prefix("backfill-accepted-tranche-bybit-bnbusdc-")
            .expect("Bybit BNBUSDC record id carries archive date");
        let gate_root = reference_root.join(format!("backfill-gates/bybit-bnbusdc-{archive_date}"));
        let source_proof_path = reference_root.join(format!(
            "backtesting-vertical-slice-accepted-source-proof.bybit-bnbusdc-{archive_date}.json"
        ));
        assert_binance_gate_matches_generic_evaluators(&gate_root, &source_proof_path);
    }
}

#[test]
fn bybit_bnbusdc_venue_coverage_ledger_binds_all_accepted_tranches() {
    let ledger_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/023-nt-research-analytics-platform/reference/backfill-coverage-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01",
    );
    let spec_path = ledger_root.join("backfill-coverage-ledger.toml");
    let ledger_path = ledger_root.join("ledger/backfill-coverage-ledger.json");
    let spec: toml::Value =
        toml::from_str(&read_required_string(&spec_path)).expect("coverage ledger spec parses");
    let manifests = spec["manifest"]
        .as_array()
        .expect("coverage ledger spec has manifest entries");
    assert_eq!(manifests.len(), 93);
    for manifest in manifests {
        assert_eq!(manifest["coverage_axis"].as_str(), Some("archive_date"));
        assert_eq!(manifest["source_proof_status"].as_str(), Some("accepted"));
        assert_eq!(manifest["write_mode"].as_str(), Some("s3_staging"));
        assert_eq!(manifest["canonical_s3_write"].as_bool(), Some(false));
    }

    let ledger: BackfillCoverageLedger =
        serde_json::from_str(&read_required_string(&ledger_path)).expect("coverage ledger parses");
    assert_eq!(
        ledger.ledger_id,
        "backfill-coverage-ledger-bybit-bnbusdc-2026-03-01-2026-06-01"
    );
    assert_eq!(ledger.summary.total_records, 93);
    assert_eq!(ledger.summary.accepted_records, 93);
    assert_eq!(ledger.summary.accepted_objects, 93);
    assert_eq!(ledger.summary.accepted_bytes, 1_156_784);
    assert_eq!(ledger.summary.rejected_records, 0);
    assert_eq!(ledger.summary.physical_only_records, 0);
    assert_eq!(ledger.summary.canonical_ready_records, 0);
    assert!(
        ledger
            .records
            .iter()
            .all(|record| record.status == BackfillCoverageStatus::Accepted),
        "all Bybit BNBUSDC venue coverage records must be accepted"
    );
    assert_eq!(
        ledger
            .records
            .first()
            .map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-bybit-bnbusdc-2026-03-01")
    );
    assert_eq!(
        ledger
            .records
            .last()
            .map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-bybit-bnbusdc-2026-06-01")
    );
}

#[test]
fn bybit_bnbusdc_venue_conversion_batch_binds_accepted_coverage_to_operator_inputs() {
    let batch_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/bybit-bnbusdc-2026-03-01-2026-06-01",
    );
    let spec_path = batch_root.join("backfill-conversion-batch-plan.toml");
    let spec: toml::Value =
        toml::from_str(&read_required_string(&spec_path)).expect("conversion batch spec parses");
    let inputs = spec["input"]
        .as_array()
        .expect("conversion batch spec has input entries");
    assert_eq!(inputs.len(), 93);
    assert_eq!(spec["selection"]["max_records"].as_integer(), Some(93));
    assert_eq!(
        spec["selection"]["max_accepted_objects"].as_integer(),
        Some(93)
    );
    assert_eq!(
        spec["selection"]["max_accepted_bytes"].as_integer(),
        Some(1_156_784)
    );
    assert_eq!(
        spec["selection"]["require_uniform_source_binding"].as_bool(),
        Some(true)
    );
    assert_eq!(spec["selection"]["allow_gaps"].as_bool(), Some(false));

    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let plan = generated_bybit_bnbusdc_conversion_batch_plan(&reference_root);
    assert_eq!(
        plan.batch_id,
        "backfill-conversion-batch-bybit-bnbusdc-2026-03-01-2026-06-01"
    );
    assert_eq!(
        plan.coverage_ledger_id,
        "backfill-coverage-ledger-bybit-bnbusdc-2026-03-01-2026-06-01"
    );
    assert_eq!(plan.status, BackfillConversionBatchStatus::Ready);
    assert_eq!(plan.record_count, 93);
    assert_eq!(plan.total_accepted_objects, 93);
    assert_eq!(plan.total_accepted_bytes, 1_156_784);
    assert_eq!(plan.canonical_ready_records, 0);
    assert!(plan.blocking_issues.is_empty());
    assert!(
        plan.records.iter().all(|record| {
            record.source_binding == "bybit-spot-tick-trades"
                && record.table_family == "trades"
                && record.coverage_axis == "archive_date"
                && !record.canonical_ready
                && record.accepted_objects == 1
        }),
        "all Bybit BNBUSDC venue conversion records must bind one accepted staging object"
    );
    assert_eq!(
        plan.records.first().map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-bybit-bnbusdc-2026-03-01")
    );
    assert_eq!(
        plan.records.last().map(|record| record.record_id.as_str()),
        Some("backfill-accepted-tranche-bybit-bnbusdc-2026-06-01")
    );
}

#[test]
fn bybit_public_archive_tick_trade_source_universe_covers_all_staged_categories_and_instruments() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let universe_path = reference_root.join(
        "backfill-source-universes/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-source-universe.json",
    );
    let universe: serde_json::Value = serde_json::from_str(&read_required_string(&universe_path))
        .expect("Bybit source universe parses");

    assert_eq!(
        universe["schema_version"].as_str(),
        Some("backfill-source-universe.v1")
    );
    assert_eq!(universe["venue"].as_str(), Some("bybit"));
    assert_eq!(universe["source"].as_str(), Some("public_archive"));
    assert_eq!(universe["family"].as_str(), Some("tick_trades"));
    assert_eq!(
        universe["accepted_scope"]["commit_granularity"].as_str(),
        Some("venue_source_family_instrument_universe")
    );
    assert_eq!(
        universe["accepted_scope"]["symbol_or_day_commit_granularity"].as_str(),
        Some("rejected")
    );

    let summary = &universe["summary"];
    assert_eq!(summary["object_count"].as_u64(), Some(5_857));
    assert_eq!(summary["category_count"].as_u64(), Some(3));
    assert_eq!(summary["unique_symbol_count"].as_u64(), Some(97));
    assert_eq!(summary["category_symbol_count"].as_u64(), Some(106));
    assert_eq!(summary["archive_date_count"].as_u64(), Some(94));
    assert_eq!(summary["first_archive_date"].as_str(), Some("2025-06-01"));
    assert_eq!(summary["last_archive_date"].as_str(), Some("2026-06-01"));
    assert_eq!(summary["compressed_bytes"].as_u64(), Some(20_309_079_098));

    let categories = universe["categories"]
        .as_array()
        .expect("universe categories array");
    let categories_by_name: std::collections::BTreeMap<&str, &serde_json::Value> = categories
        .iter()
        .map(|category| {
            (
                category["category"].as_str().expect("category name"),
                category,
            )
        })
        .collect();
    assert_eq!(
        categories_by_name.keys().copied().collect::<Vec<_>>(),
        vec!["inverse", "linear", "spot"]
    );

    for (
        category,
        source_binding,
        instruments,
        objects,
        bytes,
        sample_symbol,
        timestamp_unit,
        trade_id_column,
        size_column,
        schema_column_count,
    ) in [
        (
            "spot",
            "bybit-spot-tick-trades",
            58,
            3_304,
            1_264_254_131,
            "BNBUSDC",
            "milliseconds",
            "id",
            "volume",
            6,
        ),
        (
            "linear",
            "bybit-linear-tick-trades",
            38,
            1_851,
            18_419_832_484,
            "BTCUSDT",
            "decimal_seconds",
            "trdMatchID",
            "size",
            11,
        ),
        (
            "inverse",
            "bybit-inverse-tick-trades",
            10,
            702,
            624_992_483,
            "BTCUSD",
            "decimal_seconds",
            "trdMatchID",
            "size",
            11,
        ),
    ] {
        let category_entry = categories_by_name
            .get(category)
            .unwrap_or_else(|| panic!("missing category {category}"));
        assert_eq!(
            category_entry["source_binding"].as_str(),
            Some(source_binding)
        );
        assert!(
            category_entry["source_uri_template"]
                .as_str()
                .expect("source uri template")
                .contains("{symbol}")
        );
        assert_eq!(
            category_entry["instrument_count"].as_u64(),
            Some(instruments)
        );
        assert_eq!(category_entry["object_count"].as_u64(), Some(objects));
        assert_eq!(category_entry["compressed_bytes"].as_u64(), Some(bytes));
        assert_eq!(
            category_entry["schema_columns"]
                .as_array()
                .expect("schema columns array")
                .len(),
            schema_column_count
        );
        assert_eq!(
            category_entry["converter_csv"]["timestamp_unit"].as_str(),
            Some(timestamp_unit)
        );
        assert_eq!(
            category_entry["converter_csv"]["trade_id_column"].as_str(),
            Some(trade_id_column)
        );
        assert_eq!(
            category_entry["converter_csv"]["size_column"].as_str(),
            Some(size_column)
        );
        let instrument_entries = category_entry["instruments"]
            .as_array()
            .expect("category instruments array");
        assert_eq!(instrument_entries.len() as u64, instruments);
        assert!(
            instrument_entries.iter().all(|instrument| {
                instrument["category"].as_str() == Some(category)
                    && instrument["source_binding"].as_str() == Some(source_binding)
                    && instrument["sample_staged_object"]
                        .as_str()
                        .is_some_and(|uri| uri.starts_with("s3://bolt-parquet/backfill-staging/"))
            }),
            "all {category} instruments must bind the category source binding and staged S3 object"
        );
        assert!(
            instrument_entries
                .iter()
                .any(|instrument| instrument["symbol"].as_str() == Some(sample_symbol)),
            "{category} universe missing representative symbol {sample_symbol}"
        );
    }

    let registry: toml::Value = toml::from_str(&read_required_string(
        &reference_root.join("backfill-source-bindings.v1.toml"),
    ))
    .expect("source bindings registry parses");
    let configured_keys: std::collections::BTreeSet<&str> = registry["source_binding"]
        .as_array()
        .expect("source bindings array")
        .iter()
        .filter_map(|binding| binding["key"].as_str())
        .collect();
    let universe_binding_keys: std::collections::BTreeSet<&str> = universe["source_bindings"]
        .as_array()
        .expect("universe source bindings array")
        .iter()
        .map(|binding| binding["key"].as_str().expect("universe binding key"))
        .collect();
    assert_eq!(
        universe_binding_keys,
        std::collections::BTreeSet::from([
            "bybit-inverse-tick-trades",
            "bybit-linear-tick-trades",
            "bybit-spot-tick-trades",
        ])
    );
    assert!(
        universe_binding_keys
            .iter()
            .all(|binding| configured_keys.contains(binding)),
        "all universe bindings must exist in the committed source-binding registry"
    );
}

#[test]
fn bybit_public_archive_tick_trade_conversion_plan_covers_all_instruments_and_categories() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let universe_path = reference_root.join(
        "backfill-source-universes/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-source-universe.json",
    );
    let plan_path = reference_root.join(
        "backfill-source-universe-conversion-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-conversion-plan.json",
    );
    let universe: serde_json::Value = serde_json::from_str(&read_required_string(&universe_path))
        .expect("Bybit source universe parses");
    let plan: serde_json::Value =
        serde_json::from_str(&read_required_string(&plan_path)).expect("conversion plan parses");

    assert_eq!(
        plan["schema_version"].as_str(),
        Some("backfill-source-universe-conversion-plan.v1")
    );
    assert_eq!(
        plan["universe_id"].as_str(),
        universe["universe_id"].as_str()
    );
    assert_eq!(
        plan["status"].as_str(),
        Some("conversion_run_plan_materialized")
    );
    assert_eq!(
        plan["selection"]["instrument_universe"].as_str(),
        Some("all_staged_category_symbols")
    );
    assert_eq!(
        plan["selection"]["commit_granularity"].as_str(),
        Some("venue_source_family_instrument_universe")
    );
    assert_eq!(
        plan["category_split_policy"]["symbol_or_day_commit_granularity"].as_str(),
        Some("rejected")
    );
    assert_eq!(
        plan["category_split_policy"]["allowed_only_for_source_contract_or_schema_boundaries"]
            .as_bool(),
        Some(true)
    );

    let universe_summary = &universe["summary"];
    let plan_summary = &plan["source_universe_summary"];
    for summary_field in [
        "object_count",
        "category_count",
        "unique_symbol_count",
        "category_symbol_count",
        "archive_date_count",
        "compressed_bytes",
    ] {
        assert_eq!(
            plan_summary[summary_field].as_u64(),
            universe_summary[summary_field].as_u64(),
            "conversion plan summary field {summary_field} must match the source universe"
        );
    }
    for summary_field in ["first_archive_date", "last_archive_date"] {
        assert_eq!(
            plan_summary[summary_field].as_str(),
            universe_summary[summary_field].as_str(),
            "conversion plan summary field {summary_field} must match the source universe"
        );
    }

    assert_eq!(
        plan["operator_contract"]["run_spec_gate_granularity"].as_str(),
        Some("single_staged_object")
    );
    assert_eq!(
        plan["operator_contract"]["required_gate_records"].as_u64(),
        universe_summary["object_count"].as_u64()
    );
    assert_eq!(
        plan["operator_contract"]["required_category_batches"].as_u64(),
        universe_summary["category_count"].as_u64()
    );

    let plan_categories = plan["category_batches"]
        .as_array()
        .expect("conversion plan category batches");
    let universe_categories = universe["categories"]
        .as_array()
        .expect("source universe categories");
    let universe_categories_by_name: std::collections::BTreeMap<&str, &serde_json::Value> =
        universe_categories
            .iter()
            .map(|category| {
                (
                    category["category"].as_str().expect("category name"),
                    category,
                )
            })
            .collect();
    let plan_category_names: std::collections::BTreeSet<&str> = plan_categories
        .iter()
        .map(|category| category["category"].as_str().expect("plan category name"))
        .collect();
    assert_eq!(
        plan_category_names,
        std::collections::BTreeSet::from(["inverse", "linear", "spot"])
    );

    let mut planned_gate_records = 0;
    for category in plan_categories {
        let category_name = category["category"].as_str().expect("plan category name");
        let universe_category = universe_categories_by_name
            .get(category_name)
            .unwrap_or_else(|| panic!("missing source universe category {category_name}"));
        assert_eq!(
            category["source_binding"].as_str(),
            universe_category["source_binding"].as_str()
        );
        assert_eq!(
            category["instrument_count"].as_u64(),
            universe_category["instrument_count"].as_u64()
        );
        assert_eq!(
            category["object_count"].as_u64(),
            universe_category["object_count"].as_u64()
        );
        assert_eq!(
            category["compressed_bytes"].as_u64(),
            universe_category["compressed_bytes"].as_u64()
        );
        assert_eq!(
            category["converter_csv"], universe_category["converter_csv"],
            "{category_name} converter mapping must match the source universe contract"
        );
        assert_eq!(
            category["category_split_reason"].as_str(),
            Some("source_contract_or_schema_boundary")
        );
        assert_eq!(
            category["status"].as_str(),
            Some("converter_mapping_configured")
        );
        planned_gate_records += category["required_gate_records"]
            .as_u64()
            .expect("category required gate records");
    }
    assert_eq!(
        Some(planned_gate_records),
        universe_summary["object_count"].as_u64()
    );
}

#[test]
fn bybit_public_archive_tick_trade_instrument_metadata_snapshot_covers_all_category_symbols() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let universe_path = reference_root.join(
        "backfill-source-universes/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-source-universe.json",
    );
    let snapshot_path = reference_root.join(
        "backfill-instrument-metadata/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-instrument-metadata-snapshot.json",
    );
    let universe: serde_json::Value = serde_json::from_str(&read_required_string(&universe_path))
        .expect("Bybit source universe parses");
    let snapshot: serde_json::Value = serde_json::from_str(&read_required_string(&snapshot_path))
        .expect("Bybit instrument metadata snapshot parses");

    assert_eq!(
        snapshot["schema_version"].as_str(),
        Some("bybit-instrument-metadata-snapshot.v1")
    );
    assert_eq!(
        snapshot["universe_id"].as_str(),
        universe["universe_id"].as_str()
    );
    assert_eq!(
        snapshot["selection"]["instrument_universe"].as_str(),
        Some("all_staged_category_symbols")
    );
    assert_eq!(
        snapshot["selection"]["metadata_query_strategy"].as_str(),
        Some("category_symbol_exact")
    );
    assert_eq!(
        snapshot["category_symbol_count"].as_u64(),
        universe["summary"]["category_symbol_count"].as_u64()
    );

    let records = snapshot["records"]
        .as_array()
        .expect("metadata snapshot records");
    assert_eq!(
        records.len() as u64,
        universe["summary"]["category_symbol_count"]
            .as_u64()
            .expect("category-symbol count")
    );
    let mut records_by_category_symbol =
        std::collections::BTreeMap::<(String, String), &serde_json::Value>::new();
    for record in records {
        let category = record["category"]
            .as_str()
            .expect("metadata record category");
        let symbol = record["symbol"].as_str().expect("metadata record symbol");
        assert!(
            records_by_category_symbol
                .insert((category.to_string(), symbol.to_string()), record)
                .is_none(),
            "duplicate Bybit metadata record for {category}/{symbol}"
        );
    }

    let mut closed_linear_futures = std::collections::BTreeSet::<String>::new();
    for category in universe["categories"]
        .as_array()
        .expect("universe categories")
    {
        let category_name = category["category"].as_str().expect("category name");
        let source_binding = category["source_binding"]
            .as_str()
            .expect("category source binding");
        for instrument in category["instruments"]
            .as_array()
            .expect("category instruments")
        {
            let symbol = instrument["symbol"].as_str().expect("instrument symbol");
            let record = records_by_category_symbol
                .get(&(category_name.to_string(), symbol.to_string()))
                .unwrap_or_else(|| panic!("missing Bybit metadata for {category_name}/{symbol}"));
            assert_eq!(record["source_binding"].as_str(), Some(source_binding));
            assert_eq!(record["api_ret_code"].as_i64(), Some(0));
            assert_eq!(record["instrument_count"].as_u64(), Some(1));
            assert!(
                record["source_uri"]
                    .as_str()
                    .expect("metadata source uri")
                    .contains(&format!("category={category_name}"))
            );
            assert!(
                record["source_uri"]
                    .as_str()
                    .expect("metadata source uri")
                    .contains(&format!("symbol={symbol}"))
            );

            let metadata = &record["instrument"];
            assert_eq!(metadata["symbol"].as_str(), Some(symbol));
            assert!(
                metadata["baseCoin"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert!(
                metadata["quoteCoin"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert!(
                metadata["status"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );
            assert!(
                metadata["priceFilter"]["tickSize"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty())
            );

            if category_name == "spot" {
                assert!(metadata["contractType"].is_null());
                assert!(
                    metadata["lotSizeFilter"]["basePrecision"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
                assert!(
                    metadata["lotSizeFilter"]["minOrderAmt"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
            } else {
                let contract_type = metadata["contractType"]
                    .as_str()
                    .expect("derivative metadata contract type");
                assert!(
                    contract_type.starts_with("Linear") || contract_type.starts_with("Inverse")
                );
                assert!(
                    metadata["lotSizeFilter"]["qtyStep"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
                assert!(
                    metadata["lotSizeFilter"]["minNotionalValue"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
                if symbol.ends_with("-05JUN26") {
                    closed_linear_futures.insert(symbol.to_string());
                    assert_eq!(category_name, "linear");
                    assert_eq!(contract_type, "LinearFutures");
                    assert_eq!(metadata["status"].as_str(), Some("Closed"));
                    assert!(
                        metadata["deliveryTime"]
                            .as_str()
                            .and_then(|value| value.parse::<u64>().ok())
                            .expect("delivery time")
                            > metadata["launchTime"]
                                .as_str()
                                .and_then(|value| value.parse::<u64>().ok())
                                .expect("launch time")
                    );
                }
            }
        }
    }

    assert_eq!(
        closed_linear_futures,
        std::collections::BTreeSet::from([
            "BTCUSDT-05JUN26".to_string(),
            "DOGEUSDT-05JUN26".to_string(),
            "ETHUSDT-05JUN26".to_string(),
            "SOLUSDT-05JUN26".to_string(),
            "XRPUSDT-05JUN26".to_string(),
        ])
    );
    assert_eq!(
        snapshot["coverage"]["missing_category_symbols"]
            .as_array()
            .expect("missing category symbols")
            .len(),
        0
    );
}

#[test]
fn bybit_public_archive_tick_trade_object_manifest_covers_all_staged_objects() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let universe_path = reference_root.join(
        "backfill-source-universes/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-source-universe.json",
    );
    let manifest_path = reference_root.join(
        "backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json",
    );
    let universe: serde_json::Value = serde_json::from_str(&read_required_string(&universe_path))
        .expect("Bybit source universe parses");
    let manifest: serde_json::Value = serde_json::from_str(&read_required_string(&manifest_path))
        .expect("Bybit source universe object manifest parses");

    assert_eq!(
        manifest["schema_version"].as_str(),
        Some("backfill-source-universe-object-manifest.v1")
    );
    assert_eq!(
        manifest["universe_id"].as_str(),
        universe["universe_id"].as_str()
    );
    assert_eq!(
        manifest["object_count"].as_u64(),
        universe["summary"]["object_count"].as_u64()
    );
    assert_eq!(
        manifest["accepted_bytes"].as_u64(),
        universe["summary"]["compressed_bytes"].as_u64()
    );

    let payload_records = manifest["payload_records"]
        .as_array()
        .expect("object manifest payload records");
    assert_eq!(
        payload_records.len() as u64,
        universe["summary"]["object_count"]
            .as_u64()
            .expect("object count")
    );

    let universe_categories = universe["categories"]
        .as_array()
        .expect("source universe categories");
    let universe_categories_by_name: std::collections::BTreeMap<&str, &serde_json::Value> =
        universe_categories
            .iter()
            .map(|category| {
                (
                    category["category"].as_str().expect("category name"),
                    category,
                )
            })
            .collect();

    let mut object_counts_by_category = std::collections::BTreeMap::<String, u64>::new();
    let mut bytes_by_category = std::collections::BTreeMap::<String, u64>::new();
    let mut symbols_by_category =
        std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    let mut total_bytes = 0_u64;
    for record in payload_records {
        let category = record["category"]
            .as_str()
            .expect("payload record category");
        let symbol = record["symbol"].as_str().expect("payload record symbol");
        let archive_date = record["archive_date"]
            .as_str()
            .expect("payload record archive date");
        let sha256 = record["sha256"].as_str().expect("payload record sha256");
        let s3_uri = record["s3_uri"].as_str().expect("payload record s3 uri");
        let source_url = record["source_url"]
            .as_str()
            .expect("payload record source url");
        let bytes = record["bytes"].as_u64().expect("payload record bytes");
        let universe_category = universe_categories_by_name
            .get(category)
            .unwrap_or_else(|| panic!("missing source universe category {category}"));

        assert_eq!(
            record["source_binding"].as_str(),
            universe_category["source_binding"].as_str()
        );
        assert_eq!(
            record["schema_columns"],
            universe_category["schema_columns"]
        );
        assert_eq!(sha256.len(), 64);
        assert!(
            sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert!(s3_uri.starts_with("s3://bolt-parquet/backfill-staging/"));
        assert!(s3_uri.contains(&format!("/category={category}/")));
        assert!(s3_uri.contains(&format!("/dt={archive_date}/")));
        assert!(s3_uri.contains(&format!("/symbol={symbol}/")));
        assert!(s3_uri.ends_with(&format!("/object={sha256}.csv.gz")));
        if category == "spot" {
            assert!(source_url.ends_with(&format!("/{symbol}_{archive_date}.csv.gz")));
        } else {
            assert!(source_url.ends_with(&format!("/{symbol}{archive_date}.csv.gz")));
        }

        *object_counts_by_category
            .entry(category.to_string())
            .or_default() += 1;
        *bytes_by_category.entry(category.to_string()).or_default() += bytes;
        symbols_by_category
            .entry(category.to_string())
            .or_default()
            .insert(symbol.to_string());
        total_bytes += bytes;
    }
    assert_eq!(
        total_bytes,
        universe["summary"]["compressed_bytes"]
            .as_u64()
            .expect("universe compressed bytes")
    );

    for (category, universe_category) in universe_categories_by_name {
        assert_eq!(
            object_counts_by_category.get(category).copied(),
            universe_category["object_count"].as_u64()
        );
        assert_eq!(
            bytes_by_category.get(category).copied(),
            universe_category["compressed_bytes"].as_u64()
        );
        assert_eq!(
            symbols_by_category
                .get(category)
                .map(std::collections::BTreeSet::len),
            universe_category["instrument_count"]
                .as_u64()
                .map(|count| count as usize)
        );
    }
}

#[test]
fn bybit_public_archive_tick_trade_category_source_proofs_cover_all_staged_objects() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let manifest_path = reference_root.join(
        "backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json",
    );
    let manifest: serde_json::Value = serde_json::from_str(&read_required_string(&manifest_path))
        .expect("Bybit source universe object manifest parses");
    let category_summaries = manifest["category_summaries"]
        .as_array()
        .expect("category summaries array");
    let summaries_by_category: std::collections::BTreeMap<&str, &serde_json::Value> =
        category_summaries
            .iter()
            .map(|summary| {
                (
                    summary["category"].as_str().expect("summary category"),
                    summary,
                )
            })
            .collect();
    assert_eq!(
        summaries_by_category.keys().copied().collect::<Vec<_>>(),
        vec!["inverse", "linear", "spot"]
    );

    let proof_root = reference_root
        .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01");
    let mut total_completed_objects = 0_u64;
    let mut total_accepted_bytes = 0_u64;
    for (category, proof_file, product_family, source_binding) in [
        (
            "inverse",
            "source-proof-bybit-inverse-public-archive-tick-trades.json",
            "inverse",
            "bybit-inverse-tick-trades",
        ),
        (
            "linear",
            "source-proof-bybit-linear-public-archive-tick-trades.json",
            "linear",
            "bybit-linear-tick-trades",
        ),
        (
            "spot",
            "source-proof-bybit-spot-public-archive-tick-trades.json",
            "spot",
            "bybit-spot-tick-trades",
        ),
    ] {
        let proof_path = proof_root.join(proof_file);
        let proof: SourceProofReport = serde_json::from_str(&read_required_string(&proof_path))
            .unwrap_or_else(|error| panic!("parse {proof_file}: {error}"));
        let summary = summaries_by_category
            .get(category)
            .unwrap_or_else(|| panic!("missing category summary {category}"));
        assert_eq!(proof.status, SourceProofStatus::Accepted);
        proof
            .evaluate_acceptance()
            .unwrap_or_else(|error| panic!("{category} source proof must be accepted: {error}"));
        assert_eq!(proof.source_binding, source_binding);
        assert_eq!(proof.venue, "bybit");
        assert_eq!(proof.product_family, product_family);
        assert_eq!(proof.product_category, category);
        assert_eq!(proof.table_family, "trades");
        assert_eq!(
            proof.instrument_universe_id,
            "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
        );
        let acceptance_scope = proof
            .acceptance_scope
            .as_ref()
            .expect("accepted proof has acceptance scope");
        assert_eq!(
            acceptance_scope.planned_objects,
            summary["object_count"].as_u64().expect("object count")
        );
        assert_eq!(
            acceptance_scope.completed_objects,
            summary["object_count"].as_u64().expect("object count")
        );
        assert_eq!(acceptance_scope.failed_objects, 0);
        assert_eq!(acceptance_scope.skipped_objects, 0);
        assert_eq!(
            acceptance_scope.accepted_bytes,
            summary["compressed_bytes"]
                .as_u64()
                .expect("compressed bytes")
        );
        assert_eq!(acceptance_scope.selector_scope_violations, 0);
        assert!(
            manifest["payload_records"]
                .as_array()
                .expect("payload records array")
                .iter()
                .any(|record| {
                    record["category"].as_str() == Some(category)
                        && record["s3_uri"].as_str() == Some(proof.raw_sample_uri.as_str())
                        && record["sha256"].as_str() == Some(proof.raw_sample_hash.as_str())
                }),
            "{category} proof raw sample must be a staged object in the object manifest"
        );
        total_completed_objects += acceptance_scope.completed_objects;
        total_accepted_bytes += acceptance_scope.accepted_bytes;
    }

    assert_eq!(
        total_completed_objects,
        manifest["object_count"]
            .as_u64()
            .expect("manifest object count")
    );
    assert_eq!(
        total_accepted_bytes,
        manifest["accepted_bytes"]
            .as_u64()
            .expect("manifest accepted bytes")
    );
}

#[test]
fn bybit_public_archive_tick_trade_category_object_manifests_cover_all_staged_objects() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let manifest_path = reference_root.join(
        "backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json",
    );
    let full_manifest: serde_json::Value =
        serde_json::from_str(&read_required_string(&manifest_path))
            .expect("Bybit source universe object manifest parses");
    let full_records = full_manifest["payload_records"]
        .as_array()
        .expect("full manifest payload records");
    let summaries_by_category: std::collections::BTreeMap<&str, &serde_json::Value> = full_manifest
        ["category_summaries"]
        .as_array()
        .expect("category summaries array")
        .iter()
        .map(|summary| {
            (
                summary["category"].as_str().expect("summary category"),
                summary,
            )
        })
        .collect();
    let category_manifest_root = reference_root.join(
        "backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/category-manifests",
    );
    let proof_root = reference_root
        .join("backfill-source-proofs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01");

    let mut covered_object_uris = std::collections::BTreeSet::<String>::new();
    let mut covered_bytes = 0_u64;
    for (category, category_manifest_file, proof_file, source_binding) in [
        (
            "inverse",
            "bybit-public-archive-tick-trades-object-manifest-inverse.json",
            "source-proof-bybit-inverse-public-archive-tick-trades.json",
            "bybit-inverse-tick-trades",
        ),
        (
            "linear",
            "bybit-public-archive-tick-trades-object-manifest-linear.json",
            "source-proof-bybit-linear-public-archive-tick-trades.json",
            "bybit-linear-tick-trades",
        ),
        (
            "spot",
            "bybit-public-archive-tick-trades-object-manifest-spot.json",
            "source-proof-bybit-spot-public-archive-tick-trades.json",
            "bybit-spot-tick-trades",
        ),
    ] {
        let category_manifest_path = category_manifest_root.join(category_manifest_file);
        let category_manifest_text = read_required_string(&category_manifest_path);
        let category_manifest: serde_json::Value = serde_json::from_str(&category_manifest_text)
            .unwrap_or_else(|error| panic!("parse {category_manifest_file}: {error}"));
        let proof_text = read_required_string(&proof_root.join(proof_file));
        let summary = summaries_by_category
            .get(category)
            .unwrap_or_else(|| panic!("missing category summary {category}"));

        assert_eq!(
            category_manifest["schema_version"].as_str(),
            Some("backfill-source-universe-object-manifest.v1")
        );
        assert_eq!(
            category_manifest["parent_manifest_id"].as_str(),
            full_manifest["manifest_id"].as_str()
        );
        assert_eq!(
            category_manifest["universe_id"].as_str(),
            full_manifest["universe_id"].as_str()
        );
        assert_eq!(category_manifest["category"].as_str(), Some(category));
        assert_eq!(
            category_manifest["source_binding"].as_str(),
            Some(source_binding)
        );
        assert_eq!(
            category_manifest["object_count"].as_u64(),
            summary["object_count"].as_u64()
        );
        assert_eq!(
            category_manifest["accepted_bytes"].as_u64(),
            summary["compressed_bytes"].as_u64()
        );

        let category_records = category_manifest["payload_records"]
            .as_array()
            .expect("category manifest payload records");
        assert_eq!(
            category_records.len() as u64,
            summary["object_count"].as_u64().expect("object count")
        );
        let filtered_full_records = full_records
            .iter()
            .filter(|record| record["category"].as_str() == Some(category))
            .collect::<Vec<_>>();
        assert_eq!(
            category_records.len(),
            filtered_full_records.len(),
            "{category} category manifest must contain every staged object from the full manifest"
        );
        assert_eq!(
            category_records
                .iter()
                .map(|record| record["s3_uri"].as_str().expect("category record s3 uri"))
                .collect::<std::collections::BTreeSet<_>>(),
            filtered_full_records
                .iter()
                .map(|record| record["s3_uri"].as_str().expect("full record s3 uri"))
                .collect::<std::collections::BTreeSet<_>>()
        );

        let mut category_bytes = 0_u64;
        for record in category_records {
            assert_eq!(record["category"].as_str(), Some(category));
            assert_eq!(record["source_binding"].as_str(), Some(source_binding));
            let s3_uri = record["s3_uri"].as_str().expect("category record s3 uri");
            assert!(
                covered_object_uris.insert(s3_uri.to_string()),
                "duplicate category object {s3_uri}"
            );
            let bytes = record["bytes"].as_u64().expect("category record bytes");
            assert!(
                bytes
                    <= summary["compressed_bytes"]
                        .as_u64()
                        .expect("category compressed bytes")
            );
            category_bytes += bytes;
            covered_bytes += bytes;
        }
        assert_eq!(
            category_bytes,
            summary["compressed_bytes"]
                .as_u64()
                .expect("category compressed bytes")
        );

        for record in [
            category_records.first().expect("first category record"),
            category_records.last().expect("last category record"),
        ] {
            let s3_uri = record["s3_uri"].as_str().expect("selected record s3 uri");
            let sha256 = record["sha256"].as_str().expect("selected record sha256");
            let scope = evaluate_backfill_source_proof_scope_for_selected_object(
                format!("backfill-source-proof-scope-bybit-public-archive-tick-trades-{category}-{sha256}"),
                &proof_text,
                &category_manifest_text,
                s3_uri,
            )
            .unwrap_or_else(|error| panic!("{category} selected source scope evaluates: {error}"));
            assert_eq!(scope.status, BackfillSourceProofScopeStatus::CandidateFound);
            assert!(scope.blocking_issues.is_empty());
            assert_eq!(scope.matching_object_count, 1);
            assert!(scope.object_level_tranche_required);
            assert_eq!(
                scope
                    .selected_object
                    .as_ref()
                    .expect("selected object")
                    .s3_uri,
                s3_uri
            );
            let tranche = evaluate_backfill_accepted_tranche(
                format!(
                    "backfill-accepted-tranche-bybit-public-archive-tick-trades-{category}-{sha256}"
                ),
                &scope,
                &source_proof_scope_hash(&scope),
            )
            .unwrap_or_else(|error| panic!("{category} accepted tranche evaluates: {error}"));
            assert_eq!(tranche.status, BackfillAcceptedTrancheStatus::Accepted);
            assert!(tranche.blocking_issues.is_empty());
            assert!(tranche.object_level_tranche_required);
            assert_eq!(tranche.object_count, 1);
            assert_eq!(
                tranche.accepted_bytes,
                record["bytes"].as_u64().expect("selected record bytes")
            );
            assert_eq!(
                tranche.objects.first().expect("tranche object").s3_uri,
                s3_uri
            );
        }
    }

    assert_eq!(
        covered_object_uris.len() as u64,
        full_manifest["object_count"]
            .as_u64()
            .expect("full manifest object count")
    );
    assert_eq!(
        covered_bytes,
        full_manifest["accepted_bytes"]
            .as_u64()
            .expect("full manifest accepted bytes")
    );
}

#[test]
fn bybit_bnbusdc_venue_publication_and_mapping_evidence_cover_all_accepted_tranches() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let plan = generated_bybit_bnbusdc_conversion_batch_plan(&reference_root);
    assert_eq!(plan.records.len(), 93);

    for record in &plan.records {
        let archive_date = record
            .record_id
            .strip_prefix("backfill-accepted-tranche-bybit-bnbusdc-")
            .expect("Bybit BNBUSDC record id carries archive date");
        let publication_path = single_reference_file_with_prefix_suffix(
            &reference_root,
            &format!("bybit-bnbusdc-{archive_date}-accepted-publication-evidence."),
            ".json",
        );
        let mapping_path = single_reference_file_with_prefix_suffix(
            &reference_root,
            &format!(
                "source-proof-nt-catalog-mapping-evaluation.backtesting-engine.bybit-bnbusdc-{archive_date}."
            ),
            ".json",
        );

        let publication: serde_json::Value =
            serde_json::from_str(&read_required_string(&publication_path))
                .expect("accepted publication evidence parses");
        assert_eq!(
            publication["scope"]["archive_date"].as_str(),
            Some(archive_date)
        );
        assert_eq!(
            publication["scope"]["status"].as_str(),
            Some("accepted_gate_committed_and_s3_published")
        );
        assert_eq!(
            publication["accepted_conversion_and_publication"]["published_catalog_direct_s3"]
                .as_bool(),
            Some(true)
        );

        let gate_root = reference_root.join(format!("backfill-gates/bybit-bnbusdc-{archive_date}"));
        let readiness_spec_path = gate_root.join("source-catalog-mapping-readiness.toml");
        let readiness_spec: SourceCatalogMappingReadinessSpec =
            toml::from_str(&read_required_string(&readiness_spec_path))
                .expect("source catalog-mapping readiness spec parses");
        assert_eq!(
            repo_relative_path(&readiness_spec.catalog_mapping_evaluation_path),
            mapping_path
        );
        let readiness_report_path = gate_root
            .join("source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json");
        let readiness_report: SourceCatalogMappingReadinessReport =
            serde_json::from_str(&read_required_string(&readiness_report_path))
                .expect("source catalog-mapping readiness report parses");
        assert_eq!(
            readiness_report.status,
            SourceCatalogMappingReadinessStatus::Ready
        );
        assert_eq!(
            readiness_report.catalog_mapping_evaluation_hash,
            sha256_hex(&read_required_bytes(&mapping_path))
        );
    }
}

fn source_proof_scope_hash(report: &BackfillSourceProofScopeReport) -> String {
    let bytes = serde_json::to_vec(report).expect("scope report serializes");
    sha256_hex(&bytes)
}

fn assert_binance_gate_matches_generic_evaluators(gate_root: &Path, source_proof_path: &Path) {
    let source_proof = read_required_string(source_proof_path);
    let object_staging_manifest = read_required_string(
        &gate_root.join("object-staging/backfill-object-staging-manifest.json"),
    );
    let source_proof_scope_report = read_required_string(
        &gate_root.join("source-proof-scope/backfill-source-proof-scope-report.json"),
    );
    let accepted_tranche_manifest = read_required_string(
        &gate_root.join("accepted-tranche/backfill-accepted-tranche-manifest.json"),
    );
    let accepted_tranche_manifest_bytes = read_required_bytes(
        &gate_root.join("accepted-tranche/backfill-accepted-tranche-manifest.json"),
    );
    let materialization_spec =
        read_required_string(&gate_root.join("backfill-run-spec-materialization.toml"));
    let run_spec_path = gate_root.join("materialized-run-spec/backfill-run-spec.toml");
    let run_spec = read_required_string(&run_spec_path);
    let run_spec_bytes = read_required_bytes(&run_spec_path);
    let execution_plan_spec = read_required_string(&gate_root.join("backfill-execution-plan.toml"));
    let execution_plan =
        read_required_string(&gate_root.join("execution-plan/backfill-execution-plan.json"));
    let execution_plan_bytes =
        read_required_bytes(&gate_root.join("execution-plan/backfill-execution-plan.json"));
    let source_catalog_mapping_readiness_report = read_required_string(
        &gate_root
            .join("source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"),
    );
    let source_catalog_mapping_readiness_spec =
        read_required_string(&gate_root.join("source-catalog-mapping-readiness.toml"));
    let source_catalog_mapping_readiness_spec: SourceCatalogMappingReadinessSpec =
        toml::from_str(&source_catalog_mapping_readiness_spec)
            .expect("source catalog-mapping readiness spec parses");
    let catalog_mapping_evaluation_path =
        repo_relative_path(&source_catalog_mapping_readiness_spec.catalog_mapping_evaluation_path);
    let catalog_mapping_evaluation_bytes = read_required_bytes(&catalog_mapping_evaluation_path);
    let source_catalog_mapping_readiness_report_bytes = read_required_bytes(
        &gate_root
            .join("source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"),
    );
    let execution_readiness_report = read_required_string(
        &gate_root.join("execution-readiness/backfill-execution-readiness-report.json"),
    );
    let artifact_index_required_execution_readiness_report = read_required_string(&gate_root.join(
        "execution-readiness-artifact-index-required/backfill-execution-readiness-report.json",
    ));

    assert!(
        run_spec.contains(r#"usage_scope = "canonical_backfill_input""#),
        "Binance materialized run spec must explicitly bind canonical source usage scope"
    );
    assert!(
        materialization_spec.contains("materialized-run-spec"),
        "Binance gate must commit the materialized run-spec output directory"
    );
    assert!(
        execution_plan_spec.contains("materialized-run-spec/backfill-run-spec.toml"),
        "Binance execution plan must consume the materialized run spec"
    );

    let expected_scope: BackfillSourceProofScopeReport =
        serde_json::from_str(&source_proof_scope_report).expect("scope report parses");
    let actual_scope = evaluate_backfill_source_proof_scope(
        expected_scope.report_id.clone(),
        &source_proof,
        &object_staging_manifest,
    )
    .expect("source-proof scope evaluates");

    assert_eq!(actual_scope, expected_scope);

    let expected_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_str(&accepted_tranche_manifest).expect("accepted tranche parses");
    let actual_tranche = evaluate_backfill_accepted_tranche(
        expected_tranche.tranche_id.clone(),
        &actual_scope,
        &source_proof_scope_hash(&actual_scope),
    )
    .expect("accepted tranche evaluates");

    assert_eq!(actual_tranche, expected_tranche);

    let expected_plan: BackfillExecutionPlan =
        serde_json::from_str(&execution_plan).expect("execution plan parses");
    let run_spec: RunSpec = toml::from_str(&run_spec).expect("run spec parses");
    let actual_plan = evaluate_backfill_execution_plan(
        expected_plan.plan_id.clone(),
        sha256_hex(&accepted_tranche_manifest_bytes),
        &actual_tranche,
        sha256_hex(&run_spec_bytes),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: expected_plan.max_source_rows,
            max_projected_row_groups: expected_plan.max_projected_row_groups,
            max_wall_seconds: expected_plan.max_wall_seconds,
            require_object_selection_metadata: expected_plan.require_object_selection_metadata,
        },
    );

    assert_eq!(actual_plan, expected_plan);
    assert!(actual_plan.blocking_issues.is_empty());

    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_slice(&catalog_mapping_evaluation_bytes)
            .expect("mapping evaluation parses");
    let expected_catalog_mapping_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(&source_catalog_mapping_readiness_report)
            .expect("source catalog-mapping readiness parses");
    let actual_catalog_mapping_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_catalog_mapping_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(&catalog_mapping_evaluation_bytes),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_catalog_mapping_readiness.source_proof_id,
            source_proof_version: expected_catalog_mapping_readiness.source_proof_version,
            source_binding: &expected_catalog_mapping_readiness.source_binding,
            required_table_family: &expected_catalog_mapping_readiness.required_table_family,
            required_nt_data_types: expected_catalog_mapping_readiness
                .required_nt_data_types
                .clone(),
            required_claim_evidence_refs: expected_catalog_mapping_readiness
                .required_claim_evidence_refs
                .clone(),
            allowed_current_bte_statuses: expected_catalog_mapping_readiness
                .allowed_current_bte_statuses
                .clone(),
            allowed_parquet_catalog_statuses: expected_catalog_mapping_readiness
                .allowed_parquet_catalog_statuses
                .clone(),
            allowed_usage_scopes: expected_catalog_mapping_readiness
                .allowed_usage_scopes
                .clone(),
        });

    assert_eq!(
        actual_catalog_mapping_readiness,
        expected_catalog_mapping_readiness
    );

    let accepted_tranche_manifest_hash = sha256_hex(&accepted_tranche_manifest_bytes);
    let execution_plan_hash = sha256_hex(&execution_plan_bytes);
    let source_catalog_mapping_readiness_hash =
        sha256_hex(&source_catalog_mapping_readiness_report_bytes);
    let expected_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(&execution_readiness_report).expect("execution readiness parses");
    let readiness = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: &expected_readiness.readiness_id,
        accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
        tranche: &actual_tranche,
        execution_plan_hash: &execution_plan_hash,
        plan: &actual_plan,
        required_table_family: &actual_plan.table_family,
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
            table_family: actual_plan.table_family.clone(),
            nt_data_type: "TradeTick".to_string(),
        }],
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some(&source_catalog_mapping_readiness_hash),
        source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
    });

    assert_eq!(readiness, expected_readiness);
    assert_eq!(readiness.status, BackfillExecutionReadinessStatus::Ready);
    assert!(readiness.blockers.is_empty());

    let artifact_index_proof: ArtifactIndexCommitProofEvidence =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("Artifact Index IAM-scope proof report parses");
    let mut expected_artifact_index_required_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(&artifact_index_required_execution_readiness_report)
            .expect("Artifact Index-required execution readiness parses");
    expected_artifact_index_required_readiness.blockers.insert(
        0,
        BackfillExecutionReadinessBlocker::ArtifactIndexCommitMechanicsUnproven,
    );
    let artifact_index_required_readiness =
        evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
            readiness_id: &expected_artifact_index_required_readiness.readiness_id,
            accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
            tranche: &actual_tranche,
            execution_plan_hash: &execution_plan_hash,
            plan: &actual_plan,
            required_table_family: &actual_plan.table_family,
            required_nt_data_type: "TradeTick",
            required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
                table_family: actual_plan.table_family.clone(),
                nt_data_type: "TradeTick".to_string(),
            }],
            artifact_index_commit_required: true,
            required_artifact_index_kind: Some(ArtifactKind::Backtests),
            artifact_index_commit_proof_report_hash: Some(&sha256_hex(
                ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES,
            )),
            artifact_index_commit_proof_report: Some(&artifact_index_proof),
            source_selection_readiness_required: false,
            source_selection_readiness_report_hash: None,
            source_selection_readiness_report: None,
            source_catalog_mapping_readiness_required: true,
            source_catalog_mapping_readiness_report_hash: Some(
                &source_catalog_mapping_readiness_hash,
            ),
            source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
        });

    assert_eq!(
        artifact_index_required_readiness,
        expected_artifact_index_required_readiness
    );
    assert_eq!(
        artifact_index_required_readiness.status,
        BackfillExecutionReadinessStatus::Blocked
    );
    assert_eq!(
        artifact_index_required_readiness.blockers,
        vec![
            BackfillExecutionReadinessBlocker::ArtifactIndexCommitMechanicsUnproven,
            BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven,
        ]
    );
}

fn read_required_string(path: &Path) -> String {
    assert!(
        path.exists(),
        "required reference artifact missing: {}",
        path.display()
    );
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read reference artifact {}: {error}", path.display()))
}

fn generated_binance_bnbusdc_conversion_batch_plan(
    reference_root: &Path,
) -> BackfillConversionBatchPlan {
    generated_evicted_conversion_batch_plan(
        reference_root,
        "binance-bnbusdc-2026-03-01-2026-05-31",
        PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    )
}

fn generated_bybit_bnbusdc_conversion_batch_plan(
    reference_root: &Path,
) -> BackfillConversionBatchPlan {
    generated_evicted_conversion_batch_plan(
        reference_root,
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    )
}

fn read_required_bytes(path: &Path) -> Vec<u8> {
    assert!(
        path.exists(),
        "required reference artifact missing: {}",
        path.display()
    );
    fs::read(path)
        .unwrap_or_else(|error| panic!("read reference artifact {}: {error}", path.display()))
}

fn single_reference_file_with_prefix_suffix(
    reference_root: &Path,
    prefix: &str,
    suffix: &str,
) -> std::path::PathBuf {
    let matches = fs::read_dir(reference_root)
        .unwrap_or_else(|error| panic!("read reference root {}: {error}", reference_root.display()))
        .map(|entry| entry.expect("reference root entry reads").path())
        .filter(|path| {
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                return false;
            };
            file_name.starts_with(prefix) && file_name.ends_with(suffix)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one reference artifact matching {prefix}*{suffix}, got {matches:?}"
    );
    matches.into_iter().next().expect("one reference match")
}

fn repo_relative_path(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

#[derive(Debug, Deserialize)]
struct SourceCatalogMappingEvaluation {
    nt_surface_evidence: NtSurfaceEvidence,
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

#[derive(Debug, Deserialize)]
struct NtSurfaceEvidence {
    current_bte_manifest_exposure: CurrentBteManifestExposure,
}

#[derive(Debug, Deserialize)]
struct CurrentBteManifestExposure {
    accepted_data_classes: Vec<String>,
    rejected_for_now: Vec<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
