#!/usr/bin/env python3
"""Reject inline Bolt-v3 protocol payloads and local fixture literals."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_polymarket_fee_provider.rs",
    "tests/bolt_v3_order_lifecycle_tracer.rs",
    "tests/bolt_v3_reconciliation_restart.rs",
)
PROTOCOL_FIXTURE_PATHS = (
    "tests/fixtures/bolt_v3_existing_strategy/order_lifecycle_tracer.toml",
    "tests/fixtures/bolt_v3_existing_strategy/polymarket_fee_provider.toml",
)
NUMERIC_FIXTURE_KEYS = frozenset(
    (
        "price_to_beat",
        "liquidity_num",
        "reference_orderbook_half_spread",
    )
)
RAW_STRING_PATTERN = re.compile(
    r'(?<![A-Za-z0-9_])r(?P<hashes>#*)"(?P<body>.*?)"(?P=hashes)',
    re.DOTALL,
)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
NUMBER_PATTERN = re.compile(r"(?<![A-Za-z0-9_.])\d[\d_]*(?:\.\d[\d_]*)?(?![A-Za-z0-9_.])")
ORDER_LIFECYCLE_PRICE_PRECISION_PATTERN = re.compile(
    r"Price::new\(\s*[^,\n]+,\s*\d+\s*\)"
)
ORDER_LIFECYCLE_DURATION_LITERAL_PATTERN = re.compile(
    r"Duration::from_(?:secs|millis)\(\s*\d[\d_]*\s*\)"
)
ORDER_LIFECYCLE_DURATION_MARGIN_LITERAL_PATTERN = re.compile(
    r"Duration::from_(?:secs|millis)\(\s*[^)]*?\+\s*\d[\d_]*\s*,?\s*\)",
    re.DOTALL,
)
ORDER_LIFECYCLE_FEE_REQUEST_COUNT_PATTERN = re.compile(
    r"spawn_fee_rate_server\(\s*\d[\d_]*\s*\)"
)
ORDER_LIFECYCLE_POLL_ATTEMPT_PATTERN = re.compile(r"for\s+_\s+in\s+0\.\.\d[\d_]*\s*\{")
ORDER_LIFECYCLE_TIMESTAMP_OFFSET_PATTERN = re.compile(r"\bstart_ts_ms\s*\+\s*\d[\d_]*")


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


def first_nonblank_line(text: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped
    return ""


def looks_like_json_object(text: str) -> bool:
    candidates = (
        text,
        text.replace("{{", "{").replace("}}", "}"),
    )
    for candidate in candidates:
        try:
            value = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            return True
    return False


def string_value(literal: str) -> str:
    return bytes(literal[1:-1], "utf-8").decode("unicode_escape")


def normalized_number(value: str) -> str:
    return value.replace("_", "")


def collect_fixture_values(
    key: str | None,
    value: Any,
    strings: set[str],
    numbers: set[str],
) -> None:
    if isinstance(value, str) and value:
        strings.add(value)
        return
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if key in NUMERIC_FIXTURE_KEYS:
            numbers.add(normalized_number(str(value)))
        return
    if isinstance(value, dict):
        for child_key, child in value.items():
            collect_fixture_values(str(child_key), child, strings, numbers)
        return
    if isinstance(value, list):
        for child in value:
            collect_fixture_values(key, child, strings, numbers)


def protocol_fixture_literals(root: Path) -> tuple[set[str], set[str]]:
    strings: set[str] = set()
    numbers: set[str] = set()
    for relative_path in PROTOCOL_FIXTURE_PATHS:
        path = root / relative_path
        if path.is_file():
            collect_fixture_values(
                None,
                tomllib.loads(path.read_text(encoding="utf-8")),
                strings,
                numbers,
            )
    return strings, numbers


def scan_file(
    root: Path,
    path: Path,
    fixture_literals: set[str],
    numeric_fixture_literals: set[str],
) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in RAW_STRING_PATTERN.finditer(text):
        body = match.group("body")
        if not looks_like_json_object(body):
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="inline protocol mock payload; move payload to tests/fixtures and load it",
                excerpt=first_nonblank_line(body),
            )
        )
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        if string_value(literal) not in fixture_literals:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="fixture-owned literal; derive from TOML fixture",
                excerpt=literal,
            )
        )
    for match in NUMBER_PATTERN.finditer(text):
        number = normalized_number(match.group(0))
        if number not in numeric_fixture_literals:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="numeric fixture-owned literal; derive from TOML fixture",
                excerpt=match.group(0),
            )
        )
    if rel == "tests/bolt_v3_order_lifecycle_tracer.rs":
        for match in ORDER_LIFECYCLE_PRICE_PRECISION_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message=(
                        "order-lifecycle price precision literal; derive from "
                        "selected binary option price increment"
                    ),
                    excerpt=match.group(0),
                )
            )
        for match in ORDER_LIFECYCLE_DURATION_LITERAL_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="order-lifecycle duration literal; derive from TOML fixture",
                    excerpt=match.group(0),
                )
            )
        for match in ORDER_LIFECYCLE_DURATION_MARGIN_LITERAL_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="order-lifecycle duration margin literal; derive from TOML fixture",
                    excerpt=first_nonblank_line(match.group(0)),
                )
            )
        for match in ORDER_LIFECYCLE_FEE_REQUEST_COUNT_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="order-lifecycle fee request count literal; derive from TOML fixture",
                    excerpt=match.group(0),
                )
            )
        for match in ORDER_LIFECYCLE_POLL_ATTEMPT_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="order-lifecycle poll attempt literal; derive from TOML fixture",
                    excerpt=match.group(0),
                )
            )
        for match in ORDER_LIFECYCLE_TIMESTAMP_OFFSET_PATTERN.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="order-lifecycle timestamp offset literal; derive from TOML fixture",
                    excerpt=match.group(0),
                )
            )
    return findings


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    fixture_literals, numeric_fixture_literals = protocol_fixture_literals(root)
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, fixture_literals, numeric_fixture_literals))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 protocol mock payload verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
