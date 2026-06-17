#!/usr/bin/env python3
"""Verify RA-012 Artifact Index commit coverage for RA-owned artifacts."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_STORE_PATH = Path("crates/backtesting-vertical-slice/src/artifact_store.rs")
ARTIFACT_INDEX_PATH = Path("crates/backtesting-vertical-slice/src/artifact_index.rs")
ARTIFACT_STORE_TEST_PATH = Path("crates/backtesting-vertical-slice/tests/artifact_store_contract.rs")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA012 = re.compile(r"^- \[[xX]\] RA-012\b", re.MULTILINE)


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


def read(root: Path, rel_path: Path) -> str:
    path = root / rel_path
    return path.read_text(encoding="utf-8") if path.exists() else ""


def require_pattern(rel_path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if not re.search(pattern, text, re.DOTALL):
        findings.append(f"{rel_path}: missing real {label}")


def reject_pattern(rel_path: Path, text: str, label: str, pattern: str, findings: list[str]) -> None:
    if re.search(pattern, text, re.DOTALL):
        findings.append(f"{rel_path}: forbidden active {label}")


def scan_root(root: Path) -> list[str]:
    findings: list[str] = []
    artifact_store_text = read(root, ARTIFACT_STORE_PATH)
    artifact_index_text = read(root, ARTIFACT_INDEX_PATH)
    test_text = read(root, ARTIFACT_STORE_TEST_PATH)
    just_text = read(root, JUSTFILE_PATH)
    tasks_text = read(root, TASKS_PATH)

    artifact_store_code = strip_rust_comments_and_literals(artifact_store_text)
    artifact_index_code = strip_rust_comments_and_literals(artifact_index_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA012.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-012 must be checked only when RA Artifact Index commits are implemented")

    for family in ("datasets", "feature-tables", "experiment-results"):
        if family not in artifact_store_text:
            findings.append(f"{ARTIFACT_STORE_PATH}: missing RA artifact family literal {family!r}")

    for label, pattern in (
        ("RA artifact family constants", r"\bRESEARCH_ANALYTICS_ARTIFACT_FAMILIES\b.*\bRESEARCH_ANALYTICS_DATASETS_SUBFAMILY\b.*\bRESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY\b.*\bRESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY\b"),
        ("all artifact kinds require typed-root URI validation", r"\bfn\s+validate_artifact_uri_for_kind\b.*\btyped_root\s*=\s*artifact_root\s*\.\s*typed_root\s*\(\s*kind\s*\).*?\bstarts_with\b"),
        ("RA kind URI validator", r"\bfn\s+validate_artifact_uri_for_kind\b.*\bArtifactKind\s*::\s*ResearchAnalytics\b.*\bRESEARCH_ANALYTICS_ARTIFACT_FAMILIES\b.*?\bstarts_with\b"),
        ("event validation uses kind URI validator", r"\bimpl\s+ArtifactIndexEvent\b.*\bfn\s+validate\b.*\bvalidate_artifact_uri_for_kind\b.*\bartifact_uri\b.*\bvalidate_artifact_uri_for_kind\b.*\bmanifest_uri\b"),
        ("snapshot row validation uses kind URI validator", r"\bimpl\s+ArtifactIndexSnapshotRow\b.*\bfn\s+validate\b.*\bvalidate_artifact_uri_for_kind\b.*\bartifact_uri\b.*\bvalidate_artifact_uri_for_kind\b.*\bmanifest_uri\b"),
        ("RA writer authority supports RA kind", r"\bArtifactIndexWriteAuthority\b.*\bauthorize_commit\b.*\bArtifactKind\b"),
    ):
        require_pattern(ARTIFACT_STORE_PATH, artifact_store_code, label, pattern, findings)

    reject_pattern(
        ARTIFACT_INDEX_PATH,
        artifact_index_code,
        "RA promotion-packages artifact family",
        r"\bPromotionPackages\b",
        findings,
    )

    for label, pattern in (
        ("RA all-family commit test", r"\bfn\s+research_analytics_writer_commits_all_owned_families_to_one_kind_snapshot\b.*\bcommit_event\b.*\bread_verified_latest_snapshot\b.*\bread_committed_row\b"),
        ("RA active lifecycle assertion", r"\bArtifactLifecycleState\s*::\s*Active\b"),
        ("RA committed row assertion", r"\bArtifactIndexCommitState\s*::\s*Committed\b"),
        ("RA consumer mutation rejection test", r"\bfn\s+artifact_index_writer_rejects_consumer_mutation_of_research_analytics_records\b.*\bexpect_err\b"),
        ("cross-kind artifact URI rejection test", r"\bfn\s+artifact_index_rejects_cross_kind_artifact_uri_squatting\b.*\bexpect_err\b"),
        ("RA promotion package rejection test", r"\bfn\s+research_analytics_index_rejects_promotion_package_family\b.*\bput_event\b.*\bexpect_err\b"),
    ):
        require_pattern(ARTIFACT_STORE_TEST_PATH, test_code, label, pattern, findings)
    for literal in (
        "datasets",
        "feature-tables",
        "experiment-results",
        "promotion-packages",
        "kind=research-analytics/latest.json",
    ):
        if literal not in test_text:
            findings.append(f"{ARTIFACT_STORE_TEST_PATH}: missing RA commit test literal {literal!r}")

    for command in (
        "python3 scripts/test_verify_ra_artifact_index_commit.py",
        "python3 scripts/verify_ra_artifact_index_commit.py",
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
        print("FAIL: RA Artifact Index commit violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA Artifact Index commit passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
