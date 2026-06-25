#!/usr/bin/env python3
"""Migrate Bolt-v3 decision-evidence JSONL files from schema v13 to v14."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


SUPPORTED_OLD_SCHEMA_VERSION = 13
SUPPORTED_CURRENT_SCHEMA_VERSION = 14
VERSION_RE = re.compile(rb'("schema_version"\s*:\s*)(?P<version>-?\d+)\b')

KEY_STRING_REPLACEMENTS: tuple[tuple[re.Pattern[bytes], bytes], ...] = (
    (
        re.compile(rb'("kind"\s*:\s*)"position_sizer_rebuild"'),
        b"capital_admission_rebuild",
    ),
    (
        re.compile(rb'("gate_id"\s*:\s*)"bolt_v3.position_sizer_rebuild"'),
        b"bolt_v3.capital_admission_rebuild",
    ),
    (
        re.compile(rb'("outcome"\s*:\s*)"rejected_position_sizing"'),
        b"rejected_capital_admission",
    ),
    (
        re.compile(rb'("source"\s*:\s*)"nt_sizing_state"'),
        b"nt_capital_admission_state",
    ),
    (
        re.compile(rb'("source"\s*:\s*)"nt_position_sizer_runtime_components"'),
        b"nt_capital_admission_runtime_components",
    ),
)


class MigrationError(RuntimeError):
    """Raised when evidence cannot be migrated safely."""


@dataclass(frozen=True)
class PlannedFileMigration:
    path: Path
    before: bytes
    after: bytes


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def jsonl_files(root: Path) -> list[Path]:
    if not root.exists():
        raise MigrationError(f"{root}: directory does not exist")
    if not root.is_dir():
        raise MigrationError(f"{root}: expected a directory")
    return sorted(path for path in root.rglob("*.jsonl") if path.is_file())


def schema_version_for_line(path: Path, line_number: int, line: bytes) -> int | None:
    if not line.strip():
        return None
    matches = list(VERSION_RE.finditer(line))
    if len(matches) != 1:
        raise MigrationError(
            f"{path}:{line_number}: expected exactly one schema_version field, found {len(matches)}"
        )
    version = int(matches[0].group("version"))
    if version not in {SUPPORTED_OLD_SCHEMA_VERSION, SUPPORTED_CURRENT_SCHEMA_VERSION}:
        raise MigrationError(f"{path}:{line_number}: unsupported schema_version={version}")
    return version


def replace_key_string(line: bytes, pattern: re.Pattern[bytes], new_value: bytes) -> bytes:
    return pattern.sub(lambda match: match.group(1) + b'"' + new_value + b'"', line)


def migrate_v13_line(line: bytes) -> bytes:
    migrated = VERSION_RE.sub(lambda match: match.group(1) + b"14", line, count=1)
    for pattern, new_value in KEY_STRING_REPLACEMENTS:
        migrated = replace_key_string(migrated, pattern, new_value)
    return migrated


def migrate_file_bytes(path: Path, payload: bytes) -> bytes:
    migrated_lines: list[bytes] = []
    for line_number, line in enumerate(payload.splitlines(keepends=True), start=1):
        content = line.rstrip(b"\r\n")
        suffix = line[len(content) :]
        version = schema_version_for_line(path, line_number, content)
        if version == SUPPORTED_OLD_SCHEMA_VERSION:
            migrated_lines.append(migrate_v13_line(content) + suffix)
        else:
            migrated_lines.append(line)
    return b"".join(migrated_lines)


def plan_migrations(root: Path) -> list[PlannedFileMigration]:
    planned: list[PlannedFileMigration] = []
    for path in jsonl_files(root):
        before = path.read_bytes()
        after = migrate_file_bytes(path, before)
        if after != before:
            planned.append(PlannedFileMigration(path=path, before=before, after=after))
    return planned


def atomic_write_bytes(path: Path, payload: bytes) -> None:
    stat = path.stat()
    tmp_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
            tmp_name = tmp.name
            tmp.write(payload)
            tmp.flush()
            os.fsync(tmp.fileno())
        os.chmod(tmp_name, stat.st_mode & 0o777)
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
            except FileNotFoundError:
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


def migrate_directory(root: Path, *, dry_run: bool = False) -> dict[str, object]:
    planned = plan_migrations(root)
    if not dry_run:
        for item in planned:
            atomic_write_bytes(item.path, item.after)
    return manifest_for(planned)


def migrate_cli(argv: Sequence[str] | None = None) -> dict[str, object]:
    parser = argparse.ArgumentParser(
        description="Migrate Bolt-v3 decision-evidence JSONL files from schema v13 to v14.",
    )
    parser.add_argument("directory", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    return migrate_directory(args.directory, dry_run=args.dry_run)


def main(argv: Sequence[str] | None = None) -> int:
    try:
        manifest = migrate_cli(argv)
    except MigrationError as error:
        print(error, file=sys.stderr)
        return 1
    print(json.dumps(manifest, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
