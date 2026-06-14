#!/usr/bin/env python3
"""Verify RA-007 lifts lead-lag strategy-fidelity reads onto the NT catalog."""

from __future__ import annotations

import argparse
import ast
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
READER_PATH = Path("crates/backtesting-vertical-slice/src/leadlag_catalog_reader.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")
SESSION_PATH = Path("scripts/leadlag_session4.py")
CLOCK_PATH = Path("scripts/leadlag_clock_alignment.py")
SUBSECOND_PATH = Path("scripts/leadlag_subsecond.py")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")
CHECKED_RA007 = re.compile(r"^- \[[xX]\] RA-007\b", re.MULTILINE)

CATALOG_CONFIG_PATTERNS = (
    ("catalog URI", r"\bcatalog_uri\s*:\s*String\b"),
    ("storage options", r"\bstorage_options\s*:\s*Option\s*<\s*AHashMap\s*<\s*String\s*,\s*String\s*>\s*>\s*"),
    ("instrument IDs", r"\binstrument_ids\s*:\s*Vec\s*<\s*String\s*>\s*"),
    ("start filter", r"\bstart\s*:\s*Option\s*<\s*UnixNanos\s*>\s*"),
    ("end filter", r"\bend\s*:\s*Option\s*<\s*UnixNanos\s*>\s*"),
    ("where pushdown", r"\bwhere_clause\s*:\s*Option\s*<\s*String\s*>\s*"),
    ("file filter", r"\bfiles\s*:\s*Option\s*<\s*Vec\s*<\s*String\s*>\s*>\s*"),
    ("optimize_file_loading", r"\boptimize_file_loading\s*:\s*bool\b"),
    ("book type", r"\bbook_type\s*:\s*String\b|\bbook_type\s*:\s*LeadLagCatalogBookType\b"),
    ("clock selector", r"\bclock\s*:\s*String\b|\bclock\s*:\s*LeadLagCatalogClock\b"),
    ("instrument aliases", r"\binstrument_aliases\s*:\s*Vec\s*<\s*LeadLagInstrumentAlias\s*>\s*"),
)
ALIAS_PATTERNS = (
    ("instrument alias id", r"\binstrument_id\s*:\s*String\b"),
    ("instrument alias asset", r"\basset_id\s*:\s*String\b"),
)
TOP_OF_BOOK_PATTERNS = (
    ("CatalogQuerySpec construction", r"\bCatalogQuerySpec\s*\{|\bquery_spec\s*\("),
    ("OrderBookDelta catalog query", r"\bquery_catalog_typed\s*::\s*<\s*OrderBookDelta\s*>\s*\("),
    ("NT order book quote projection", r"\bOrderBook\s*::\s*deltas_to_quotes\s*\("),
)
TRADES_PATTERNS = (
    ("CatalogQuerySpec construction", r"\bCatalogQuerySpec\s*\{|\bquery_spec\s*\("),
    ("TradeTick catalog query", r"\bquery_catalog_typed\s*::\s*<\s*TradeTick\s*>\s*\("),
)
FORBIDDEN_READER_SNIPPETS = (
    "duckdb",
    "polars",
    "read_parquet",
    "aws",
    "s3 cp",
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


def verify_rust_reader(text: str, findings: list[str]) -> None:
    code = rust_code_only(text)
    for snippet in FORBIDDEN_READER_SNIPPETS:
        if re.search(rf"\b{re.escape(snippet)}\b", code):
            findings.append(f"{READER_PATH}: catalog reader must not shell/read raw archive via {snippet}")
    for label, pattern in (
        ("OrderBookDelta catalog query", r"\bquery_catalog_typed\s*::\s*<\s*OrderBookDelta\s*>\s*\("),
        ("TradeTick catalog query", r"\bquery_catalog_typed\s*::\s*<\s*TradeTick\s*>\s*\("),
        ("NT order book quote projection", r"\bOrderBook\s*::\s*deltas_to_quotes\s*\("),
    ):
        if not re.search(pattern, code, re.DOTALL):
            findings.append(f"{READER_PATH}: missing real {label}")

    config = braced_body_after(code, r"pub\s+struct\s+LeadLagCatalogReadConfig\b")
    if config is None:
        findings.append(f"{READER_PATH}: missing real LeadLagCatalogReadConfig")
    else:
        require_patterns(READER_PATH, config, CATALOG_CONFIG_PATTERNS, findings)

    alias = braced_body_after(code, r"pub\s+struct\s+LeadLagInstrumentAlias\b")
    if alias is None:
        findings.append(f"{READER_PATH}: missing real LeadLagInstrumentAlias")
    else:
        require_patterns(READER_PATH, alias, ALIAS_PATTERNS, findings)

    top_of_book = braced_body_after(code, r"pub\s+fn\s+read_leadlag_top_of_book_from_catalog\b")
    if top_of_book is None:
        findings.append(f"{READER_PATH}: missing real read_leadlag_top_of_book_from_catalog")
    else:
        require_patterns(READER_PATH, top_of_book, TOP_OF_BOOK_PATTERNS, findings)

    trades = braced_body_after(code, r"pub\s+fn\s+read_leadlag_trades_from_catalog\b")
    if trades is None:
        findings.append(f"{READER_PATH}: missing real read_leadlag_trades_from_catalog")
    else:
        require_patterns(READER_PATH, trades, TRADES_PATTERNS, findings)


def parse_python(rel_path: Path, text: str, findings: list[str]) -> ast.Module | None:
    try:
        return ast.parse(text, filename=str(rel_path))
    except SyntaxError as exc:
        findings.append(f"{rel_path}: Python syntax error: {exc}")
        return None


def function_defs(tree: ast.AST) -> dict[str, ast.FunctionDef | ast.AsyncFunctionDef]:
    return {node.name: node for node in ast.walk(tree) if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))}


