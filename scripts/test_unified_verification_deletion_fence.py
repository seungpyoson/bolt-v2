#!/usr/bin/env python3
"""Reject superseded verification and review authority paths."""

from __future__ import annotations

import pathlib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]

FORBIDDEN_TRACKED_PATHS = (
    "scripts/remote_evidence.py",
    "scripts/review_readiness.py",
    "scripts/review_readiness_core.py",
    "scripts/test_remote_evidence.py",
    "scripts/test_review_readiness.py",
    "scripts/ai_review_deliverables.py",
    "scripts/test_ai_review_glm_fallback.py",
    ".github/workflows/merge-readiness-finalizer.yml",
    ".github/workflows/ci.yml",
    ".github/workflows/backtester-ci.yml",
    ".github/workflows/actionlint.yml",
    ".github/workflows/ai-review-coding-plan-smoke.yml",
    ".github/workflows/ai-review-glm-pr-agent.yml",
    ".github/workflows/debug-test.yml",
    ".github/workflows/coverage-enforcer.yml",
    ".github/workflows/dispatch-ci-cancel.yml",
    "scripts/coverage_enforcer.py",
    "scripts/merge_readiness.py",
    "scripts/find_same_sha_main_evidence.py",
    "scripts/nextest_fingerprint.py",
    "scripts/verify_ci_path_filters.py",
    "scripts/cancel_obsolete_dispatch_runs.py",
    "scripts/test_lane_governor.py",
    "ci/chainlink-reference-fixture-capture-provenance.toml",
    "ci/nextest-fingerprint.toml",
    "ci/rust-ci-inputs.toml",
    "scripts/ci_input_sets.py",
    "scripts/test_ci_input_sets.py",
    "scripts/governance_diff_analysis.py",
    "scripts/test_governance_diff_analysis.py",
    "scripts/test_workflow_expression_analysis.py",
    "docs/ci/nextest-artifact-cache.md",
)

AUTHORITY_PATHS = (
    "AGENTS.md",
    ".specify/memory/constitution.md",
    ".specify/templates/plan-template.md",
    "REASONIX.md",
    "justfile",
    ".no-mistakes.yaml",
    "ci/rust-verification.toml",
    "ci/ai-review.toml",
    "ci/github-actions-runners.toml",
    ".mergify.yml",
    "crates/backtesting-vertical-slice/ci/rust-verification.toml",
    ".github/workflows/claude-code-review.yml",
    ".github/workflows/ai-review-kimi-cli.yml",
    ".github/workflows/ai-review-glm.yml",
    "scripts/rust_verification.py",
    "scripts/verify_ci_workflow_hygiene.py",
)

FORBIDDEN_TEXT = (
    "review-ready",
    "verify-remote",
    "CERTIFIED",
    "UNCERTIFIED",
    "INCONCLUSIVE",
    "final-ai-review",
    "fallback_",
    "smart_trigger",
    "remote_probe.separate_workspaces",
    "check-success",
    "ci_provenance.required_checks",
    "ci_provenance.proof_owners",
    "gate-iteration",
    "backtester-gate",
    "pre-cutover",
    "post-cutover",
)


def violations(repo: pathlib.Path = REPO_ROOT) -> tuple[str, ...]:
    found: list[str] = []
    for relative in FORBIDDEN_TRACKED_PATHS:
        if (repo / relative).exists():
            found.append(f"forbidden path exists: {relative}")
    for relative in AUTHORITY_PATHS:
        path = repo / relative
        if not path.exists():
            continue
        text = path.read_text(encoding="utf-8")
        for forbidden in FORBIDDEN_TEXT:
            if forbidden in text:
                found.append(f"forbidden authority text {forbidden!r}: {relative}")
    hygiene = repo / "scripts/verify_ci_workflow_hygiene.py"
    if hygiene.is_file() and "('.github/workflows/ci.yml', workflow)" in hygiene.read_text(encoding="utf-8"):
        found.append("retired ci.yml analysis branch remains in workflow hygiene")
    return tuple(found)


def main() -> int:
    found = violations()
    if found:
        raise AssertionError("\n".join(found))
    print("OK: superseded verification paths are absent.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
