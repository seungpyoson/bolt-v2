#!/usr/bin/env python3
"""Verify Bolt-v3 NT-owned naming rules from nt-owned-name-audit.yaml."""

from __future__ import annotations

import fnmatch
import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:
    sys.stderr.write(
        "ERROR: PyYAML is required. Install with `python3 -m pip install pyyaml`.\n"
    )
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent
AUDIT_PATH = REPO_ROOT / "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml"
CANONICAL_DOCS = [
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md",
    REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-contract-ledger.md",
]
SCAN_GLOBS = [
    "docs/bolt-v3/2026-04-25-bolt-v3-*.md",
    "docs/bolt-v3/research/runtime-capture/*.yaml",
    "src/**/*.rs",
    "tests/**/*.rs",
    "scripts/*.py",
    "*.toml",
    "config/**/*.toml",
    "configs/**/*.toml",
    "contracts/**/*.toml",
    "tests/**/*.toml",
]
EXCLUDED_RELATIVE_PATHS = {
    "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
}
WORD_RE_TEMPLATE = r"(?<![A-Za-z0-9_]){}(?![A-Za-z0-9_])"


def word_re(term: str) -> re.Pattern[str]:
    return re.compile(WORD_RE_TEMPLATE.format(re.escape(term)))


def load_audit() -> dict:
    return yaml.safe_load(AUDIT_PATH.read_text(encoding="utf-8")) or {}


def scan_paths() -> list[Path]:
    paths: set[Path] = set()
    for pattern in SCAN_GLOBS:
        paths.update(REPO_ROOT.glob(pattern))
    return sorted(
        path
        for path in paths
        if path.is_file()
        and str(path.relative_to(REPO_ROOT)) not in EXCLUDED_RELATIVE_PATHS
        and ".git" not in path.parts
        and "target" not in path.parts
        and not fnmatch.fnmatch(str(path.relative_to(REPO_ROOT)), "reviews/**")
    )


def matches_any(path: Path, patterns: list[str]) -> bool:
    rel = str(path.relative_to(REPO_ROOT))
    return any(fnmatch.fnmatch(rel, pattern) for pattern in patterns)


def main() -> int:
    audit = load_audit()
    rename_rows = audit.get("renamed_in_current_audit", [])
    defensive_rows = audit.get("defensive_forbidden", [])
    scoped_rows = audit.get("path_scoped_forbidden", [])
    forbidden = {
        row["from"]: f"use {row['to']}"
        for row in [*rename_rows, *defensive_rows]
        if row.get("from") and row.get("to")
    }
    required_names = [row["to"] for row in rename_rows if row.get("to")]

    findings: list[str] = []
    for path in scan_paths():
        text = path.read_text(encoding="utf-8")
        for forbidden_name, replacement in forbidden.items():
            if word_re(forbidden_name).search(text):
                findings.append(
                    f"{path.relative_to(REPO_ROOT)}: forbidden {forbidden_name!r}; "
                    f"{replacement}"
                )
        for row in scoped_rows:
            include = row.get("include_globs") or []
            if not include or not matches_any(path, include):
                continue
            forbidden_name = row.get("from")
            replacement = row.get("to")
            if forbidden_name and replacement and word_re(forbidden_name).search(text):
                findings.append(
                    f"{path.relative_to(REPO_ROOT)}: forbidden {forbidden_name!r}; "
                    f"use {replacement} ({row.get('reason', 'path-scoped rule')})"
                )

    combined_canonical = "\n".join(path.read_text(encoding="utf-8") for path in CANONICAL_DOCS)
    for required_name in required_names:
        if not word_re(required_name).search(combined_canonical):
            findings.append(f"required canonical name {required_name!r} is absent")

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 canonical naming audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
