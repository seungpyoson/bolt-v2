#!/usr/bin/env python3
"""Reject hardcoded Bolt-v3 scale/process scenario literals in Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
SCENARIO_FIXTURE = (
    "tests/fixtures/bolt_v3_existing_strategy/"
    "scale_process_selection_topic_isolation.toml"
)
ENFORCED_TEST_FILES = ("tests/bolt_v3_scale_process.rs",)
STRING_KEYS = (
    "condition_suffix",
    "up_token_suffix",
    "down_token_suffix",
    "question_suffix",
)
NUMERIC_OR_BOOL_KEYS = (
    "event_settle_milliseconds",
    "delay_post_stop_seconds",
    "timeout_disconnection_seconds",
    "price_to_beat",
    "accepting_orders",
    "liquidity_num",
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


def scenario_fixture(root: Path) -> dict[str, Any]:
    path = root / SCENARIO_FIXTURE
    if not path.is_file():
        return {}
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    return data if isinstance(data, dict) else {}


def fixture_string_values(scenario: dict[str, Any]) -> set[str]:
    values: set[str] = set()
    for key in STRING_KEYS:
        value = scenario.get(key)
        if isinstance(value, str) and value:
            values.add(value)
    return values


def fixture_numeric_or_bool_patterns(scenario: dict[str, Any]) -> list[re.Pattern[str]]:
    patterns: list[re.Pattern[str]] = []
    for key in NUMERIC_OR_BOOL_KEYS:
        value = scenario.get(key)
        if isinstance(value, bool):
            literal = str(value).lower()
        elif isinstance(value, int | float):
            literal = str(value)
        else:
            continue
        patterns.append(
            re.compile(rf"\b{re.escape(key)}\b[^\n;]*\b{re.escape(literal)}\b")
        )
    return patterns


def scan_file(
    root: Path,
    path: Path,
    string_values: set[str],
    numeric_or_bool_patterns: list[re.Pattern[str]],
) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        if string_value(literal) not in string_values:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="scale-process scenario fixture literal; derive from loaded TOML fixture instead",
                excerpt=literal,
            )
        )
    for pattern in numeric_or_bool_patterns:
        for match in pattern.finditer(text):
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="scale-process scenario fixture literal; derive from loaded TOML fixture instead",
                    excerpt=match.group(0).strip(),
                )
            )
    return findings


def scan_root(root: Path) -> list[Finding]:
    scenario = scenario_fixture(root)
    string_values = fixture_string_values(scenario)
    numeric_or_bool_patterns = fixture_numeric_or_bool_patterns(scenario)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(
                scan_file(root, path, string_values, numeric_or_bool_patterns)
            )
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 scale-process fixture-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
