#!/usr/bin/env python3
from __future__ import annotations

import argparse
import fnmatch
import hashlib
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Iterable, TextIO


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = Path("ci/rust-ci-inputs.toml")
GLOB_CHARS = frozenset("*?[")


def normalize_repo_pathspec(pathspec: str) -> str | None:
    if pathspec.startswith(":(top)"):
        pathspec = pathspec[len(":(top)") :]
    elif pathspec.startswith(":/"):
        pathspec = pathspec[len(":/") :]
    elif pathspec.startswith(":"):
        return None

    parts: list[str] = []
    for part in pathspec.split("/"):
        if part == "" or part == ".":
            continue
        if part == "..":
            if not parts:
                return None
            parts.pop()
            continue
        parts.append(part)
    return "/".join(parts) if parts else "."


def pathspec_may_cover(pathspec: str, path: str) -> bool:
    normalized_pathspec = normalize_repo_pathspec(pathspec)
    normalized_path = normalize_repo_pathspec(path)
    if normalized_pathspec is None or normalized_path is None:
        return False
    if normalized_pathspec == ".":
        return True
    if normalized_pathspec == normalized_path:
        return True
    if not any(char in normalized_pathspec for char in GLOB_CHARS):
        return normalized_path.startswith(normalized_pathspec + "/")
    return fnmatch.fnmatchcase(normalized_path, normalized_pathspec)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_config(path: Path) -> dict[str, object]:
    try:
        config = tomllib.loads(read_text(path))
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"{path} is invalid TOML: {exc}") from exc
    sets = config.get("sets")
    if not isinstance(sets, dict):
        raise SystemExit(f"{path} must define [sets.<name>] tables")
    return config


def string_list(value: object, *, label: str) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise SystemExit(f"{label} must be a list of non-empty strings")
    return value


def resolve_set(config: dict[str, object], name: str) -> list[str]:
    sets = config.get("sets")
    if not isinstance(sets, dict):
        raise SystemExit("config must define [sets.<name>] tables")
    seen: set[str] = set()
    expanded: list[str] = []

    def visit(set_name: str, stack: tuple[str, ...]) -> None:
        if set_name in stack:
            cycle = " -> ".join((*stack, set_name))
            raise SystemExit(f"input set cycle: {cycle}")
        table = sets.get(set_name)
        if not isinstance(table, dict):
            raise SystemExit(f"unknown input set: {set_name}")
        for parent in string_list(table.get("include_sets"), label=f"sets.{set_name}.include_sets"):
            visit(parent, (*stack, set_name))
        for path in string_list(table.get("paths"), label=f"sets.{set_name}.paths"):
            if path not in seen:
                seen.add(path)
                expanded.append(path)

    visit(name, ())
    return expanded


def run_git(repo: Path, args: list[str]) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True)


def tracked_files(repo: Path, pathspecs: list[str]) -> list[str]:
    if not pathspecs:
        return []
    output = subprocess.check_output(["git", "ls-files", "-z", "--", *pathspecs], cwd=repo)
    return sorted(path for path in output.decode("utf-8").split("\0") if path)


def exact_pathspec_has_tracked_match(pathspec: str, files: list[str]) -> bool:
    if pathspec in files:
        return True
    prefix = pathspec.rstrip("/") + "/"
    return any(path.startswith(prefix) for path in files)


def absent_exact_pathspecs(repo: Path, pathspecs: list[str], files: list[str]) -> list[str]:
    absent: list[str] = []
    for pathspec in pathspecs:
        if any(char in pathspec for char in GLOB_CHARS):
            continue
        if not exact_pathspec_has_tracked_match(pathspec, files) and not (repo / pathspec).exists():
            absent.append(pathspec)
    return absent


def pathspec_has_history(repo: Path, pathspec: str) -> bool:
    output = run_git(repo, ["log", "--all", "--format=%H", "--", pathspec])
    return bool(output.strip())


def validate_input_pathspecs(repo: Path, pathspecs: list[str]) -> list[str]:
    files = tracked_files(repo, pathspecs)
    errors: list[str] = []
    for pathspec in absent_exact_pathspecs(repo, pathspecs, files):
        if not pathspec_has_history(repo, pathspec):
            errors.append(f"input path does not resolve to a tracked or historical path: {pathspec}")
    return errors


def digest_input_set(repo: Path, pathspecs: list[str]) -> str:
    digest = hashlib.sha256()
    for pathspec in pathspecs:
        digest.update(b"pathspec\0")
        digest.update(pathspec.encode("utf-8"))
        digest.update(b"\0")
    files = tracked_files(repo, pathspecs)
    for relative in absent_exact_pathspecs(repo, pathspecs, files):
        print(f"warning: input path absent: {relative}", file=sys.stderr)
        digest.update(b"absent\0")
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
    for relative in files:
        path = repo / relative
        digest.update(b"file\0")
        digest.update(relative.encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def changed_paths(repo: Path, pathspecs: list[str], *, base: str, head: str) -> list[str]:
    if not pathspecs:
        return []
    output = run_git(repo, ["diff", "--name-only", f"{base}...{head}", "--", *pathspecs])
    return [line for line in output.splitlines() if line]


def print_lines(lines: Iterable[str], *, file: TextIO = sys.stdout) -> None:
    for line in lines:
        print(line, file=file)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Resolve named CI input sets.")
    parser.add_argument("--repo", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    subparsers = parser.add_subparsers(dest="command", required=True)

    for command in ("list", "hash"):
        subparser = subparsers.add_parser(command)
        subparser.add_argument("set_name")

    validate = subparsers.add_parser("validate")
    validate.add_argument("set_names", nargs="+")

    changed = subparsers.add_parser("changed")
    changed.add_argument("set_name")
    changed.add_argument("--base", required=True)
    changed.add_argument("--head", default="HEAD")

    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = args.repo.resolve()
    config_path = args.config
    if not config_path.is_absolute():
        config_path = repo / config_path
    config = load_config(config_path)
    if args.command == "list":
        pathspecs = resolve_set(config, args.set_name)
        print_lines(pathspecs)
    elif args.command == "hash":
        pathspecs = resolve_set(config, args.set_name)
        print(digest_input_set(repo, pathspecs))
    elif args.command == "validate":
        errors: list[str] = []
        for set_name in args.set_names:
            errors.extend(validate_input_pathspecs(repo, resolve_set(config, set_name)))
        if errors:
            print_lines(errors, file=sys.stderr)
            return 1
    elif args.command == "changed":
        pathspecs = resolve_set(config, args.set_name)
        print_lines(changed_paths(repo, pathspecs, base=args.base, head=args.head))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
