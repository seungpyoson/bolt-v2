#!/usr/bin/env python3
"""Reject hardcoded provider forbidden-env literals in Bolt-v3 tests."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_client_registration.rs",
    "tests/bolt_v3_readiness.rs",
)
FORBIDDEN_ENV_LITERAL_PATTERN = re.compile(r'"POLYMARKET_PK"')


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


def scan_file(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in FORBIDDEN_ENV_LITERAL_PATTERN.finditer(text):
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="forbidden-env literal; use provider-owned FORBIDDEN_ENV_VARS",
                excerpt=match.group(0),
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

    print("OK: Bolt-v3 forbidden-env test-literal verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
