#!/usr/bin/env python3
"""Reject runtime fallback selection in venue economics quote adapters."""

from __future__ import annotations

import pathlib
import re
import sys

from rust_source_scanner import strip_rust_comments_and_literals


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
ADAPTER_PATHS = (
    pathlib.Path("src/bolt_v3_providers/polymarket/economics.rs"),
    pathlib.Path("src/bolt_v3_providers/hyperliquid/economics.rs"),
)
FORBIDDEN_PATTERNS = (
    (re.compile(r"\.(?:unwrap_or|unwrap_or_else|map_or|map_or_else|or_else)\s*\("), "conditional fallback primitive"),
    (re.compile(r"\bfn\s+effective_protocol_rate\b"), "runtime rate-selection function"),
    (
        re.compile(r"\bif\s+self\s*\.\s*product\s*\.\s*(?:stable_pair|growth_mode|hip3)\b"),
        "runtime product-modifier branch",
    ),
)


def line_number(source: str, offset: int) -> int:
    return source.count("\n", 0, offset) + 1


def verify(root: pathlib.Path = REPO_ROOT) -> list[str]:
    errors: list[str] = []
    for relative in ADAPTER_PATHS:
        path = root / relative
        if not path.is_file():
            errors.append(f"missing economics adapter: {relative}")
            continue
        scanned = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
        for pattern, reason in FORBIDDEN_PATTERNS:
            for match in pattern.finditer(scanned):
                errors.append(
                    f"{relative}:{line_number(scanned, match.start())}: {reason}: {match.group(0)}"
                )
    return errors


def main() -> int:
    errors = verify()
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: venue economics adapters use construction-time quote plans")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
