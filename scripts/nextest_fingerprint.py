#!/usr/bin/env python3
"""Produce the root nextest archive fingerprint from tracked git inputs."""

from __future__ import annotations

import argparse
from collections.abc import Iterable
from dataclasses import dataclass
import functools
import hashlib
import os
import pathlib
import subprocess
import sys
import tomllib


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv  # noqa: E402


FORBIDDEN_SAFE_EXCLUDES = (
    "deploy/",
    "gated_source_roots.manifest",
    "config/",
    "contracts/",
    "docs/bolt-v3/",
    "specs/",
    "ci/nextest-fingerprint.toml",
    "scripts/nextest_fingerprint.py",
)

MANDATORY_TRACKED_INPUTS = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "build.rs",
    "gated_source_roots.manifest",
    "src/",
    "tests/",
    "ci/nextest-fingerprint.toml",
    "scripts/nextest_fingerprint.py",
    "scripts/root_bin_sidecars.py",
)

class FingerprintError(Exception):
    """Raised when fingerprint production must fail closed."""


require_table = functools.partial(_cv.require_table, error_cls=FingerprintError)
require_string = functools.partial(_cv.require_string, error_cls=FingerprintError)


@dataclass(frozen=True)
class SafeExclude:
    path: str
    justification: str


@dataclass(frozen=True)
class FingerprintConfig:
    schema: int
    profile: str
    shards: int
    tracked_inputs: tuple[str, ...]
    safe_excludes: tuple[SafeExclude, ...]


def run_git(
    repo_root: pathlib.Path,
    args: list[str],
    *,
    text: bool = True,
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", *args],
        cwd=repo_root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=text,
    )


def git_tree_paths(repo_root: pathlib.Path) -> tuple[str, ...]:
    result = run_git(repo_root, ["ls-tree", "-r", "-z", "--name-only", "HEAD"], text=False)
    if result.returncode != 0:
        stderr = result.stderr.decode("utf-8", "replace")
        raise FingerprintError(f"could not list HEAD tree paths: {stderr.strip()}")
    return tuple(
        record.decode("utf-8", "surrogateescape")
        for record in result.stdout.split(b"\0")
        if record
    )


def normalize_repo_path(raw: str, *, label: str) -> str:
    if not raw.strip():
        raise FingerprintError(f"{label} must be a non-empty repo-relative path")
    path = raw.strip().replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    if (
        path.startswith("/")
        or path == "."
        or path.startswith("../")
        or "/../" in path
        or path.endswith("/..")
    ):
        raise FingerprintError(f"{label} must be a normalized repo-relative path: {raw!r}")
    if "//" in path:
        raise FingerprintError(f"{label} must not contain empty path segments: {raw!r}")
    return path.rstrip("/")


def path_matches_entry(path: str, entry: str) -> bool:
    base = entry.rstrip("/")
    return path == base or path.startswith(base + "/")


def paths_overlap(left: str, right: str) -> bool:
    return path_matches_entry(left.rstrip("/"), right) or path_matches_entry(
        right.rstrip("/"),
        left,
    )


