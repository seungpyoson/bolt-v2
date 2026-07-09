#!/usr/bin/env python3
"""Verify CI prose-reference residuals are explicitly classified."""

from __future__ import annotations

import ast
import io
import sys
import tokenize
import tomllib
from pathlib import Path
from typing import Any

from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
LEDGER_PATH = Path("ci/doc-decoupling-residuals.toml")
VERIFY_SCRIPT_GLOB = "verify_*.py"
EXTRA_SCRIPT_PATHS = ("scripts/governance_diff_analysis.py",)
RUST_TEST_GLOB = "*.rs"
MARKDOWN_EXTENSION = chr(46) + chr(109) + chr(100)
DOCS_WIDE_GLOB = "/".join(("docs", "**", "*"))
PROSE_DIRECTORY_PREFIXES = ("/".join(("docs", "")), "/".join(("specs", "")))
VALID_KINDS = {
    "workflow_snippet_false_positive",
    "docstring_pointer_false_positive",
    "deliberate_guard",
    "doc_sync_exception",
}
VALID_READ_PURPOSES = {
    "none",
    "rename_guard",
    "doc_sync_exception",
}
KIND_READ_PURPOSE = {
    "workflow_snippet_false_positive": "none",
    "docstring_pointer_false_positive": "none",
    "deliberate_guard": "rename_guard",
    "doc_sync_exception": "doc_sync_exception",
}


