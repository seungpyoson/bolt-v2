#!/usr/bin/env python3
"""Self-tests for the RA BTE phase prerequisite verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_bte_phase_prerequisite.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_bte_phase_prerequisite", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def documented_plan_text(*, omit_binary_oracle: bool = False) -> str:
    binary_oracle = "" if omit_binary_oracle else "`binary_oracle_edge_taker` strategy and "
    return f"""
## Backtest Phase Prerequisite

The BTE runner today wires only an NT example strategy
(`HurstVpinDirectional`) over a single venue (`bybit-spot`). Bolt's
{binary_oracle}venue normalization must be wired into
the BTE engine before any Phase-3 sweep is real. Surface this prerequisite
explicitly in the backtest phase; do not hide it. NT's pyo3
`add_native_strategy` is `#[cfg(feature = "examples")]` and can only run NT
example strategies, not bolt's, so this wiring is a hard precondition, not an
optional optimization.
"""


def documented_spec_text() -> str:
    return """
Known prerequisite (do not hide): the BTE runner today registers only an NT
example strategy (`HurstVpinDirectional`) over one venue (`bybit-spot`); bolt's
`binary_oracle_edge_taker` + venue normalization must be wired into the BTE
before Phase-3 sweeps are real.
"""


def documented_tasks_text(*, checked: bool = True) -> str:
    mark = "x" if checked else " "
    return f"""
- [{mark}] RA-016 Document the known prerequisite: the BTE runner today registers only an NT example strategy (HurstVpinDirectional) over one venue (bybit-spot); bolt's binary_oracle_edge_taker + venue normalization must be wired into the BTE before Phase-3 sweeps produce valid results.
"""


def write_complete_fixture(root: Path) -> None:
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
        documented_plan_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md",
        documented_spec_text(),
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
        documented_tasks_text(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/Cargo.toml",
        'bolt-v2 = { path = "../.." }\nfutures-util = "=0.3.32"\n',
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/run_manifest.rs",
        """
use bolt_v2::strategies::{
    binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder,
    production_strategy_registry,
    registry::StrategyBuilder,
};
pub const STRATEGY_BINARY_ORACLE_EDGE_TAKER: &str = "binary_oracle_edge_taker";
pub const STRATEGY_PARAM_CONFIG_TOML: &str = "config_toml";
pub const STRATEGY_PARAM_FEE_BPS: &str = "fee_bps";
pub fn registered_strategies() -> &'static [&'static str] {
    &[STRATEGY_BINARY_ORACLE_EDGE_TAKER]
}
pub fn registered_strategy_parameters(registry_key: &str) -> Option<&'static [&'static str]> {
    match registry_key {
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => Some(&[
            STRATEGY_PARAM_CONFIG_TOML,
            STRATEGY_PARAM_FEE_BPS,
        ]),
        _ => None,
    }
}
fn validate_strategy_source(strategy: StrategySource) -> Result<(), ManifestError> {
    match strategy.registry_key.as_str() {
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => {
            let _ = strategy.parameters.get(STRATEGY_PARAM_CONFIG_TOML);
            let fee_bps = rust_decimal::Decimal::from_str(
                strategy.parameters.get(STRATEGY_PARAM_FEE_BPS).unwrap()
            ).unwrap();
            if fee_bps < rust_decimal::Decimal::ZERO {
                return Err(ManifestError::MissingField("strategy.parameters.fee_bps"));
            }
            let raw_config = toml::from_str::<toml::Value>(
                strategy.parameters.get(STRATEGY_PARAM_CONFIG_TOML).unwrap()
            ).unwrap();
            let registry = production_strategy_registry().unwrap();
            registry.validate(
                BinaryOracleEdgeTakerBuilder::kind(),
                &raw_config,
                "strategy.parameters.config_toml",
                &mut Vec::new(),
            );
        }
        _ => {}
    }
    Ok(())
}
""",
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/runner.rs",
        """
