#!/usr/bin/env python3
"""Verify RA-013 run-pointer index stays thin and catalog-backed."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
RA_PATH = Path("crates/backtesting-vertical-slice/src/research_analytics.rs")
TEST_PATH = Path("crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_analytics.rs")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA013 = re.compile(r"^- \[[xX]\] RA-013\b", re.MULTILINE)


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


def require_pattern(rel_path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if not re.search(pattern, text, re.DOTALL):
        findings.append(f"{rel_path}: missing real {label}")


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    ra_text = (root / RA_PATH).read_text(encoding="utf-8") if (root / RA_PATH).exists() else ""
    test_text = (root / TEST_PATH).read_text(encoding="utf-8") if (root / TEST_PATH).exists() else ""
    just_text = (
        (root / JUSTFILE_PATH).read_text(encoding="utf-8") if (root / JUSTFILE_PATH).exists() else ""
    )
    tasks_text = (
        (root / TASKS_PATH).read_text(encoding="utf-8") if (root / TASKS_PATH).exists() else ""
    )

    ra_code = strip_rust_comments_and_literals(ra_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA013.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-013 must be checked only when run-pointer index is implemented")

    for label, pattern in (
        ("BacktestRunCatalogList trait", r"\bpub\s+trait\s+BacktestRunCatalogList\b"),
        (
            "catalog list_backtest_runs trait method",
            r"\bfn\s+list_backtest_runs\s*\(\s*&\s*self\s*\)\s*->\s*anyhow\s*::\s*Result\s*<\s*Vec\s*<\s*String\s*>\s*>\s*;",
        ),
        (
            "ParquetDataCatalog list_backtest_runs delegate",
            r"\bimpl\s+BacktestRunCatalogList\s+for\s+ParquetDataCatalog\b.*\.\s*list_backtest_runs\s*\(",
        ),
        ("RunPointerIndex", r"\bpub\s+struct\s+RunPointerIndex\b"),
        ("RunPointerIndexRecord", r"\bpub\s+struct\s+RunPointerIndexRecord\b"),
        ("RunPointerResult", r"\bpub\s+struct\s+RunPointerResult\b"),
        ("deterministic params map", r"\bparams\s*:\s*BTreeMap\s*<\s*String\s*,\s*serde_json\s*::\s*Value\s*>"),
        ("single artifact_root", r"\bartifact_root\s*:\s*String\b"),
        ("result pointer URI", r"\bresult_contract_uri\s*:\s*String\b"),
        ("result pointer hash", r"\bresult_contract_hash\s*:\s*String\b"),
        ("content hash field", r"\bcontent_hash\s*:\s*String\b"),
        ("catalog-backed builder", r"\bpub\s+fn\s+build_run_pointer_index_from_catalog\b"),
        ("listed-run exact set tracking", r"\bBTreeSet\s*<\s*String\s*>"),
        ("sha256 content hashing", r"\bsha256_hex\s*\("),
        ("structured JSON hashing", r"\bserde_json\s*::\s*to_vec\s*\("),
        ("content hash validation", r"\bexpected_content_hash\s*\("),
    ):
        require_pattern(RA_PATH, ra_code, label, pattern, findings)

    index_body = braced_body_after(ra_code, r"\bpub\s+struct\s+RunPointerIndex\b")
    if index_body is None:
        findings.append(f"{RA_PATH}: missing real RunPointerIndex body")
    else:
        for forbidden in ("lifecycle", "LifecycleState", "promotion", "PromotionConfigRef"):
            if re.search(rf"\b{re.escape(forbidden)}", index_body):
                findings.append(f"{RA_PATH}: RunPointerIndex must not carry {forbidden}")

    for forbidden in ("read_dir", "BacktestNode", "run_operator_from_run_spec"):
        if re.search(rf"\b{re.escape(forbidden)}\b", ra_code):
            body = braced_body_after(ra_code, r"\bpub\s+fn\s+build_run_pointer_index_from_catalog\b") or ""
            if re.search(rf"\b{re.escape(forbidden)}\b", body):
                findings.append(f"{RA_PATH}: run-pointer index must stay thin and not use {forbidden}")

    for label, pattern in (
        (
            "catalog-backed run-pointer test",
            r"\brun_pointer_index_covers_catalog_runs_with_hash_and_no_lifecycle_or_promotion_state\b",
        ),
        (
            "single-root rejection test",
            r"\brun_pointer_index_rejects_records_not_backed_by_one_catalog_root\b",
        ),
        ("fake catalog delegate", r"\bimpl\s+BacktestRunCatalogList\s+for\s+FakeBacktestRunCatalog\b"),
        ("serialized absence proof", r"\bserde_json\s*::\s*to_value\s*\("),
    ):
        require_pattern(TEST_PATH, test_code, label, pattern, findings)

    for command in (
        "python3 scripts/test_verify_ra_run_pointer_index.py",
        "python3 scripts/verify_ra_run_pointer_index.py",
    ):
        if command not in just_text:
            findings.append(f"{JUSTFILE_PATH}: source-fence-static must run {command}")

    return findings


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()
    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA run-pointer index violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA run-pointer index passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
