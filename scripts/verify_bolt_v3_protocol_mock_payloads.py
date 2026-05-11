#!/usr/bin/env python3
"""Reject inline Bolt-v3 protocol mock payloads in selected Rust tests."""

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
RAW_STRING_PATTERN = re.compile(
    r'(?<![A-Za-z0-9_])r(?P<hashes>#*)"(?P<body>.*?)"(?P=hashes)',
    re.DOTALL,
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


def collect_strings(value: Any, strings: set[str]) -> None:
    if isinstance(value, str) and value:
        strings.add(value)
        return
    if isinstance(value, dict):
        for child in value.values():
            collect_strings(child, strings)
        return
    if isinstance(value, list):
        for child in value:
            collect_strings(child, strings)


def protocol_fixture_literals(root: Path) -> set[str]:
    strings: set[str] = set()
    for relative_path in PROTOCOL_FIXTURE_PATHS:
        path = root / relative_path
        if path.is_file():
            collect_strings(tomllib.loads(path.read_text(encoding="utf-8")), strings)
    return strings


def scan_file(root: Path, path: Path, fixture_literals: set[str]) -> list[Finding]:
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
                message="protocol fixture literal; derive from TOML fixture",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    fixture_literals = protocol_fixture_literals(root)
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

    print("OK: Bolt-v3 protocol mock payload verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
