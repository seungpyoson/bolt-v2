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

PLAN_REQUIRED_SNIPPETS = (
    "## Customer Jobs And Capability Classes",
    "Product choice is deferred",
    "Trade monitor",
    "ongoing trades/orders",
    "Trade investigation",
    "source proof/data used",
    "Annotation/review notes",
    "explicit owner/schema/audit",
    "Controlled action workflow",
    "Trading/runtime/credential/fund/order",
    "unless separately approved",
)
SPEC_REQUIRED_SNIPPETS = (
    "classify customer jobs and write capabilities",
    "before product selection",
    "Non-trading annotation/review workflow writes",
    "explicit artifact kind/schema/owner/audit",
    "Trading, runtime config, credential, and funds/order mutations remain outside",
)
TASK_REQUIRED_SNIPPETS = (
    "DASH-001 Define dashboard customer jobs and capability classes",
    "trade monitor",
    "trade investigation",
    "optional annotation/review notes",
    "controlled action workflow",
    "trading/runtime/credential/fund/order mutation outside",
)
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


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    plan_text = require_file(root, PLAN_PATH, findings)
    spec_text = require_file(root, SPEC_PATH, findings)
    tasks_text = require_file(root, TASKS_PATH, findings)

    require_snippets(PLAN_PATH, plan_text, PLAN_REQUIRED_SNIPPETS, findings)
    require_snippets(SPEC_PATH, spec_text, SPEC_REQUIRED_SNIPPETS, findings)
    require_snippets(TASKS_PATH, tasks_text, TASK_REQUIRED_SNIPPETS, findings)

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
