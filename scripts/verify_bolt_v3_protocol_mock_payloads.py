#!/usr/bin/env python3
"""Reject inline Bolt-v3 protocol mock payloads in selected Rust tests."""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_polymarket_fee_provider.rs",
)
RAW_STRING_PATTERN = re.compile(
    r'(?<![A-Za-z0-9_])r(?P<hashes>#*)"(?P<body>.*?)"(?P=hashes)',
    re.DOTALL,
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


def first_nonblank_line(text: str) -> str:
    for line in text.splitlines():
        stripped = line.strip()
        if stripped:
            return stripped
    return ""


def looks_like_json_object(text: str) -> bool:
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return False
    return isinstance(value, dict)


def scan_file(root: Path, path: Path) -> list[Finding]:
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

    print("OK: Bolt-v3 protocol mock payload verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
