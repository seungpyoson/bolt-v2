#!/usr/bin/env python3
"""Self-tests for verify_ra_domain_metrics.py."""

from __future__ import annotations

import importlib.util
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_domain_metrics.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_domain_metrics", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def compliant_run_manifest() -> str:
    return """
pub const DOMAIN_METRIC_CLOSED_POSITION_RATIO: &str = "closed_position_ratio";

pub struct ManifestDomainMetricConfig {
    pub kind: String,
}

pub struct BacktestingRunManifest {
    pub domain_metrics: Vec<ManifestDomainMetricConfig>,
}

pub fn registered_domain_metrics() -> &'static [&'static str] {
    &[DOMAIN_METRIC_CLOSED_POSITION_RATIO]
}

fn ensure_supported_domain_metrics(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    for metric in &manifest.domain_metrics {
        if !registered_domain_metrics().contains(&metric.kind.as_str()) {
            return Err(ManifestError::UnsupportedEnum {
                field: "domain_metrics.kind",
                value: metric.kind.clone(),
            });
        }
    }
    Ok(())
}

impl BacktestingRunManifest {
    fn resolved_nt_surfaces(&self) {
        for (index, metric) in self.domain_metrics.iter().enumerate() {
            resolved_surface(
                &format!("domain_metrics[{index}].kind"),
                NtSurfaceClassification::CustomOwned,
                "PortfolioAnalyzer::register_statistic",
                metric.kind.clone(),
            );
        }
    }
}

fn hash_test() {
    assert_hash_changes("domain_metrics", |manifest| {
        manifest.domain_metrics.push(ManifestDomainMetricConfig {
            kind: DOMAIN_METRIC_CLOSED_POSITION_RATIO.to_string(),
        });
    });
}

fn rejects_unknown_domain_metric_selector() {}
"""


def compliant_domain_metrics() -> str:
    return """
use nautilus_analysis::{analyzer::PortfolioAnalyzer, statistic::PortfolioStatistic};
use nautilus_model::position::Position;

pub struct ClosedPositionRatio;

impl PortfolioStatistic for ClosedPositionRatio {
    type Item = f64;

    fn name(&self) -> String {
        "closed_position_ratio".to_string()
    }

    fn calculate_from_positions(&self, positions: &[Position]) -> Option<f64> {
        Some(0.0)
    }
}

pub fn register_domain_statistics(analyzer: &mut PortfolioAnalyzer) {
    analyzer.register_statistic(std::sync::Arc::new(ClosedPositionRatio));
}

pub fn domain_statistics_from_analyzer(analyzer: &PortfolioAnalyzer) -> std::collections::BTreeMap<String, f64> {
    analyzer.get_performance_stats_general().into_iter().collect()
}

fn registers_domain_statistics_with_nt_portfolio_analyzer() {}
"""


def compliant_runner() -> str:
    return """
fn run_nt_backtest_node(manifest: &BacktestingRunManifest) -> Result<NtBacktestNodeRun> {
    let domain_statistics = resolve_domain_statistics(&manifest.domain_metrics)?;
    let mut node = BacktestNode::new(vec![])?;
    let engine = node.get_engine_mut(&manifest.run_id)?;
    let mut analyzer = PortfolioAnalyzer::default();
    register_domain_statistics(&mut analyzer, &domain_statistics)?;
    let mut results = node.run()?;
    let engine = node.get_engine(&manifest.run_id)?;
    let metrics = domain_statistics_from_analyzer(engine, &domain_statistics)?;
    let mut nt_result = results.remove(0);
    for (name, value) in metrics {
        nt_result.stats_general.insert(name, value);
    }
    Ok(NtBacktestNodeRun { result: nt_result })
}
"""


def write_common(root: Path, *, run_manifest: str | None = None, runner: str | None = None, domain_metrics: str | None = None) -> None:
    write(root / "crates/backtesting-vertical-slice/src/run_manifest.rs", compliant_run_manifest() if run_manifest is None else run_manifest)
    write(root / "crates/backtesting-vertical-slice/src/runner.rs", compliant_runner() if runner is None else runner)
    write(root / "crates/backtesting-vertical-slice/src/domain_metrics.rs", compliant_domain_metrics() if domain_metrics is None else domain_metrics)
    write(root / "crates/backtesting-vertical-slice/src/lib.rs", "pub mod domain_metrics;\n")
    write(root / "crates/backtesting-vertical-slice/Cargo.toml", 'nautilus-analysis = { git = "https://github.com/nautechsystems/nautilus_trader.git" }\n')
    write(root / "justfile", "source-fence-static:\n    python3 scripts/test_verify_ra_domain_metrics.py\n    python3 scripts/verify_ra_domain_metrics.py\n")


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
            run_manifest='// pub struct ManifestDomainMetricConfig\nconst TOKEN: &str = "domain_metrics: Vec<ManifestDomainMetricConfig>";\n',
            domain_metrics='// impl PortfolioStatistic for CommentOnly\nconst TOKEN: &str = "register_statistic(";',
        )
        findings = verifier.scan_root(root)
        assert any("missing real ManifestDomainMetricConfig" in finding for finding in findings)
        assert any("missing real PortfolioStatistic impl" in finding for finding in findings)


def test_runner_must_insert_metrics_into_nt_result_before_contract_persistence() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, runner="fn run_nt_backtest_node() { let results = node.run(); }\n")
        findings = verifier.scan_root(root)
        assert any("runner copies domain metrics into BacktestResult" in finding for finding in findings)


def main() -> int:
    test_compliant_fixture_passes()
    test_comments_and_strings_only_do_not_satisfy_code_patterns()
    test_runner_must_insert_metrics_into_nt_result_before_contract_persistence()
    print("OK: verify_ra_domain_metrics self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
