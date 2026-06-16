#!/usr/bin/env python3
"""Verify market-family implementations do not depend on sibling family modules."""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import (
    production_text,
    strip_rust_comments_and_literals,
)


FORBIDDEN_STATIC_BINARY_EVENT_DEPENDENCY = "updown"
STATIC_BINARY_EVENT_PATH = "src/bolt_v3_market_families/static_binary_event.rs"
FORBIDDEN_STATIC_BINARY_EVENT_PATTERN = re.compile(
    rf"\b{re.escape(FORBIDDEN_STATIC_BINARY_EVENT_DEPENDENCY)}\b"
)


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    excerpt: str


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def _excerpt(text: str, start: int, end: int) -> str:
    line_start = text.rfind("\n", 0, start) + 1
    line_end = text.find("\n", end)
    if line_end == -1:
        line_end = len(text)
    return text[line_start:line_end].strip()


def find_static_binary_event_violations_in_text(text: str) -> list[Violation]:
    scan_text = strip_rust_comments_and_literals(text)
    violations: list[Violation] = []
    for match in FORBIDDEN_STATIC_BINARY_EVENT_PATTERN.finditer(scan_text):
        violations.append(
            Violation(
                path=STATIC_BINARY_EVENT_PATH,
                line=line_number(scan_text, match.start()),
                excerpt=_excerpt(text, match.start(), match.end()),
            )
        )
    return violations


def collect_violations() -> list[Violation]:
    path = REPO_ROOT / STATIC_BINARY_EVENT_PATH
    if not path.is_file():
        raise RuntimeError(f"static binary-event family module missing: {STATIC_BINARY_EVENT_PATH}")
    return find_static_binary_event_violations_in_text(production_text(path))


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 market-family coupling fence: "
                f"{violation.path}:{violation.line} depends on sibling family "
                f"`{FORBIDDEN_STATIC_BINARY_EVENT_DEPENDENCY}`; move shared binary-outcome maker "
                f"primitives into a neutral module and call that instead: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1

    print("OK: Bolt-v3 market-family coupling fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
