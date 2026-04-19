#!/usr/bin/env python3
"""Restore/capture tracked file mtimes for CI cache-backed warm reruns."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
from typing import Iterable


def tracked_entries(repo: Path) -> list[tuple[str, str]]:
    proc = subprocess.run(
        ["git", "-C", str(repo), "ls-files", "-s", "-z"],
        check=True,
        capture_output=True,
    )
    entries: list[tuple[str, str]] = []
    for record in proc.stdout.split(b"\0"):
        if not record:
            continue
        meta, rel_path = record.split(b"\t", 1)
        _, blob, _ = meta.decode("utf-8").split(" ", 2)
        entries.append((rel_path.decode("utf-8"), blob))
    return entries


def load_manifest(path: Path) -> dict[str, dict[str, int | str]]:
    if not path.exists():
        return {}
    with path.open("r", encoding="utf-8") as fh:
        payload = json.load(fh)
    if payload.get("version") != 1:
        raise ValueError(f"unsupported manifest version in {path}")
    return {entry["path"]: entry for entry in payload.get("files", [])}


def atomic_write(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(f"{path.suffix}.tmp")
    with tmp.open("w", encoding="utf-8") as fh:
        json.dump(payload, fh, sort_keys=True)
        fh.write("\n")
    tmp.replace(path)


def capture(repo: Path, manifest: Path) -> int:
    files: list[dict[str, int | str]] = []
    for rel_path, blob in tracked_entries(repo):
        full_path = repo / rel_path
        if not full_path.is_file():
            continue
        stat_result = full_path.stat()
        files.append(
            {
                "path": rel_path,
                "blob": blob,
                "mtime_ns": stat_result.st_mtime_ns,
            }
        )
    atomic_write(manifest, {"version": 1, "files": files})
    print(f"Captured tracked mtimes for {len(files)} files into {manifest}")
    return 0


def restore(repo: Path, manifest: Path) -> int:
    saved = load_manifest(manifest)
    if not saved:
        print(f"No tracked mtime manifest at {manifest}; skipping restore")
        return 0

    restored = 0
    skipped = 0
    for rel_path, blob in tracked_entries(repo):
        entry = saved.get(rel_path)
        if entry is None or entry["blob"] != blob:
            skipped += 1
            continue
        full_path = repo / rel_path
        if not full_path.is_file():
            skipped += 1
            continue
        stat_result = full_path.stat()
        mtime_ns = int(entry["mtime_ns"])
        os.utime(full_path, ns=(stat_result.st_atime_ns, mtime_ns))
        restored += 1

    print(
        f"Restored tracked mtimes for {restored} files from {manifest}"
        f" (skipped {skipped})"
    )
    return 0


def parse_args(argv: Iterable[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    for name in ("capture", "restore"):
        sub = subparsers.add_parser(name)
        sub.add_argument("--repo", required=True)
        sub.add_argument("--manifest", required=True)

    return parser.parse_args(list(argv))


def main(argv: Iterable[str]) -> int:
    args = parse_args(argv)
    repo = Path(args.repo).resolve()
    manifest = Path(args.manifest).resolve()

    if args.command == "capture":
        return capture(repo, manifest)
    if args.command == "restore":
        return restore(repo, manifest)

    raise AssertionError(f"unexpected command {args.command}")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
