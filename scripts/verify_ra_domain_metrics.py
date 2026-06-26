#!/usr/bin/env python3
"""Verify RA-010 domain metrics are wired through NT PortfolioAnalyzer."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

from justfile_recipe_checks import missing_recipe_commands


REPO_ROOT = Path(__file__).resolve().parent.parent
RUN_MANIFEST_PATH = Path("crates/backtesting-vertical-slice/src/run_manifest.rs")
RUNNER_PATH = Path("crates/backtesting-vertical-slice/src/runner.rs")
DOMAIN_METRICS_PATH = Path("crates/backtesting-vertical-slice/src/domain_metrics.rs")
LIB_PATH = Path("crates/backtesting-vertical-slice/src/lib.rs")
CARGO_PATH = Path("crates/backtesting-vertical-slice/Cargo.toml")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA010 = re.compile(r"^- \[[xX]\] RA-010\b", re.MULTILINE)


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
    run_manifest_text = read(root, RUN_MANIFEST_PATH)
    runner_text = read(root, RUNNER_PATH)
    domain_metrics_text = read(root, DOMAIN_METRICS_PATH)
    lib_text = read(root, LIB_PATH)
    cargo_text = read(root, CARGO_PATH)
    just_text = read(root, JUSTFILE_PATH)
    tasks_text = read(root, TASKS_PATH)

    run_manifest_code = strip_rust_comments_and_literals(run_manifest_text)
    runner_code = strip_rust_comments_and_literals(runner_text)
    domain_metrics_code = strip_rust_comments_and_literals(domain_metrics_text)
    lib_code = strip_rust_comments_and_literals(lib_text)

    if not CHECKED_RA010.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-010 must be checked only when domain metrics are implemented")

    if "nautilus-analysis" not in cargo_text:
        findings.append(f"{CARGO_PATH}: missing direct nautilus-analysis dependency")

    for label, pattern in (
        ("domain_metrics module export", r"\bpub\s+mod\s+domain_metrics\s*;"),
    ):
        require_pattern(LIB_PATH, lib_code, label, pattern, findings)

    for label, pattern in (
        ("ManifestDomainMetricConfig", r"\bpub\s+struct\s+ManifestDomainMetricConfig\b"),
        ("domain_metrics manifest field", r"\bpub\s+domain_metrics\s*:\s*Vec\s*<\s*ManifestDomainMetricConfig\s*>"),
        ("registered domain metrics resolver", r"\bpub\s+fn\s+registered_domain_metrics\s*\(\s*\)\s*->\s*&'static\s*\[\s*&'static\s+str\s*\]"),
        ("domain metric validation", r"\bfn\s+ensure_supported_domain_metrics\b.*\bdomain_metrics\b.*\bUnsupportedEnum\b"),
        ("domain metric resolved surface", r"\bfor\s*\(\s*index\s*,\s*metric\s*\).*?\bresolved_surface\s*\(\s*&\s*format!\s*\("),
        ("domain metric hash coverage test", r"\bassert_hash_changes\s*\(\s*\"\".*?\bdomain_metrics\s*\.\s*push\s*\("),
        ("domain metric unsupported selector test", r"\bfn\s+rejects_unknown_domain_metric_selector\b"),
    ):
        require_pattern(RUN_MANIFEST_PATH, run_manifest_code, label, pattern, findings)

    for label, pattern in (
        ("PortfolioStatistic impl", r"\bimpl\s+PortfolioStatistic\s+for\s+\w+"),
        ("position-based domain metric", r"\bfn\s+calculate_from_positions\s*\(\s*&self\s*,\s*positions\s*:\s*&\s*\[\s*Position\s*\]\s*\)\s*->\s*Option\s*<\s*f64\s*>"),
        ("stat registration helper", r"\bpub\s+fn\s+register_domain_statistics\b.*\bPortfolioAnalyzer\b.*\bregister_statistic\s*\("),
        ("stat extraction helper", r"\bpub\s+fn\s+domain_statistics_from_analyzer\b.*\bget_performance_stats_general\s*\("),
        ("unit test for registration", r"\bfn\s+registers_domain_statistics_with_nt_portfolio_analyzer\b"),
    ):
        require_pattern(DOMAIN_METRICS_PATH, domain_metrics_code, label, pattern, findings)

    for label, pattern in (
        ("runner domain metric registration before run", r"\blet\s+domain_statistics\s*=.*\bresolve_domain_statistics\b.*?\bregister_domain_statistics\s*\("),
        ("runner copies domain metrics into BacktestResult", r"\bdomain_statistics_from_analyzer\s*\(.*?\bnt_result\s*\.\s*stats_general\s*\.insert\s*\("),
        ("runner avoids persisted contract recomputation", r"\bnode\s*\.\s*run\s*\(\s*\).*?\bget_engine\s*\(.*?\bdomain_statistics_from_analyzer"),
    ):
        require_pattern(RUNNER_PATH, runner_code, label, pattern, findings)

    for command in missing_recipe_commands(
        just_text,
        (
            "python3 scripts/test_verify_ra_domain_metrics.py",
            "python3 scripts/verify_ra_domain_metrics.py",
        ),
    ):
        findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def read(root: Path, rel_path: Path) -> str:
    path = root / rel_path
    return path.read_text(encoding="utf-8") if path.exists() else ""


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()
    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA domain metric violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA domain metrics passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
