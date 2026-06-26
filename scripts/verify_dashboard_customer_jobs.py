#!/usr/bin/env python3
"""Verify DASH-001 customer jobs and capability classes are defined."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_PATH = Path("specs/023-nt-research-analytics-platform/3-dashboard/plan.md")
SPEC_PATH = Path("specs/023-nt-research-analytics-platform/3-dashboard/spec.md")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/3-dashboard/tasks.md")

PLAN_MARKER = "dashboard-customer-job-ids"
PLAN_REQUIRED_IDS = (
    "trade_monitor",
    "trade_investigation",
    "annotation_review_notes",
    "controlled_action_workflow",
)
SPEC_MARKER = "dashboard-capability-boundary-ids"
SPEC_REQUIRED_IDS = ("no_trading_runtime_credential_fund_order_mutation",)
TASK_DASH001 = re.compile(r"^- \[[ xX]\] DASH-001\b", re.MULTILINE)


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


def marker_ids(text: str, marker: str) -> set[str] | None:
    match = re.search(rf"<!--\s*{re.escape(marker)}\s*:\s*(?P<ids>.*?)-->", text, re.DOTALL)
    if match is None:
        return None
    return {part.strip() for part in match.group("ids").replace("\n", " ").split(",") if part.strip()}


def require_marker_ids(rel_path: Path, text: str, marker: str, required_ids: tuple[str, ...], findings: list[str]) -> None:
    ids = marker_ids(text, marker)
    if ids is None:
        findings.append(f"{rel_path}: missing `{marker}` marker")
        return
    for required_id in required_ids:
        if required_id not in ids:
            findings.append(f"{rel_path}: `{marker}` missing `{required_id}`")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    plan_text = require_file(root, PLAN_PATH, findings)
    spec_text = require_file(root, SPEC_PATH, findings)
    tasks_text = require_file(root, TASKS_PATH, findings)

    require_marker_ids(PLAN_PATH, plan_text, PLAN_MARKER, PLAN_REQUIRED_IDS, findings)
    require_marker_ids(SPEC_PATH, spec_text, SPEC_MARKER, SPEC_REQUIRED_IDS, findings)

    if tasks_text and not TASK_DASH001.search(tasks_text):
        findings.append(f"{TASKS_PATH}: DASH-001 task row is missing")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: dashboard customer-jobs violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: dashboard customer jobs passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
