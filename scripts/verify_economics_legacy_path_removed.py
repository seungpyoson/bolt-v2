#!/usr/bin/env python3
"""Reject the retired scalar-fee and provider-selected economics path."""

from __future__ import annotations

import pathlib
import re
import sys

from rust_source_scanner import strip_rust_comments_and_literals


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RUST_ROOTS = (
    pathlib.Path("src"),
    pathlib.Path("tests"),
    pathlib.Path("crates/backtesting-vertical-slice/src"),
    pathlib.Path("crates/backtesting-vertical-slice/tests"),
)
FORBIDDEN_IDENTIFIERS = (
    "FeeProvider",
    "build_fee_provider",
    "resolve_fee_provider",
    "maker_binary_fee_curve",
    "ManifestFeeProvider",
    "PARAM_FEE_BPS",
    "STRATEGY_PARAM_FEE_BPS",
    "fee_inclusive_admission_notional",
    "checked_fee_inclusive_admission_notional",
    "max_fee_bps_for_price",
)
FORBIDDEN_PATTERN = re.compile(
    r"\b(?:" + "|".join(re.escape(name) for name in FORBIDDEN_IDENTIFIERS) + r")\b"
)
STRATEGY_FORBIDDEN_PATTERNS = (
    re.compile(r"\bhistorical_entry_fee_bps\b"),
    re.compile(r"\bfee_cost_cents\b"),
    re.compile(r"\brefresh_fee_readiness\b"),
    re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*fee_bps\b|\bfee_bps\b"),
)


def line_number(source: str, offset: int) -> int:
    return source.count("\n", 0, offset) + 1


def rust_paths(root: pathlib.Path) -> list[pathlib.Path]:
    paths: set[pathlib.Path] = set()
    for relative_root in RUST_ROOTS:
        search_root = root / relative_root
        if search_root.is_dir():
            paths.update(search_root.rglob("*.rs"))
    return sorted(paths)


def verify(root: pathlib.Path = REPO_ROOT) -> list[str]:
    errors: list[str] = []
    for path in rust_paths(root):
        relative = path.relative_to(root)
        scanned = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
        for match in FORBIDDEN_PATTERN.finditer(scanned):
            errors.append(
                f"{relative}:{line_number(scanned, match.start())}: "
                f"retired economics path: {match.group(0)}"
            )
        if relative.parts[:2] == ("src", "strategies"):
            for pattern in STRATEGY_FORBIDDEN_PATTERNS:
                for match in pattern.finditer(scanned):
                    errors.append(
                        f"{relative}:{line_number(scanned, match.start())}: "
                        f"strategy-owned economics: {match.group(0)}"
                    )
    return errors


def main() -> int:
    errors = verify()
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: retired scalar-fee economics path is absent")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
