use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::ArtifactIndexCommitProofReport,
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheManifest, evaluate_backfill_accepted_tranche,
    },
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
        BackfillSourceProofScopeReport, evaluate_backfill_source_proof_scope,
    },
    operator::RunSpec,
    source_catalog_mapping_readiness::{
        SourceCatalogMappingReadinessInput, SourceCatalogMappingReadinessReport,
        SourceCatalogMappingReadinessSpec, SourceCatalogMappingReadinessStatus,
        SourceCatalogMappingStatusEntry, evaluate_source_catalog_mapping_readiness,
    },
    source_proof::SourceProofUsageScope,
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
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const CATALOG_MAPPING_EVALUATION_BYTES: &[u8] = include_bytes!(
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

    let artifact_index_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("Artifact Index IAM-scope proof report parses");
    let expected_artifact_index_required_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(ARTIFACT_INDEX_REQUIRED_EXECUTION_READINESS_REPORT)
            .expect("Artifact Index-required execution readiness parses");
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
        vec![BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven]
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
fn blocked_canonical_source_catalog_mapping_reference_artifact_matches_generic_evaluator() {
    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(CATALOG_MAPPING_EVALUATION).expect("mapping evaluation parses");
    let expected_blocked_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("blocked source catalog-mapping readiness parses");

    let actual_blocked_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_blocked_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(CATALOG_MAPPING_EVALUATION_BYTES),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_blocked_readiness.source_proof_id,
            source_proof_version: expected_blocked_readiness.source_proof_version,
            source_binding: &expected_blocked_readiness.source_binding,
            required_table_family: &expected_blocked_readiness.required_table_family,
            required_nt_data_types: expected_blocked_readiness.required_nt_data_types.clone(),
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
    assert!(!actual_blocked_readiness.blockers.is_empty());
}

#[test]
fn artifact_index_commit_status_references_committed_proof_reports() {
    let status: serde_json::Value =
        serde_json::from_str(ARTIFACT_INDEX_COMMIT_STATUS).expect("Artifact Index status parses");
    let direct_s3_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT)
            .expect("direct S3 proof report parses");
    let iam_scope_proof: ArtifactIndexCommitProofReport =
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
        status["store_commit_mechanics"]["report_content_hash"]
            .as_str()
            .expect("store commit proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["report_content_hash"]
            .as_str()
            .expect("producer IAM proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES)
    );
    assert_eq!(status["bte_006_can_close"].as_bool(), Some(false));
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

    let artifact_index_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("Artifact Index IAM-scope proof report parses");
    let expected_artifact_index_required_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(&artifact_index_required_execution_readiness_report)
            .expect("Artifact Index-required execution readiness parses");
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
        vec![BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven]
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

fn read_required_bytes(path: &Path) -> Vec<u8> {
    assert!(
        path.exists(),
        "required reference artifact missing: {}",
        path.display()
    );
    fs::read(path)
        .unwrap_or_else(|error| panic!("read reference artifact {}: {error}", path.display()))
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
