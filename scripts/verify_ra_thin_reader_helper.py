#!/usr/bin/env python3
"""Verify the RA reader helper remains a thin NautilusTrader delegate."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
HELPER_PATH = Path("crates/backtesting-vertical-slice/src/research_reader.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")

REQUIRED_SNIPPETS = (
    "pub struct CatalogQuerySpec",
    "pub struct SqlBatchQuerySpec",
    "pub fn query_catalog_typed",
    "pub fn query_sql_arrow_batches",
    "ParquetDataCatalog::new",
    "query_typed_data::<T>",
    "DataBackendSession::new",
    "collect_query_batches",
)
FORBIDDEN_SNIPPETS = (
    "BacktestNode",
    "nautilus_backtest",
    "duckdb",
    "polars",
    "read_dir",
)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    helper = root / HELPER_PATH
    if not helper.exists():
        findings.append(f"{HELPER_PATH}: research_reader.rs is missing")
        return findings

    text = helper.read_text(encoding="utf-8")
    for snippet in REQUIRED_SNIPPETS:
        if snippet not in text:
            findings.append(f"{HELPER_PATH}: missing thin-reader delegate `{snippet}`")
    for snippet in FORBIDDEN_SNIPPETS:
        if snippet in text:
            findings.append(f"{HELPER_PATH}: RA reader helper must not reference {snippet}")

    lib = root / LIB_PATH
    if not lib.exists():
        findings.append(f"{LIB_PATH}: lib.rs is missing")
    elif "pub mod research_reader;" not in lib.read_text(encoding="utf-8"):
        findings.append(f"{LIB_PATH}: missing public research_reader module export")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA thin reader helper violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA thin reader helper passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
