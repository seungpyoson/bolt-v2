#!/usr/bin/env python3
"""Keep the shared economics domain independent of venues and runtimes."""

from __future__ import annotations

import pathlib
import re
import sys

from rust_source_scanner import retain_rust_string_literals, strip_rust_comments_and_literals


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
FORBIDDEN_SOURCE_PATTERNS = (
    (re.compile(r"\bnautilus_[A-Za-z0-9_]*\b"), "execution-substrate dependency"),
    (re.compile(r"\bbolt_v3_[A-Za-z0-9_]*\b"), "Bolt runtime dependency"),
    (
        re.compile(r"\b(?:polymarket|hyperliquid|binance|bybit|deribit|kraken|coinbase|bitmex|okx)\b", re.IGNORECASE),
        "venue-specific dependency",
    ),
    (
        re.compile(r"\b(?:reqwest|tokio|alloy|aws_sdk[A-Za-z0-9_]*)\b"),
        "transport or infrastructure dependency",
    ),
    (
        re.compile(r"\bstd\s*::\s*(?:fs|net|path|process|time)\b"),
        "standard-library infrastructure dependency",
    ),
    (
        re.compile(r"\bcrate\s*::\s*(?!economics\b)[A-Za-z_][A-Za-z0-9_]*"),
        "dependency outside the shared economics domain",
    ),
    (
        re.compile(r"\bsuper\s*::\s*super\s*::\s*[A-Za-z_][A-Za-z0-9_]*"),
        "dependency outside the shared economics domain",
    ),
    (
        re.compile(r"::\s*bolt_v2\s*::"),
        "absolute dependency on the Bolt runtime crate",
    ),
)
ESTIMATE_TO_ACTUAL_PATTERNS = (
    re.compile(
        r"\bFrom\s*<\s*EstimatedEconomicComponent\s*>\s*for\s*ActualEconomicEntry\b"
    ),
    re.compile(
        r"\bInto\s*<\s*ActualEconomicEntry\s*>\s*for\s*EstimatedEconomicComponent\b"
    ),
)
VENUE_LITERAL_PATTERN = re.compile(
    r"\b(?:polymarket|hyperliquid|binance|bybit|deribit|kraken|coinbase|bitmex|okx)\b",
    re.IGNORECASE,
)
ALLOWED_NT_CURRENCY_IMPORT = re.compile(
    r"\b(?:pub\s+)?use\s+nautilus_model\s*::\s*types\s*::\s*Currency\s*;"
)


def line_number(source: str, offset: int) -> int:
    return source.count("\n", 0, offset) + 1


def verify(root: pathlib.Path = REPO_ROOT) -> list[str]:
    economics_root = root / "src" / "economics"
    files = sorted(economics_root.rglob("*.rs")) if economics_root.is_dir() else []
    if not files:
        return ["src/economics contains no Rust sources"]

    errors: list[str] = []
    for path in files:
        source = path.read_text(encoding="utf-8")
        scanned = strip_rust_comments_and_literals(source)
        scanned = ALLOWED_NT_CURRENCY_IMPORT.sub("", scanned)
        string_literals = retain_rust_string_literals(source)
        relative = path.relative_to(root)
        for pattern, reason in FORBIDDEN_SOURCE_PATTERNS:
            for match in pattern.finditer(scanned):
                errors.append(
                    f"{relative}:{line_number(scanned, match.start())}: {reason}: {match.group(0)}"
                )
        for pattern in ESTIMATE_TO_ACTUAL_PATTERNS:
            for match in pattern.finditer(scanned):
                errors.append(
                    f"{relative}:{line_number(scanned, match.start())}: "
                    "estimate-to-actual conversion is forbidden"
                )
        for match in VENUE_LITERAL_PATTERN.finditer(string_literals):
            errors.append(
                f"{relative}:{line_number(string_literals, match.start())}: "
                f"venue-specific runtime literal: {match.group(0)}"
            )
    return errors


def main() -> int:
    errors = verify()
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: economics dependency-direction verifier passed")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
