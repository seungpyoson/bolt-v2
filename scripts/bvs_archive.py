#!/usr/bin/env python3
"""Build the BVS nextest archive with every discovered test-bearing target."""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

from rust_test_targets import archive_args


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--owner", required=True, type=pathlib.Path)
    parser.add_argument("--repo", required=True, type=pathlib.Path)
    parser.add_argument("--archive", required=True, type=pathlib.Path)
    args = parser.parse_args(argv)
    repo = args.repo.resolve()
    archive = args.archive.resolve()
    archive.parent.mkdir(parents=True, exist_ok=True)
    command = (
        "python3",
        str(args.owner.resolve()),
        "cargo",
        "--repo",
        str(repo),
        "--",
        "nextest",
        "archive",
        "--locked",
        "--archive-file",
        str(archive),
        *archive_args(repo),
    )
    return subprocess.run(command, check=False, close_fds=True).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
