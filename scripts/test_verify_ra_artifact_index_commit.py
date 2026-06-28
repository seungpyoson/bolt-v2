#!/usr/bin/env python3
"""Self-tests for verify_ra_artifact_index_commit.py."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_artifact_index_commit.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_artifact_index_commit", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def compliant_artifact_store() -> str:
    return """
pub enum ArtifactKind {
    Backtests,
    ResearchAnalytics,
}

const RESEARCH_ANALYTICS_DATASETS_SUBFAMILY: &str = "datasets";
const RESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY: &str = "feature-tables";
const RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY: &str = "experiment-results";
const RESEARCH_ANALYTICS_ARTIFACT_FAMILIES: &[&str] = &[
    RESEARCH_ANALYTICS_DATASETS_SUBFAMILY,
    RESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY,
    RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY,
];

fn validate_artifact_uri_for_kind(
    artifact_root: &ResolvedArtifactRoot,
    kind: ArtifactKind,
    field: &str,
    uri: &str,
) -> Result<()> {
    artifact_root.object_path_for_uri(uri)?;
    let typed_root = artifact_root.typed_root(kind);
    uri.starts_with(&format!("{typed_root}/"));
    if kind != ArtifactKind::ResearchAnalytics {
        return Ok(());
    }
    RESEARCH_ANALYTICS_ARTIFACT_FAMILIES
        .iter()
        .any(|family| uri.starts_with(&format!("{typed_root}/{family}/")));
    Ok(())
}

impl ArtifactIndexEvent {
    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        validate_artifact_uri_for_kind(artifact_root, self.artifact_kind, "artifact_uri", &self.artifact_uri)?;
        validate_artifact_uri_for_kind(artifact_root, self.artifact_kind, "manifest_uri", &self.manifest_uri)?;
        Ok(())
    }
}

impl ArtifactIndexSnapshotRow {
    fn validate(&self, snapshot_kind: ArtifactKind, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        validate_artifact_uri_for_kind(artifact_root, self.artifact_kind, "artifact_uri", &self.artifact_uri)?;
        validate_artifact_uri_for_kind(artifact_root, self.artifact_kind, "manifest_uri", &self.manifest_uri)?;
        Ok(())
    }
}

pub struct ArtifactIndexWriteAuthority;
impl ArtifactIndexWriteAuthority {
    fn authorize_commit(&self, writer_id: &str, kind: ArtifactKind) -> Result<()> {
        Ok(())
    }
}
"""


def compliant_tests() -> str:
    return """
async fn research_analytics_writer_commits_all_owned_families_to_one_kind_snapshot() {
    let dataset = research_analytics_event(&root, "datasets", "ra-event-001", "dataset-001", 'a');
    let feature_table = research_analytics_event(&root, "feature-tables", "ra-event-002", "feature-table-001", 'c');
    let experiment_result = research_analytics_event(&root, "experiment-results", "ra-event-003", "experiment-result-001", 'd');
    writer.commit_event(&root, plan).await.unwrap();
    let snapshot = writer.read_verified_latest_snapshot(&root, ArtifactKind::ResearchAnalytics).await.unwrap();
    let row = writer.read_committed_row(&root, ArtifactKind::ResearchAnalytics, "experiment-result-001").await.unwrap();
    assert_eq!(row.lifecycle_state, ArtifactLifecycleState::Active);
    assert_eq!(row.commit_state, ArtifactIndexCommitState::Committed);
    assert_eq!(root.latest_pointer(ArtifactKind::ResearchAnalytics), "s3://root/artifact-index/v1/pointers/kind=research-analytics/latest.json");
}

async fn artifact_index_writer_rejects_consumer_mutation_of_research_analytics_records() {
    writer.commit_event(&root, plan).await.expect_err("consumer writer must not mutate RA index records");
}

async fn research_analytics_index_rejects_promotion_package_family() {
    let event = research_analytics_event(&root, "promotion-packages", "ra-event", "promotion-package-001", 'f');
    writer.put_event(&root, &event).await.expect_err("promotion-packages is not an RA artifact family");
}

async fn artifact_index_rejects_cross_kind_artifact_uri_squatting() {
    writer.put_event(&root, &event).await.expect_err("backtest event must not claim an NT catalog artifact URI");
}
"""


def write_common(
    root: Path,
    *,
    artifact_store: str | None = None,
    artifact_index: str = "pub enum ResearchAnalyticsSubfamily { Datasets, FeatureTables, ExperimentResults }\n",
    tests: str | None = None,
) -> None:
    write(root / "crates/backtesting-vertical-slice/src/artifact_store.rs", compliant_artifact_store() if artifact_store is None else artifact_store)
    write(root / "crates/backtesting-vertical-slice/src/artifact_index.rs", artifact_index)
    write(root / "crates/backtesting-vertical-slice/tests/artifact_store_contract.rs", compliant_tests() if tests is None else tests)
    write(
        root / "justfile",
        """source-fence-static:
    python3 scripts/test_verify_ra_artifact_index_commit.py
    python3 scripts/verify_ra_artifact_index_commit.py
""",
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
            artifact_store="""
// fn validate_artifact_uri_for_kind() { ArtifactKind::ResearchAnalytics typed_root RESEARCH_ANALYTICS_ARTIFACT_FAMILIES starts_with }
const TOKEN_STUFFING: &str = "impl ArtifactIndexEvent { fn validate() { validate_artifact_uri_for_kind artifact_uri manifest_uri } }";
const RESEARCH_ANALYTICS_DATASETS_SUBFAMILY: &str = "datasets";
const RESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY: &str = "feature-tables";
const RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY: &str = "experiment-results";
""",
        )
        findings = verifier.scan_root(root)
        assert any("missing real RA kind URI validator" in finding for finding in findings)
        assert any("missing real event validation uses kind URI validator" in finding for finding in findings)


def test_promotion_package_family_is_rejected_in_model() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            artifact_index="pub enum ResearchAnalyticsSubfamily { Datasets, FeatureTables, ExperimentResults, PromotionPackages }\n",
        )
        findings = verifier.scan_root(root)
        assert any("forbidden active RA promotion-packages artifact family" in finding for finding in findings)


def test_required_commit_test_literals_are_enforced() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, tests=compliant_tests().replace("feature-tables", "datasets"))
        findings = verifier.scan_root(root)
        assert any("missing RA commit test literal 'feature-tables'" in finding for finding in findings)


def main() -> int:
    test_compliant_fixture_passes()
    test_comments_and_strings_only_do_not_satisfy_code_patterns()
    test_promotion_package_family_is_rejected_in_model()
    test_required_commit_test_literals_are_enforced()
    print("OK: verify_ra_artifact_index_commit self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