def non_empty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def non_empty_string_list(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(non_empty_string(item) for item in value)


def string_literal_values(line: str) -> list[str]:
    values: list[str] = []
    try:
        tokens = tokenize.generate_tokens(io.StringIO(line).readline)
        for token in tokens:
            if token.type != tokenize.STRING:
                continue
            try:
                value = ast.literal_eval(token.string)
            except (SyntaxError, ValueError):
                continue
            if isinstance(value, str):
                values.append(value)
    except tokenize.TokenError:
        return values
    return values


def prose_reference_count(text: str) -> int:
    raw_count = text.count(MARKDOWN_EXTENSION) + text.count(DOCS_WIDE_GLOB)
    literal_values = string_literal_values(text)
    literal_text = "".join(literal_values)
    directory_count = sum(1 for value in literal_values if prose_directory_literal(value))
    if not literal_text:
        return raw_count + directory_count
    split_literal_count = (
        0
        if raw_count
        else literal_text.count(MARKDOWN_EXTENSION) + literal_text.count(DOCS_WIDE_GLOB)
    )
    return raw_count + split_literal_count + directory_count


def prose_directory_literal(value: str) -> bool:
    normalized = value.replace("\\", "/").strip()
    if not normalized.startswith(PROSE_DIRECTORY_PREFIXES):
        return False
    if normalized.startswith(PROSE_DIRECTORY_PREFIXES[1]) and "/reference" in normalized:
        return False
    if normalized.startswith(PROSE_DIRECTORY_PREFIXES[0]) and "/research" in normalized:
        return False
    if any(char in normalized for char in "*?[]"):
        return False
    suffix = Path(normalized).suffix
    return suffix == ""


def load_ledger(root: Path, findings: list[str]) -> list[dict[str, Any]]:
    path = root / LEDGER_PATH
    if not path.is_file():
        findings.append(f"{LEDGER_PATH}: missing doc-decoupling residual ledger")
        return []
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        findings.append(f"{LEDGER_PATH}: invalid TOML: {exc}")
        return []

    table = data.get("doc_decoupling_residuals")
    if not isinstance(table, dict):
        findings.append(f"{LEDGER_PATH}: missing [doc_decoupling_residuals]")
        return []
    if table.get("version") != 1:
        findings.append(f"{LEDGER_PATH}: doc_decoupling_residuals.version must be 1")
    entries = table.get("allowed_markdown_references")
    if not isinstance(entries, list) or not entries:
        findings.append(f"{LEDGER_PATH}: allowed_markdown_references must be a non-empty table array")
        return []
    return [entry for entry in entries if isinstance(entry, dict)]


def validate_entry(entry: dict[str, Any], index: int, findings: list[str]) -> None:
    prefix = f"{LEDGER_PATH}: allowed_markdown_references[{index}]"
    path = entry.get("path")
    if not non_empty_string(path):
        findings.append(f"{prefix}.path must be non-empty")
    else:
        path_text = str(path)
        if not allowed_residual_path(path_text):
            findings.append(f"{prefix}.path must target scripts/verify_*.py or tests/*.rs")

    kind = entry.get("kind")
    if kind not in VALID_KINDS:
        findings.append(f"{prefix}.kind must be one of {sorted(VALID_KINDS)}")

    read_purpose = entry.get("read_purpose")
    if read_purpose not in VALID_READ_PURPOSES:
        findings.append(f"{prefix}.read_purpose must be one of {sorted(VALID_READ_PURPOSES)}")

    snippets = entry.get("snippets")
    if not non_empty_string_list(snippets):
        findings.append(f"{prefix}.snippets must be a non-empty string list")
    else:
        for snippet in snippets:
            if prose_reference_count(str(snippet)) != 1:
                findings.append(f"{prefix}.snippets entries must contain exactly one prose reference")

    tracking_issue = entry.get("tracking_issue")
    if not non_empty_string(tracking_issue) or not str(tracking_issue).startswith("#"):
        findings.append(f"{prefix}.tracking_issue must be an issue reference")

    if kind == "doc_sync_exception" and entry.get("owner_issue") != "#559":
        findings.append(f"{prefix}: doc_sync_exception must declare owner_issue #559")
    if kind == "deliberate_guard" and entry.get("owner_issue") != "#711":
        findings.append(f"{prefix}: deliberate_guard must declare owner_issue #711")
    expected_read_purpose = KIND_READ_PURPOSE.get(str(kind))
    if expected_read_purpose is not None and read_purpose != expected_read_purpose:
        findings.append(f"{prefix}: {kind} must declare read_purpose {expected_read_purpose}")


def allowed_residual_path(path_text: str) -> bool:
    return path_text in EXTRA_SCRIPT_PATHS or (
        path_text.startswith("scripts/verify_")
        and path_text.endswith(".py")
        and "/" not in path_text.removeprefix("scripts/")
    ) or (
        path_text.startswith("tests/")
        and path_text.endswith(".rs")
        and "/" not in path_text.removeprefix("tests/")
    )


def scanned_source_paths(root: Path) -> list[Path]:
    paths: list[Path] = []
    scripts = root / "scripts"
    if scripts.is_dir():
        paths.extend(scripts.glob(VERIFY_SCRIPT_GLOB))
    for relative_path in EXTRA_SCRIPT_PATHS:
        path = root / relative_path
        if path.is_file():
            paths.append(path)
    tests = root / "tests"
    if tests.is_dir():
        paths.extend(tests.glob(RUST_TEST_GLOB))
    return sorted(paths)


def prose_reference_lines(root: Path, paths: list[Path] | None = None) -> list[tuple[str, int, str]]:
    lines: list[tuple[str, int, str]] = []
    for path in scanned_source_paths(root) if paths is None else paths:
        rel = path.relative_to(root).as_posix()
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        for line_number, line in enumerate(text.splitlines(), 1):
            if prose_reference_count(line):
                lines.append((rel, line_number, line.strip()))
    return lines


def collect_findings(root: Path = REPO_ROOT) -> list[str]:
    findings: list[str] = []
    source_paths = scanned_source_paths(root)
    if not require_nonempty(source_paths, "doc-decoupling scanned source paths", findings):
        return findings

    entries = load_ledger(root, findings)
    for index, entry in enumerate(entries):
        validate_entry(entry, index, findings)

    allowed: dict[str, list[str]] = {}
    for entry in entries:
        path = entry.get("path")
        snippets = entry.get("snippets")
        if non_empty_string(path) and non_empty_string_list(snippets):
            allowed.setdefault(str(path), []).extend(str(snippet).strip() for snippet in snippets)

    for rel, snippets in sorted(allowed.items()):
        path = root / rel
        if not path.is_file():
            findings.append(f"{LEDGER_PATH}: residual ledger path is missing: {rel}")
            continue
        text = path.read_text(encoding="utf-8")
        source_lines = {line.strip() for line in text.splitlines()}
        for snippet in snippets:
            if snippet not in source_lines:
                findings.append(f"{LEDGER_PATH}: stale residual ledger snippet for {rel}: {snippet!r}")

    for rel, line_number, line in prose_reference_lines(root, source_paths):
        snippets = allowed.get(rel, [])
        if line not in snippets:
            findings.append(f"{rel}:{line_number}: unclassified prose reference: {line}")

    return findings


def main() -> int:
    findings = collect_findings()
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1
    print("OK: doc-decoupling residual ledger passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
