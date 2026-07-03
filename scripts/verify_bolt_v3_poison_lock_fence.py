#!/usr/bin/env python3
"""Verify production `src/` code does not recover poisoned locks."""

from __future__ import annotations

import sys
from dataclasses import dataclass
from collections.abc import Iterable
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST: frozenset[str] = frozenset()
FORBIDDEN_NEEDLE = "poisoned.into_inner"


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    text: str


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    if is_allowed_path(path):
        return []
    violations: list[Violation] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        if FORBIDDEN_NEEDLE in line:
            violations.append(Violation(path=path, line=line_number, text=line.strip()))
    return violations


def is_allowed_path(path: str) -> bool:
    # Test code may intentionally recover a poisoned lock so a second panic does
    # not mask the original test failure. Production code must fail closed.
    return path in ALLOWLIST or path.startswith("src/") and "/tests/" in path


def src_rust_paths() -> tuple[str, ...]:
    src_root = REPO_ROOT / "src"
    if not src_root.is_dir():
        return ()
    return tuple(
        sorted(
            path.relative_to(REPO_ROOT).as_posix()
            for path in src_root.rglob("*.rs")
            if path.is_file()
        )
    )


def collect_violations(paths: Iterable[str] | None = None) -> list[Violation]:
    violations: list[Violation] = []
    for relative_path in paths or src_rust_paths():
        path = REPO_ROOT / relative_path
        if not path.is_file():
            violations.append(
                Violation(path=relative_path, line=0, text="missing expected source file")
            )
            continue
        violations.extend(
            find_violations_in_text(relative_path, path.read_text(encoding="utf-8"))
        )
    return violations


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: poison-lock recovery banned at "
                f"{violation.path}:{violation.line}: {violation.text}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 poison-lock fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