use bolt_v2::{
    bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    strategies::{
        production_strategy_registry,
        registry::StrategyBuildContext,
    },
};
use nautilus_model::identifiers::Venue;
use crate::run_manifest::STRATEGY_BINARY_ORACLE_EDGE_TAKER;
fn add_manifest_strategy(engine: &mut Engine, manifest: &Manifest) -> Result<()> {
    match manifest.strategy.registry_key.as_str() {
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => {
            let raw_config = manifest.strategy.parameters.get(PARAM_CONFIG_TOML).unwrap();
            let raw_config = toml::from_str::<toml::Value>(raw_config).unwrap();
            let fee_bps_raw = manifest.strategy.parameters.get(PARAM_FEE_BPS).unwrap();
            let fee_bps = Decimal::from_str(fee_bps_raw).unwrap();
            ensure!(fee_bps >= Decimal::ZERO);
            let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> =
                Arc::new(BacktestDecisionEvidenceWriter);
            let submit_admission =
                Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
            let fee_provider = Arc::new(ManifestFeeProvider { fee_bps });
            let build_context = StrategyBuildContext::new(
                fee_provider,
                decision_evidence,
                submit_admission,
                Venue::from(manifest.venue.nt_venue.as_str()),
            );
            let registry = production_strategy_registry().unwrap();
            registry
                .register_strategy(
                    STRATEGY_BINARY_ORACLE_EDGE_TAKER,
                    &raw_config,
                    &build_context,
                    engine.kernel().trader(),
                )
                .unwrap();
        }
        _ => {}
    }
    Ok(())
}
""",
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_documented_prerequisite_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)

        assert verifier.scan_root(root) == []


def test_missing_bolt_strategy_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
            documented_plan_text(omit_binary_oracle=True),
        )

        findings = verifier.scan_root(root)

    assert any("binary_oracle_edge_taker" in finding for finding in findings)


def test_missing_bte_runner_wiring_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/runner.rs",
            "use crate::run_manifest::STRATEGY_BINARY_ORACLE_EDGE_TAKER;\n",
        )

        findings = verifier.scan_root(root)

    assert any("runner" in finding or "add_manifest_strategy" in finding for finding in findings)


def test_runner_tokens_in_comments_and_strings_do_not_satisfy_wiring() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "crates/backtesting-vertical-slice/src/runner.rs",
            """
const FAKE: &str = "STRATEGY_BINARY_ORACLE_EDGE_TAKER BinaryOracleEdgeTakerBuilder \
BoltV3SubmitAdmissionState BoltV3DecisionEvidenceWriter StrategyBuildContext::new \
Venue::from(manifest.venue.nt_venue.as_str()) production_strategy_registry \
registry.register_strategy engine.kernel().trader";

// STRATEGY_BINARY_ORACLE_EDGE_TAKER
// BinaryOracleEdgeTakerBuilder
// BoltV3SubmitAdmissionState
// BoltV3DecisionEvidenceWriter
// StrategyBuildContext::new
// Venue::from(manifest.venue.nt_venue.as_str())
// production_strategy_registry
// registry.register_strategy
// engine.kernel().trader
fn add_manifest_strategy() {}
""",
        )

        findings = verifier.scan_root(root)

    assert any("runner" in finding and "binary_oracle_edge_taker" in finding for finding in findings)


def test_unchecked_task_still_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_complete_fixture(root)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
            documented_tasks_text(checked=False),
        )

        assert verifier.scan_root(root) == []


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/plan.md",
            documented_plan_text(omit_binary_oracle=True),
        )
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/spec.md",
            documented_spec_text(),
        )
        write_file(
            root,
            "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
            documented_tasks_text(),
        )

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "binary_oracle_edge_taker" in result.stderr


def main() -> int:
    tests = [
        test_documented_prerequisite_passes,
        test_missing_bolt_strategy_is_a_finding,
        test_missing_bte_runner_wiring_is_a_finding,
        test_runner_tokens_in_comments_and_strings_do_not_satisfy_wiring,
        test_unchecked_task_still_passes,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA BTE phase prerequisite verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
