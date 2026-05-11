#!/usr/bin/env python3
"""Reject hardcoded Bolt-v3 fixture strategy/target IDs in Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
STRATEGY_FIXTURE_GLOB = "tests/fixtures/bolt_v3_existing_strategy/strategies/*.toml"
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


def fixture_strategy_target_literals(root: Path) -> set[str]:
    values: set[str] = set()
    for path in sorted(root.glob(STRATEGY_FIXTURE_GLOB)):
        if not path.is_file():
            continue
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        add_strategy_values(values, data)
    return values


def add_strategy_values(values: set[str], data: dict[str, Any]) -> None:
    strategy_id = data.get("strategy_instance_id")
    if isinstance(strategy_id, str) and strategy_id:
        values.add(strategy_id)
    target = data.get("target")
    if isinstance(target, dict):
        configured_target_id = target.get("configured_target_id")
        if isinstance(configured_target_id, str) and configured_target_id:
            values.add(configured_target_id)


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
                message="fixture strategy/target literal; derive from loaded TOML fixture instead",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    fixture_literals = fixture_strategy_target_literals(root)
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

    print("OK: Bolt-v3 fixture strategy/target verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
