#!/usr/bin/env python3
"""Shared file/snippet checks for repository verifier scripts."""

from __future__ import annotations

from dataclasses import dataclass
from collections.abc import Sized
from pathlib import Path


REQUIRED_DISCOVERY_FLOOR_INVARIANT = (
    "Required discovery floors are preflight-terminal: when an enforced "
    "discovery set is empty or missing, the verifier emits only the relevant "
    "floor finding before scan work, stale checks, allowlist validation, "
    "ledger validation, source-fence wiring, missing-file checks, and "
    "supplemental broad scans."
)


@dataclass(frozen=True)
class RequiredDiscoveryFloorContract:
    verifier: str
    label: str
    classification: str
    entrypoint: str
    proof: str
    call_count: int = 1


REQUIRED_DISCOVERY_FLOOR_CONTRACTS = (
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_boundary_evidence.py",
        "Bolt-v3 boundary Rust source files",
        "helper-and-entrypoint-terminal",
        "scan_root",
        "scan_root checks the source floor before registry, exemption, fixture, and static checks; scan_wire_boundary repeats the helper guard.",
        call_count=2,
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_dependency_direction.py",
        "Bolt-v3 dependency direction source files",
        "entrypoint-terminal",
        "main",
        "find_violations returns the floor finding and main returns before stale-allowance checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_legacy_default_fence.py",
        "runtime source paths",
        "helper-terminal",
        "main",
        "collect_violations returns floor violations before iterating source paths.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_naming.py",
        "Bolt-v3 naming audit rule rows",
        "aggregate-then-terminal",
        "main",
        "main aggregates naming floors and returns before forbidden-name and misnomer scans.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_naming.py",
        "Bolt-v3 naming scan paths",
        "aggregate-then-terminal",
        "main",
        "main aggregates naming floors and returns before forbidden-name and misnomer scans.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_naming.py",
        "capital-admission misnomer scan paths",
        "helper-and-entrypoint-terminal",
        "main",
        "main preflights the misnomer scan floor before naming scans; the helper also floors before loading the allowlist.",
        call_count=2,
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_no_exit_market_command.py",
        "Rust source files under src",
        "helper-terminal",
        "main",
        "collect_violations_from_files returns floor violations before scanning source text.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_no_venue_name_branch.py",
        "Rust source files under src",
        "helper-terminal",
        "main",
        "collect_violations_from_files returns floor violations before scanning source text.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_poison_lock_fence.py",
        "Rust source files under src",
        "helper-terminal",
        "main",
        "collect_violations returns floor violations before scanning source text.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_provider_leaks.py",
        "Bolt-v3 provider-leak core files",
        "aggregate-then-terminal",
        "scan_root",
        "rules_for_root records all discovery floors; scan_root returns them before rule matching.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_provider_leaks.py",
        "bolt_v3_providers",
        "aggregate-then-terminal",
        "scan_root",
        "rules_for_root records all discovery floors; scan_root returns them before rule matching.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_provider_leaks.py",
        "NT provider crate stems",
        "aggregate-then-terminal",
        "scan_root",
        "rules_for_root records all discovery floors; scan_root returns them before rule matching.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_pure_rust_runtime.py",
        "Cargo manifests",
        "aggregate-then-terminal",
        "main",
        "main aggregates manifest/source/runtime floors and returns before cargo, lock, source, runtime, and entrypoint checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_pure_rust_runtime.py",
        "Rust source files under src",
        "aggregate-then-terminal",
        "main",
        "main aggregates manifest/source/runtime floors and returns before cargo, lock, source, runtime, and entrypoint checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_pure_rust_runtime.py",
        "Bolt-v3 runtime source paths",
        "aggregate-then-terminal",
        "main",
        "main aggregates manifest/source/runtime floors and returns before cargo, lock, source, runtime, and entrypoint checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_requote_construction.py",
        "Rust source files under src",
        "helper-terminal",
        "main",
        "collect_violations returns source-floor findings and main skips visibility scans for source-floor findings.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_runtime_literals.py",
        "Bolt-v3 runtime literal scan paths",
        "entrypoint-terminal",
        "main",
        "main prints the floor and returns before literal scanning and stale allowlist checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_strategy_policy_fence.py",
        "strategy policy source files",
        "aggregate-then-terminal",
        "collect_violations",
        "collect_violations floors configured source discovery before root checks, supplemental scans, and policy scans.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_strategy_policy_fence.py",
        "mutation policy source files",
        "aggregate-then-terminal",
        "collect_violations",
        "collect_violations returns floor findings before mutation policy scans.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bolt_v3_usable_mu_sole_mint.py",
        "Rust source files under src",
        "helper-terminal",
        "main",
        "collect_violations_from_files returns floor violations before scanning source text.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_bte_test_topology.py",
        "backtester integration test files",
        "entrypoint-terminal",
        "verify_root",
        "verify_root returns the harness floor before manifest and source-proof checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_ci_workflow_hygiene.py",
        "ci/github-actions-runners.toml",
        "helper-terminal",
        "main",
        "runner config floor helpers return before runner-contract checks; main deduplicates repeated floor messages.",
        call_count=2,
    ),
    RequiredDiscoveryFloorContract(
        "verify_doc_decoupling_residuals.py",
        "doc-decoupling scanned source paths",
        "entrypoint-terminal",
        "collect_findings",
        "collect_findings returns the source floor before ledger validation, stale snippets, and prose scans.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_fail_closed_contracts.py",
        "fail-closed contract selected paths",
        "entrypoint-terminal",
        "collect_findings",
        "collect_findings returns the selected-paths floor before source-fence wiring, raw scans, and stale exceptions.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_outcome_group_nt_reuse.py",
        "outcome-group source files",
        "entrypoint-terminal",
        "collect_findings",
        "collect_findings returns the source floor before ledger and justfile checks.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_ra_notebook_read_only_boundary.py",
        "RA notebook read-only code files",
        "entrypoint-terminal",
        "scan_root",
        "scan_root returns the source floor before per-file boundary scanning.",
    ),
    RequiredDiscoveryFloorContract(
        "verify_ra_single_engine_import_boundary.py",
        "RA single-engine code files",
        "entrypoint-terminal",
        "scan_root",
        "scan_root returns the source floor before per-file boundary scanning.",
    ),
)


def require_text_file(root: Path, rel_path: Path, findings: list[str]) -> str | None:
    path = root / rel_path
    if not path.exists():
        findings.append(f"{rel_path}: file is missing")
        return None
    return path.read_text(encoding="utf-8")


def require_nonempty(items: Sized, what: str, findings: list[str]) -> bool:
    """Append an empty-set floor while preserving the preflight-terminal invariant."""
    if len(items) == 0:
        findings.append(f"{what}: enforcement set is empty")
        return False
    return True


def require_snippets(
    rel_path: Path,
    text: str | None,
    snippets: tuple[str, ...],
    findings: list[str],
) -> None:
    if text is None:
        return
    for snippet in snippets:
        if snippet not in text:
            findings.append(f"{rel_path}: missing `{snippet}`")
