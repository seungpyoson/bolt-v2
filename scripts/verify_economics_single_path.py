#!/usr/bin/env python3
"""Reject runtime fallback selection in venue economics quote adapters."""

from __future__ import annotations

import pathlib
import re
import sys

from rust_source_scanner import strip_rust_comments_and_literals


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
ADAPTER_ROOT = pathlib.Path("src/bolt_v3_providers")
UNCHECKED_DECIMAL_ARITHMETIC = (
    re.compile(
        r"\b(?:core_total|forecast_total|gross_expected_value|core_net_edge|"
        r"forecast_net_edge|normalized_amount|protocol_basis|staking_scale|"
        r"scheduled|order_price|order_quantity|ceiling|total|fee|rate)\s*"
        r"(?:\+=|-=|\*=|/=|[+\-*/])"
    ),
    "unchecked Decimal arithmetic",
)
SEALED_CONSUMER_RULES = {
    pathlib.Path("src/bolt_v3_basket_admission.rs"): (
        (re.compile(r"\bscanner_evidence\s*\.\s*total_adjusted_cost\b"), "scanner economics used by basket admission"),
    ),
    pathlib.Path("src/bolt_v3_capital_admission.rs"): (
        (
            re.compile(
                r"\b(?:limit_price|effective_price|quantity)\s*(?:\.\s*checked_mul\s*\(|\*)|"
                r"\b(?:limit_price|effective_price|quantity)\s*\*"
            ),
            "capital admission re-derived price/quantity economics",
        ),
    ),
    pathlib.Path("src/economics/edge.rs"): (UNCHECKED_DECIMAL_ARITHMETIC,),
    pathlib.Path("src/economics/quote.rs"): (UNCHECKED_DECIMAL_ARITHMETIC,),
    pathlib.Path("src/economics/valuation.rs"): (UNCHECKED_DECIMAL_ARITHMETIC,),
    pathlib.Path("src/bolt_v3_providers/hyperliquid/economics.rs"): (
        UNCHECKED_DECIMAL_ARITHMETIC,
    ),
    pathlib.Path("src/bolt_v3_providers/polymarket/economics.rs"): (
        UNCHECKED_DECIMAL_ARITHMETIC,
    ),
    pathlib.Path("src/bolt_v3_submit_admission.rs"): (UNCHECKED_DECIMAL_ARITHMETIC,),
}
FORBIDDEN_PATTERNS = (
    (
        re.compile(
            r"\.(?:unwrap_or|unwrap_or_else|unwrap_or_default|map_or|map_or_else|or|or_else)\s*\("
        ),
        "conditional fallback primitive",
    ),
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
    adapter_root = root / ADAPTER_ROOT
    paths = (
        sorted(
            path
            for path in adapter_root.rglob("*.rs")
            if path.name == "economics.rs" or "economics" in path.relative_to(adapter_root).parts
        )
        if adapter_root.is_dir()
        else []
    )
    if not paths:
        return [f"no venue economics adapters found under {ADAPTER_ROOT}"]
    for path in paths:
        relative = path.relative_to(root)
        scanned = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
        for pattern, reason in FORBIDDEN_PATTERNS:
            for match in pattern.finditer(scanned):
                errors.append(
                    f"{relative}:{line_number(scanned, match.start())}: {reason}: {match.group(0)}"
                )
    for relative, rules in SEALED_CONSUMER_RULES.items():
        path = root / relative
        if not path.is_file():
            errors.append(f"missing sealed economics consumer {relative}")
            continue
        scanned = strip_rust_comments_and_literals(path.read_text(encoding="utf-8"))
        for pattern, reason in rules:
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
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
