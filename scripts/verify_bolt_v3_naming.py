#!/usr/bin/env python3
"""Verify Bolt-v3 NT-owned names and capital-admission rename fence."""

from __future__ import annotations

import fnmatch
import re
import sys
from pathlib import Path

from verifier_io import require_nonempty

try:
    import yaml
except ImportError:
    sys.stderr.write(
        "ERROR: PyYAML is required. Install with `python3 -m pip install pyyaml`.\n"
    )
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent
AUDIT_PATH = REPO_ROOT / "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml"
MISNOMER_ALLOWLIST_PATH = (
    REPO_ROOT / "specs/711-capital-admission-rename/misnomer-allowlist.txt"
)
SCAN_GLOBS = [
    "docs/bolt-v3/*.md",
    "docs/bolt-v3/research/**/*.toml",
    "docs/bolt-v3/research/**/*.yaml",
    "docs/bolt-v3/research/runtime-capture/*.yaml",
    "src/**/*.rs",
    "tests/**/*.rs",
    "scripts/*.py",
    "*.toml",
    "config/**/*.toml",
    "contracts/**/*.toml",
    "tests/**/*.toml",
    "tests/fixtures/**/*.toml",
]
MISNOMER_SCAN_GLOBS = [
    "src/**/*.rs",
    "crates/**/*.rs",
    "tests/**/*.rs",
    "tests/fixtures/**/*",
    "config/**/*",
    "scripts/*.py",
    "docs/**/*",
    "specs/**/*.md",
]
MISNOMER_TEXT_SUFFIXES = {
    ".json",
    ".jsonl",
    ".md",
    ".py",
    ".rs",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
EXCLUDED_RELATIVE_PATHS = {
    "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml",
}
MISNOMER_EXCLUDED_RELATIVE_PATHS = {
    "scripts/verify_bolt_v3_naming.py",
    "scripts/test_verify_bolt_v3_naming.py",
}
MISNOMER_PATTERNS = [
    (
        "position_sizer stem",
        re.compile(
            r"(?<![A-Za-z0-9])(?:position[-_]?siz[A-Za-z0-9_]*|position\s+sizing[A-Za-z0-9_]*)",
            re.IGNORECASE,
        ),
    ),
    ("sizing_policy", re.compile(r"sizing[_]?policy", re.IGNORECASE)),
    ("sizing_state", re.compile(r"sizing[_]?state", re.IGNORECASE)),
    ("sized_quantity", re.compile(r"sized[_]?quantity[A-Za-z0-9_]*", re.IGNORECASE)),
    ("sized_admission", re.compile(r"sized[_]?admission[A-Za-z0-9_]*", re.IGNORECASE)),
    ("nt_sizing_state", re.compile(r"\bnt_sizing_state\b", re.IGNORECASE)),
    ("nt_position_sizer", re.compile(r"\bnt_position_sizer\b", re.IGNORECASE)),
    (
        "gate sizing evidence type",
        re.compile(
            r"(?:compiled[_]?order[_]?sizing[_]?evidence|missing[_]?sizing[_]?evidence|sizing[_]?rejected)",
            re.IGNORECASE,
        ),
    ),
]
LEGITIMATE_SIZER_PATHS = {
    "src/bolt_v3_sizing.rs",
}
LEGITIMATE_SIZER_LINE_PATTERNS = [
    re.compile(r"\bchoose_robust_size\b"),
    re.compile(r"\bRobustSize\w*\b"),
    re.compile(r"\bSUPPORTED_STRATEGY_SCHEMA_VERSION\b"),
]


def word_re(term: str) -> re.Pattern[str]:
    prefix = r"(?<![A-Za-z0-9_])" if term[:1].isalnum() or term[:1] == "_" else ""
    suffix = r"(?![A-Za-z0-9_])" if term[-1:].isalnum() or term[-1:] == "_" else ""
    return re.compile(f"{prefix}{re.escape(term)}{suffix}")


def load_audit() -> object:
    return yaml.safe_load(AUDIT_PATH.read_text(encoding="utf-8")) or {}


def scan_paths_for_globs(
    patterns: list[str],
    excluded_relative_paths: set[str],
    allowed_suffixes: set[str] | None = None,
) -> list[Path]:
    paths: set[Path] = set()
    for pattern in patterns:
        paths.update(REPO_ROOT.glob(pattern))
    return sorted(
        path
        for path in paths
        if path.is_file()
        and (
            allowed_suffixes is None
            or path.suffix in allowed_suffixes
            or path.name in {"justfile", "Justfile"}
        )
        and str(path.relative_to(REPO_ROOT)) not in excluded_relative_paths
        and ".git" not in path.parts
        and "target" not in path.parts
        and not fnmatch.fnmatch(str(path.relative_to(REPO_ROOT)), "reviews/**")
    )


def scan_paths() -> list[Path]:
    return scan_paths_for_globs(SCAN_GLOBS, EXCLUDED_RELATIVE_PATHS)


def scan_misnomer_paths() -> list[Path]:
    allowlist_rel = str(MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT))
    return scan_paths_for_globs(
        MISNOMER_SCAN_GLOBS,
        EXCLUDED_RELATIVE_PATHS | MISNOMER_EXCLUDED_RELATIVE_PATHS | {allowlist_rel},
        MISNOMER_TEXT_SUFFIXES,
    )


