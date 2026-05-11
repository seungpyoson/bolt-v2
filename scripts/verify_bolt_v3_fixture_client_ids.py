#!/usr/bin/env python3
"""Reject hardcoded Bolt-v3 fixture client IDs in selected Rust tests."""

from __future__ import annotations

import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURE_ROOT_GLOB = "tests/fixtures/bolt_v3*/root*.toml"
ENFORCED_TEST_FILES = (
    "tests/bolt_v3_client_registration.rs",
    "tests/bolt_v3_adapter_mapping.rs",
    "tests/bolt_v3_provider_binding.rs",
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


def fixture_client_ids(root: Path) -> set[str]:
    client_ids: set[str] = set()
    for path in sorted(root.glob(FIXTURE_ROOT_GLOB)):
        if not path.is_file():
            continue
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        clients = data.get("clients", {})
        if isinstance(clients, dict):
            client_ids.update(str(client_id) for client_id in clients)
    return client_ids


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def string_value(literal: str) -> str:
    return bytes(literal[1:-1], "utf-8").decode("unicode_escape")


def scan_file(root: Path, path: Path, client_ids: set[str]) -> list[Finding]:
    text = path.read_text(encoding="utf-8")
    rel = path.relative_to(root).as_posix()
    findings: list[Finding] = []
    for match in STRING_PATTERN.finditer(text):
        literal = match.group(0)
        if string_value(literal) not in client_ids:
            continue
        findings.append(
            Finding(
                path=rel,
                line=line_number(text, match.start()),
                message="fixture client-id literal; derive from loaded TOML fixture instead",
                excerpt=literal,
            )
        )
    return findings


def scan_root(root: Path) -> list[Finding]:
    client_ids = fixture_client_ids(root)
    findings: list[Finding] = []
    for relative_path in ENFORCED_TEST_FILES:
        path = root / relative_path
        if path.is_file():
            findings.extend(scan_file(root, path, client_ids))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 fixture client-id verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
