#!/usr/bin/env python3
"""Verify DASH-002 dashboard field-source matrix coverage."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
PLAN_PATH = Path("specs/023-nt-research-analytics-platform/3-dashboard/plan.md")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/3-dashboard/tasks.md")

PLAN_REQUIRED_SNIPPETS = (
    "## Field Source Matrix Seed",
    "Matrix semantics come from `../reference/contracts.md`",
    "Trade explanation fields",
    "source_proof_id",
    "run_purpose",
    "proof_pin_reason_code",
    "proof_pin_reason_detail",
    "fidelity_class",
    "claim_limits",
    "warning_fields",
    "source_role",
    "data_status",
    "gap_reason",
)
TASK_REQUIRED_SNIPPETS = (
    "DASH-002 Define dashboard field-source matrix",
    "trade explanation fields",
    "source proof id",
    "run purpose",
    "proof pin reason code/detail",
    "fidelity class",
    "claim limits",
    "warning fields",
    "source role",
    "data status/gap reason",
)
CHECKED_DASH002 = re.compile(r"^- \[[xX]\] DASH-002\b", re.MULTILINE)


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
    tasks_text = require_file(root, TASKS_PATH, findings)

    require_snippets(PLAN_PATH, plan_text, PLAN_REQUIRED_SNIPPETS, findings)
    require_snippets(TASKS_PATH, tasks_text, TASK_REQUIRED_SNIPPETS, findings)

    if tasks_text and not CHECKED_DASH002.search(tasks_text):
        findings.append(f"{TASKS_PATH}: DASH-002 must be checked once the field-source matrix is defined")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: dashboard field-source matrix violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: dashboard field-source matrix passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
