#!/usr/bin/env python3
"""Reject repeated existing-strategy runtime fixture literals in Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
ETH_CHAINLINK_RUNTIME_TEST_FILE = "tests/eth_chainlink_taker_runtime.rs"
ETH_CHAINLINK_RUNTIME_TEST_NODE_FIXTURE = (
    "tests/fixtures/eth_chainlink_taker_runtime/test_node.toml"
)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
STRATEGY_CONFIG_SPAN_PATTERN = re.compile(
    r"fn\s+strategy_raw_config\s*\([^)]*\)\s*->\s*Value\s*\{.*?^\}",
    re.MULTILINE | re.DOTALL,
)
REFERENCE_TOPIC_HELPER_SPAN_PATTERN = re.compile(
    r"fn\s+fixture_reference_publish_topic\s*\([^)]*\)\s*->\s*&'static\s+str\s*\{.*?^\}",
    re.MULTILINE | re.DOTALL,
)
UP_INSTRUMENT_HELPER_SPAN_PATTERN = re.compile(
    r"fn\s+eth_up_instrument_id\s*\([^)]*\)\s*->\s*InstrumentId\s*\{.*?^\}",
    re.MULTILINE | re.DOTALL,
)
DOWN_INSTRUMENT_HELPER_SPAN_PATTERN = re.compile(
    r"fn\s+eth_down_instrument_id\s*\([^)]*\)\s*->\s*InstrumentId\s*\{.*?^\}",
    re.MULTILINE | re.DOTALL,
)
FORBIDDEN_LITERALS = {
    "eth_chainlink_taker": (
        "existing-strategy runtime archetype literal; use ETH_CHAINLINK_TAKER_KIND"
    ),
    "ETHCHAINLINKTAKER-RT-001": "existing-strategy runtime strategy id literal; derive from strategy_raw_config",
    "platform.reference.test.chainlink": (
        "existing-strategy runtime reference topic literal; use fixture_reference_publish_topic"
    ),
    "condition-eth-MKT-ETH-1-UP.POLYMARKET": (
        "existing-strategy runtime instrument id literal; use eth_up_instrument_id"
    ),
    "condition-eth-MKT-ETH-1-DOWN.POLYMARKET": (
        "existing-strategy runtime instrument id literal; use eth_down_instrument_id"
    ),
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


def span_for(pattern: re.Pattern[str], text: str) -> tuple[int, int] | None:
    match = pattern.search(text)
    if match is None:
        return None
    return (match.start(), match.end())


def within(span: tuple[int, int] | None, offset: int) -> bool:
    return span is not None and span[0] <= offset < span[1]


def add_string(value: Any, literals: set[str]) -> None:
    if isinstance(value, str) and value:
        literals.add(value)


def existing_strategy_test_node_literals(root: Path) -> set[str]:
    path = root / ETH_CHAINLINK_RUNTIME_TEST_NODE_FIXTURE
    if not path.is_file():
        return set()
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    literals: set[str] = set()

    node = data.get("node", {})
    if isinstance(node, dict):
        add_string(node.get("name"), literals)
        add_string(node.get("trader_id"), literals)

    for client in data.get("data_clients", []):
        if not isinstance(client, dict):
            continue
        add_string(client.get("name"), literals)
        config = client.get("config", {})
        if not isinstance(config, dict):
            continue
        add_string(config.get("venue"), literals)
        event_slugs = config.get("event_slugs", [])
        if isinstance(event_slugs, list):
            for event_slug in event_slugs:
                add_string(event_slug, literals)

    for client in data.get("exec_clients", []):
        if not isinstance(client, dict):
            continue
        add_string(client.get("name"), literals)
        config = client.get("config", {})
        if not isinstance(config, dict):
            continue
        add_string(config.get("account_id"), literals)
        add_string(config.get("venue"), literals)

    return literals


def scan_runtime_file(root: Path, path: Path, test_node_literals: set[str]) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    strategy_config_span = span_for(STRATEGY_CONFIG_SPAN_PATTERN, text)
    reference_topic_helper_span = span_for(REFERENCE_TOPIC_HELPER_SPAN_PATTERN, text)
    up_instrument_helper_span = span_for(UP_INSTRUMENT_HELPER_SPAN_PATTERN, text)
    down_instrument_helper_span = span_for(DOWN_INSTRUMENT_HELPER_SPAN_PATTERN, text)
    findings: list[Finding] = []

    for match in STRING_PATTERN.finditer(text):
        value = string_value(match.group(0))
        if value in test_node_literals:
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "existing-strategy runtime test-node fixture literal; "
                        "derive from test_node.toml"
                    ),
                    excerpt=match.group(0),
                )
            )
            continue
        if value not in FORBIDDEN_LITERALS:
            continue
        if value == "ETHCHAINLINKTAKER-RT-001" and within(
            strategy_config_span, match.start()
        ):
            continue
        if value == "platform.reference.test.chainlink" and within(
            reference_topic_helper_span, match.start()
        ):
            continue
        if value == "condition-eth-MKT-ETH-1-UP.POLYMARKET" and within(
            up_instrument_helper_span, match.start()
        ):
            continue
        if value == "condition-eth-MKT-ETH-1-DOWN.POLYMARKET" and within(
            down_instrument_helper_span, match.start()
        ):
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message=FORBIDDEN_LITERALS[value],
                excerpt=match.group(0),
            )
        )

    return findings


def scan_root(root: Path) -> list[Finding]:
    path = root / ETH_CHAINLINK_RUNTIME_TEST_FILE
    if not path.is_file():
        return []
    return scan_runtime_file(root, path, existing_strategy_test_node_literals(root))


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 existing-strategy runtime literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