def call_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = call_name(node.value)
        return f"{base}.{node.attr}" if base else node.attr
    return None


def function_calls(function: ast.AST) -> set[str]:
    calls: set[str] = set()
    for node in ast.walk(function):
        if isinstance(node, ast.Call):
            name = call_name(node.func)
            if name:
                calls.add(name)
    return calls


def verify_python_session(text: str, findings: list[str]) -> None:
    tree = parse_python(SESSION_PATH, text, findings)
    if tree is None:
        return
    funcs = function_defs(tree)
    for name in (
        "load_pm_catalog_extract_config",
        "run_leadlag_catalog_extract",
        "write_catalog_extract_frames",
        "cmd_extract_pm_catalog",
    ):
        if name not in funcs:
            findings.append(f"{SESSION_PATH}: missing real {name}")

    loader = funcs.get("load_pm_catalog_extract_config")
    if loader and not {"tomllib.load", "tomllib.loads"} & function_calls(loader):
        findings.append(f"{SESSION_PATH}: catalog extract config must be read from TOML via tomllib")

    runner = funcs.get("run_leadlag_catalog_extract")
    if runner and "subprocess.run" not in function_calls(runner):
        findings.append(f"{SESSION_PATH}: catalog extractor must delegate to the Rust catalog reader process")

    command_names = {
        node.value
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant) and isinstance(node.value, str)
    }
    if "extract-pm-catalog" not in command_names:
        findings.append(f"{SESSION_PATH}: missing extract-pm-catalog command")

    lower = text.lower()
    if "raw fallback" not in lower or "#677" not in text or "ts_init = capture_time" not in text:
        findings.append(f"{SESSION_PATH}: missing raw fallback sunset tied to #677 and ts_init = capture_time")


def verify_latency_carveout(rel_path: Path, text: str, findings: list[str]) -> None:
    lower = text.lower()
    if "#677" not in text or "ts_init = capture_time" not in text or "receive-offset" not in lower:
        findings.append(f"{rel_path}: raw receive-offset carve-out must name #677 and ts_init = capture_time")


def verify_justfile(text: str, findings: list[str]) -> None:
    required = (
        "python3 scripts/test_verify_ra_leadlag_catalog_lift.py",
        "python3 scripts/verify_ra_leadlag_catalog_lift.py",
    )
    for command in required:
        if command not in text:
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    tasks = root / TASKS_PATH
    if not tasks.exists():
        findings.append(f"{TASKS_PATH}: tasks.md is missing")
    elif not CHECKED_RA007.search(tasks.read_text(encoding="utf-8")):
        findings.append(f"{TASKS_PATH}: RA-007 must be checked once the lead-lag catalog lift is implemented")

    reader = root / READER_PATH
    if not reader.exists():
        findings.append(f"{READER_PATH}: leadlag_catalog_reader.rs is missing")
    else:
        verify_rust_reader(reader.read_text(encoding="utf-8"), findings)

    lib = root / LIB_PATH
    if not lib.exists():
        findings.append(f"{LIB_PATH}: lib.rs is missing")
    elif "pub mod leadlag_catalog_reader;" not in rust_code_only(lib.read_text(encoding="utf-8")):
        findings.append(f"{LIB_PATH}: missing public leadlag_catalog_reader module export")

    session = root / SESSION_PATH
    if not session.exists():
        findings.append(f"{SESSION_PATH}: leadlag_session4.py is missing")
    else:
        verify_python_session(session.read_text(encoding="utf-8"), findings)

    for rel_path in (CLOCK_PATH, SUBSECOND_PATH):
        path = root / rel_path
        if not path.exists():
            findings.append(f"{rel_path}: latency carve-out script is missing")
        else:
            verify_latency_carveout(rel_path, path.read_text(encoding="utf-8"), findings)

    justfile = root / JUSTFILE_PATH
    if not justfile.exists():
        findings.append(f"{JUSTFILE_PATH}: justfile is missing")
    else:
        verify_justfile(justfile.read_text(encoding="utf-8"), findings)

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA lead-lag catalog-lift violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA lead-lag catalog lift passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
