#!/usr/bin/env python3
"""Verify RA-014/RA-015 BI surface and binding-key join contracts."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from justfile_recipe_checks import missing_recipe_commands


REPO_ROOT = Path(__file__).resolve().parent.parent
HELPER_PATH = Path("crates/backtesting-vertical-slice/src/research_reader.rs")
TEST_PATH = Path("crates/backtesting-vertical-slice/tests/research_reader_contract.rs")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")
JUSTFILE_PATH = Path("justfile")

CHECKED_RA014 = re.compile(r"^- \[[xX]\] RA-014\b", re.MULTILINE)
CHECKED_RA015 = re.compile(r"^- \[[xX]\] RA-015\b", re.MULTILINE)


def strip_rust_comments_and_literals(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    block_depth = 0
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
                out.extend('""')
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
            if c == "\\":
                i += 2
                continue
            if c == '"':
                state = "code"
            i += 1
            continue
    return "".join(out)


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


def require_pattern(path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if not re.search(pattern, text, re.DOTALL):
        findings.append(f"{path}: missing real {label}")


def require_patterns(path: Path, text: str, patterns: tuple[tuple[str, str], ...], findings: list[str]) -> None:
    for label, pattern in patterns:
        require_pattern(path, text, label, pattern, findings)


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    helper_text = (root / HELPER_PATH).read_text(encoding="utf-8") if (root / HELPER_PATH).exists() else ""
    test_text = (root / TEST_PATH).read_text(encoding="utf-8") if (root / TEST_PATH).exists() else ""
    tasks_text = (root / TASKS_PATH).read_text(encoding="utf-8") if (root / TASKS_PATH).exists() else ""
    just_text = (root / JUSTFILE_PATH).read_text(encoding="utf-8") if (root / JUSTFILE_PATH).exists() else ""

    helper_code = strip_rust_comments_and_literals(helper_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA014.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-014 must be checked once BI surface contract is implemented")
    if not CHECKED_RA015.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-015 must be checked once binding-key join contract is implemented")

    for label, pattern in (
        ("NotebookBiSurfaceSpec", r"\bpub\s+struct\s+NotebookBiSurfaceSpec\b"),
        ("NotebookBiSurface", r"\bpub\s+struct\s+NotebookBiSurface\b"),
        ("NotebookQueryEngine", r"\bpub\s+struct\s+NotebookQueryEngine\b"),
        ("NotebookErgonomics", r"\bpub\s+struct\s+NotebookErgonomics\b"),
        ("CustomUiDecision", r"\bpub\s+enum\s+CustomUiDecision\b"),
        ("build_notebook_bi_surface", r"\bpub\s+fn\s+build_notebook_bi_surface\b"),
        ("AnalyticsSourceBinding", r"\bpub\s+struct\s+AnalyticsSourceBinding\b"),
        ("FeatureJoinSpec", r"\bpub\s+struct\s+FeatureJoinSpec\b"),
        ("validate_feature_join_bindings", r"\bpub\s+fn\s+validate_feature_join_bindings\b"),
    ):
        require_pattern(HELPER_PATH, helper_code, label, pattern, findings)

    bi_spec = braced_body_after(helper_code, r"\bpub\s+struct\s+NotebookBiSurfaceSpec\b") or ""
    require_patterns(
        HELPER_PATH,
        bi_spec,
        (
            ("artifact_root field", r"\bartifact_root\s*:\s*String\b"),
            ("NT catalog Arrow URI field", r"\bnt_catalog_arrow_uri\s*:\s*String\b"),
            ("query engines field", r"\bquery_engines\s*:\s*Vec\s*<\s*NotebookQueryEngine\s*>\s*"),
            ("dashboard product refs field", r"\bdashboard_product_refs\s*:\s*Vec\s*<\s*String\s*>\s*"),
            ("notebook ergonomics field", r"\bnotebook\s*:\s*NotebookErgonomics\b"),
            ("custom UI decision field", r"\bcustom_ui\s*:\s*CustomUiDecision\b"),
        ),
        findings,
    )

    bi_builder = braced_body_after(helper_code, r"\bpub\s+fn\s+build_notebook_bi_surface\b") or ""
    require_patterns(
        HELPER_PATH,
        bi_builder,
        (
            ("notebook ergonomics validation delegate", r"\bvalidate_notebook_ergonomics\b"),
            ("custom UI decision validation delegate", r"\bvalidate_custom_ui_decision\b"),
            ("artifact-root containment validation", r"\bensure_uri_under_artifact_root\b"),
        ),
        findings,
    )
    require_patterns(
        HELPER_PATH,
        helper_code,
        (
            ("read-only notebook validation", r"\bread_only\b"),
            ("mutation action rejection", r"\bmutation_actions_enabled\b"),
            ("Arrow batch ergonomics validation", r"\bexposes_arrow_batches\b"),
            ("SQL example ergonomics validation", r"\bexposes_sql_examples\b"),
        ),
        findings,
    )
    custom_validator = braced_body_after(helper_code, r"\bfn\s+validate_custom_ui_decision\b") or ""
    require_pattern(
        HELPER_PATH,
        custom_validator,
        "custom UI product-gate validation",
        r"\bAllowedAfterProductGate\b.*\bconfirmed_requirement_refs\b.*\brejected_product_refs\b",
        findings,
    )

    join_validator = braced_body_after(helper_code, r"\bpub\s+fn\s+validate_feature_join_bindings\b") or ""
    require_patterns(
        HELPER_PATH,
        join_validator,
        (
            ("source binding key set", r"\bsource_binding_key\b"),
            ("venue identity rejection context", r"\bvenue_key\b"),
            ("provider identity rejection context", r"\bprovider_key\b"),
            ("left binding key", r"\bleft_source_binding_key\b"),
            ("right binding key", r"\bright_source_binding_key\b"),
            ("as-of column validation", r"\bas_of_column\b"),
            ("freshness column validation", r"\bfreshness_column\b"),
        ),
        findings,
    )

    for forbidden in ("BacktestNode", "run_operator_from_run_spec", "nautilus_trader.backtest.engine"):
        if forbidden in helper_code:
            findings.append(f"{HELPER_PATH}: BI surface must not use {forbidden}")

    for label, pattern in (
        (
            "DuckDB/Polars BI contract test",
            r"\bnotebook_bi_surface_exposes_duckdb_and_polars_over_nt_catalog_arrow_without_custom_ui\b",
        ),
        ("DuckDB metadata test value", r'"duckdb"'),
        ("Polars metadata test value", r'"polars"'),
        (
            "custom UI gate test",
            r"\bnotebook_bi_surface_requires_product_gate_before_custom_ui\b",
        ),
        (
            "binding-key join test",
            r"\banalytics_feature_joins_use_source_binding_keys_not_venue_or_provider_literals\b",
        ),
        ("source binding key negative assertion", r"source_binding_key"),
    ):
        require_pattern(TEST_PATH, test_text if '"' in pattern else test_code, label, pattern, findings)

    for command in missing_recipe_commands(
        just_text,
        (
            "python3 scripts/test_verify_ra_bi_surface_and_feature_joins.py",
            "python3 scripts/verify_ra_bi_surface_and_feature_joins.py",
        ),
    ):
        findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()
    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA BI surface / feature join violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA BI surface / feature joins passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
