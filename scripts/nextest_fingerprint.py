#!/usr/bin/env python3
"""Produce the root nextest archive fingerprint from tracked git inputs."""

from __future__ import annotations

import argparse
from collections.abc import Iterable
from dataclasses import dataclass
import hashlib
import os
import pathlib
import subprocess
import sys
import tomllib


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


class FingerprintError(Exception):
    """Raised when fingerprint production must fail closed."""


@dataclass(frozen=True)
class SafeExclude:
    path: str
    justification: str


@dataclass(frozen=True)
class FingerprintConfig:
    schema: int
    profile: str
    shards: int
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


def require_table(parent: dict[str, object], key: str, label: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise FingerprintError(f"{label}.{key} must be a table")
    return value


def require_positive_int(parent: dict[str, object], key: str, label: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or value <= 0:
        raise FingerprintError(f"{label}.{key} must be a positive integer")
    return value


def require_string(parent: dict[str, object], key: str, label: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise FingerprintError(f"{label}.{key} must be a non-empty string")
    return value


def load_fingerprint_config(path: pathlib.Path) -> FingerprintConfig:
    data = load_toml(path, "nextest fingerprint config")
    archive = require_table(data, "nextest_archive", "nextest fingerprint config")
    schema = require_positive_int(archive, "schema", "nextest_archive")
    profile = require_string(archive, "profile", "nextest_archive")
    shards = require_positive_int(archive, "shards", "nextest_archive")
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
        safe_excludes.append(SafeExclude(path=path, justification=justification))
    return FingerprintConfig(
        schema=schema,
        profile=profile,
        shards=shards,
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


def ensure_clean_tracked_worktree(repo_root: pathlib.Path) -> None:
    result = run_git(repo_root, ["diff", "--quiet", "HEAD", "--"])
    if result.returncode == 1:
        raise FingerprintError("worktree must match HEAD before computing nextest fingerprint")
    if result.returncode != 0:
        raise FingerprintError(f"could not inspect worktree: {result.stderr.strip()}")


def safe_excluded(path: str, safe_excludes: tuple[SafeExclude, ...]) -> bool:
    return any(path_matches_entry(path, safe_exclude.path) for safe_exclude in safe_excludes)


def tracked_tree_entries(
    repo_root: pathlib.Path,
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
        if safe_excluded(path, safe_excludes):
            continue
        entries.append((path_bytes, mode, object_type, object_id))
    return sorted(entries, key=lambda entry: entry[0])


def tree_digest(repo_root: pathlib.Path, safe_excludes: tuple[SafeExclude, ...]) -> str:
    digest = hashlib.sha256()
    digest.update(b"bolt-v2-nextest-tree-digest-v1\0")
    for path, mode, object_type, object_id in tracked_tree_entries(repo_root, safe_excludes):
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

    runner_os = require_cli_value(args.runner_os, "runner-os")
    runner_arch = require_cli_value(args.runner_arch, "runner-arch")
    digest = tree_digest(repo_root, config.safe_excludes)
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
