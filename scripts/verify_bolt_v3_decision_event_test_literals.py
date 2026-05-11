#!/usr/bin/env python3
"""Reject inline Bolt-v3 decision-event contract literals in selected tests."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ENFORCED_TEST_FILES = ("tests/bolt_v3_decision_event_handoff.rs",)
EVENT_FACT_GET_PATTERN = re.compile(
    r"event_facts\s*\.\s*get\s*\(\s*\"(?P<literal>[^\"]+)\"",
    re.DOTALL,
)
DECISION_EVENT_TYPE_LITERAL_PATTERN = re.compile(
    r"decision_event_type\s*,\s*\"(?P<literal>[a-z_]+)\"",
)
JSON_OBJECT_MACRO_PATTERN = re.compile(r"json!\s*\(\s*\{")
STRING_LITERAL_PATTERN = re.compile(r'"(?P<literal>[a-z_][a-z0-9_]*)"')
DECISION_REASON_VALUES = {
    "active_book_not_priced",
    "fast_venue_incoherent",
    "freeze",
    "insufficient_edge",
    "instrument_id_missing",
    "invalid_quantity",
    "market_cooling_down",
    "metadata_mismatch",
    "one_position_invariant",
    "recovery_mode",
    "thin_book",
    "exit_order_mechanical_rejection",
}


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    message: str
    excerpt: str

    def render(self) -> str:
        return f"FAIL: {self.path}:{self.line}: {self.message}: {self.excerpt}"


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def scan_file(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []

    for match in EVENT_FACT_GET_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="inline decision-event fact key; use exported event contract constant",
                excerpt=match.group("literal"),
            )
        )

    for match in DECISION_EVENT_TYPE_LITERAL_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="inline decision-event type value; use exported event contract constant",
                excerpt=match.group("literal"),
            )
        )

    for match in JSON_OBJECT_MACRO_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="inline decision-event JSON object fixture; move fixture data out of Rust test",
                excerpt="json!({",
            )
        )

    for match in STRING_LITERAL_PATTERN.finditer(text):
        literal = match.group("literal")
        if literal not in DECISION_REASON_VALUES:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="inline decision-event reason value; use exported event contract constant",
                excerpt=literal,
            )
        )

    return findings


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 decision-event test-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
