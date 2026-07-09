#!/usr/bin/env python3
"""Shared file/snippet checks for repository verifier scripts."""

from __future__ import annotations

from dataclasses import dataclass
from collections.abc import Sized
from pathlib import Path

from rust_source_scanner import strip_rust_comments_and_literals


REQUIRED_DISCOVERY_FLOOR_INVARIANT = (
    "Required discovery floors are preflight-terminal: when an enforced "
    "discovery set is empty or missing, the verifier emits only the relevant "
    "floor finding before scan work, stale checks, allowlist validation, "
    "downstream config validation, ledger validation, source-fence wiring, "
    "missing-file checks, and supplemental broad scans. Required discovery "
    "configuration needed to compute the set must fail as a normal finding, "
    "not a traceback."
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
        "verify_bolt_v3_no_exit_market_command.py",
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
        "verify_bolt_v3_runtime_literals.py",
        "Bolt-v3 runtime literal scan paths",
        "entrypoint-terminal",
        "main",
        "main prints the floor and returns before runtime literal audit config validation, literal scanning, and stale allowlist checks.",
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
        "verify_ci_workflow_hygiene.py",
        "ci/github-actions-runners.toml",
        "helper-terminal",
        "main",
        "runner config floor helpers return before runner-contract checks; main deduplicates repeated floor messages.",
        call_count=2,
    ),
    RequiredDiscoveryFloorContract(
        "verify_fail_closed_contracts.py",
        "fail-closed contract selected paths",
        "entrypoint-terminal",
        "collect_findings",
        "collect_findings returns the selected-paths floor before exception config validation, source-fence wiring, raw scans, and stale exceptions.",
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


DECLARED_SOURCE_PRESENT = "present"
DECLARED_SOURCE_ABSENT = "absent"


def require_declared_source_files(
    items: Sized | None,
    what: str,
    source_path: str,
    declared_state: str,
    findings: list[str],
) -> bool:
    """Validate declared source state before scanning to preserve the preflight-terminal invariant."""
    if declared_state == DECLARED_SOURCE_PRESENT:
        if items is None:
            findings.append(f"{what}: configured source path {source_path} is declared present but is not present")
            return False
        return require_nonempty(items, what, findings)
    if declared_state == DECLARED_SOURCE_ABSENT:
        if items is not None:
            findings.append(f"{what}: configured source path {source_path} is declared absent; flip the declaration consciously")
        return False
    findings.append(f"{what}: configured source path {source_path} has invalid declaration {declared_state!r}")
    return False


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


def require_rust_snippets(
    rel_path: Path,
    text: str | None,
    snippets: tuple[str, ...],
    findings: list[str],
) -> None:
    if text is None:
        return
    require_snippets(rel_path, strip_rust_comments_and_literals(text), snippets, findings)
