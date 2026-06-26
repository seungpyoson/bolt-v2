#!/usr/bin/env python3
"""Verify ROOT-009 cross-project status, label, and legend registry coverage."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRACTS_PATH = Path("specs/023-nt-research-analytics-platform/reference/contracts.md")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/tasks.md")
JUSTFILE_PATH = Path("justfile")

REGISTRY_SECTION_ID = "023-status-legend-registry"
REGISTRY_SECTION_ANCHOR = f'<a id="{REGISTRY_SECTION_ID}"></a>'
TASK_ROOT009 = re.compile(r"^- \[[ xX]\] ROOT-009\b", re.MULTILINE)
REGISTRY_ROW = re.compile(
    r"^\|\s*`(?P<key>[^`]+)`\s*\|\s*(?P<concept>[^|]+?)\s*\|\s*(?P<label>[^|]+?)\s*\|"
    r"\s*(?P<legend>[^|]+?)\s*\|\s*(?P<owner>[^|]+?)\s*\|\s*(?P<setters>[^|]+?)\s*\|"
    r"\s*(?P<displayers>[^|]+?)\s*\|$",
    re.MULTILINE,
)

REQUIRED_COLUMNS = (
    "| Registry key | Concept | Display label | Legend meaning | Owner/source of truth | May set | May display |",
)
REQUIRED_CONCEPTS = (
    "fidelity_class",
    "source_role",
    "data_status",
    "gap_reason",
    "lifecycle_state",
    "run_purpose",
    "artifact_kind",
    "proof_status",
    "warning_label",
    "product_gate_outcome",
)
REQUIRED_VALUES = (
    "L2_REPLAY",
    "TRADE_BAR_REPLAY",
    "SIGNAL_ONLY",
    "FORWARD_CAPTURE_PENDING",
    "SOURCE_PROVEN",
    "USER_ASSUMPTION",
    "GAP",
    "DECISION_NEEDED",
    "authoritative",
    "derived",
    "exploratory",
    "current",
    "stale",
    "partial",
    "unavailable",
    "excluded",
    "missing_source",
    "upstream_blocked",
    "scope_excluded",
    "active",
    "inactive",
    "normal",
    "reproduction",
    "audit",
    "regression",
    "migration",
    "raw",
    "nt-catalog",
    "source-proofs",
    "backtests",
    "artifact-index",
    "research-analytics",
    "accepted",
    "superseded",
    "blocked",
    "mechanical_blocker",
    "claim_limit",
    "selected_existing_product",
    "custom_ui_requires_exception",
)
REQUIRED_OWNER_SNIPPETS = (
    "reference/contracts.md",
    "reference/data-model.md",
    "Backtesting Engine",
    "Research Analytics",
    "Dashboard",
)
JUSTFILE_COMMANDS = (
    "python3 scripts/test_verify_023_status_legend_registry.py",
    "python3 scripts/verify_023_status_legend_registry.py",
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


def parse_registry_rows(text: str) -> list[dict[str, str]]:
    return [match.groupdict() for match in REGISTRY_ROW.finditer(text)]


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    contracts_text = require_file(root, CONTRACTS_PATH, findings)
    tasks_text = require_file(root, TASKS_PATH, findings)
    justfile_text = require_file(root, JUSTFILE_PATH, findings)

    require_snippets(CONTRACTS_PATH, contracts_text, (REGISTRY_SECTION_ANCHOR, *REQUIRED_COLUMNS), findings)
    require_snippets(CONTRACTS_PATH, contracts_text, REQUIRED_CONCEPTS, findings)
    require_snippets(CONTRACTS_PATH, contracts_text, REQUIRED_VALUES, findings)
    require_snippets(CONTRACTS_PATH, contracts_text, REQUIRED_OWNER_SNIPPETS, findings)

    rows = parse_registry_rows(contracts_text)
    if len(rows) < len(REQUIRED_VALUES):
        findings.append(
            f"{CONTRACTS_PATH}: registry must contain at least {len(REQUIRED_VALUES)} value rows; found {len(rows)}"
        )
    row_keys: set[str] = set()
    for row in rows:
        if row["key"] in row_keys:
            findings.append(f"{CONTRACTS_PATH}: duplicate registry row for `{row['key']}`")
        row_keys.add(row["key"])
        if not row["label"].strip():
            findings.append(f"{CONTRACTS_PATH}: `{row['key']}` registry row has empty display label")
        if len(row["legend"].strip()) < 12:
            findings.append(f"{CONTRACTS_PATH}: `{row['key']}` registry row has too-short legend meaning")
        if "may not set" not in row["setters"].lower() and row["setters"].strip() == "":
            findings.append(f"{CONTRACTS_PATH}: `{row['key']}` registry row has empty setter rule")
        if row["displayers"].strip() == "":
            findings.append(f"{CONTRACTS_PATH}: `{row['key']}` registry row has empty display rule")
    for required_value in REQUIRED_VALUES:
        if required_value not in row_keys:
            findings.append(f"{CONTRACTS_PATH}: missing registry row for `{required_value}`")

    if tasks_text and not TASK_ROOT009.search(tasks_text):
        findings.append(f"{TASKS_PATH}: ROOT-009 task row is missing")

    for command in JUSTFILE_COMMANDS:
        if justfile_text and command not in justfile_text:
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: 023 status/legend registry violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: 023 status/legend registry passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
