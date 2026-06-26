#!/usr/bin/env python3
"""Verify RA-008 sweep orchestration stays on the existing BTE path."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from justfile_recipe_checks import missing_recipe_commands


REPO_ROOT = Path(__file__).resolve().parent.parent
RA_PATH = Path("crates/backtesting-vertical-slice/src/research_analytics.rs")
OPERATOR_PATH = Path("crates/backtesting-vertical-slice/src/operator.rs")
TEST_PATH = Path("crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_analytics.rs")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA008 = re.compile(r"^- \[[xX]\] RA-008\b", re.MULTILINE)


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


def require_pattern(rel_path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if not re.search(pattern, text, re.DOTALL):
        findings.append(f"{rel_path}: missing real {label}")


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    ra_text = (root / RA_PATH).read_text(encoding="utf-8") if (root / RA_PATH).exists() else ""
    operator_text = (
        (root / OPERATOR_PATH).read_text(encoding="utf-8") if (root / OPERATOR_PATH).exists() else ""
    )
    test_text = (root / TEST_PATH).read_text(encoding="utf-8") if (root / TEST_PATH).exists() else ""
    just_text = (
        (root / JUSTFILE_PATH).read_text(encoding="utf-8") if (root / JUSTFILE_PATH).exists() else ""
    )
    tasks_text = (
        (root / TASKS_PATH).read_text(encoding="utf-8") if (root / TASKS_PATH).exists() else ""
    )

    ra_code = strip_rust_comments_and_literals(ra_text)
    operator_code = strip_rust_comments_and_literals(operator_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA008.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-008 must be checked only when sweep orchestration is implemented")

    for label, pattern in (
        ("BacktestSweepPlan", r"\bpub\s+struct\s+BacktestSweepPlan\b"),
        ("BacktestSweepRun", r"\bpub\s+struct\s+BacktestSweepRun\b"),
        ("BacktestSweepReport", r"\bpub\s+struct\s+BacktestSweepReport\b"),
        ("run_backtest_sweep", r"\bpub\s+fn\s+run_backtest_sweep\b"),
        ("run_backtest_sweep_with_executor", r"\bpub\s+fn\s+run_backtest_sweep_with_executor\b"),
        ("typed run-spec TOML serialization", r"\btoml\s*::\s*to_string_pretty\s*\(\s*&\s*run\s*\.\s*run_spec\s*\)"),
        ("run-spec path freshness check", r"\brun_spec_path\s*\.\s*try_exists\s*\(\s*\)"),
        ("output-dir freshness check", r"\boutput_dir\s*\.\s*try_exists\s*\(\s*\)"),
        ("run-spec create-only write", r"\bOpenOptions\s*::\s*new\s*\(\s*\)\s*\.\s*write\s*\(\s*true\s*\)\s*\.\s*create_new\s*\(\s*true\s*\)\s*\.\s*open\s*\(\s*&\s*run_spec_path\s*\)"),
        ("fresh per-run output-dir create", r"\bfs\s*::\s*create_dir\s*\(\s*&\s*output_dir\s*\)"),
        ("duplicate run-spec preflight", r"\bseen_run_spec_file_names\b.*?\binsert\s*\(\s*run\s*\.\s*run_spec_file_name\s*\.\s*clone\s*\(\s*\)\s*\)"),
        ("duplicate output-dir preflight", r"\bseen_output_dir_names\b.*?\binsert\s*\(\s*run\s*\.\s*output_dir_name\s*\.\s*clone\s*\(\s*\)\s*\)"),
        ("existing BTE operator invocation", r"\brun_operator_from_run_spec\s*\("),
        ("accepted object bytes passed to executor", r"\baccepted_object_bytes\b"),
        ("result contract filename", r"\bRESULT_CONTRACT_FILE\b"),
        ("persisted result-contract JSON read", r"\bread_result_contract\s*\("),
        ("result contract validation", r"\bcontract\s*\.\s*validate\s*\("),
        ("result contract run binding helper", r"\bvalidate_result_contract_matches_run\s*\("),
        (
            "result contract manifest hash binding",
            r"\bcontract\s*\.\s*manifest_hash\s*==\s*expected_manifest_hash\b",
        ),
        (
            "result contract accepted-object hash binding",
            r"\bcontract\s*\.\s*accepted_object_sha256\s*==\s*expected_accepted_object_sha256\b",
        ),
        (
            "run-spec accepted-object hash binding",
            r"\brun\s*\.\s*run_spec\s*\.\s*accepted_object\s*\.\s*sha256\s*==\s*expected_accepted_object_sha256\b",
        ),
        (
            "result contract strategy config hash binding",
            r"\bcontract\s*\.\s*strategy_config_hash\s*==\s*run\s*\.\s*run_spec\s*\.\s*manifest\s*\.\s*strategy_config_hash\b",
        ),
        (
            "result contract converter config hash binding",
            r"\bcontract\s*\.\s*converter_config_hash\s*==\s*expected_converter_config_hash\b",
        ),
    ):
        require_pattern(RA_PATH, ra_code, label, pattern, findings)

    for forbidden in ("nautilus_backtest", "BacktestEngine", "BacktestNode"):
        if re.search(rf"\b{re.escape(forbidden)}\b", ra_code):
            findings.append(f"{RA_PATH}: RA sweep orchestration must not own runner code via {forbidden}")

    for label, pattern in (
        ("overwrite-prone run-spec write", r"\bfs\s*::\s*write\s*\(\s*&\s*run_spec_path\b"),
        ("reuse-prone per-run output-dir mkdir", r"\bfs\s*::\s*create_dir_all\s*\(\s*&\s*output_dir\b"),
    ):
        if re.search(pattern, ra_code):
            findings.append(f"{RA_PATH}: RA sweep orchestration must not use {label}")

    require_pattern(
        OPERATOR_PATH,
        operator_code,
        "serializable RunSpec",
        r"\bderive\s*\([^\)]*Serialize[^\)]*Deserialize[^\)]*\)\s*\]\s*(?:#\s*\[[^\]]+\]\s*)*pub\s+struct\s+RunSpec\b",
        findings,
    )
    require_pattern(
        TEST_PATH,
        test_code,
        "sweep orchestration contract test",
        r"\bsweep_orchestration_writes_typed_run_specs_invokes_bte_and_reads_contracts\b",
        findings,
    )
    for label, pattern in (
        (
            "existing run-spec regression test",
            r"\bsweep_orchestration_rejects_existing_run_spec_file_before_executor\b",
        ),
        (
            "existing output-dir regression test",
            r"\bsweep_orchestration_rejects_existing_output_dir_before_executor\b",
        ),
        (
            "duplicate materialization path regression test",
            r"\bsweep_orchestration_rejects_duplicate_materialization_paths_before_executor\b",
        ),
        (
            "result contract run binding regression test",
            r"\bsweep_orchestration_rejects_contract_not_bound_to_run_spec\b",
        ),
    ):
        require_pattern(TEST_PATH, test_code, label, pattern, findings)
    require_pattern(
        TEST_PATH,
        test_code,
        "fake executor proving sequential BTE handoff",
        r"\brun_backtest_sweep_with_executor\s*\(",
        findings,
    )
    for command in missing_recipe_commands(
        just_text,
        (
            "python3 scripts/test_verify_ra_sweep_orchestration.py",
            "python3 scripts/verify_ra_sweep_orchestration.py",
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
        print("FAIL: RA sweep orchestration violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA sweep orchestration passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
