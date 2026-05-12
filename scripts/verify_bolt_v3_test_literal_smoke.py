#!/usr/bin/env python3
"""Reject broad hardcoded runtime-like literals in Bolt-v3 Rust tests."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ENFORCED_PATTERNS = (
    "tests/bolt_v3*.rs",
    "tests/eth_chainlink_taker_runtime.rs",
)
SMOKE_PATTERNS = (
    re.compile(r"Duration::from_millis\([0-9]"),
    re.compile(r"Duration::from_secs\([0-9]"),
    re.compile(r"UnixNanos::from\(\s*[0-9]"),
    re.compile(r"saturating_mul\(1_000_000\)"),
    re.compile(r"Some\([0-9]+\.[0-9]+\)"),
    re.compile(r"price:\s*[0-9]+\.[0-9]+"),
    re.compile(r'"updown"'),
    re.compile(r'"POLYMARKET_PK"'),
)
MARKET_IDENTITY_GUARD = "tests/bolt_v3_market_identity.rs"


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


def line_text(text: str, offset: int) -> str:
    start = text.rfind("\n", 0, offset) + 1
    end = text.find("\n", offset)
    if end == -1:
        end = len(text)
    return text[start:end]


def is_allowed_market_identity_guard(rel: str, text: str, offset: int) -> bool:
    if rel != MARKET_IDENTITY_GUARD:
        return False
    if line_text(text, offset).strip() != '"updown",':
        return False
    guard_name = "core_market_identity_module_must_be_market_family_neutral"
    guard_start = text.rfind(f"fn {guard_name}", 0, offset)
    if guard_start == -1:
        return False
    next_test = text.find("\n#[test]", guard_start + 1)
    return next_test == -1 or offset < next_test


def scan_file(root: Path, path: Path) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for pattern in SMOKE_PATTERNS:
        for match in pattern.finditer(text):
            if is_allowed_market_identity_guard(rel, text, match.start()):
                continue
            findings.append(
                Finding(
                    path=rel,
                    line=line_number(text, match.start()),
                    message="literal matched smoke pattern; derive from fixture/config or owning contract",
                    excerpt=line_text(text, match.start()).strip(),
                )
            )
    return findings


def enforced_files(root: Path) -> list[Path]:
    files: set[Path] = set()
    for pattern in ENFORCED_PATTERNS:
        files.update(root.glob(pattern))
    return sorted(path for path in files if path.is_file())


def scan_root(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for path in enforced_files(root):
        findings.extend(scan_file(root, path))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 test-literal smoke verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
