#!/usr/bin/env python3
"""Verify CI path-filter docs and docs-only pass-stub wiring."""

from __future__ import annotations

import argparse
import fnmatch
import pathlib
import re
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
DEFAULT_PASS_STUB_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci-docs-pass-stub.yml"
DEFAULT_DOCS = REPO_ROOT / "docs" / "ci" / "paths-ignore-behavior.md"
EXPECTED_SAFE_PATHS = (
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "REASONIX.md",
    "LICENSE",
    ".github/ISSUE_TEMPLATE/**",
    ".codex/**",
    ".gemini/**",
    ".opencode/**",
    ".pi/**",
    ".specify/**",
)
REQUIRED_DOC_SCENARIOS = (
    "docs-only root agent doc",
    "workflow change",
    "Rust source change",
    "managed rust-verification config",
    "lockfile change",
    "mixed docs and source",
    "ignored config dir",
)


class PathFilterError(RuntimeError):
    """Raised when CI path-filter evidence is missing or unsafe."""


def strip_comment(line: str) -> str:
    quote: str | None = None
    for index, char in enumerate(line):
        if quote is not None:
            if char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
            continue
        if char == "#":
            return line[:index].rstrip()
    return line.rstrip()


def unquote(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def extract_trigger_list(workflow_text: str, key: str) -> list[str]:
    lines = workflow_text.splitlines()
    for index, line in enumerate(lines):
        if strip_comment(line).strip() != f"{key}:":
            continue
        paths: list[str] = []
        for nested in lines[index + 1 :]:
            clean = strip_comment(nested)
            if not clean.strip():
                continue
            if re.match(r"^\s{6}-\s+", clean):
                paths.append(unquote(clean.split("-", 1)[1].strip()))
                continue
            if len(clean) - len(clean.lstrip(" ")) <= 4:
                break
        return paths
    raise PathFilterError(f"workflow missing pull_request {key}")


def extract_ci_paths_ignore(workflow_text: str) -> list[str]:
    try:
        return extract_trigger_list(workflow_text, "paths-ignore")
    except PathFilterError as exc:
        raise PathFilterError("ci workflow missing pull_request paths-ignore") from exc


def path_matches_pattern(path: str, pattern: str) -> bool:
    normalized = path.strip()
    if normalized.startswith("./"):
        normalized = normalized[2:]
    if not normalized:
        return False
    if pattern.endswith("/**"):
        prefix = pattern[:-3]
        return normalized == prefix.rstrip("/") or normalized.startswith(prefix)
    return fnmatch.fnmatchcase(normalized, pattern)


def docs_only_safe(changed_files: tuple[str, ...] | list[str], safe_paths: tuple[str, ...] | list[str]) -> bool:
    if not changed_files:
        raise PathFilterError("changed file list is empty")
    for path in changed_files:
        if not path.strip():
            raise PathFilterError("changed file list contains an empty path")
        if not any(path_matches_pattern(path, pattern) for pattern in safe_paths):
            return False
    return True


def read_changed_files(path: pathlib.Path) -> tuple[str, ...]:
    if not path.exists():
        raise PathFilterError(f"changed-files path does not exist: {path}")
    files = tuple(line.strip() for line in path.read_text(encoding="utf-8").splitlines() if line.strip())
    if not files:
        raise PathFilterError("changed file list is empty")
    return files


def write_github_output(output_path: pathlib.Path, docs_only: bool) -> None:
    with output_path.open("a", encoding="utf-8") as handle:
        handle.write(f"docs_only={'true' if docs_only else 'false'}\n")


def classify_changed_file_path(
    changed_files_path: pathlib.Path,
    output_path: pathlib.Path | None = None,
    *,
    require_docs_only: bool = False,
    verbose: bool = True,
) -> bool:
    safe_paths = extract_ci_paths_ignore(DEFAULT_CI_WORKFLOW.read_text(encoding="utf-8"))
    verify_safe_path_contract(safe_paths)
    changed_files = read_changed_files(changed_files_path)
    docs_only = docs_only_safe(changed_files, safe_paths)
    if output_path is not None:
        write_github_output(output_path, docs_only)
    if require_docs_only and not docs_only:
        raise PathFilterError("changed files are not docs-only ignored-safe")
    if verbose:
        print(f"docs_only={'true' if docs_only else 'false'}")
        for path in changed_files:
            print(f"changed={path}")
    return docs_only


def verify_safe_path_contract(paths: list[str]) -> None:
    if tuple(paths) != EXPECTED_SAFE_PATHS:
        raise PathFilterError(f"ci paths-ignore drift: expected {EXPECTED_SAFE_PATHS}, got {tuple(paths)}")


def verify_pass_stub_workflow(workflow_text: str) -> None:
    text = "\n".join(strip_comment(line) for line in workflow_text.splitlines())
    paths = extract_trigger_list(workflow_text, "paths")
    if tuple(paths) != EXPECTED_SAFE_PATHS:
        raise PathFilterError(f"pass-stub paths drift: expected {EXPECTED_SAFE_PATHS}, got {tuple(paths)}")
    if "needs:" in text:
        raise PathFilterError("pass-stub gate job must fail directly without dependent skipped jobs")
    if re.search(r"^\s+if:\s+", text, flags=re.MULTILINE):
        raise PathFilterError("pass-stub gate job must not use job-level if")
    required_literals = (
        "name: CI docs pass stub",
        "pull_request:",
        "gate:",
        "name: gate",
        "python3 scripts/verify_ci_path_filters.py",
        "$GITHUB_OUTPUT",
        "--require-docs-only",
    )
    for literal in required_literals:
        if literal not in text:
            if literal == "name: gate":
                raise PathFilterError("pass-stub gate job must be named gate")
            if literal == "python3 scripts/verify_ci_path_filters.py":
                raise PathFilterError("pass-stub must run changed-file classifier")
            raise PathFilterError(f"pass-stub workflow missing {literal}")


def verify_docs_table(docs_text: str) -> None:
    for scenario in REQUIRED_DOC_SCENARIOS:
        if scenario not in docs_text:
            raise PathFilterError(f"docs missing required scenario {scenario}")


def verify_repository(
    *,
    ci_workflow: pathlib.Path = DEFAULT_CI_WORKFLOW,
    pass_stub_workflow: pathlib.Path = DEFAULT_PASS_STUB_WORKFLOW,
    docs: pathlib.Path = DEFAULT_DOCS,
) -> list[str]:
    errors: list[str] = []
    try:
        paths = extract_ci_paths_ignore(ci_workflow.read_text(encoding="utf-8"))
        verify_safe_path_contract(paths)
    except Exception as exc:  # noqa: BLE001 - collect verifier failures.
        errors.append(str(exc))
    try:
        verify_pass_stub_workflow(pass_stub_workflow.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        errors.append(str(exc))
    try:
        verify_docs_table(docs.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        errors.append(str(exc))
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--changed-files", type=pathlib.Path)
    parser.add_argument("--github-output", type=pathlib.Path)
    parser.add_argument("--require-docs-only", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.changed_files is not None:
            classify_changed_file_path(args.changed_files, args.github_output, require_docs_only=args.require_docs_only)
            return 0
        errors = verify_repository()
        if errors:
            for error in errors:
                print(f"ERROR: {error}", file=sys.stderr)
            return 1
        print("OK: CI path-filter verifier passed.")
        return 0
    except PathFilterError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
