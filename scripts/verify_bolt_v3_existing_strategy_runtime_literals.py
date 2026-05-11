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
EXISTING_STRATEGY_ROOT_FIXTURE = "tests/fixtures/bolt_v3_existing_strategy/root.toml"
UPDOWN_SELECTED_MARKETS_FIXTURE = (
    "tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml"
)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
STRATEGY_CONFIG_SPAN_PATTERN = re.compile(
    r"fn\s+strategy_raw_config\s*\([^)]*\)\s*->\s*Value\s*\{.*?^\}",
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
MARKET_SELECTION_CONTEXT_SPAN_PATTERN = re.compile(
    r"BoltV3MarketSelectionContext\s*\{.*?^\s*\}",
    re.MULTILINE | re.DOTALL,
)
MARKET_SELECTION_CONTEXT_LITERAL_FIELD_PATTERN = re.compile(
    r"\b(?:market_selection_type|rotating_market_family|underlying_asset|cadence_seconds|market_selection_rule|retry_interval_seconds|blocked_after_seconds)\s*:\s*(?:\"[^\"]*\"|Some\s*\(\s*\"[^\"]*\"|Some\s*\(\s*[0-9])",
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
    "PRIMARY": (
        "existing-strategy runtime selection ruleset literal; derive from v3 strategy TOML"
    ),
    "freeze window": (
        "existing-strategy runtime selection freeze reason literal; use "
        "SELECTION_FREEZE_WINDOW_REASON"
    ),
    "polymarket_gamma_market_anchor": (
        "existing-strategy runtime price-to-beat source literal; use "
        "POLYMARKET_GAMMA_MARKET_ANCHOR_SOURCE"
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


def selected_market_fixture_literals(root: Path) -> set[str]:
    path = root / UPDOWN_SELECTED_MARKETS_FIXTURE
    if not path.is_file():
        return set()
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    literals: set[str] = set()
    markets = data.get("markets", [])
    if not isinstance(markets, list):
        return literals

    for market in markets:
        if not isinstance(market, dict):
            continue
        for key in ("condition_id", "question_id", "market_slug"):
            add_string(market.get(key), literals)
        legs = market.get("legs", [])
        if not isinstance(legs, list):
            continue
        for leg in legs:
            if not isinstance(leg, dict):
                continue
            add_string(leg.get("token_id"), literals)
            add_string(leg.get("instrument_id"), literals)

    return literals


def existing_strategy_reference_stream_literals(root: Path) -> set[str]:
    path = root / EXISTING_STRATEGY_ROOT_FIXTURE
    if not path.is_file():
        return set()
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    literals: set[str] = set()
    streams = data.get("reference_streams", {})
    if not isinstance(streams, dict):
        return literals

    for stream in streams.values():
        if not isinstance(stream, dict):
            continue
        add_string(stream.get("publish_topic"), literals)
        inputs = stream.get("inputs", [])
        if not isinstance(inputs, list):
            continue
        for input_block in inputs:
            if not isinstance(input_block, dict):
                continue
            add_string(input_block.get("source_id"), literals)
            add_string(input_block.get("instrument_id"), literals)

    return literals


def existing_strategy_resolution_basis_literals(root: Path) -> set[str]:
    path = root / EXISTING_STRATEGY_ROOT_FIXTURE
    if not path.is_file():
        return set()
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    clients = data.get("clients", {})
    streams = data.get("reference_streams", {})
    if not isinstance(clients, dict) or not isinstance(streams, dict):
        return set()

    literals: set[str] = set()
    for stream in streams.values():
        if not isinstance(stream, dict):
            continue
        inputs = stream.get("inputs", [])
        if not isinstance(inputs, list):
            continue
        for input_block in inputs:
            if not isinstance(input_block, dict):
                continue
            if input_block.get("source_type") != "oracle":
                continue
            client_id = input_block.get("data_client_id")
            instrument_id = input_block.get("instrument_id")
            if not isinstance(client_id, str) or not isinstance(instrument_id, str):
                continue
            client = clients.get(client_id)
            if not isinstance(client, dict):
                continue
            venue = client.get("venue")
            if not isinstance(venue, str) or "." not in instrument_id:
                continue
            symbol = instrument_id.split(".", maxsplit=1)[0]
            literals.add(f"{venue.lower()}_{symbol.lower()}")

    return literals


def scan_runtime_file(
    root: Path,
    path: Path,
    test_node_literals: set[str],
    selected_market_literals: set[str],
    reference_stream_literals: set[str],
    resolution_basis_literals: set[str],
) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    strategy_config_span = span_for(STRATEGY_CONFIG_SPAN_PATTERN, text)
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
        if value in selected_market_literals:
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "existing-strategy runtime selected-market fixture literal; "
                        "derive from updown_selected_markets.toml"
                    ),
                    excerpt=match.group(0),
                )
            )
            continue
        if value in reference_stream_literals:
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "existing-strategy runtime reference stream fixture literal; "
                        "derive from v3 root TOML"
                    ),
                    excerpt=match.group(0),
                )
            )
            continue
        if value in resolution_basis_literals:
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "existing-strategy runtime resolution-basis fixture literal; "
                        "derive from v3 root TOML"
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

    for context_match in MARKET_SELECTION_CONTEXT_SPAN_PATTERN.finditer(text):
        context_block = context_match.group(0)
        for field_match in MARKET_SELECTION_CONTEXT_LITERAL_FIELD_PATTERN.finditer(
            context_block
        ):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, context_match.start() + field_match.start()),
                    message=(
                        "existing-strategy runtime market-selection context literal; "
                        "derive from v3 strategy TOML"
                    ),
                    excerpt=field_match.group(0),
                )
            )

    return findings


def scan_root(root: Path) -> list[Finding]:
    path = root / ETH_CHAINLINK_RUNTIME_TEST_FILE
    if not path.is_file():
        return []
    return scan_runtime_file(
        root,
        path,
        existing_strategy_test_node_literals(root),
        selected_market_fixture_literals(root),
        existing_strategy_reference_stream_literals(root),
        existing_strategy_resolution_basis_literals(root),
    )


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
