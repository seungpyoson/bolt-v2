#!/usr/bin/env python3
"""Reject hardcoded Bolt-v3 instrument fixture literals in Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
INSTRUMENT_FIXTURE = (
    "tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml"
)
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_instrument_gate.rs",
    "tests/bolt_v3_instrument_readiness.rs",
)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')


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


def instrument_fixture_literals(root: Path) -> set[str]:
    path = root / INSTRUMENT_FIXTURE
    if not path.is_file():
        return set()

    data = tomllib.loads(path.read_text(encoding="utf-8"))
    values: set[str] = set()
    for market in data.get("markets", []):
        if not isinstance(market, dict):
            continue
        add_market_values(values, market)
    return values


def add_market_values(values: set[str], market: dict[str, Any]) -> None:
    for key in ("name", "condition_id", "question_id", "market_slug"):
        value = market.get(key)
        if isinstance(value, str) and value:
            values.add(value)

    for leg in market.get("legs", []):
        if not isinstance(leg, dict):
            continue
        for key in ("token_id", "instrument_id"):
            value = leg.get(key)
            if isinstance(value, str) and value:
                values.add(value)


def scan_file(root: Path, path: Path, fixture_literals: set[str]) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        if string_value(literal) not in fixture_literals:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="instrument fixture literal; derive from selected-market fixture instead",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    fixture_literals = instrument_fixture_literals(root)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, fixture_literals))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 instrument fixture-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
