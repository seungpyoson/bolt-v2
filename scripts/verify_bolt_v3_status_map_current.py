#!/usr/bin/env python3
"""Verify the active Bolt-v3 status map is source-grounded and current."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
PURE_RUST_VERIFIER = "scripts/verify_bolt_v3_pure_rust_runtime.py"
SCRIPT_REF_RE = re.compile(r"`(scripts/[^`]+\.py)`")


@dataclass(frozen=True)
class StatusRow:
    number: str
    area: str
    status: str
    source_evidence: str
    test_evidence: str
    gap: str


def parse_rows(text: str) -> list[StatusRow]:
    rows: list[StatusRow] = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|") or stripped.startswith("|---"):
            continue

        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if len(cells) != 6 or not cells[0].isdigit():
            continue

        rows.append(
            StatusRow(
                number=cells[0],
                area=cells[1],
                status=cells[2],
                source_evidence=cells[3],
                test_evidence=cells[4],
                gap=cells[5],
            )
        )
    return rows


def missing_evidence(value: str) -> bool:
    normalized = value.strip().lower()
    return normalized in {"", "missing"} or normalized.startswith("no ")


def main() -> int:
    text = STATUS_MAP.read_text(encoding="utf-8")
    rows = parse_rows(text)
    findings: list[str] = []

    if not rows:
        findings.append(f"{STATUS_MAP.relative_to(REPO_ROOT)}: no status rows parsed")

    by_number = {row.number: row for row in rows}
    pure_rust = by_number.get("3")
    if pure_rust is None:
        findings.append("status row 3 for no Python runtime layer is missing")
    else:
        if pure_rust.area != "No Python runtime layer":
            findings.append(f"row 3 area changed unexpectedly: {pure_rust.area!r}")
        if "missing verifier" in pure_rust.status.lower():
            findings.append("row 3 still says the pure Rust runtime verifier is missing")
        if PURE_RUST_VERIFIER not in pure_rust.test_evidence:
            findings.append(f"row 3 test/verifier evidence must cite `{PURE_RUST_VERIFIER}`")
        if "No dedicated verifier found" in pure_rust.gap:
            findings.append("row 3 gap still says no dedicated verifier was found")

    for row in rows:
        status = row.status.lower()
        if status.startswith("implemented") or status.startswith("partial"):
            if missing_evidence(row.source_evidence):
                findings.append(f"row {row.number} {row.area!r}: status {row.status!r} lacks source evidence")
            if missing_evidence(row.test_evidence):
                findings.append(f"row {row.number} {row.area!r}: status {row.status!r} lacks test/verifier evidence")

    for rel in sorted(set(SCRIPT_REF_RE.findall(text))):
        if not (REPO_ROOT / rel).exists():
            findings.append(f"{STATUS_MAP.relative_to(REPO_ROOT)} references missing verifier `{rel}`")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 status map evidence audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
