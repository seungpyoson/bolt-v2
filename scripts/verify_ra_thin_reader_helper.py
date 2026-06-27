#!/usr/bin/env python3
"""Verify the RA reader helper remains a thin NautilusTrader delegate."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
HELPER_PATH = Path("crates/backtesting-vertical-slice/src/research_reader.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")

CATALOG_SPEC_PATTERNS = (
    ("catalog URI", r"\bcatalog_uri\s*:\s*String\b"),
    ("storage options", r"\bstorage_options\s*:\s*Option\s*<\s*AHashMap\s*<\s*String\s*,\s*String\s*>\s*>\s*"),
    ("instrument filter", r"\binstrument_ids\s*:\s*Option\s*<\s*Vec\s*<\s*String\s*>\s*>\s*"),
    ("start time filter", r"\bstart\s*:\s*Option\s*<\s*UnixNanos\s*>\s*"),
    ("end time filter", r"\bend\s*:\s*Option\s*<\s*UnixNanos\s*>\s*"),
    ("SQL where pushdown", r"\bwhere_clause\s*:\s*Option\s*<\s*String\s*>\s*"),
    ("file filter", r"\bfiles\s*:\s*Option\s*<\s*Vec\s*<\s*String\s*>\s*>\s*"),
    ("NT file loading optimization passthrough", r"\boptimize_file_loading\s*:\s*bool\b"),
)
CATALOG_QUERY_PATTERNS = (
    ("NT catalog URI constructor", r"\bParquetDataCatalog\s*::\s*from_uri\s*\("),
    ("TOML/storage option passthrough", r"\bspec\s*\.\s*storage_options\s*\.\s*clone\s*\(\s*\)"),
    ("NT typed query", r"\bquery_typed_data\s*::\s*<\s*T\s*>\s*\("),
    ("instrument filter passthrough", r"\bspec\s*\.\s*instrument_ids\s*\.\s*clone\s*\(\s*\)"),
    ("start time passthrough", r"\bspec\s*\.\s*start\b"),
    ("end time passthrough", r"\bspec\s*\.\s*end\b"),
    ("SQL where passthrough", r"\bspec\s*\.\s*where_clause\s*\.\s*as_deref\s*\(\s*\)"),
    ("file filter passthrough", r"\bspec\s*\.\s*files\s*\.\s*clone\s*\(\s*\)"),
    ("NT file loading optimization passthrough", r"\bspec\s*\.\s*optimize_file_loading\b"),
)
SQL_SPEC_PATTERNS = (
    ("table name", r"\btable_name\s*:\s*String\b"),
    ("file path", r"\bfile_path\s*:\s*PathBuf\b"),
    ("SQL query", r"\bsql\s*:\s*Option\s*<\s*String\s*>\s*"),
    ("chunk size", r"\bchunk_size\s*:\s*usize\b"),
)
SQL_QUERY_PATTERNS = (
    ("UTF-8 file path conversion", r"\bspec\s*\.\s*file_path\s*\.\s*to_str\s*\(\s*\)"),
    ("NT DataBackendSession", r"\bDataBackendSession\s*::\s*new\s*\(\s*spec\s*\.\s*chunk_size\s*\)"),
    ("NT Arrow batch collection", r"\bcollect_query_batches\s*\("),
    ("table name passthrough", r"&\s*spec\s*\.\s*table_name\b"),
    ("SQL passthrough", r"\bspec\s*\.\s*sql\s*\.\s*as_deref\s*\(\s*\)"),
)
FORBIDDEN_SNIPPETS = (
    "BacktestNode",
    "nautilus_backtest",
    "duckdb",
    "polars",
    "read_dir",
)


def strip_rust_comments(text: str) -> str:
    out: list[str] = []
    i = 0
    block_depth = 0
    state = "code"
    while i < len(text):
        c = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if c == "/" and nxt == "/":
                state = "line_comment"
                out.extend("  ")
                i += 2
                continue
            if c == "/" and nxt == "*":
                state = "block_comment"
                block_depth = 1
                out.extend("  ")
                i += 2
                continue
            if c == '"':
                state = "string"
                out.append(c)
                i += 1
                continue
            out.append(c)
            i += 1
            continue
        if state == "line_comment":
            if c == "\n":
                state = "code"
                out.append(c)
            else:
                out.append(" ")
            i += 1
            continue
        if state == "block_comment":
            if c == "/" and nxt == "*":
                block_depth += 1
                out.extend("  ")
                i += 2
                continue
            if c == "*" and nxt == "/":
                block_depth -= 1
                out.extend("  ")
                i += 2
                if block_depth == 0:
                    state = "code"
                continue
            out.append("\n" if c == "\n" else " ")
            i += 1
            continue
        if state == "string":
            out.append(c)
            if c == "\\":
                if i + 1 < len(text):
                    out.append(text[i + 1])
                    i += 2
                else:
                    i += 1
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def strip_rust_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    while i < len(text):
        c = text[i]
        if state == "code":
            if c == '"':
                state = "string"
                out.extend('""')
                i += 1
                continue
            out.append(c)
            i += 1
            continue
        if state == "string":
            if c == "\\":
                i += 2
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


def rust_code_only(text: str) -> str:
    return strip_rust_literals(strip_rust_comments(text))


def braced_body_after(text: str, pattern: str) -> str | None:
    match = re.search(pattern, text, re.DOTALL)
    if match is None:
        return None
    open_brace = text.find("{", match.end())
    if open_brace == -1:
        return None
    depth = 0
    for index in range(open_brace, len(text)):
        char = text[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace + 1 : index]
    return None


def require_patterns(rel_path: Path, text: str, patterns: tuple[tuple[str, str], ...], findings: list[str]) -> None:
    for label, pattern in patterns:
        if not re.search(pattern, text, re.DOTALL):
            findings.append(f"{rel_path}: missing real {label}")


def verify_helper_shape(text: str, findings: list[str]) -> None:
    code = rust_code_only(text)
    for snippet in FORBIDDEN_SNIPPETS:
        if re.search(rf"\b{re.escape(snippet)}\b", code):
            findings.append(f"{HELPER_PATH}: RA reader helper must not reference {snippet}")

    catalog_spec = braced_body_after(code, r"pub\s+struct\s+CatalogQuerySpec\b")
    if catalog_spec is None:
        findings.append(f"{HELPER_PATH}: missing real CatalogQuerySpec")
    else:
        require_patterns(HELPER_PATH, catalog_spec, CATALOG_SPEC_PATTERNS, findings)

    catalog_query = braced_body_after(code, r"pub\s+fn\s+query_catalog_typed\b")
    if catalog_query is None:
        findings.append(f"{HELPER_PATH}: missing real query_catalog_typed")
    else:
        require_patterns(HELPER_PATH, catalog_query, CATALOG_QUERY_PATTERNS, findings)

    sql_spec = braced_body_after(code, r"pub\s+struct\s+SqlBatchQuerySpec\b")
    if sql_spec is None:
        findings.append(f"{HELPER_PATH}: missing real SqlBatchQuerySpec")
    else:
        require_patterns(HELPER_PATH, sql_spec, SQL_SPEC_PATTERNS, findings)

    sql_query = braced_body_after(code, r"pub\s+fn\s+query_sql_arrow_batches\b")
    if sql_query is None:
        findings.append(f"{HELPER_PATH}: missing real query_sql_arrow_batches")
    else:
        require_patterns(HELPER_PATH, sql_query, SQL_QUERY_PATTERNS, findings)


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    helper = root / HELPER_PATH
    if not helper.exists():
        findings.append(f"{HELPER_PATH}: research_reader.rs is missing")
        return findings

    text = helper.read_text(encoding="utf-8")
    verify_helper_shape(text, findings)

    lib = root / LIB_PATH
    if not lib.exists():
        findings.append(f"{LIB_PATH}: lib.rs is missing")
    elif "pub mod research_reader;" not in rust_code_only(lib.read_text(encoding="utf-8")):
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
