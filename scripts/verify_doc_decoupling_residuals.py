#!/usr/bin/env python3
"""Verify prose-reading verifier residuals are explicitly classified."""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parent.parent
LEDGER_PATH = Path("ci/doc-decoupling-residuals.toml")
VERIFY_SCRIPT_GLOB = "verify_*.py"
MARKDOWN_EXTENSION = "." + "md"
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


def non_empty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def non_empty_string_list(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(non_empty_string(item) for item in value)


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
        if not path_text.startswith("scripts/verify_") or not path_text.endswith(".py"):
            findings.append(f"{prefix}.path must target scripts/verify_*.py")

    kind = entry.get("kind")
    if kind not in VALID_KINDS:
        findings.append(f"{prefix}.kind must be one of {sorted(VALID_KINDS)}")

    read_purpose = entry.get("read_purpose")
    if read_purpose not in VALID_READ_PURPOSES:
        findings.append(f"{prefix}.read_purpose must be one of {sorted(VALID_READ_PURPOSES)}")

    snippets = entry.get("snippets")
    if not non_empty_string_list(snippets):
        findings.append(f"{prefix}.snippets must be a non-empty string list")

    tracking_issue = entry.get("tracking_issue")
    if not non_empty_string(tracking_issue) or not str(tracking_issue).startswith("#"):
        findings.append(f"{prefix}.tracking_issue must be an issue reference")

    if kind == "doc_sync_exception" and entry.get("owner_issue") != "#559":
        findings.append(f"{prefix}: doc_sync_exception must declare owner_issue #559")
    if kind == "deliberate_guard" and entry.get("owner_issue") != "#711":
        findings.append(f"{prefix}: deliberate_guard must declare owner_issue #711")
    if kind in {"workflow_snippet_false_positive", "docstring_pointer_false_positive"} and read_purpose != "none":
        findings.append(f"{prefix}: false positives must declare read_purpose none")


def verify_script_paths(root: Path) -> list[Path]:
    scripts = root / "scripts"
    if not scripts.is_dir():
        return []
    return sorted(scripts.glob(VERIFY_SCRIPT_GLOB))


def markdown_reference_lines(root: Path) -> list[tuple[str, int, str]]:
    lines: list[tuple[str, int, str]] = []
    for path in verify_script_paths(root):
        rel = path.relative_to(root).as_posix()
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        for line_number, line in enumerate(text.splitlines(), 1):
            if MARKDOWN_EXTENSION in line:
                lines.append((rel, line_number, line.strip()))
    return lines


def collect_findings(root: Path = REPO_ROOT) -> list[str]:
    findings: list[str] = []
    entries = load_ledger(root, findings)
    for index, entry in enumerate(entries):
        validate_entry(entry, index, findings)

    allowed: dict[str, list[str]] = {}
    for entry in entries:
        path = entry.get("path")
        snippets = entry.get("snippets")
        if non_empty_string(path) and non_empty_string_list(snippets):
            allowed.setdefault(str(path), []).extend(str(snippet) for snippet in snippets)

    for rel, snippets in sorted(allowed.items()):
        path = root / rel
        if not path.is_file():
            findings.append(f"{LEDGER_PATH}: residual ledger path is missing: {rel}")
            continue
        text = path.read_text(encoding="utf-8")
        for snippet in snippets:
            if snippet not in text:
                findings.append(f"{LEDGER_PATH}: stale residual ledger snippet for {rel}: {snippet!r}")

    for rel, line_number, line in markdown_reference_lines(root):
        snippets = allowed.get(rel, [])
        if not any(snippet in line for snippet in snippets):
            findings.append(f"{rel}:{line_number}: unclassified markdown reference: {line}")

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
