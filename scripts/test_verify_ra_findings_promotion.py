#!/usr/bin/env python3
"""Self-tests for verify_ra_findings_promotion.py."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_findings_promotion.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_findings_promotion", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def compliant_ra() -> str:
    return """
pub enum RaVerdictKind {
    Go,
    NoGo,
    ConditionalGo,
}

pub enum ForbiddenPromotionAction {
    AutoMerge,
    AutoEnableStrategy,
    ScheduleLiveTrading,
    TouchSsmCredentials,
    MutateProductionRuntimeConfig,
}

pub struct SourceProofEvidenceRef {
    pub accepted: bool,
}

pub struct BacktestEvidenceRef {
    pub objective: bool,
}

pub struct ArtifactPointerRef;

pub struct RaVerdict {
    pub verdict: RaVerdictKind,
    pub scope: String,
    pub source_proof_refs: Vec<SourceProofEvidenceRef>,
    pub backtest_result_refs: Vec<BacktestEvidenceRef>,
    pub evidence_report_refs: Vec<ArtifactPointerRef>,
    pub requested_claim_fidelity: SourceProofFidelityClass,
    pub preserved_claim_limits: Vec<String>,
    pub remeasurement_cadence: String,
    pub recorded_at: String,
    pub recorded_by: String,
}

impl RaVerdict {
    fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        if self.verdict == RaVerdictKind::Go && !self.is_real_go_finding() {
            return Err(ResearchAnalyticsArtifactError::PromotionConfigRequiresGo);
        }
        Ok(())
    }

    fn is_real_go_finding(&self) -> bool {
        self.verdict == RaVerdictKind::Go
            && self.source_proof_refs.iter().all(|source_ref| source_ref.accepted)
            && self.backtest_result_refs.iter().all(|backtest_ref| backtest_ref.objective)
    }
}

pub struct PromotionConfigRef {
    pub typed_config_uri: String,
    pub typed_config_hash: String,
    pub reviewer_policy_refs: Vec<String>,
    pub non_live_boundary: bool,
}

pub struct ExperimentResultArtifact {
    pub artifact_uri: String,
    pub lifecycle_state: LifecycleState,
    pub verdict: RaVerdict,
    pub promotion_config: Option<PromotionConfigRef>,
    pub accepts_source_proofs: bool,
    pub mutates_source_proofs: bool,
    pub mutates_backtest_result_contracts: bool,
    pub weakens_forbidden_claims: bool,
    pub post_verdict_actions: Vec<ForbiddenPromotionAction>,
}

impl ExperimentResultArtifact {
    pub fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        if let Some(promotion_config) = &self.promotion_config {
            if !self.verdict.is_real_go_finding() {
                return Err(ResearchAnalyticsArtifactError::PromotionConfigRequiresGo);
            }
            promotion_config.validate()?;
        }
        Ok(())
    }

    fn forbidden_behavior_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.accepts_source_proofs {}
        if self.mutates_source_proofs {}
        if self.mutates_backtest_result_contracts {}
        if self.weakens_forbidden_claims {}
        self.post_verdict_actions.iter();
        violations
    }
}

pub enum ResearchAnalyticsArtifactError {
    ArtifactOutsideExperimentResults,
    PromotionConfigRequiresGo,
}

fn validate_experiment_results_uri(uri: &str) -> Result<(), ResearchAnalyticsArtifactError> {
    let expected_prefix = experiment_results_prefix("s3://bucket/root");
    if uri.starts_with(&expected_prefix) {
        Ok(())
    } else {
        Err(ResearchAnalyticsArtifactError::ArtifactOutsideExperimentResults)
    }
}
"""


def compliant_artifact_index() -> str:
    return """
pub enum ResearchAnalyticsSubfamily {
    Datasets,
    FeatureTables,
    ExperimentResults,
}
"""


def compliant_run_manifest() -> str:
    return """
const RESEARCH_ANALYTICS_EXPERIMENT_RESULT_PREFIX: &str =
    "research-analytics/v1/experiment-results";

pub enum StrategySourceKind {
    CompiledRustRegistry,
    HumanTypedConfig,
    ResearchAnalyticsExperimentResult,
}

pub struct StrategySource {
    pub experiment_result_uri: Option<String>,
    pub experiment_result_hash: Option<String>,
}

