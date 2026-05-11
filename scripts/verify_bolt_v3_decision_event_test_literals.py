#!/usr/bin/env python3
"""Reject inline Bolt-v3 decision-event contract literals in selected tests."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
DECISION_EVENT_HANDOFF_TEST_FILE = "tests/bolt_v3_decision_event_handoff.rs"
ORDER_INTENT_GATE_TEST_FILE = "tests/bolt_v3_order_intent_gate.rs"
DECISION_EVENT_CONTEXT_TEST_FILE = "tests/bolt_v3_decision_event_context.rs"
ETH_CHAINLINK_RUNTIME_TEST_FILE = "tests/eth_chainlink_taker_runtime.rs"
ENFORCED_TEST_FILES = (
    DECISION_EVENT_HANDOFF_TEST_FILE,
    DECISION_EVENT_CONTEXT_TEST_FILE,
    ETH_CHAINLINK_RUNTIME_TEST_FILE,
    ORDER_INTENT_GATE_TEST_FILE,
)
EVENT_FACT_GET_PATTERN = re.compile(
    r"event_facts\s*\.\s*get\s*\(\s*\"(?P<literal>[^\"]+)\"",
    re.DOTALL,
)
DECISION_EVENT_TYPE_LITERAL_PATTERN = re.compile(
    r"decision_event_type\s*,\s*\"(?P<literal>[a-z_]+)\"",
)
JSON_OBJECT_MACRO_PATTERN = re.compile(r"json!\s*\(\s*\{")
STRING_LITERAL_PATTERN = re.compile(r'"(?P<literal>[a-z_][a-z0-9_]*)"')
RUST_STRING_LITERAL_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
DIRECT_COMMON_FIELDS_PATTERN = re.compile(r"=\s*BoltV3DecisionEventCommonFields\s*\{")
DIRECT_ORDER_SUBMISSION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3OrderSubmissionFacts\s*\{|^\s*BoltV3OrderSubmissionFacts\s*\{",
    re.MULTILINE,
)
DECISION_EVENT_CONTEXT_FORBIDDEN_LITERAL_VALUES = {
    "release-sha",
    "config-hash",
    "38b912a8b0fe14e4046773973ff46a3b798b1e3e",
    "123e4567-e89b-12d3-a456-426614174002",
    "strategy-alpha",
    "POLY-A",
    "eth_updown_5m",
    "target-eth-updown",
}
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


def string_value(literal: str) -> str:
    return bytes(literal[1:-1], "utf-8").decode("unicode_escape")


def scan_file(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []

    if rel == DECISION_EVENT_HANDOFF_TEST_FILE:
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

    if rel == ORDER_INTENT_GATE_TEST_FILE:
        for match in DIRECT_COMMON_FIELDS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct decision-event common-field fixture construction; "
                        "derive common fields from v3 TOML and release identity"
                    ),
                    excerpt="BoltV3DecisionEventCommonFields {",
                )
            )
        for match in DIRECT_ORDER_SUBMISSION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct order-submission fact fixture construction; "
                        "load order fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
                )
            )

    if rel in {
        DECISION_EVENT_CONTEXT_TEST_FILE,
        DECISION_EVENT_HANDOFF_TEST_FILE,
        ETH_CHAINLINK_RUNTIME_TEST_FILE,
    }:
        for match in RUST_STRING_LITERAL_PATTERN.finditer(text):
            value = string_value(match.group(0))
            if value not in DECISION_EVENT_CONTEXT_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline decision-event context fixture literal; "
                        "derive from v3 TOML, release identity, or generated trace id"
                    ),
                    excerpt=match.group(0),
                )
            )

    if rel == DECISION_EVENT_HANDOFF_TEST_FILE:
        for match in DIRECT_ORDER_SUBMISSION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct order-submission fact fixture construction; "
                        "load order fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
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