def matches_any(path: Path, patterns: list[str]) -> bool:
    rel = str(path.relative_to(REPO_ROOT))
    return any(glob_pattern_re(pattern).match(rel) for pattern in patterns)


def glob_pattern_re(pattern: str) -> re.Pattern[str]:
    pieces: list[str] = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            pieces.append("(?:.*/)?")
            i += 3
            continue
        if pattern.startswith("**", i):
            pieces.append(".*")
            i += 2
            continue
        char = pattern[i]
        if char == "*":
            pieces.append("[^/]*")
        elif char == "?":
            pieces.append("[^/]")
        else:
            pieces.append(re.escape(char))
        i += 1
    return re.compile(f"^{''.join(pieces)}$")


def load_misnomer_allowlist() -> tuple[dict[tuple[str, int], tuple[str, str]], list[str]]:
    if not MISNOMER_ALLOWLIST_PATH.exists():
        return {}, [f"missing capital-admission misnomer allowlist: {MISNOMER_ALLOWLIST_PATH}"]

    entries: dict[tuple[str, int], tuple[str, str]] = {}
    errors: list[str] = []
    for row_number, row in enumerate(
        MISNOMER_ALLOWLIST_PATH.read_text(encoding="utf-8").splitlines(),
        start=1,
    ):
        if not row.strip() or row.lstrip().startswith("#"):
            continue
        parts = row.split("\t", 2)
        if len(parts) != 3:
            errors.append(
                f"{MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT)}:{row_number}: "
                "expected path:line<TAB>exact stripped line<TAB>justification"
            )
            continue
        location, exact_line, reason = parts
        rel, separator, line_number_text = location.rpartition(":")
        if (
            not separator
            or not rel
            or not line_number_text.isdigit()
            or int(line_number_text) <= 0
        ):
            errors.append(
                f"{MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT)}:{row_number}: "
                f"invalid allowlist location {location!r}"
            )
            continue
        if Path(rel).is_absolute() or ".." in Path(rel).parts:
            errors.append(
                f"{MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT)}:{row_number}: "
                f"allowlist path must be repo-relative: {rel!r}"
            )
            continue
        if not exact_line.strip() or not reason.strip():
            errors.append(
                f"{MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT)}:{row_number}: "
                "allowlist exact line and justification must both be non-empty"
            )
            continue
        key = (rel, int(line_number_text))
        if key in entries:
            errors.append(
                f"{MISNOMER_ALLOWLIST_PATH.relative_to(REPO_ROOT)}:{row_number}: "
                f"duplicate allowlist location {location}"
            )
            continue
        entries[key] = (exact_line, reason)
    return entries, errors


