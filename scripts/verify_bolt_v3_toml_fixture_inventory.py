#!/usr/bin/env python3
"""Verify every Bolt-v3 TOML fixture is listed in the fixture inventory."""

from __future__ import annotations

import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
INVENTORY_PATH = REPO_ROOT / "tests/fixtures/bolt_v3_fixture_inventory.toml"
FIXTURE_GLOBS = (
    "tests/fixtures/bolt_v3/**/*.toml",
    "tests/fixtures/bolt_v3_existing_strategy/**/*.toml",
    "tests/fixtures/bolt_v3_reference_policy/**/*.toml",
    "tests/fixtures/bolt_v3_reference_producer/**/*.toml",
)


@dataclass(frozen=True)
class Finding:
    message: str

    def render(self) -> str:
        return f"FAIL: {self.message}"


def discovered_fixture_paths(root: Path) -> set[str]:
    paths: set[str] = set()
    for pattern in FIXTURE_GLOBS:
        for path in root.glob(pattern):
            if path.is_file():
                paths.add(path.relative_to(root).as_posix())
    return paths


def inventory_fixture_paths(root: Path) -> tuple[set[str], list[Finding]]:
    inventory_path = root / INVENTORY_PATH.relative_to(REPO_ROOT)
    if not inventory_path.is_file():
        return set(), [Finding(f"missing Bolt-v3 TOML fixture inventory: {inventory_path}")]

    try:
        data = tomllib.loads(inventory_path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as error:
        return set(), [Finding(f"invalid Bolt-v3 TOML fixture inventory: {error}")]

    fixtures = data.get("fixtures")
    if not isinstance(fixtures, list):
        return set(), [Finding("inventory must contain [[fixtures]] entries")]

    paths: set[str] = set()
    findings: list[Finding] = []
    for index, entry in enumerate(fixtures, start=1):
        if not isinstance(entry, dict):
            findings.append(Finding(f"inventory fixture #{index} must be a table"))
            continue
        path = entry.get("path")
        purpose = entry.get("purpose")
        if not isinstance(path, str) or not path:
            findings.append(Finding(f"inventory fixture #{index} must define nonempty path"))
            continue
        if path in paths:
            findings.append(Finding(f"duplicate inventory fixture path: {path}"))
        paths.add(path)
        if not isinstance(purpose, str) or not purpose.strip():
            findings.append(Finding(f"inventory fixture {path} must define nonempty purpose"))

    return paths, findings


def scan_root(root: Path) -> list[Finding]:
    discovered = discovered_fixture_paths(root)
    inventoried, findings = inventory_fixture_paths(root)
    for path in sorted(discovered - inventoried):
        findings.append(Finding(f"unlisted Bolt-v3 TOML fixture: {path}"))
    for path in sorted(inventoried - discovered):
        findings.append(Finding(f"stale Bolt-v3 TOML fixture inventory path: {path}"))
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(finding.render(), file=sys.stderr)
        return 1

    print("OK: Bolt-v3 TOML fixture inventory verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
