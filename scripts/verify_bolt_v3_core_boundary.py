#!/usr/bin/env python3
"""Verify Bolt-v3 closed identity dispatch is out of core.

This gate covers the first core-boundary correction slice. It does not
claim provider adapter mapping, secret resolution, or NT factory registration
are fully provider-owned yet; those remain separate residuals.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

CHECKS = [
    (
        "src/bolt_v3_config.rs",
        [
            r"\benum\s+VenueKind\b",
            r"\benum\s+StrategyArchetype\b",
            r"\bVenueKind::",
            r"\bStrategyArchetype::",
        ],
    ),
    (
        "src/bolt_v3_providers/mod.rs",
        [
            r"\bmatch\s+venue\.kind\b",
            r"\bVenueKind\b",
        ],
    ),
    (
        "src/bolt_v3_archetypes/mod.rs",
        [
            r"\bmatch\s+strategy\.strategy_archetype\b",
            r"\bStrategyArchetype\b",
        ],
    ),
    (
        "src/bolt_v3_market_families/mod.rs",
        [
            r"\bRotatingMarketFamily\b",
            r"\bRotatingMarketFamily::",
        ],
    ),
]


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def main() -> int:
    findings: list[str] = []
    for rel, patterns in CHECKS:
        path = REPO_ROOT / rel
        text = path.read_text(encoding="utf-8")
        for pattern in patterns:
            regex = re.compile(pattern)
            for match in regex.finditer(text):
                findings.append(
                    f"{rel}:{line_number(text, match.start())}: forbidden pattern {pattern!r}"
                )

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 core boundary audit passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
