#!/usr/bin/env python3
"""Verify CI path-filter docs and docs-only policy wiring."""

from __future__ import annotations

import argparse
import dataclasses
import fnmatch
import pathlib
import re
import sys

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import ci_provenance  # noqa: E402

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
DEFAULT_RUST_POLICY = REPO_ROOT / "ci" / "rust-verification.toml"
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
LEGACY_RUST_POLICY = REPO_ROOT / ".claude" / "rust-verification.toml"
MAX_TEXT_BYTES = 1_000_000


class PathFilterError(RuntimeError):
    """Raised when CI path-filter evidence is missing or unsafe."""


@dataclasses.dataclass(frozen=True)
class DocsPathRegistry:
    safe_paths: tuple[str, ...]
    forbidden_ignored_build_paths: tuple[str, ...]


def load_docs_path_registry(config_path: pathlib.Path = DEFAULT_CONFIG) -> DocsPathRegistry:
    try:
        data = ci_provenance.load_toml(config_path)
        ci_table = ci_provenance.require_table(data, "ci_provenance", "config")
        docs_table = ci_provenance.require_table(ci_table, "docs", "ci_provenance")
        safe_paths = ci_provenance.require_string_list(
            docs_table,
            "safe_paths",
            "ci_provenance.docs",
        )
        forbidden_paths = ci_provenance.require_string_list(
            docs_table,
            "forbidden_ignored_build_paths",
            "ci_provenance.docs",
        )
        errors = ci_provenance.docs_safe_path_contract_errors(safe_paths)
        if errors:
            raise ci_provenance.ProvenanceError("; ".join(errors))
    except ci_provenance.ProvenanceError as exc:
        raise PathFilterError(str(exc)) from exc
    return DocsPathRegistry(
        safe_paths=safe_paths,
        forbidden_ignored_build_paths=forbidden_paths,
    )


def read_text_bounded(path: pathlib.Path, label: str, limit: int = MAX_TEXT_BYTES) -> str:
    if limit <= 0:
        raise PathFilterError(f"{label} size limit must be positive")
    if not path.exists():
        raise PathFilterError(f"{label} path does not exist: {path}")
    if path.stat().st_size > limit:
        raise PathFilterError(f"{label} file exceeds size limit ({limit} bytes): {path}")
    return path.read_text(encoding="utf-8")


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
        return normalized == prefix.rstrip("/") or normalized.startswith(f"{prefix}/")
    return fnmatch.fnmatchcase(normalized, pattern)


def docs_only_safe(
    changed_files: tuple[str, ...] | list[str],
    safe_paths: tuple[str, ...] | list[str],
    forbidden_ignored_build_paths: tuple[str, ...] | list[str] = (),
) -> bool:
    if not changed_files:
        raise PathFilterError("changed file list is empty")
    forbidden_paths = set(forbidden_ignored_build_paths)
    for path in changed_files:
        normalized = path.strip()
        if normalized.startswith("./"):
            normalized = normalized[2:]
        if not normalized:
            raise PathFilterError("changed file list contains an empty path")
        if normalized in forbidden_paths:
            return False
        if not any(path_matches_pattern(normalized, pattern) for pattern in safe_paths):
            return False
    return True


def read_changed_files(path: pathlib.Path, limit: int = MAX_TEXT_BYTES) -> tuple[str, ...]:
    text = read_text_bounded(path, "changed-files", limit=limit)
    files = tuple(stripped for line in text.splitlines() if (stripped := line.strip()))
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
    config_path: pathlib.Path = DEFAULT_CONFIG,
    require_docs_only: bool = False,
    verbose: bool = True,
) -> bool:
    registry = load_docs_path_registry(config_path)
    changed_files = read_changed_files(changed_files_path)
    docs_only = docs_only_safe(
        changed_files,
        registry.safe_paths,
        registry.forbidden_ignored_build_paths,
    )
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
    errors = ci_provenance.docs_safe_path_contract_errors(tuple(paths))
    if errors:
        raise PathFilterError("; ".join(errors))


def verify_ci_workflow_has_no_pull_request_paths_ignore(workflow_text: str) -> None:
    try:
        paths = extract_ci_paths_ignore(workflow_text)
    except PathFilterError:
        return
    raise PathFilterError(f"ci workflow pull_request paths-ignore must be removed, got {tuple(paths)}")


def verify_rust_policy_location(
    policy_path: pathlib.Path = DEFAULT_RUST_POLICY,
    legacy_path: pathlib.Path = LEGACY_RUST_POLICY,
) -> None:
    if not policy_path.exists():
        raise PathFilterError(f"managed rust-verification config missing: {policy_path}")
    if legacy_path.exists():
        raise PathFilterError(f"legacy managed rust-verification config must not exist: {legacy_path}")


def verify_repository(
    *,
    ci_workflow: pathlib.Path = DEFAULT_CI_WORKFLOW,
    config: pathlib.Path = DEFAULT_CONFIG,
) -> list[str]:
    errors: list[str] = []
    try:
        registry = load_docs_path_registry(config)
        verify_safe_path_contract(list(registry.safe_paths))
    except Exception as exc:  # noqa: BLE001 - collect verifier failures.
        errors.append(str(exc))
    try:
        verify_ci_workflow_has_no_pull_request_paths_ignore(read_text_bounded(ci_workflow, "CI workflow"))
    except Exception as exc:  # noqa: BLE001
        errors.append(str(exc))
    try:
        verify_rust_policy_location()
    except Exception as exc:  # noqa: BLE001
        errors.append(str(exc))
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--changed-files", type=pathlib.Path)
    parser.add_argument("--github-output", type=pathlib.Path)
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    parser.add_argument("--require-docs-only", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.changed_files is not None:
            classify_changed_file_path(
                args.changed_files,
                args.github_output,
                config_path=args.config,
                require_docs_only=args.require_docs_only,
            )
            return 0
        errors = verify_repository(config=args.config)
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
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
