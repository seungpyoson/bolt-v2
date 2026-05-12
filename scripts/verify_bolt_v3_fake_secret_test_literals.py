#!/usr/bin/env python3
"""Reject hardcoded fake resolver secret values in Bolt-v3 tests."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SUPPORT_FILE = "tests/support/mod.rs"
ENFORCED_TEST_FILES = ("tests/bolt_v3_readiness.rs",)
STRING_PATTERN = re.compile(r'"(?:\\.|[^"\\])*"')
CONST_SECRET_PATTERN = re.compile(
    r"const\s+FAKE_BOLT_V3_[A-Z0-9_]+:\s*&str\s*=\s*(\"(?:\\.|[^\"\\])*\")\s*;"
)
INLINE_OK_SECRET_PATTERN = re.compile(r"Ok\(\s*(\"(?:\\.|[^\"\\])*\")\.to_string\(\)\s*\)")


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


def fake_secret_values(root: Path) -> set[str]:
    path = root / SUPPORT_FILE
    if not path.is_file():
        return set()
    text = path.read_text(encoding="utf-8")
    values: set[str] = set()
    for pattern in (CONST_SECRET_PATTERN, INLINE_OK_SECRET_PATTERN):
        for match in pattern.finditer(text):
            values.add(string_value(match.group(1)))
    return values


def scan_file(root: Path, path: Path, secrets: set[str]) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        if string_value(literal) not in secrets:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="fake-secret literal; derive through fake_bolt_v3_resolver",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    secrets = fake_secret_values(root)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, secrets))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 fake-secret test-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
