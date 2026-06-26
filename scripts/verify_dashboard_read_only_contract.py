#!/usr/bin/env python3
"""Verify DASH-003..014 dashboard read-only contract and product-gate coverage."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
DASHBOARD_RS = Path("crates/backtesting-vertical-slice/src/dashboard_contract.rs")
DASHBOARD_TEST = Path("crates/backtesting-vertical-slice/tests/dashboard_contract.rs")
DASHBOARD_PLAN = Path("specs/023-nt-research-analytics-platform/3-dashboard/plan.md")
DASHBOARD_TASKS = Path("specs/023-nt-research-analytics-platform/3-dashboard/tasks.md")
JUSTFILE = Path("justfile")

CHECKED_TASKS = (
    "DASH-003",
    "DASH-004",
    "DASH-005",
    "DASH-006",
    "DASH-007",
    "DASH-008",
    "DASH-009",
    "DASH-010",
    "DASH-011",
    "DASH-012",
    "DASH-013",
    "DASH-014",
)
CODE_SNIPPETS = (
    "pub fn validate_dashboard_read_model",
    "DashboardReadModelSpec",
    "DashboardFieldSource",
    "DashboardSourceBinding",
    "PortfolioSnapshot",
    "DurableTradeHistoryPnl",
    "ArtifactIndexBulkListSource",
    "IndependentLatestPointers",
    "DashboardMutationKind",
    "DerivedFromBteMetrics",
    "MutatesFindingReviewArtifact",
    "upgrades_proof_strength",
    "weakens_forbidden_claims",
    "relabels_historical_result_after_supersession",
    "redemption_realized_pnl_included",
    "source_binding_key",
    "venue/provider identity",
    "artifact_root",
    "committed snapshot",
)
TEST_SNIPPETS = (
    "dashboard_read_model_accepts_read_only_sources_with_config_binding_keys",
    "dashboard_rejects_source_reclassification_and_ra_verdict_derivation",
    "dashboard_field_source_resolution_uses_source_binding_key_not_venue_or_provider",
    "dashboard_artifact_links_and_index_reads_stay_under_artifact_root",
    "dashboard_rejects_mutation_actions_and_canonical_artifact_writes",
    "dashboard_rejects_unmapped_stale_missing_pnl_and_strategy_truth_fields",
)
PLAN_SNIPPETS = (
    "## Dependency Decisions",
    "#409 / `PortfolioSnapshot`",
    "#77 durable trade-history/PnL",
    "#36 redemption-realized PnL",
    "#369 is non-closure context",
    "## Read-Only Source Contract Validation",
    "## Product Gate",
    "Decision: select Metabase Open Source/self-hosted",
    "Grafana Cloud",
    "Metabase",
    "Preset/Superset",
    "Retool",
    "Plotly/Dash",
    "Custom UI",
    "source_binding_key",
    "no-mutation-controls",
    "annotation/review",
)
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_dashboard_read_only_contract.py",
    "python3 scripts/verify_dashboard_read_only_contract.py",
)


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


def task_is_present(tasks_text: str, task_id: str) -> bool:
    return re.search(rf"^- \[[ xX]\] {re.escape(task_id)}\b", tasks_text, re.MULTILINE) is not None


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    code_text = require_file(root, DASHBOARD_RS, findings)
    test_text = require_file(root, DASHBOARD_TEST, findings)
    plan_text = require_file(root, DASHBOARD_PLAN, findings)
    tasks_text = require_file(root, DASHBOARD_TASKS, findings)
    justfile_text = require_file(root, JUSTFILE, findings)

    require_snippets(DASHBOARD_RS, code_text, CODE_SNIPPETS, findings)
    require_snippets(DASHBOARD_TEST, test_text, TEST_SNIPPETS, findings)
    require_snippets(DASHBOARD_PLAN, plan_text, PLAN_SNIPPETS, findings)

    for task_id in CHECKED_TASKS:
        if tasks_text and not task_is_present(tasks_text, task_id):
            findings.append(f"{DASHBOARD_TASKS}: {task_id} task row is missing")

    for command in JUSTFILE_COMMANDS:
        if justfile_text and command not in justfile_text:
            findings.append(f"{JUSTFILE}: source-fence-static must run {command}")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: dashboard read-only contract violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: dashboard read-only contract passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
