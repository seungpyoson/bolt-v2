use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheManifest, evaluate_backfill_accepted_tranche,
    },
    backfill_execution_plan::{
        BackfillExecutionPlan, BackfillExecutionRunBinding, evaluate_backfill_execution_plan,
    },
    backfill_execution_readiness::{
        BackfillExecutionReadinessStatus, BackfillExecutionReadinessSupportedDataPath,
        evaluate_backfill_execution_readiness,
    },
    backfill_source_proof_scope::{
        BackfillSourceProofScopeReport, evaluate_backfill_source_proof_scope,
    },
    operator::RunSpec,
};
use sha2::{Digest, Sha256};

const SOURCE_PROOF: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-01.json"
);
const RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml"
);
const RUN_SPEC_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml"
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
const EXECUTION_PLAN: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json"
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
    );

    assert_eq!(actual_plan, expected_plan);
    assert!(actual_plan.blocking_issues.is_empty());

    let readiness = evaluate_backfill_execution_readiness(
        "binance-bnbusdc-2026-03-01-reference-readiness",
        sha256_hex(ACCEPTED_TRANCHE_MANIFEST_BYTES),
        &actual_tranche,
        execution_plan_hash(&actual_plan),
        &actual_plan,
        &actual_plan.table_family,
        "TradeTick",
        vec![BackfillExecutionReadinessSupportedDataPath {
            table_family: actual_plan.table_family.clone(),
            nt_data_type: "TradeTick".to_string(),
        }],
    );

    assert_eq!(readiness.status, BackfillExecutionReadinessStatus::Ready);
    assert!(readiness.blockers.is_empty());
}

fn source_proof_scope_hash(report: &BackfillSourceProofScopeReport) -> String {
    let bytes = serde_json::to_vec(report).expect("scope report serializes");
    sha256_hex(&bytes)
}

fn execution_plan_hash(plan: &BackfillExecutionPlan) -> String {
    let bytes = serde_json::to_vec(plan).expect("execution plan serializes");
    sha256_hex(&bytes)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