fn validate_strategy_source_provenance(strategy: &StrategySource) -> Result<(), ManifestError> {
    match strategy.source_kind {
        StrategySourceKind::ResearchAnalyticsExperimentResult => {
            validate_strategy_artifact_ref(
                "strategy.typed_config_uri",
                "strategy.typed_config_hash",
                strategy.typed_config_uri.as_deref(),
                strategy.typed_config_hash.as_deref(),
                &research_analytics_experiment_result_prefix("s3://bucket/root"),
            )?;
            validate_strategy_artifact_ref(
                "strategy.experiment_result_uri",
                "strategy.experiment_result_hash",
                strategy.experiment_result_uri.as_deref(),
                strategy.experiment_result_hash.as_deref(),
                &research_analytics_experiment_result_prefix("s3://bucket/root"),
            )
        }
    }
}

fn research_analytics_experiment_result_prefix(artifact_root: &str) -> String {
    format!("{artifact_root}/{RESEARCH_ANALYTICS_EXPERIMENT_RESULT_PREFIX}/")
}
"""


def compliant_tests() -> str:
    return """
fn experiment_result_verdict_requires_required_field_set() {}
fn promotion_gate_stays_inert_without_go_finding() {}
fn go_finding_can_carry_typed_config_only_on_experiment_result() {}
fn go_promotion_requires_accepted_source_proof_and_objective_backtest_refs() {}
fn experiment_result_rejects_forbidden_promotion_actions() {}
fn experiment_result_rejects_cross_family_fidelity_claims() {}
const NEGATIVE_PATH: &str = "s3://bucket/root/research-analytics/v1/promotion-packages/package-123/config.toml";
"""


def write_common(
    root: Path,
    *,
    ra: str | None = None,
    artifact_index: str | None = None,
    run_manifest: str | None = None,
    tests: str | None = None,
    tasks_checked: bool = True,
) -> None:
    write(root / "crates/backtesting-vertical-slice/src/research_analytics.rs", compliant_ra() if ra is None else ra)
    write(root / "crates/backtesting-vertical-slice/src/artifact_index.rs", compliant_artifact_index() if artifact_index is None else artifact_index)
    write(root / "crates/backtesting-vertical-slice/src/run_manifest.rs", compliant_run_manifest() if run_manifest is None else run_manifest)
    write(root / "crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_analytics.rs", compliant_tests() if tests is None else tests)
    write(
        root / "justfile",
        """source-fence-static:
    python3 scripts/test_verify_ra_findings_promotion.py
    python3 scripts/verify_ra_findings_promotion.py
""",
    )
    task_mark = "x" if tasks_checked else " "
    write(
        root / "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
        f"- [{task_mark}] RA-011 Implement the Findings & Promotion model.\n",
    )


def test_compliant_fixture_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root)
        assert verifier.scan_root(root) == []


def test_comments_and_strings_only_do_not_satisfy_code_patterns() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            ra="""
// pub enum RaVerdictKind { Go, NoGo, ConditionalGo }
const TOKENS: &str = "pub struct PromotionConfigRef { typed_config_uri: String }";
""",
        )
        findings = verifier.scan_root(root)
        assert any("missing real RaVerdictKind enum" in finding for finding in findings)
        assert any("missing real PromotionConfigRef as typed field ref" in finding for finding in findings)


def test_active_promotion_package_model_is_rejected() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            ra=compliant_ra() + "\npub struct PromotionPackage;\n",
            artifact_index=compliant_artifact_index().replace("ExperimentResults,", "ExperimentResults,\nPromotionPackages,"),
        )
        findings = verifier.scan_root(root)
        assert any("forbidden active PromotionPackage model" in finding for finding in findings)
        assert any("forbidden active RA promotion-packages artifact family" in finding for finding in findings)


def test_task_checkbox_is_required() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, tasks_checked=False)
        findings = verifier.scan_root(root)
        assert any("RA-011 must be checked" in finding for finding in findings)


def test_go_verdict_real_evidence_gate_is_required() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            ra=compliant_ra().replace(
                """
    fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        if self.verdict == RaVerdictKind::Go && !self.is_real_go_finding() {
            return Err(ResearchAnalyticsArtifactError::PromotionConfigRequiresGo);
        }
        Ok(())
    }

""",
                "",
            ),
        )
        findings = verifier.scan_root(root)
        assert any("GO verdict requires real evidence" in finding for finding in findings)


def test_negative_promotion_package_path_test_is_required() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, tests=compliant_tests().replace("promotion-packages", "experiment-results"))
        findings = verifier.scan_root(root)
        assert any("negative proof that promotion-packages paths are rejected" in finding for finding in findings)


def main() -> int:
    test_compliant_fixture_passes()
    test_comments_and_strings_only_do_not_satisfy_code_patterns()
    test_active_promotion_package_model_is_rejected()
    test_task_checkbox_is_required()
    test_go_verdict_real_evidence_gate_is_required()
    test_negative_promotion_package_path_test_is_required()
    print("OK: verify_ra_findings_promotion self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