def load_toml(path: pathlib.Path, label: str) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise FingerprintError(f"{label} missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise FingerprintError(f"{label} invalid TOML: {exc}") from exc


def require_positive_int(parent: dict[str, object], key: str, label: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or value <= 0:
        raise FingerprintError(f"{label}.{key} must be a positive integer")
    return value


def require_string_list(parent: dict[str, object], key: str, label: str) -> tuple[str, ...]:
    value = parent.get(key)
    if not isinstance(value, list) or not value:
        raise FingerprintError(f"{label}.{key} must be a non-empty string list")
    strings: list[str] = []
    for index, item in enumerate(value, start=1):
        if not isinstance(item, str) or not item:
            raise FingerprintError(f"{label}.{key}[{index}] must be a non-empty string")
        strings.append(item)
    return tuple(strings)


def load_fingerprint_config(path: pathlib.Path) -> FingerprintConfig:
    data = load_toml(path, "nextest fingerprint config")
    archive = require_table(data, "nextest_archive", "nextest fingerprint config")
    schema = require_positive_int(archive, "schema", "nextest_archive")
    profile = require_string(archive, "profile", "nextest_archive")
    shards = require_positive_int(archive, "shards", "nextest_archive")
    tracked_inputs = tuple(
        normalize_repo_path(raw, label="nextest_archive.tracked_inputs")
        for raw in require_string_list(archive, "tracked_inputs", "nextest_archive")
    )
    if len(set(tracked_inputs)) != len(tracked_inputs):
        raise FingerprintError("nextest_archive.tracked_inputs must not contain duplicates")
    for mandatory in MANDATORY_TRACKED_INPUTS:
        mandatory_path = normalize_repo_path(mandatory, label="mandatory tracked input")
        if not any(paths_overlap(mandatory_path, tracked_input) for tracked_input in tracked_inputs):
            raise FingerprintError(f"nextest_archive.tracked_inputs must include {mandatory}")
    raw_excludes = data.get("safe_excludes", [])
    if not isinstance(raw_excludes, list):
        raise FingerprintError("safe_excludes must be an array of tables")

    safe_excludes: list[SafeExclude] = []
    for index, raw_exclude in enumerate(raw_excludes, start=1):
        if not isinstance(raw_exclude, dict):
            raise FingerprintError(f"safe_excludes[{index}] must be a table")
        path = normalize_repo_path(
            require_string(raw_exclude, "path", f"safe_excludes[{index}]"),
            label=f"safe_excludes[{index}].path",
        )
        justification = require_string(
            raw_exclude,
            "justification",
            f"safe_excludes[{index}]",
        )
        for forbidden in FORBIDDEN_SAFE_EXCLUDES:
            forbidden_path = normalize_repo_path(forbidden, label="forbidden safe exclude")
            if paths_overlap(path, forbidden_path):
                raise FingerprintError(f"safe-listed path is forbidden: {path}")
        if any(paths_overlap(path, tracked_input) for tracked_input in tracked_inputs):
            raise FingerprintError(f"safe-listed path overlaps tracked input: {path}")
        safe_excludes.append(SafeExclude(path=path, justification=justification))
    return FingerprintConfig(
        schema=schema,
        profile=profile,
        shards=shards,
        tracked_inputs=tracked_inputs,
        safe_excludes=tuple(safe_excludes),
    )


def load_artifact_prefix(path: pathlib.Path) -> str:
    data = load_toml(path, "managed runner config")
    meter = require_table(data, "meter", "managed runner config")
    prefix = require_string(meter, "fingerprint_artifact_prefix", "meter")
    if not prefix.endswith("fingerprint-"):
        raise FingerprintError("meter.fingerprint_artifact_prefix must end with fingerprint-")
    return prefix


def collect_path_dependencies(value: object) -> Iterable[str]:
    if isinstance(value, dict):
        path = value.get("path")
        if isinstance(path, str):
            yield path
        for nested in value.values():
            yield from collect_path_dependencies(nested)
    elif isinstance(value, list):
        for nested in value:
            yield from collect_path_dependencies(nested)


def require_separate_workspace(repo_root: pathlib.Path, safe_exclude: SafeExclude) -> None:
    workspace_root = repo_root / safe_exclude.path
    workspace_toml = workspace_root / "Cargo.toml"
    if not workspace_root.is_dir() or not workspace_toml.is_file():
        raise FingerprintError(
            "safe-listed path must be a separate Cargo workspace: "
            f"{safe_exclude.path}"
        )
    try:
        cargo = tomllib.loads(workspace_toml.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise FingerprintError(
            f"safe-listed workspace Cargo.toml invalid TOML: {safe_exclude.path}"
        ) from exc
    if not isinstance(cargo.get("workspace"), dict):
        raise FingerprintError(
            "safe-listed path must be a separate Cargo workspace: "
            f"{safe_exclude.path}"
        )


def validate_safe_excludes(repo_root: pathlib.Path, config: FingerprintConfig) -> None:
    cargo_toml = repo_root / "Cargo.toml"
    if not cargo_toml.exists():
        return
    try:
        cargo = tomllib.loads(cargo_toml.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise FingerprintError(f"root Cargo.toml invalid TOML: {exc}") from exc

    workspace = cargo.get("workspace", {})
    members = workspace.get("members", []) if isinstance(workspace, dict) else []
    if not isinstance(members, list):
        members = []
    normalized_members = {
        normalize_repo_path(member, label="workspace.members")
        for member in members
        if isinstance(member, str) and "*" not in member
    }
    path_dependencies = {
        normalize_repo_path(path, label="root Cargo.toml path dependency")
        for path in collect_path_dependencies(cargo)
    }

    for safe_exclude in config.safe_excludes:
        require_separate_workspace(repo_root, safe_exclude)
        if any(paths_overlap(safe_exclude.path, member) for member in normalized_members):
            raise FingerprintError(
                f"safe-listed path is a root workspace member: {safe_exclude.path}"
            )
        if any(paths_overlap(safe_exclude.path, dependency) for dependency in path_dependencies):
            raise FingerprintError(
                "safe-listed path is referenced by root Cargo.toml path dependency: "
                f"{safe_exclude.path}"
            )


def validate_tracked_inputs_match_tree(repo_root: pathlib.Path, config: FingerprintConfig) -> None:
    tree_paths = git_tree_paths(repo_root)
    for tracked_input in config.tracked_inputs:
        if not any(path_matches_entry(path, tracked_input) for path in tree_paths):
            raise FingerprintError(
                "nextest_archive.tracked_inputs entry matches no tracked files: "
                f"{tracked_input}"
            )


def rust_identifier_char(char: str) -> bool:
    return char == "_" or char.isalnum()


def rust_string_literal_at(text: str, index: int) -> tuple[str, int] | None:
    start = index
    prefix = ""
    if text.startswith(("br", "rb"), index):
        prefix = text[index : index + 2]
        index += 2
    elif index < len(text) and text[index] in {"b", "r"}:
        prefix = text[index]
        index += 1

    if "r" in prefix:
        hashes = 0
        while index < len(text) and text[index] == "#":
            hashes += 1
            index += 1
        if index >= len(text) or text[index] != '"':
            return None
        terminator = '"' + ("#" * hashes)
        end = text.find(terminator, index + 1)
        if end == -1:
            return None
        end += len(terminator)
        return text[start:end], end

    if index >= len(text) or text[index] != '"':
        return None
    index += 1
    escaped = False
    while index < len(text):
        char = text[index]
        if escaped:
            escaped = False
            index += 1
            continue
        if char == "\\":
            escaped = True
            index += 1
            continue
        if char == '"':
            return text[start : index + 1], index + 1
        index += 1
    return None


def rust_block_comment_end(text: str, index: int) -> int:
    block_depth = 0
    while index < len(text) - 1:
        char = text[index]
        next_char = text[index + 1]
        if char == "/" and next_char == "*":
            block_depth += 1
            index += 2
            continue
        if char == "*" and next_char == "/":
            block_depth -= 1
            index += 2
            if block_depth == 0:
                return index
            continue
        index += 1
    return len(text)


def rust_char_literal_end(text: str, index: int) -> int | None:
    if index + 2 >= len(text) or text[index] != "'":
        return None
    cursor = index + 1
    if text[cursor] == "\\":
        cursor += 2
    else:
        cursor += 1
    if cursor < len(text) and text[cursor] == "'":
        return cursor + 1
    return None


def rust_whitespace_or_comment_end(text: str, index: int) -> int:
    cursor = index
    while cursor < len(text):
        if text[cursor].isspace():
            cursor += 1
            continue
        if text.startswith("//", cursor):
            newline = text.find("\n", cursor + 2)
            cursor = len(text) if newline == -1 else newline + 1
            continue
        if text.startswith("/*", cursor):
            cursor = rust_block_comment_end(text, cursor)
            continue
        return cursor
    return cursor


def rust_include_literals(text: str) -> Iterable[str]:
    index = 0
    while index < len(text):
        char = text[index]
        next_char = text[index + 1] if index + 1 < len(text) else ""
        string_literal = rust_string_literal_at(text, index)
        if string_literal is not None:
            _literal, end = string_literal
            index = end
            continue
        char_literal_end = rust_char_literal_end(text, index)
        if char_literal_end is not None:
            index = char_literal_end
            continue
        if char == "/" and next_char == "/":
            newline = text.find("\n", index + 2)
            index = len(text) if newline == -1 else newline + 1
            continue
        if char == "/" and next_char == "*":
            index = rust_block_comment_end(text, index)
            continue

        macro_name = ""
        if text.startswith("include_str", index):
            macro_name = "include_str"
        elif text.startswith("include_bytes", index):
            macro_name = "include_bytes"
        if not macro_name:
            index += 1
            continue

        macro_end = index + len(macro_name)
        before = text[index - 1] if index > 0 else ""
        after = text[macro_end] if macro_end < len(text) else ""
        if rust_identifier_char(before) or rust_identifier_char(after):
            index += 1
            continue

        cursor = macro_end
        cursor = rust_whitespace_or_comment_end(text, cursor)
        if cursor >= len(text) or text[cursor] != "!":
            index += 1
            continue
        cursor += 1
        cursor = rust_whitespace_or_comment_end(text, cursor)
        if cursor >= len(text) or text[cursor] not in {"(", "[", "{"}:
            index += 1
            continue
        cursor += 1
        cursor = rust_whitespace_or_comment_end(text, cursor)
        argument_literal = rust_string_literal_at(text, cursor)
        if argument_literal is None:
            raise FingerprintError(
                f"compile-time include argument must be a direct string literal: {macro_name}"
            )
        literal, end = argument_literal
        yield literal
        index = end


def rust_string_literal_value(literal: str) -> str:
    prefix = ""
    while literal and literal[0] in {"b", "r"}:
        prefix += literal[0]
        literal = literal[1:]
    if "r" in prefix:
        hashes = len(literal) - len(literal.lstrip("#"))
        start = hashes + 1
        end = -(hashes + 1)
        return literal[start:end]
    body = literal[1:-1]
    return bytes(body, "utf-8").decode("unicode_escape")


def compile_time_include_targets(repo_root: pathlib.Path, config: FingerprintConfig) -> Iterable[tuple[str, str]]:
    tree_paths = set(git_tree_paths(repo_root))
    for path in tree_paths:
        if not path.endswith(".rs"):
            continue
        if not tracked_input_included(path, config.tracked_inputs) or safe_excluded(path, config.safe_excludes):
            continue
        source_path = repo_root / path
        try:
            text = source_path.read_text(encoding="utf-8")
        except UnicodeDecodeError as exc:
            raise FingerprintError(f"could not read Rust source as UTF-8: {path}") from exc
        for literal in rust_include_literals(text):
            target = (source_path.parent / rust_string_literal_value(literal)).resolve()
            try:
                relative_target = target.relative_to(repo_root.resolve()).as_posix()
            except ValueError as exc:
                raise FingerprintError(f"compile-time include target escapes repo: {path}") from exc
            if not target.exists():
                raise FingerprintError(f"compile-time include target missing: {relative_target}")
            if relative_target not in tree_paths:
                raise FingerprintError(f"compile-time include target is not tracked in HEAD: {relative_target}")
            yield path, normalize_repo_path(relative_target, label="compile-time include target")


def validate_compile_time_includes(repo_root: pathlib.Path, config: FingerprintConfig) -> None:
    for source, target in compile_time_include_targets(repo_root, config):
        if target.endswith(".md"):
            raise FingerprintError(
                "compile-time include target must not be a prose doc: "
                f"{target} (referenced by {source})"
            )
        if not tracked_input_included(target, config.tracked_inputs) or safe_excluded(target, config.safe_excludes):
            raise FingerprintError(
                "compile-time include target is outside nextest tracked inputs: "
                f"{target} (referenced by {source})"
            )


def ensure_clean_tracked_worktree(repo_root: pathlib.Path) -> None:
    result = run_git(repo_root, ["diff", "--quiet", "HEAD", "--"])
    if result.returncode == 1:
        raise FingerprintError("worktree must match HEAD before computing nextest fingerprint")
    if result.returncode != 0:
        raise FingerprintError(f"could not inspect worktree: {result.stderr.strip()}")


def safe_excluded(path: str, safe_excludes: tuple[SafeExclude, ...]) -> bool:
    return any(path_matches_entry(path, safe_exclude.path) for safe_exclude in safe_excludes)


def tracked_input_included(path: str, tracked_inputs: tuple[str, ...]) -> bool:
    return any(path_matches_entry(path, tracked_input) for tracked_input in tracked_inputs)


def tracked_tree_entries(
    repo_root: pathlib.Path,
    tracked_inputs: tuple[str, ...],
    safe_excludes: tuple[SafeExclude, ...],
) -> list[tuple[bytes, bytes, bytes, bytes]]:
    result = run_git(repo_root, ["ls-tree", "-r", "-z", "HEAD"], text=False)
    if result.returncode != 0:
        stderr = result.stderr.decode("utf-8", "replace")
        raise FingerprintError(f"could not list HEAD tree: {stderr.strip()}")

    entries: list[tuple[bytes, bytes, bytes, bytes]] = []
    for record in result.stdout.split(b"\0"):
        if not record:
            continue
        try:
            metadata, path_bytes = record.split(b"\t", 1)
            mode, object_type, object_id = metadata.split(b" ", 2)
        except ValueError as exc:
            raise FingerprintError(f"unexpected git ls-tree record: {record!r}") from exc
        path = path_bytes.decode("utf-8", "surrogateescape")
        if not tracked_input_included(path, tracked_inputs):
            continue
        if safe_excluded(path, safe_excludes):
            continue
        entries.append((path_bytes, mode, object_type, object_id))
    return sorted(entries, key=lambda entry: entry[0])


def tree_digest(
    repo_root: pathlib.Path,
    tracked_inputs: tuple[str, ...],
    safe_excludes: tuple[SafeExclude, ...],
) -> str:
    digest = hashlib.sha256()
    digest.update(b"bolt-v2-nextest-tree-digest-v2\0")
    for path, mode, object_type, object_id in tracked_tree_entries(
        repo_root,
        tracked_inputs,
        safe_excludes,
    ):
        digest.update(mode)
        digest.update(b"\0")
        digest.update(object_type)
        digest.update(b"\0")
        digest.update(object_id)
        digest.update(b"\0")
        digest.update(path)
        digest.update(b"\0")
    return digest.hexdigest()


def require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise FingerprintError(f"{name} must be set")
    return value


def append_github_output(values: dict[str, str]) -> None:
    output_path = pathlib.Path(require_env("GITHUB_OUTPUT"))
    with output_path.open("a", encoding="utf-8") as handle:
        for name, value in values.items():
            handle.write(f"{name}={value}\n")


def require_cli_value(value: str, label: str) -> str:
    if not value or any(character.isspace() for character in value):
        raise FingerprintError(f"{label} must be a non-empty single shell word")
    return value


def produce_fingerprint(args: argparse.Namespace) -> None:
    repo_root = pathlib.Path(args.repo_root).resolve()
    config = load_fingerprint_config(pathlib.Path(args.config).resolve())
    artifact_prefix = load_artifact_prefix(pathlib.Path(args.runners_config).resolve())
    validate_safe_excludes(repo_root, config)
    ensure_clean_tracked_worktree(repo_root)
    validate_tracked_inputs_match_tree(repo_root, config)
    validate_compile_time_includes(repo_root, config)

    runner_os = require_cli_value(args.runner_os, "runner-os")
    runner_arch = require_cli_value(args.runner_arch, "runner-arch")
    digest = tree_digest(repo_root, config.tracked_inputs, config.safe_excludes)
    archive_prefix = artifact_prefix[: -len("fingerprint-")]
    fingerprint = (
        f"{archive_prefix}v{config.schema}-{runner_os}-{runner_arch}-"
        f"{config.profile}-profile-shards-{config.shards}-{digest}"
    )
    artifact_name = (
        f"{artifact_prefix}v{config.schema}-{runner_os}-{runner_arch}-"
        f"{config.profile}-profile-shards-{config.shards}-{digest}"
    )

    output_path = pathlib.Path(args.output_path)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(f"{fingerprint}\n", encoding="utf-8")
    append_github_output(
        {
            "nextest_digest": digest,
            "nextest_fingerprint": fingerprint,
            "nextest_fingerprint_artifact_name": artifact_name,
            "nextest_archive_prefix": archive_prefix,
            "nextest_schema": str(config.schema),
            "nextest_profile": config.profile,
            "nextest_shards": str(config.shards),
        }
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--runners-config", required=True)
    parser.add_argument("--runner-os", required=True)
    parser.add_argument("--runner-arch", required=True)
    parser.add_argument("--output-path", required=True)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    try:
        produce_fingerprint(parse_args(argv))
    except FingerprintError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
