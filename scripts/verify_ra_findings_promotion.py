#!/usr/bin/env python3
"""Verify RA-011 findings and promotion boundary wiring."""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
RA_PATH = Path("crates/backtesting-vertical-slice/src/research_analytics.rs")
ARTIFACT_INDEX_PATH = Path("crates/backtesting-vertical-slice/src/artifact_index.rs")
RUN_MANIFEST_PATH = Path("crates/backtesting-vertical-slice/src/run_manifest.rs")
RA_TEST_PATH = Path("crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_analytics.rs")
JUSTFILE_PATH = Path("justfile")
TASKS_PATH = Path("specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md")

CHECKED_RA011 = re.compile(r"^- \[[xX]\] RA-011\b", re.MULTILINE)


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
    ra_text = read(root, RA_PATH)
    artifact_index_text = read(root, ARTIFACT_INDEX_PATH)
    run_manifest_text = read(root, RUN_MANIFEST_PATH)
    test_text = read(root, RA_TEST_PATH)
    just_text = read(root, JUSTFILE_PATH)
    tasks_text = read(root, TASKS_PATH)

    ra_code = strip_rust_comments_and_literals(ra_text)
    artifact_index_code = strip_rust_comments_and_literals(artifact_index_text)
    run_manifest_code = strip_rust_comments_and_literals(run_manifest_text)
    test_code = strip_rust_comments_and_literals(test_text)

    if not CHECKED_RA011.search(tasks_text):
        findings.append(f"{TASKS_PATH}: RA-011 must be checked only when findings/promotion is implemented")

    for label, pattern in (
        ("RaVerdictKind enum", r"\bpub\s+enum\s+RaVerdictKind\b.*\bGo\b.*\bNoGo\b.*\bConditionalGo\b"),
        ("RaVerdict required field set", r"\bpub\s+struct\s+RaVerdict\b.*\bscope\s*:\s*String\b.*\bsource_proof_refs\s*:\s*Vec\s*<\s*SourceProofEvidenceRef\s*>.*\bbacktest_result_refs\s*:\s*Vec\s*<\s*BacktestEvidenceRef\s*>.*\bevidence_report_refs\s*:\s*Vec\s*<\s*ArtifactPointerRef\s*>.*\brequested_claim_fidelity\s*:\s*SourceProofFidelityClass\b.*\bpreserved_claim_limits\s*:\s*Vec\s*<\s*String\s*>.*\bremeasurement_cadence\s*:\s*String\b.*\brecorded_at\s*:\s*String\b.*\brecorded_by\s*:\s*String\b"),
        ("real GO finding gate", r"\bfn\s+is_real_go_finding\b.*\bRaVerdictKind\s*::\s*Go\b.*\bsource_proof_refs\b.*\baccepted\b.*\bbacktest_result_refs\b.*\bobjective\b"),
        ("GO verdict requires real evidence", r"\bfn\s+validate\b.*\bself\s*\.\s*verdict\s*==\s*RaVerdictKind\s*::\s*Go\b.*!\s*self\s*\.\s*is_real_go_finding\s*\(\s*\).*?\bPromotionConfigRequiresGo\b"),
        ("PromotionConfigRef as typed field ref", r"\bpub\s+struct\s+PromotionConfigRef\b.*\btyped_config_uri\s*:\s*String\b.*\btyped_config_hash\s*:\s*String\b.*\breviewer_policy_refs\s*:\s*Vec\s*<\s*String\s*>.*\bnon_live_boundary\s*:\s*bool\b"),
        ("ExperimentResultArtifact verdict storage", r"\bpub\s+struct\s+ExperimentResultArtifact\b.*\bartifact_uri\s*:\s*String\b.*\blifecycle_state\s*:\s*LifecycleState\b.*\bverdict\s*:\s*RaVerdict\b.*\bpromotion_config\s*:\s*Option\s*<\s*PromotionConfigRef\s*>"),
        ("experiment-results URI guard", r"\bfn\s+validate_experiment_results_uri\b.*\bexperiment_results_prefix\b.*\bArtifactOutsideExperimentResults\b"),
        ("promotion config requires GO", r"\bif\s+let\s+Some\s*\(\s*promotion_config\s*\).*?\bif\s+!\s*self\s*\.\s*verdict\s*\.\s*is_real_go_finding\s*\(\s*\).*?\bPromotionConfigRequiresGo\b"),
        ("forbidden post-verdict action model", r"\bpub\s+enum\s+ForbiddenPromotionAction\b.*\bAutoMerge\b.*\bAutoEnableStrategy\b.*\bScheduleLiveTrading\b.*\bTouchSsmCredentials\b.*\bMutateProductionRuntimeConfig\b"),
        ("forbidden behavior validation", r"\bfn\s+forbidden_behavior_violations\b.*\baccepts_source_proofs\b.*\bmutates_source_proofs\b.*\bmutates_backtest_result_contracts\b.*\bweakens_forbidden_claims\b.*\bpost_verdict_actions\b"),
    ):
        require_pattern(RA_PATH, ra_code, label, pattern, findings)

    for label, pattern in (
        ("PromotionPackage model", r"\bPromotionPackage\b"),
        ("PromotionStatus model", r"\bPromotionStatus\b"),
        ("post-approval action model", r"\bPostApprovalAction\b"),
        ("promotion package prefix helper", r"\bpromotion_package_prefix\b"),
        ("approved-for-config state machine", r"\bApprovedForConfig\b"),
    ):
        reject_pattern(RA_PATH, ra_code, label, pattern, findings)

    require_pattern(
        ARTIFACT_INDEX_PATH,
        artifact_index_code,
        "RA experiment-results artifact family",
        r"\bpub\s+enum\s+ResearchAnalyticsSubfamily\b.*\bExperimentResults\b",
        findings,
    )
    reject_pattern(
        ARTIFACT_INDEX_PATH,
        artifact_index_code,
        "RA promotion-packages artifact family",
        r"\bPromotionPackages\b",
        findings,
    )

    for label, pattern in (
        ("RA experiment-result strategy source kind", r"\bResearchAnalyticsExperimentResult\b"),
        ("experiment result provenance fields", r"\bexperiment_result_uri\s*:\s*Option\s*<\s*String\s*>.*\bexperiment_result_hash\s*:\s*Option\s*<\s*String\s*>"),
        ("experiment-results provenance prefix", r"\bRESEARCH_ANALYTICS_EXPERIMENT_RESULT_PREFIX\b.*\bresearch_analytics_experiment_result_prefix\b"),
        ("RA strategy source prefix validation", r"\bStrategySourceKind\s*::\s*ResearchAnalyticsExperimentResult\b.*\bvalidate_strategy_artifact_ref\b.*\bstrategy\.experiment_result_uri\b.*\bstrategy\.experiment_result_hash\b"),
    ):
        require_pattern(RUN_MANIFEST_PATH, run_manifest_code, label, pattern, findings)

    for label, pattern in (
        ("old promotion package source kind", r"\bResearchAnalyticsPromotionPackage\b"),
        ("old promotion package URI field", r"\bpromotion_package_uri\b"),
        ("old promotion package hash field", r"\bpromotion_package_hash\b"),
    ):
        reject_pattern(RUN_MANIFEST_PATH, run_manifest_code, label, pattern, findings)

    for label, pattern in (
        ("verdict required-field test", r"\bfn\s+experiment_result_verdict_requires_required_field_set\b"),
        ("inert no-GO promotion gate test", r"\bfn\s+promotion_gate_stays_inert_without_go_finding\b"),
        ("typed config only on experiment result test", r"\bfn\s+go_finding_can_carry_typed_config_only_on_experiment_result\b"),
        ("GO evidence gate test", r"\bfn\s+go_promotion_requires_accepted_source_proof_and_objective_backtest_refs\b"),
        ("forbidden action test", r"\bfn\s+experiment_result_rejects_forbidden_promotion_actions\b"),
        ("cross-family fidelity test", r"\bfn\s+experiment_result_rejects_cross_family_fidelity_claims\b"),
    ):
        require_pattern(RA_TEST_PATH, test_code, label, pattern, findings)
    if "promotion-packages" not in test_text:
        findings.append(f"{RA_TEST_PATH}: missing negative proof that promotion-packages paths are rejected")

    for command in (
        "python3 scripts/test_verify_ra_findings_promotion.py",
        "python3 scripts/verify_ra_findings_promotion.py",
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
        print("FAIL: RA findings/promotion violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA findings/promotion passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
