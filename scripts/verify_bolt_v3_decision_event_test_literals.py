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
DECISION_EVENT_SOURCE_FILE = "src/bolt_v3_decision_events.rs"
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
REASON_CONST_PATTERN = re.compile(
    r"pub\s+const\s+(?P<name>[A-Z0-9_]*REASONS?[A-Z0-9_]*)\s*:[^=]+=\s*(?P<body>.*?);",
    re.DOTALL,
)
DIRECT_COMMON_FIELDS_PATTERN = re.compile(r"=\s*BoltV3DecisionEventCommonFields\s*\{")
DIRECT_ORDER_SUBMISSION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3OrderSubmissionFacts\s*\{|^\s*BoltV3OrderSubmissionFacts\s*\{",
    re.MULTILINE,
)
DIRECT_ENTRY_EVALUATION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3EntryEvaluationFacts\s*\{|^\s*BoltV3EntryEvaluationFacts\s*\{",
    re.MULTILINE,
)
DIRECT_EXIT_EVALUATION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3ExitEvaluationFacts\s*\{|^\s*BoltV3ExitEvaluationFacts\s*\{",
    re.MULTILINE,
)
DIRECT_MARKET_SELECTION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3MarketSelectionResultFacts\s*\{|^\s*BoltV3MarketSelectionResultFacts\s*\{",
    re.MULTILINE,
)
DIRECT_PRE_SUBMIT_REJECTION_FACTS_PATTERN = re.compile(
    r"(?:=|,|\()\s*BoltV3PreSubmitRejectionFacts\s*\{|^\s*BoltV3PreSubmitRejectionFacts\s*\{",
    re.MULTILINE,
)
DECISION_EVENT_TIMESTAMP_LITERAL_PATTERN = re.compile(
    r"(?:const\s+TEST_[A-Z0-9_]*TS_NANOS\s*:\s*u64\s*=\s*\d[\d_]*|UnixNanos::from\(\s*\d[\d_]*\s*\))"
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
PROVIDER_SOURCE_LABEL_FORBIDDEN_LITERAL_VALUES = {
    "polymarket_gamma_market_anchor",
}
MARKET_SELECTION_FACT_KEY_FORBIDDEN_LITERAL_VALUES = {
    "blocked_after_seconds",
    "cadence_seconds",
    "down_instrument_id",
    "market_selection_outcome",
    "market_selection_failure_reason",
    "market_selection_rule",
    "market_selection_timestamp_milliseconds",
    "market_selection_type",
    "polymarket_condition_id",
    "polymarket_market_end_timestamp_milliseconds",
    "polymarket_market_slug",
    "polymarket_market_start_timestamp_milliseconds",
    "polymarket_question_id",
    "price_to_beat_observed_timestamp",
    "price_to_beat_source",
    "price_to_beat_value",
    "retry_interval_seconds",
    "rotating_market_family",
    "selected_market_observed_timestamp",
    "underlying_asset",
    "up_instrument_id",
}
ENTRY_EVALUATION_FACT_KEY_FORBIDDEN_LITERAL_VALUES = {
    "archetype_metrics",
    "entry_decision",
    "entry_filled_notional",
    "entry_no_action_reason",
    "has_selected_market_open_orders",
    "open_entry_notional",
    "seconds_to_market_end",
    "strategy_remaining_entry_capacity",
    "updown_market_mechanical_outcome",
    "updown_market_mechanical_rejection_reason",
    "updown_side",
}
ORDER_FACT_KEY_FORBIDDEN_LITERAL_VALUES = {
    "client_order_id",
    "instrument_id",
    "is_post_only",
    "is_quote_quantity",
    "is_reduce_only",
    "order_type",
    "price",
    "quantity",
    "side",
    "time_in_force",
}
PRE_SUBMIT_REJECTION_REASON_FACT_KEY_FORBIDDEN_LITERAL_VALUES = {
    "entry_pre_submit_rejection_reason",
    "exit_pre_submit_rejection_reason",
}
EXIT_EVALUATION_FACT_KEY_FORBIDDEN_LITERAL_VALUES = {
    "authoritative_position_quantity",
    "authoritative_sellable_quantity",
    "exit_decision",
    "exit_decision_reason",
    "exit_order_mechanical_outcome",
    "exit_order_mechanical_rejection_reason",
    "open_exit_order_quantity",
    "uncovered_position_quantity",
}
DECISION_EVENT_VALUE_FORBIDDEN_LITERAL_VALUES = {
    "accepted",
    "active_exit_not_defined",
    "buy",
    "current",
    "enter",
    "entry_evaluation",
    "entry_order_submission",
    "entry_pre_submit_rejection",
    "ev_hysteresis",
    "exit",
    "exit_evaluation",
    "exit_order_submission",
    "exit_pre_submit_rejection",
    "fail_closed",
    "failed",
    "fok",
    "forced_flat",
    "gtc",
    "hold",
    "ioc",
    "limit",
    "market",
    "market_selection_result",
    "market_ended",
    "market_not_started",
    "next",
    "no_action",
    "open_exit_order_quantity_covers_position",
    "rejected",
    "sell",
    "up",
    "down",
}
DECISION_EVENT_HANDOFF_DECISION_VALUE_FORBIDDEN_LITERAL_VALUES = (
    DECISION_EVENT_VALUE_FORBIDDEN_LITERAL_VALUES | {"some_new_reason"}
)

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


def decision_reason_values(root: Path) -> set[str]:
    path = root / DECISION_EVENT_SOURCE_FILE
    if not path.is_file():
        return set()
    text = path.read_text(encoding="utf-8")
    values: set[str] = set()
    for match in REASON_CONST_PATTERN.finditer(text):
        if match.group("name").endswith("_FACT_KEY"):
            continue
        for literal_match in STRING_LITERAL_PATTERN.finditer(match.group("body")):
            values.add(literal_match.group("literal"))
    return values


def scan_file(root: Path, path: Path, reason_values: set[str]) -> list[Finding]:
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

    if rel == DECISION_EVENT_HANDOFF_TEST_FILE:
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

        for match in RUST_STRING_LITERAL_PATTERN.finditer(text):
            value = string_value(match.group(0))
            if value not in DECISION_EVENT_HANDOFF_DECISION_VALUE_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "decision-event decision value literal; use exported "
                        "event contract constants or fixture data"
                    ),
                    excerpt=match.group(0),
                )
            )

    if rel in {DECISION_EVENT_HANDOFF_TEST_FILE, ETH_CHAINLINK_RUNTIME_TEST_FILE}:
        for match in STRING_LITERAL_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in reason_values:
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

    if rel in {DECISION_EVENT_HANDOFF_TEST_FILE, ETH_CHAINLINK_RUNTIME_TEST_FILE}:
        for match in RUST_STRING_LITERAL_PATTERN.finditer(text):
            value = string_value(match.group(0))
            if value not in PROVIDER_SOURCE_LABEL_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline provider source label; use provider-owned source "
                        "label constant"
                    ),
                    excerpt=match.group(0),
                )
            )

    if rel == ETH_CHAINLINK_RUNTIME_TEST_FILE:
        for match in EVENT_FACT_GET_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in MARKET_SELECTION_FACT_KEY_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline market-selection fact key; use exported event "
                        "contract constant"
                    ),
                    excerpt=literal,
                )
            )

        for match in EVENT_FACT_GET_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in ENTRY_EVALUATION_FACT_KEY_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline entry-evaluation fact key; use exported event "
                        "contract constant"
                    ),
                    excerpt=literal,
                )
            )

        for match in EVENT_FACT_GET_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in ORDER_FACT_KEY_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline order fact key; use exported event contract constant"
                    ),
                    excerpt=literal,
                )
            )

        for match in EVENT_FACT_GET_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in PRE_SUBMIT_REJECTION_REASON_FACT_KEY_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline pre-submit rejection reason fact key; use "
                        "exported event contract constant"
                    ),
                    excerpt=literal,
                )
            )

        for match in EVENT_FACT_GET_PATTERN.finditer(text):
            literal = match.group("literal")
            if literal not in EXIT_EVALUATION_FACT_KEY_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline exit-evaluation fact key; use exported event "
                        "contract constant"
                    ),
                    excerpt=literal,
                )
            )

        for match in RUST_STRING_LITERAL_PATTERN.finditer(text):
            value = string_value(match.group(0))
            if value not in DECISION_EVENT_VALUE_FORBIDDEN_LITERAL_VALUES:
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "inline decision-event value; use exported event "
                        "contract constant"
                    ),
                    excerpt=match.group(0),
                )
            )

    if rel == DECISION_EVENT_HANDOFF_TEST_FILE:
        for match in DIRECT_ENTRY_EVALUATION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct entry-evaluation fact fixture construction; "
                        "load entry-evaluation fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
                )
            )
        for match in DIRECT_EXIT_EVALUATION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct exit-evaluation fact fixture construction; "
                        "load exit-evaluation fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
                )
            )
        for match in DIRECT_MARKET_SELECTION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct market-selection fact fixture construction; "
                        "load market-selection fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
                )
            )
        for match in DIRECT_PRE_SUBMIT_REJECTION_FACTS_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "direct pre-submit rejection fact fixture construction; "
                        "load pre-submit rejection fact fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
                )
            )
        for match in DECISION_EVENT_TIMESTAMP_LITERAL_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "decision-event timestamp literal; load event timestamps "
                        "from fixture data outside Rust test code"
                    ),
                    excerpt=match.group(0).strip(),
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

    return findings


def scan_root(root: Path) -> list[Finding]:
    reason_values = decision_reason_values(root)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, reason_values))
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
