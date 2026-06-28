#!/usr/bin/env python3
"""Migrate Bolt-v3 root TOML configs to capital_admission_policy/root schema v2."""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


ROOT_SCHEMA_RE = re.compile(rb'^(\s*schema_version\s*=\s*)1(\s*(?:#.*)?)$')
POOL_CONTEXTS = {
    ("risk", "capital_pools"),
}
OLD_POOL_POLICY_HEADER_PREFIX = ("risk", "capital_pools", "sizing_policy")
OLD_POOL_POLICY_KEY_RE = re.compile(rb"^(\s*)sizing_policy(?=\s*(?:[.=]))")


class MigrationError(RuntimeError):
    """Raised when a config cannot be migrated safely."""


@dataclass(frozen=True)
class PlannedFileMigration:
    path: Path
    before: bytes
    after: bytes


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def toml_files(path: Path) -> list[Path]:
    if not path.exists():
        raise MigrationError(f"{path}: path does not exist")
    if path.is_file():
        return [path]
    if not path.is_dir():
        raise MigrationError(f"{path}: expected a TOML file or directory")
    return sorted(candidate for candidate in path.rglob("*.toml") if candidate.is_file())


def split_line_ending(line: bytes) -> tuple[bytes, bytes]:
    if line.endswith(b"\r\n"):
        return line[:-2], b"\r\n"
    if line.endswith(b"\n"):
        return line[:-1], b"\n"
    return line, b""


def parse_table_header(line_body: bytes) -> tuple[tuple[str, ...], bytes, bytes] | None:
    stripped = line_body.lstrip()
    leading = line_body[: len(line_body) - len(stripped)]
    if stripped.startswith(b"[["):
        closing = stripped.find(b"]]")
        if closing == -1:
            return None
        body = stripped[2:closing]
        suffix = stripped[closing + 2 :]
    elif stripped.startswith(b"["):
        closing = stripped.find(b"]")
        if closing == -1:
            return None
        body = stripped[1:closing]
        suffix = stripped[closing + 1 :]
    else:
        return None
    if suffix.strip() and not suffix.lstrip().startswith(b"#"):
        return None
    try:
        segments = tuple(segment.strip() for segment in body.decode("utf-8").split("."))
    except UnicodeDecodeError as error:
        raise MigrationError("TOML table header is not valid UTF-8") from error
    if any(not segment for segment in segments):
        return None
    return segments, leading, stripped


def rewritten_header_line(line_body: bytes) -> tuple[bytes, tuple[str, ...]] | None:
    parsed = parse_table_header(line_body)
    if parsed is None:
        return None
    segments, leading, stripped = parsed
    if segments[:3] != OLD_POOL_POLICY_HEADER_PREFIX:
        return line_body, segments

    if stripped.startswith(b"[["):
        opening = b"[["
        closing = stripped.find(b"]]")
        close_token = b"]]"
    else:
        opening = b"["
        closing = stripped.find(b"]")
        close_token = b"]"
    body_start = len(opening)
    body = stripped[body_start:closing]
    rewritten_body = body.replace(b"sizing_policy", b"capital_admission_policy", 1)
    rewritten = leading + opening + rewritten_body + close_token + stripped[closing + len(close_token) :]
    rewritten_segments = tuple(
        "capital_admission_policy" if index == 2 else segment
        for index, segment in enumerate(segments)
    )
    return rewritten, rewritten_segments


def is_bolt_v3_root_candidate(payload: bytes) -> bool:
    has_strategy_files = False
    has_capital_pools = False
    before_first_table = True
    for raw_line in payload.splitlines():
        header = parse_table_header(raw_line)
        if header is not None:
            before_first_table = False
            segments, _, _ = header
            if segments[:2] == ("risk", "capital_pools"):
                has_capital_pools = True
            continue
        if before_first_table and re.match(rb"^\s*strategy_files\s*=", raw_line):
            has_strategy_files = True
    return has_strategy_files or has_capital_pools


def migrate_toml_bytes(payload: bytes) -> bytes:
    if not is_bolt_v3_root_candidate(payload):
        return payload

    migrated: list[bytes] = []
    before_first_table = True
    current_context: tuple[str, ...] = ()
    for line in payload.splitlines(keepends=True):
        line_body, line_ending = split_line_ending(line)

        rewritten_header = rewritten_header_line(line_body)
        if rewritten_header is not None:
            line_body, current_context = rewritten_header
            before_first_table = False
        elif before_first_table:
            line_body = ROOT_SCHEMA_RE.sub(rb"\g<1>2\2", line_body, count=1)
        elif current_context in POOL_CONTEXTS:
            line_body = OLD_POOL_POLICY_KEY_RE.sub(
                rb"\1capital_admission_policy", line_body, count=1
            )

        migrated.append(line_body + line_ending)
    return b"".join(migrated)


def plan_migrations(path: Path) -> list[PlannedFileMigration]:
    planned: list[PlannedFileMigration] = []
    for toml_path in toml_files(path):
        before = toml_path.read_bytes()
        after = migrate_toml_bytes(before)
        if before != after:
            planned.append(PlannedFileMigration(path=toml_path, before=before, after=after))
    return planned


def atomic_write_bytes(path: Path, payload: bytes) -> None:
    try:
        mode = path.stat().st_mode & 0o777
    except FileNotFoundError:
        mode = 0o600
    tmp_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
            tmp_name = tmp.name
            tmp.write(payload)
            tmp.flush()
            os.fsync(tmp.fileno())
        os.chmod(tmp_name, mode)
        os.replace(tmp_name, path)
        directory_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    except BaseException:
        if tmp_name is not None:
            try:
                os.unlink(tmp_name)
            except BaseException:
                pass
        raise


def manifest_for(planned: Sequence[PlannedFileMigration]) -> dict[str, object]:
    return {
        "changed_files": [
            {
                "path": str(item.path),
                "before_sha256": sha256_bytes(item.before),
                "after_sha256": sha256_bytes(item.after),
            }
            for item in planned
        ]
    }


def unified_diff_for(planned: Sequence[PlannedFileMigration]) -> str:
    chunks: list[str] = []
    for item in planned:
        before = item.before.decode("utf-8").splitlines(keepends=True)
        after = item.after.decode("utf-8").splitlines(keepends=True)
        chunks.extend(
            difflib.unified_diff(
                before,
                after,
                fromfile=str(item.path),
                tofile=str(item.path),
            )
        )
    return "".join(chunks)


def migrate_path(
    path: Path, *, dry_run: bool = False, emit_diff: bool = False
) -> dict[str, object]:
    planned = plan_migrations(path)
    if dry_run:
        if emit_diff:
            diff = unified_diff_for(planned)
            if diff:
                print(diff, end="")
    else:
        for item in planned:
            atomic_write_bytes(item.path, item.after)
    return manifest_for(planned)


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Migrate Bolt-v3 root TOML configs to capital_admission_policy/root schema v2.",
    )
    parser.add_argument("path", type=Path, help="Bolt-v3 root TOML file or directory")
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def migrate_cli(
    argv: Sequence[str] | None = None, *, emit_diff: bool = False
) -> dict[str, object]:
    args = parse_args(argv)
    return migrate_path(args.path, dry_run=args.dry_run, emit_diff=emit_diff)


def main(argv: Sequence[str] | None = None) -> int:
    try:
        manifest = migrate_cli(argv, emit_diff=True)
        print(json.dumps(manifest, sort_keys=True))
    except MigrationError as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