def line_is_legitimate_sizer(rel: str, line: str) -> bool:
    return rel in LEGITIMATE_SIZER_PATHS or any(
        pattern.search(line) for pattern in LEGITIMATE_SIZER_LINE_PATTERNS
    )


def misnomer_labels(line: str) -> list[str]:
    return [label for label, pattern in MISNOMER_PATTERNS if pattern.search(line)]


def verify_capital_admission_misnomers(paths: list[Path] | None = None) -> list[str]:
    findings: list[str] = []
    paths = scan_misnomer_paths() if paths is None else paths
    if not require_nonempty(paths, "capital-admission misnomer scan paths", findings):
        return findings

    allowlist, allowlist_findings = load_misnomer_allowlist()
    if allowlist_findings:
        return allowlist_findings

    used_allowlist: set[tuple[str, int]] = set()
    for path in paths:
        rel = path.relative_to(REPO_ROOT).as_posix()
        for line_number, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(),
            start=1,
        ):
            labels = misnomer_labels(line)
            if not labels or line_is_legitimate_sizer(rel, line):
                continue
            stripped_line = line.strip()
            key = (rel, line_number)
            allowlist_entry = allowlist.get(key)
            if allowlist_entry is not None:
                expected_line, _reason = allowlist_entry
                if expected_line == stripped_line:
                    used_allowlist.add(key)
                    continue
                findings.append(
                    f"{rel}:{line_number}: allowlist text mismatch for "
                    f"capital-admission misnomer; expected {expected_line!r}, "
                    f"found {stripped_line!r}"
                )
                continue
            findings.append(
                f"{rel}:{line_number}: capital-admission misnomer "
                f"({', '.join(labels)}): {stripped_line}"
            )

    for (rel, line_number), (_line, _reason) in sorted(allowlist.items()):
        if (rel, line_number) not in used_allowlist:
            findings.append(f"{rel}:{line_number}: stale capital-admission misnomer allowlist entry")

    return findings


def main() -> int:
    try:
        audit = load_audit()
    except FileNotFoundError:
        print(f"FAIL: missing Bolt-v3 naming audit file: {AUDIT_PATH}", file=sys.stderr)
        return 1
    except UnicodeDecodeError:
        print(f"FAIL: invalid Bolt-v3 naming audit file: {AUDIT_PATH} is not valid UTF-8", file=sys.stderr)
        return 1
    except OSError:
        print(f"FAIL: unreadable Bolt-v3 naming audit file: {AUDIT_PATH}", file=sys.stderr)
        return 1
    except yaml.YAMLError as error:
        print(f"FAIL: invalid Bolt-v3 naming audit file: {error}", file=sys.stderr)
        return 1
    if not isinstance(audit, dict):
        print(
            "FAIL: invalid Bolt-v3 naming audit file: "
            f"expected a mapping, got {type(audit).__name__}",
            file=sys.stderr,
        )
        return 1
    rename_rows = audit.get("renamed_in_current_audit", [])
    defensive_rows = audit.get("defensive_forbidden", [])
    scoped_rows = audit.get("path_scoped_forbidden", [])
    forbidden = {
        row["from"]: f"use {row['to']}"
        for row in [*rename_rows, *defensive_rows]
        if row.get("from") and row.get("to")
    }

    findings: list[str] = []
    require_nonempty(
        tuple(row for row in [*rename_rows, *defensive_rows, *scoped_rows] if row),
        "Bolt-v3 naming audit rule rows",
        findings,
    )
    paths = scan_paths()
    require_nonempty(paths, "Bolt-v3 naming scan paths", findings)
    misnomer_paths = scan_misnomer_paths()
    require_nonempty(
        misnomer_paths,
        "capital-admission misnomer scan paths",
        findings,
    )
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    for path in paths:
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

    findings.extend(verify_capital_admission_misnomers(misnomer_paths))

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 canonical naming audit passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
