#!/usr/bin/env python3
"""Verify RA-016 wires the binary-oracle BTE prerequisite."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/plan.md")
SPEC_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/spec.md")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")
BTE_CARGO_TOML = Path("crates/backtesting-vertical-slice/Cargo.toml")
BTE_RUN_MANIFEST = Path("crates/backtesting-vertical-slice/src/run_manifest.rs")
BTE_RUNNER = Path("crates/backtesting-vertical-slice/src/runner.rs")

PLAN_REQUIRED_SNIPPETS = (
    "## Backtest Phase Prerequisite",
    "HurstVpinDirectional",
    "bybit-spot",
    "binary_oracle_edge_taker",
    "venue normalization",
    "before any Phase-3 sweep is real",
    "hard precondition",
)
SPEC_REQUIRED_SNIPPETS = (
    "Known prerequisite",
    "HurstVpinDirectional",
    "bybit-spot",
    "binary_oracle_edge_taker",
    "venue normalization",
    "before Phase-3 sweeps are real",
)
TASK_REQUIRED_SNIPPETS = (
    "RA-016 Document the known prerequisite",
    "HurstVpinDirectional",
    "bybit-spot",
    "binary_oracle_edge_taker",
    "venue normalization",
    "before Phase-3 sweeps produce valid results",
)
CHECKED_RA016 = re.compile(r"^- \[[xX]\] RA-016\b", re.MULTILINE)

BTE_REQUIRED_SNIPPETS = {
    BTE_CARGO_TOML: (
        'bolt-v2 = { path = "../.." }',
        'futures-util = "=0.3.32"',
    ),
    BTE_RUN_MANIFEST: (
        'STRATEGY_BINARY_ORACLE_EDGE_TAKER',
        '"binary_oracle_edge_taker"',
        'STRATEGY_PARAM_CONFIG_TOML',
        'STRATEGY_PARAM_FEE_BPS',
        'production_strategy_registry',
        'BinaryOracleEdgeTakerBuilder::kind()',
    ),
    BTE_RUNNER: (
        'STRATEGY_BINARY_ORACLE_EDGE_TAKER',
        'BinaryOracleEdgeTakerBuilder',
        'BoltV3SubmitAdmissionState',
        'BoltV3DecisionEvidenceWriter',
        'StrategyBuildContext::new',
        'Venue::from(manifest.venue.nt_venue.as_str())',
        'BinaryOracleEdgeTakerBuilder::build_strategy',
        'engine.add_strategy(strategy)',
    ),
}


def require_file(root: Path, rel_path: Path, findings: list[str]) -> str:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return ""
    return path.read_text(encoding="utf-8")


def require_snippets(rel_path: Path, text: str, snippets: tuple[str, ...], findings: list[str]) -> None:
    for snippet in snippets:
        if snippet not in text:
            findings.append(f"{rel_path}: missing `{snippet}`")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    plan_text = require_file(root, PLAN_PATH, findings)
    spec_text = require_file(root, SPEC_PATH, findings)
    tasks_text = require_file(root, TASKS_PATH, findings)

    require_snippets(PLAN_PATH, plan_text, PLAN_REQUIRED_SNIPPETS, findings)
    require_snippets(SPEC_PATH, spec_text, SPEC_REQUIRED_SNIPPETS, findings)
    require_snippets(TASKS_PATH, tasks_text, TASK_REQUIRED_SNIPPETS, findings)

    if tasks_text and not CHECKED_RA016.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-016 must be checked once the prerequisite is documented")

    for rel_path, snippets in BTE_REQUIRED_SNIPPETS.items():
        text = require_file(root, rel_path, findings)
        require_snippets(rel_path, text, snippets, findings)

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA BTE phase prerequisite violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA BTE phase prerequisite passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
