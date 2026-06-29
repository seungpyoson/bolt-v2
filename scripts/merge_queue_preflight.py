#!/usr/bin/env python3
"""Preflight candidate PR waves before queueing them through Mergify."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys
import tempfile
import tomllib
from collections import Counter
from collections.abc import Mapping, Sequence
from typing import Any

from verify_ci_workflow_hygiene import parse_mergify_yaml, verify_mergify_config


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "rust-verification.toml"
MERGIFY_CONFIG_PATH = ".mergify.yml"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
EXPECTED_HEAD_SHA_RE = re.compile(r"^(?P<pr>[1-9][0-9]*)=(?P<sha>[0-9a-f]{40})$")
MERGIFY_REQUIRED_REVIEWER_RE = re.compile(r"(?:^|\n)approved-reviews-by = (?P<reviewer>[^\n]+)")
CONFLICT_LINE_RE = re.compile(r"^\d{6} [0-9a-f]{40} [123]\t(.+)$")
PR_REF_PREFIX = "refs/pull/"
FETCH_HEAD = "FETCH_HEAD"
PROFILE_NONE = "none"
GH_PR_CHECKS_JSON_RETURNCODES = (0, 1, 2, 8)
STATUS_READY = "ready"
STATUS_BLOCKED = "blocked"
STATUS_INCONCLUSIVE = "inconclusive"
STATUS_RESIDUAL_RISK = "residual_risk"
INPUT_FAILURE_USAGE_ERROR = "usage_error"
INPUT_FAILURE_LANE_FINDING = "lane_finding"
INPUT_FAILURE_USAGE_REASON = "preflight_usage_error"
LANE_MERGIFY_CONFIG = "mergify_config"
LANE_IDENTITY = "identity"
LANE_READINESS = "readiness"
LANE_INTEGRATION = "integration"
LANE_VERIFIER = "verifier"
CONTRACT_LANES = (
    LANE_MERGIFY_CONFIG,
    LANE_IDENTITY,
    LANE_READINESS,
    LANE_INTEGRATION,
    LANE_VERIFIER,
)
CONTRACT_STATUS_RANK = {
    STATUS_BLOCKED: 0,
    STATUS_INCONCLUSIVE: 1,
    STATUS_READY: 2,
}
VERDICT_QUEUE_AS_ONE_WAVE = "queue_as_one_wave"
VERDICT_SPLIT_ADVISED = "split_advised"
VERDICT_BLOCKED = "blocked"
VERDICT_INCONCLUSIVE = "inconclusive"
CONTRACT_READY_WAVE_VERDICTS = {
    STATUS_READY: VERDICT_QUEUE_AS_ONE_WAVE,
    VERDICT_SPLIT_ADVISED: VERDICT_SPLIT_ADVISED,
}
CONTRACT_STATUS_VERDICTS = {
    STATUS_BLOCKED: VERDICT_BLOCKED,
    STATUS_INCONCLUSIVE: VERDICT_INCONCLUSIVE,
}
CONTRACT_VERDICT_EXIT_CODES = {
    VERDICT_QUEUE_AS_ONE_WAVE: 0,
    VERDICT_SPLIT_ADVISED: 1,
    VERDICT_BLOCKED: 2,
    VERDICT_INCONCLUSIVE: 3,
}
INPUT_FAILURE_CLASSIFICATIONS = {
    "absent_input": (INPUT_FAILURE_USAGE_ERROR, INPUT_FAILURE_USAGE_REASON, 4),
    "absent_evidence": (INPUT_FAILURE_LANE_FINDING, STATUS_INCONCLUSIVE, 3),
    "empty_input": (INPUT_FAILURE_USAGE_ERROR, INPUT_FAILURE_USAGE_REASON, 4),
    "invalid": (INPUT_FAILURE_USAGE_ERROR, INPUT_FAILURE_USAGE_REASON, 4),
    "stale_base": (INPUT_FAILURE_LANE_FINDING, STATUS_INCONCLUSIVE, 3),
    "stale_head": (INPUT_FAILURE_LANE_FINDING, STATUS_BLOCKED, 2),
    "unavailable": (INPUT_FAILURE_LANE_FINDING, STATUS_INCONCLUSIVE, 3),
    "timeout": (INPUT_FAILURE_LANE_FINDING, STATUS_INCONCLUSIVE, 3),
    "ambiguous": (INPUT_FAILURE_LANE_FINDING, STATUS_INCONCLUSIVE, 3),
}
PREFLIGHT_USAGE_EXIT_CODE = INPUT_FAILURE_CLASSIFICATIONS["invalid"][2]
MERGIFY_CONFIG_SNAPSHOT_STATES = {
    True: (
        STATUS_READY,
        "mergify_config_snapshot_read",
        ".mergify.yml snapshot read from expected base",
    ),
    False: (
        STATUS_INCONCLUSIVE,
        "mergify_config_snapshot_unavailable",
        ".mergify.yml snapshot unavailable at expected base",
    ),
}
MERGIFY_CONFIG_VALIDATION_STATES = {
    True: (
        STATUS_READY,
        "mergify_config_valid",
        ".mergify.yml snapshot satisfies Mergify config contract",
    ),
    False: (
        STATUS_INCONCLUSIVE,
        "mergify_config_invalid",
        ".mergify.yml snapshot does not satisfy Mergify config contract",
    ),
}
MERGIFY_QUEUE_CONDITION_LABELS = {
    (): frozenset(),
    ("label = hotfix",): frozenset({"hotfix"}),
}
MERGIFY_BATCH_SIZE_EXTRACTORS = {
    True: lambda batch_size: int(batch_size["max"]),
    False: int,
}
MERGIFY_QUEUE_WAVE_STATUSES = {
    False: STATUS_READY,
    True: VERDICT_SPLIT_ADVISED,
}
MERGIFY_SPLIT_REASON_CODES = frozenset({"mergify_queue_batch_above_max"})
MERGIFY_QUEUE_PROOF_SOURCE_STATES = {
    True: (
        STATUS_READY,
        "mergify_queue_proof_source",
        "Mergify queue rule {queue_rule} uses queue proof context",
        "queue_proof_pr",
    ),
    False: (
        STATUS_INCONCLUSIVE,
        "mergify_in_place_proof_source",
        "Mergify queue rule {queue_rule} uses in-place proof context",
        "in_place_pr",
    ),
}
BASE_IDENTITY_FINDING_STATES = {
    True: (
        STATUS_READY,
        "base_identity_ready",
        "expected base SHA matches live base branch",
    ),
    False: (
        STATUS_INCONCLUSIVE,
        "stale_base",
        "expected base SHA differs from live base branch",
    ),
}
HEAD_IDENTITY_FINDING_STATES = {
    True: (
        STATUS_READY,
        "head_identity_ready",
        "expected PR head SHA matches fetched PR head",
    ),
    False: (
        STATUS_BLOCKED,
        "stale_head",
        "expected PR head SHA differs from fetched PR head",
    ),
}
RESIDUAL_RISK_REASON_CODES = (
    "full_ci_result",
    "mergify_proof_pr_behavior",
    "remote_runner_availability",
    "flaky_checks_and_external_services",
    "base_or_head_drift_after_preflight",
    "post_merge_config_or_workflow_changes",
    "queue_metadata_drift",
    "live_queue_ordering",
    "reset_on_external_merge",
    "max_parallel_checks_cost",
)
MERGIFY_CONFIG_FIELD_HANDLING = {
    "merge_queue.max_parallel_checks": "residual_cost_impact",
    "merge_queue.reset_on_external_merge": "residual_post_preflight_invalidation",
    "queue_rules[].name": "required_unique_queue_identity",
    "queue_rules[].queue_conditions": "effective_pr_to_queue_routing",
    "queue_rules[].merge_conditions": "required_reviewer_and_check_evidence",
    "queue_rules[].branch_protection_injection_mode": "explicit_support_or_inconclusive",
    "queue_rules[].batch_size": "batch_min_max_scalar_model",
    "queue_rules[].batch_max_wait_time": "below_min_wait_model",
    "queue_rules[].batch_max_failure_resolution_attempts": "explicit_support_or_inconclusive",
    "queue_rules[].checks_timeout": "residual_proof_time_risk",
    "queue_rules[].draft_bot_account": "explicit_support_or_inconclusive",
    "queue_rules[].merge_method": "explicit_support_or_inconclusive",
    "priority_rules[].conditions": "effective_routing_priority_conditions",
    "priority_rules[].name": "required_unique_priority_identity",
    "priority_rules[].priority": "residual_live_order_risk",
    "priority_rules[].allow_checks_interruption": "residual_interruption_risk",
}
PREFLIGHT_ARTIFACT_CLASSIFICATIONS = {
    "base_conflict": (LANE_INTEGRATION, "pr", STATUS_BLOCKED),
    "batch_conflict": (LANE_INTEGRATION, "batch", STATUS_READY),
    "batch_verifier_failed": (LANE_VERIFIER, "batch", STATUS_READY),
    "base_mismatch": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_mismatch": (LANE_IDENTITY, "pr", STATUS_BLOCKED),
    "head_fetch_failed": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_unavailable": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "metadata_unavailable": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_failed": (LANE_READINESS, "pr", STATUS_BLOCKED),
    "required_check_pending": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "readiness_failed": (LANE_READINESS, "pr", STATUS_BLOCKED),
    "verifier_failed": (LANE_VERIFIER, "pr", STATUS_BLOCKED),
}
CHECK_STATE_CLASSIFICATIONS = {
    "success": (STATUS_READY, "required_check_ready"),
    "pass": (STATUS_READY, "required_check_ready"),
    "failure": (STATUS_BLOCKED, "required_check_failed"),
    "error": (STATUS_BLOCKED, "required_check_failed"),
    "cancelled": (STATUS_BLOCKED, "required_check_failed"),
    "action_required": (STATUS_BLOCKED, "required_check_failed"),
    "startup_failure": (STATUS_BLOCKED, "required_check_failed"),
    "pending": (STATUS_INCONCLUSIVE, "required_check_pending"),
    "queued": (STATUS_INCONCLUSIVE, "required_check_pending"),
    "requested": (STATUS_INCONCLUSIVE, "required_check_pending"),
    "waiting": (STATUS_INCONCLUSIVE, "required_check_pending"),
    "in_progress": (STATUS_INCONCLUSIVE, "required_check_pending"),
    "skipped": (STATUS_INCONCLUSIVE, "required_check_skipped"),
    "neutral": (STATUS_INCONCLUSIVE, "required_check_skipped"),
    "missing": (STATUS_INCONCLUSIVE, "required_check_missing"),
}
CHECK_STATE_UNKNOWN = (STATUS_INCONCLUSIVE, "required_check_unknown")
CHECK_STATE_STALE = (STATUS_INCONCLUSIVE, "required_check_stale")
VERIFIER_STREAMS = ("stdout", "stderr")
PREFLIGHT_MODE_FINDINGS = {
    True: (),
    False: (
        {
            "lane": LANE_READINESS,
            "scope": "run",
            "status": STATUS_INCONCLUSIVE,
            "reason_code": "readiness_disabled_by_no_gh",
            "message": "--no-gh disables authoritative readiness evidence",
            "evidence": {"use_gh": False},
        },
    ),
}


class PreflightError(RuntimeError):
    """Raised when preflight input or repository state is invalid."""


class PreflightArgumentParser(argparse.ArgumentParser):
    def error(self, message: str) -> None:
        self.print_usage(sys.stderr)
        self.exit(PREFLIGHT_USAGE_EXIT_CODE, f"{self.prog}: error: {message}\n")


@dataclasses.dataclass(frozen=True)
class ContractEvidence:
    findings: tuple[dict[str, object], ...]
    artifacts: tuple[Mapping[str, object], ...]
    wave_status: str


@dataclasses.dataclass(frozen=True)
class ExpectedHead:
    pr: int
    sha: str


@dataclasses.dataclass(frozen=True)
class ExpectedHeadMapViolation:
    prs: tuple[int, ...]
    message_template: str

    def message(self) -> str:
        return self.message_template.format(prs=format_pr_numbers(self.prs))


def normalize_check_state(raw_state: str) -> str:
    return re.sub(r"[-\s]+", "_", str(raw_state).strip().lower())


def contract_lane_status(findings: Sequence[dict[str, object]], lane: str) -> str:
    statuses = tuple(
        str(finding["status"])
        for finding in findings
        if finding["lane"] == lane and finding["status"] != STATUS_RESIDUAL_RISK
    )
    return min(statuses, key=CONTRACT_STATUS_RANK.__getitem__, default=STATUS_INCONCLUSIVE)


def preflight_mode_findings(*, use_gh: bool) -> tuple[dict[str, object], ...]:
    return tuple(
        {
            **finding,
            "evidence": dict(finding["evidence"]),
        }
        for finding in PREFLIGHT_MODE_FINDINGS[use_gh]
    )


def matching_base_identity_findings(
    *,
    expected_base_sha: str,
    actual_base_sha: str,
) -> tuple[dict[str, object], ...]:
    status, reason_code, message = BASE_IDENTITY_FINDING_STATES[True]
    return (
        {
            "lane": LANE_IDENTITY,
            "scope": "run",
            "status": status,
            "reason_code": reason_code,
            "message": message,
            "evidence": {
                "expected_base_sha": expected_base_sha,
                "actual_base_sha": actual_base_sha,
            },
        },
    )


def stale_base_identity_findings(
    *,
    expected_base_sha: str,
    actual_base_sha: str,
) -> tuple[dict[str, object], ...]:
    status, reason_code, message = BASE_IDENTITY_FINDING_STATES[False]
    return (
        {
            "lane": LANE_IDENTITY,
            "scope": "run",
            "status": status,
            "reason_code": reason_code,
            "message": message,
            "evidence": {
                "expected_base_sha": expected_base_sha,
                "actual_base_sha": actual_base_sha,
            },
        },
    )


BASE_IDENTITY_FINDING_BUILDERS = {
    True: matching_base_identity_findings,
    False: stale_base_identity_findings,
}


def base_identity_findings(
    *,
    expected_base_sha: str,
    actual_base_sha: str,
) -> tuple[dict[str, object], ...]:
    return BASE_IDENTITY_FINDING_BUILDERS[expected_base_sha == actual_base_sha](
        expected_base_sha=expected_base_sha,
        actual_base_sha=actual_base_sha,
    )


def matching_head_identity_findings(
    *,
    pr: int,
    expected_head_sha: str,
    actual_head_sha: str,
) -> tuple[dict[str, object], ...]:
    status, reason_code, message = HEAD_IDENTITY_FINDING_STATES[True]
    return (
        {
            "lane": LANE_IDENTITY,
            "scope": "pr",
            "status": status,
            "reason_code": reason_code,
            "message": message,
            "evidence": {
                "pr": pr,
                "expected_head_sha": expected_head_sha,
                "actual_head_sha": actual_head_sha,
            },
        },
    )


def stale_head_identity_findings(
    *,
    pr: int,
    expected_head_sha: str,
    actual_head_sha: str,
) -> tuple[dict[str, object], ...]:
    status, reason_code, message = HEAD_IDENTITY_FINDING_STATES[False]
    return (
        {
            "lane": LANE_IDENTITY,
            "scope": "pr",
            "status": status,
            "reason_code": reason_code,
            "message": message,
            "evidence": {
                "pr": pr,
                "expected_head_sha": expected_head_sha,
                "actual_head_sha": actual_head_sha,
            },
        },
    )


HEAD_IDENTITY_FINDING_BUILDERS = {
    True: matching_head_identity_findings,
    False: stale_head_identity_findings,
}


def head_identity_findings(
    *,
    expected_heads: Mapping[int, str],
    actual_heads: Mapping[int, PrHead],
) -> tuple[dict[str, object], ...]:
    return tuple(
        finding
        for pr, actual_head in actual_heads.items()
        for expected_head_sha in (expected_heads[pr],)
        for finding in HEAD_IDENTITY_FINDING_BUILDERS[
            expected_head_sha == actual_head.sha
        ](
            pr=pr,
            expected_head_sha=expected_head_sha,
            actual_head_sha=actual_head.sha,
        )
    )


def matching_head_identity_blocks(
    *,
    pr: int,
    expected_head_sha: str,
    actual_head_sha: str,
) -> tuple[dict[str, object], ...]:
    return ()


def stale_head_identity_blocks(
    *,
    pr: int,
    expected_head_sha: str,
    actual_head_sha: str,
) -> tuple[dict[str, object], ...]:
    return (
        {
            "pr": pr,
            "reason": "expected PR head SHA differs from fetched PR head",
            "type": "head_mismatch",
        },
    )


HEAD_IDENTITY_BLOCK_BUILDERS = {
    True: matching_head_identity_blocks,
    False: stale_head_identity_blocks,
}


def head_identity_blocks(
    *,
    expected_heads: Mapping[int, str],
    actual_heads: Mapping[int, PrHead],
) -> list[dict[str, object]]:
    return [
        block
        for pr, actual_head in actual_heads.items()
        for expected_head_sha in (expected_heads[pr],)
        for block in HEAD_IDENTITY_BLOCK_BUILDERS[
            expected_head_sha == actual_head.sha
        ](
            pr=pr,
            expected_head_sha=expected_head_sha,
            actual_head_sha=actual_head.sha,
        )
    ]


def residual_risk_findings() -> tuple[dict[str, object], ...]:
    return tuple(
        {
            "lane": "residual_risk",
            "scope": "run",
            "status": STATUS_RESIDUAL_RISK,
            "reason_code": reason_code,
            "message": reason_code,
            "evidence": {},
        }
        for reason_code in RESIDUAL_RISK_REASON_CODES
    )


def integration_batch_ready_finding(batch: Batch) -> dict[str, object]:
    return {
        "lane": LANE_INTEGRATION,
        "scope": "batch",
        "status": STATUS_READY,
        "reason_code": "integration_batch_ready",
        "message": f"batch {batch.index} synthetic merge is conflict-free",
        "evidence": {
            "index": batch.index,
            "prs": list(batch.prs),
        },
    }


def integration_batch_ready_findings(batches: Sequence[Batch]) -> tuple[dict[str, object], ...]:
    return tuple(integration_batch_ready_finding(batch) for batch in batches)


def verifier_batch_ready_finding(batch: Batch, output_policy: OutputPolicy) -> dict[str, object]:
    return {
        "lane": LANE_VERIFIER,
        "scope": "batch",
        "status": STATUS_READY,
        "reason_code": "verifier_batch_ready",
        "message": f"batch {batch.index} verifier commands passed",
        "evidence": {
            "index": batch.index,
            "prs": list(batch.prs),
            "verifiers": [result.as_public_json(output_policy) for result in batch.verifiers],
        },
    }


def verifier_batch_ready_findings(
    batches: Sequence[Batch],
    output_policy: OutputPolicy,
) -> tuple[dict[str, object], ...]:
    return tuple(verifier_batch_ready_finding(batch, output_policy) for batch in batches)


def mergify_config_snapshot_finding(*, repo: pathlib.Path, base_sha: str) -> dict[str, object]:
    result = git(repo, "rev-parse", f"{base_sha}:{MERGIFY_CONFIG_PATH}", check=False)
    blob_sha = result.stdout.strip()
    status, reason_code, message = MERGIFY_CONFIG_SNAPSHOT_STATES[
        result.returncode == 0 and SHA_RE.fullmatch(blob_sha) is not None
    ]
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "run",
        "status": status,
        "reason_code": reason_code,
        "message": message,
        "evidence": {
            "path": MERGIFY_CONFIG_PATH,
            "base_sha": base_sha,
            "blob_sha": blob_sha,
            "git_returncode": result.returncode,
            "git_stderr": result.stderr.strip(),
        },
    }


def mergify_config_validation_finding(
    *,
    repo: pathlib.Path,
    base_sha: str,
    blob_sha: str,
) -> dict[str, object]:
    result = git(repo, "cat-file", "-p", blob_sha, check=False)
    errors = tuple(verify_mergify_config(result.stdout, config_name=MERGIFY_CONFIG_PATH))
    status, reason_code, message = MERGIFY_CONFIG_VALIDATION_STATES[
        result.returncode == 0 and not errors
    ]
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "run",
        "status": status,
        "reason_code": reason_code,
        "message": message,
        "evidence": {
            "path": MERGIFY_CONFIG_PATH,
            "base_sha": base_sha,
            "blob_sha": blob_sha,
            "validator": "verify_ci_workflow_hygiene.verify_mergify_config",
            "git_returncode": result.returncode,
            "git_stderr": result.stderr.strip(),
            "errors": list(errors),
        },
    }


def mergify_config_data(*, repo: pathlib.Path, blob_sha: str) -> Any:
    result = git(repo, "cat-file", "-p", blob_sha, check=False)
    config, _ = parse_mergify_yaml(result.stdout, MERGIFY_CONFIG_PATH)
    return config


def readiness_label_names(readiness: Mapping[str, object]) -> tuple[str, ...]:
    metadata = dict(readiness["metadata"])
    labels = tuple(metadata["labels"])
    return tuple(sorted(str(dict(label)["name"]) for label in labels))


def mergify_queue_rule_matches(rule: Mapping[str, object], labels: frozenset[str]) -> bool:
    condition_labels = MERGIFY_QUEUE_CONDITION_LABELS[tuple(rule["queue_conditions"])]
    return condition_labels.issubset(labels)


def selected_mergify_queue_rule(
    config: Mapping[str, object],
    labels: tuple[str, ...],
) -> Mapping[str, object]:
    return next(
        filter(
            lambda rule: mergify_queue_rule_matches(rule, frozenset(labels)),
            tuple(config["queue_rules"]),
        )
    )


def mergify_queue_route_finding(
    readiness: Mapping[str, object],
    rule: Mapping[str, object],
    labels: tuple[str, ...],
) -> dict[str, object]:
    pr = int(readiness["pr"])
    queue_rule = str(rule["name"])
    queue_conditions = [str(condition) for condition in tuple(rule["queue_conditions"])]
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "pr",
        "status": STATUS_READY,
        "reason_code": "mergify_queue_route_selected",
        "message": f"PR #{pr} routes to Mergify queue rule {queue_rule}",
        "evidence": {
            "pr": pr,
            "queue_rule": queue_rule,
            "labels": list(labels),
            "queue_conditions": queue_conditions,
        },
    }


def mergify_route_queue_groups(route_findings: Sequence[Mapping[str, object]]) -> dict[str, list[int]]:
    groups: dict[str, list[int]] = {}
    for finding in route_findings:
        evidence = dict(finding["evidence"])
        groups.setdefault(str(evidence["queue_rule"]), []).append(int(evidence["pr"]))
    return groups


def mergify_queue_proof_source_finding(rule: Mapping[str, object]) -> dict[str, object]:
    queue_rule = str(rule["name"])
    queue_conditions = [str(condition) for condition in tuple(rule["queue_conditions"])]
    merge_conditions = [str(condition) for condition in tuple(rule["merge_conditions"])]
    status, reason_code, message, proof_source = MERGIFY_QUEUE_PROOF_SOURCE_STATES[
        tuple(merge_conditions) != tuple(queue_conditions)
    ]
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "queue",
        "status": status,
        "reason_code": reason_code,
        "message": message.format(queue_rule=queue_rule),
        "evidence": {
            "queue_rule": queue_rule,
            "proof_source": proof_source,
            "queue_conditions": queue_conditions,
            "merge_conditions": merge_conditions,
        },
    }


def selected_mergify_queue_proof_source_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    return tuple(
        mergify_queue_proof_source_finding(rules_by_name[queue_rule])
        for queue_rule in sorted(mergify_route_queue_rules(route_findings))
    )


def mergify_required_reviewer_finding(rule: Mapping[str, object]) -> dict[str, object]:
    queue_rule = str(rule["name"])
    merge_conditions = [str(condition) for condition in tuple(rule["merge_conditions"])]
    reviewers = MERGIFY_REQUIRED_REVIEWER_RE.findall("\n".join(merge_conditions))
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "queue",
        "status": STATUS_READY,
        "reason_code": "mergify_required_reviewer",
        "message": f"Mergify queue rule {queue_rule} requires review from {', '.join(reviewers)}",
        "evidence": {
            "queue_rule": queue_rule,
            "reviewers": reviewers,
            "merge_conditions": merge_conditions,
        },
    }


def selected_mergify_required_reviewer_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    return tuple(
        mergify_required_reviewer_finding(rules_by_name[queue_rule])
        for queue_rule in sorted(mergify_route_queue_rules(route_findings))
    )


def mergify_queue_rules_by_name(config: Mapping[str, object]) -> dict[str, Mapping[str, object]]:
    return {
        str(rule["name"]): rule
        for rule in tuple(config["queue_rules"])
    }


def mergify_queue_batch_max(rule: Mapping[str, object]) -> int:
    batch_size = rule["batch_size"]
    return MERGIFY_BATCH_SIZE_EXTRACTORS[isinstance(batch_size, Mapping)](batch_size)


def mergify_queue_batch_above_max_finding(
    *,
    queue_rule: str,
    prs: Sequence[int],
    max_batch_size: int,
) -> dict[str, object]:
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "queue",
        "status": STATUS_READY,
        "reason_code": "mergify_queue_batch_above_max",
        "message": f"Mergify queue rule {queue_rule} selected {len(prs)} PRs above max batch size {max_batch_size}",
        "evidence": {
            "queue_rule": queue_rule,
            "prs": list(prs),
            "selected_count": len(prs),
            "max_batch_size": max_batch_size,
        },
    }


def selected_mergify_queue_batch_above_max_findings(
    *,
    queue_rule: str,
    prs: Sequence[int],
    max_batch_size: int,
) -> tuple[dict[str, object], ...]:
    return (
        mergify_queue_batch_above_max_finding(
            queue_rule=queue_rule,
            prs=prs,
            max_batch_size=max_batch_size,
        ),
    )


def selected_mergify_queue_batch_within_max_findings(
    *,
    queue_rule: str,
    prs: Sequence[int],
    max_batch_size: int,
) -> tuple[dict[str, object], ...]:
    return ()


MERGIFY_BATCH_SIZE_FINDING_BUILDERS = {
    True: selected_mergify_queue_batch_above_max_findings,
    False: selected_mergify_queue_batch_within_max_findings,
}


def mergify_queue_batch_size_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    groups = mergify_route_queue_groups(route_findings)
    return tuple(
        finding
        for queue_rule, prs in groups.items()
        for max_batch_size in (mergify_queue_batch_max(rules_by_name[queue_rule]),)
        for finding in MERGIFY_BATCH_SIZE_FINDING_BUILDERS[len(prs) > max_batch_size](
            queue_rule=queue_rule,
            prs=prs,
            max_batch_size=max_batch_size,
        )
    )


def available_mergify_queue_route_findings(
    *,
    config: Mapping[str, object],
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    return tuple(
        mergify_queue_route_finding(
            item,
            selected_mergify_queue_rule(config, labels),
            labels,
        )
        for item in filter(lambda candidate: "metadata" in candidate, readiness)
        for labels in (readiness_label_names(item),)
    )


def available_mergify_config_route_and_batch_findings(
    *,
    config: Mapping[str, object],
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    route_findings = available_mergify_queue_route_findings(config=config, readiness=readiness)
    return (
        *route_findings,
        *selected_mergify_queue_proof_source_findings(
            config=config,
            route_findings=route_findings,
        ),
        *selected_mergify_required_reviewer_findings(
            config=config,
            route_findings=route_findings,
        ),
        *mergify_queue_batch_size_findings(config=config, route_findings=route_findings),
    )


def unavailable_mergify_queue_route_findings(
    *,
    config: object,
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    return ()


MERGIFY_QUEUE_ROUTE_FINDING_BUILDERS = {
    True: available_mergify_config_route_and_batch_findings,
    False: unavailable_mergify_queue_route_findings,
}


def mergify_config_findings(
    *,
    repo: pathlib.Path,
    base_sha: str,
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    snapshot = mergify_config_snapshot_finding(repo=repo, base_sha=base_sha)
    config = mergify_config_data(repo=repo, blob_sha=str(snapshot["evidence"]["blob_sha"]))
    return (
        snapshot,
        mergify_config_validation_finding(
            repo=repo,
            base_sha=base_sha,
            blob_sha=str(snapshot["evidence"]["blob_sha"]),
        ),
        *MERGIFY_QUEUE_ROUTE_FINDING_BUILDERS[isinstance(config, Mapping)](
            config=config,
            readiness=readiness,
        ),
    )


def contract_result(findings: Sequence[dict[str, object]], *, wave_status: str) -> dict[str, object]:
    lane_statuses = {
        lane: contract_lane_status(findings, lane)
        for lane in CONTRACT_LANES
    }
    aggregate_status = min(lane_statuses.values(), key=CONTRACT_STATUS_RANK.__getitem__)
    verdict = {
        **CONTRACT_STATUS_VERDICTS,
        STATUS_READY: CONTRACT_READY_WAVE_VERDICTS[wave_status],
    }[aggregate_status]
    return {
        "verdict": verdict,
        "exit_code": CONTRACT_VERDICT_EXIT_CODES[verdict],
        "lane_statuses": lane_statuses,
    }


def classify_required_check_state(
    *,
    check_name: str,
    raw_state: str,
    expected_head: str,
    actual_head: str,
    evidence: Mapping[str, object],
) -> dict[str, object]:
    normalized_state = normalize_check_state(raw_state)
    status, reason_code = CHECK_STATE_CLASSIFICATIONS.get(
        normalized_state,
        CHECK_STATE_UNKNOWN,
    )
    if actual_head != expected_head:
        status, reason_code = CHECK_STATE_STALE
    return {
        "lane": LANE_READINESS,
        "scope": "pr",
        "status": status,
        "reason_code": reason_code,
        "message": f"required check {check_name} is {reason_code}",
        "evidence": {
            **evidence,
            "check_name": check_name,
            "raw_state": raw_state,
            "normalized_state": normalized_state,
            "expected_head": expected_head,
            "actual_head": actual_head,
        },
    }


def preflight_artifact_finding(artifact: Mapping[str, object]) -> dict[str, object]:
    artifact_type = str(artifact["type"])
    lane, scope, status = PREFLIGHT_ARTIFACT_CLASSIFICATIONS[artifact_type]
    return {
        "lane": lane,
        "scope": scope,
        "status": status,
        "reason_code": artifact_type,
        "message": artifact_type,
        "evidence": dict(artifact),
    }


def evaluate_preflight_contract(evidence: ContractEvidence) -> dict[str, object]:
    findings = (
        *evidence.findings,
        *(preflight_artifact_finding(artifact) for artifact in evidence.artifacts),
    )
    result = contract_result(findings, wave_status=evidence.wave_status)
    return {
        "verdict": result["verdict"],
        "exit_code": result["exit_code"],
        "lane_statuses": result["lane_statuses"],
        "findings": list(findings),
        "wave_status": evidence.wave_status,
    }


def mergify_route_queue_rules(findings: Sequence[Mapping[str, object]]) -> frozenset[str]:
    return frozenset(
        str(dict(finding["evidence"])["queue_rule"])
        for finding in filter(
            lambda candidate: candidate["reason_code"] == "mergify_queue_route_selected",
            findings,
        )
    )


def mergify_wave_status(findings: Sequence[Mapping[str, object]]) -> str:
    return MERGIFY_QUEUE_WAVE_STATUSES[
        len(mergify_route_queue_rules(findings)) > 1
        or bool(MERGIFY_SPLIT_REASON_CODES & frozenset(str(finding["reason_code"]) for finding in findings))
    ]


@dataclasses.dataclass(frozen=True)
class CommandResult:
    args: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str


@dataclasses.dataclass(frozen=True)
class MergeResult:
    clean: bool
    tree: str | None
    files: tuple[str, ...]
    raw: str


@dataclasses.dataclass(frozen=True)
class PrHead:
    number: int
    sha: str


@dataclasses.dataclass(frozen=True)
class SyntheticCommit:
    commit: str
    prs: tuple[int, ...]


@dataclasses.dataclass(frozen=True)
class VerifierResult:
    command: str
    returncode: int
    stdout: str
    stderr: str

    def as_public_json(self, output_policy: OutputPolicy) -> dict[str, object]:
        payload: dict[str, object] = {
            "command": self.command,
            "returncode": self.returncode,
        }
        if self.returncode != 0:
            for stream in VERIFIER_STREAMS:
                preview = bounded_stream(self.stream(stream), output_policy)
                payload.update(preview.as_fields(stream))
        return payload

    def stream(self, name: str) -> str:
        if name == "stdout":
            return self.stdout
        if name == "stderr":
            return self.stderr
        raise PreflightError(f"unknown verifier stream {name!r}")


@dataclasses.dataclass(frozen=True)
class OutputPolicy:
    verifier_stream_max_lines: int
    verifier_stream_max_bytes: int

    def as_json(self) -> dict[str, int]:
        return {
            "verifier_stream_max_lines": self.verifier_stream_max_lines,
            "verifier_stream_max_bytes": self.verifier_stream_max_bytes,
        }


@dataclasses.dataclass(frozen=True)
class StreamPreview:
    text: str
    truncated: bool

    def as_fields(self, stream: str) -> dict[str, object]:
        return {
            f"{stream}_preview": self.text,
            f"{stream}_truncated": self.truncated,
        }


@dataclasses.dataclass(frozen=True)
class ReadinessIssue:
    code: str
    message: str

    def as_json(self) -> dict[str, str]:
        return {
            "code": self.code,
            "message": self.message,
        }


@dataclasses.dataclass(frozen=True)
class MetadataExpectation:
    code: str
    field: str
    expected: object
    message: str
    warn_when_missing: bool = True

    def evaluate(self, payload: dict[str, object]) -> ReadinessIssue | None:
        actual = payload.get(self.field)
        if actual == self.expected:
            return None
        if actual is None and not self.warn_when_missing:
            return None
        return ReadinessIssue(
            code=self.code,
            message=self.message.format(actual=actual, expected=self.expected),
        )


@dataclasses.dataclass(frozen=True)
class DynamicExpectation:
    code: str
    field: str
    expected_name: str
    message: str

    def evaluate(
        self,
        payload: dict[str, object],
        expected_values: dict[str, str | None],
    ) -> ReadinessIssue | None:
        expected = expected_values[self.expected_name]
        if expected is None:
            return None
        actual = payload.get(self.field)
        if actual == expected:
            return None
        return ReadinessIssue(
            code=self.code,
            message=self.message.format(actual=actual, expected=expected),
        )


@dataclasses.dataclass(frozen=True)
class Batch:
    index: int
    commit: str
    prs: tuple[int, ...]
    verifiers: tuple[VerifierResult, ...]

    def as_json(self, output_policy: OutputPolicy) -> dict[str, object]:
        return {
            "index": self.index,
            "prs": list(self.prs),
            "status": STATUS_READY,
            "verifiers": [result.as_public_json(output_policy) for result in self.verifiers],
        }


@dataclasses.dataclass(frozen=True)
class PreflightConfig:
    origin: str
    base: str
    default_verifier_profile: str
    verifier_profiles: dict[str, tuple[str, ...]]
    output_policy: OutputPolicy


STATIC_READINESS_EXPECTATIONS = (
    MetadataExpectation("not_open", "state", "OPEN", "PR is not open"),
    MetadataExpectation("draft", "isDraft", False, "PR is draft", warn_when_missing=False),
    MetadataExpectation(
        "not_mergeable",
        "mergeable",
        "MERGEABLE",
        "PR mergeable state is {actual}",
    ),
    MetadataExpectation(
        "review_not_approved",
        "reviewDecision",
        "APPROVED",
        "review decision is {actual}",
    ),
)
DYNAMIC_READINESS_EXPECTATIONS = (
    DynamicExpectation(
        "base_mismatch",
        "baseRefName",
        "expected_base",
        "PR targets base {actual!r}, expected {expected!r}",
    ),
    DynamicExpectation(
        "head_mismatch",
        "headRefOid",
        "fetched_head",
        "GitHub headRefOid {actual} does not match fetched PR head {expected}",
    ),
)
CHECK_BUCKET_ISSUES = {
    "fail": ("required_check_failed", "required check failed: {name}"),
    "cancel": ("required_check_failed", "required check failed: {name}"),
    "pending": ("required_check_pending", "required check pending: {name}"),
}
READINESS_ISSUE_ARTIFACT_TYPES = {
    "base_mismatch": "base_mismatch",
    "draft": "readiness_failed",
    "head_mismatch": "head_mismatch",
    "not_mergeable": "readiness_failed",
    "not_open": "readiness_failed",
    "required_check_failed": "required_check_failed",
    "required_check_pending": "required_check_pending",
    "review_not_approved": "readiness_failed",
}
READINESS_ISSUE_STATUS_RANKS = {
    issue_code: CONTRACT_STATUS_RANK[PREFLIGHT_ARTIFACT_CLASSIFICATIONS[artifact_type][2]]
    for issue_code, artifact_type in READINESS_ISSUE_ARTIFACT_TYPES.items()
}


def run_command(
    args: Sequence[str],
    *,
    cwd: pathlib.Path,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> CommandResult:
    completed = subprocess.run(
        list(args),
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
    )
    result = CommandResult(
        args=tuple(args),
        returncode=completed.returncode,
        stdout=completed.stdout,
        stderr=completed.stderr,
    )
    if check and result.returncode != 0:
        rendered = " ".join(shlex.quote(part) for part in result.args)
        raise PreflightError(
            f"command failed ({result.returncode}): {rendered}\n{result.stderr}{result.stdout}"
        )
    return result


def git(repo: pathlib.Path, *args: str, check: bool = True) -> CommandResult:
    return run_command(["git", *args], cwd=repo, check=check)


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise PreflightError(f"config missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise PreflightError(f"config is invalid TOML: {exc}") from exc
    if not isinstance(data, dict):
        raise PreflightError("config root must be a TOML table")
    return data


def require_table(parent: dict[str, object], key: str, prefix: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise PreflightError(f"{prefix}.{key} must be a table")
    return value


def require_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value:
        raise PreflightError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise PreflightError(f"{prefix}.{key} must be a positive integer")
    return value


def load_config(path: pathlib.Path) -> PreflightConfig:
    root = load_toml(path)
    settings = require_table(root, "merge_queue_preflight", "config")
    origin = require_string(settings, "origin", "config.merge_queue_preflight")
    base = require_string(settings, "base", "config.merge_queue_preflight")
    default_profile = require_string(
        settings, "default_verifier_profile", "config.merge_queue_preflight"
    )
    profiles_root = require_table(
        settings, "verifier_profiles", "config.merge_queue_preflight"
    )
    output_settings = require_table(settings, "output", "config.merge_queue_preflight")
    output_policy = OutputPolicy(
        verifier_stream_max_lines=require_positive_int(
            output_settings,
            "verifier_stream_max_lines",
            "config.merge_queue_preflight.output",
        ),
        verifier_stream_max_bytes=require_positive_int(
            output_settings,
            "verifier_stream_max_bytes",
            "config.merge_queue_preflight.output",
        ),
    )
    profiles: dict[str, tuple[str, ...]] = {}
    for profile_name, raw_profile in profiles_root.items():
        if not isinstance(raw_profile, dict):
            raise PreflightError(
                f"config.merge_queue_preflight.verifier_profiles.{profile_name} must be a table"
            )
        raw_commands = raw_profile.get("commands")
        if not isinstance(raw_commands, list) or any(
            not isinstance(command, str) or not command for command in raw_commands
        ):
            raise PreflightError(
                f"config.merge_queue_preflight.verifier_profiles.{profile_name}.commands must be a string array"
            )
        profiles[profile_name] = tuple(raw_commands)
    if default_profile not in profiles:
        raise PreflightError(
            f"config.merge_queue_preflight.default_verifier_profile {default_profile!r} has no profile"
        )
    return PreflightConfig(
        origin=origin,
        base=base,
        default_verifier_profile=default_profile,
        verifier_profiles=profiles,
        output_policy=output_policy,
    )


def positive_pr_number(value: str) -> int:
    if not value.isdecimal() or int(value) <= 0:
        raise argparse.ArgumentTypeError("PR numbers must be positive integers")
    return int(value)


def commit_sha(value: str) -> str:
    if SHA_RE.fullmatch(value) is None:
        raise argparse.ArgumentTypeError("commit SHAs must be 40 lowercase hex characters")
    return value


def expected_head_sha(value: str) -> ExpectedHead:
    parsed = EXPECTED_HEAD_SHA_RE.fullmatch(value)
    if parsed is None:
        raise argparse.ArgumentTypeError(
            "--expected-head-sha must use PR=40-lowercase-hex-SHA"
        )
    return ExpectedHead(pr=int(parsed.group("pr")), sha=parsed.group("sha"))


def format_pr_numbers(values: Sequence[int]) -> str:
    return ", ".join(f"#{value}" for value in values)


def expected_head_map(entries: Sequence[ExpectedHead], requested: Sequence[int]) -> dict[int, str]:
    counts = Counter(entry.pr for entry in entries)
    duplicates = tuple(sorted(pr for pr, count in counts.items() if count > 1))
    expected = {entry.pr: entry.sha for entry in entries}
    requested_prs = frozenset(requested)
    expected_prs = frozenset(expected)
    missing = tuple(sorted(requested_prs - expected_prs))
    extra = tuple(sorted(expected_prs - requested_prs))
    violations = tuple(
        violation
        for violation in (
            ExpectedHeadMapViolation(duplicates, "--expected-head-sha repeated for PR {prs}"),
            ExpectedHeadMapViolation(missing, "--expected-head-sha missing for PR {prs}"),
            ExpectedHeadMapViolation(extra, "--expected-head-sha supplied for unrequested PR {prs}"),
        )
        if violation.prs
    )
    if violations:
        raise PreflightError(violations[0].message())
    return expected


def unique_preserving_order(values: Sequence[int]) -> tuple[int, ...]:
    seen: set[int] = set()
    ordered: list[int] = []
    for value in values:
        if value in seen:
            raise PreflightError(f"PR #{value} was provided more than once")
        seen.add(value)
        ordered.append(value)
    return tuple(ordered)


def fetch_base(repo: pathlib.Path, origin: str, base: str) -> str:
    git(repo, "fetch", "--quiet", origin, base)
    sha = git(repo, "rev-parse", FETCH_HEAD).stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError(f"base {base!r} did not resolve to a commit SHA")
    return sha


def fetch_pr_head(repo: pathlib.Path, origin: str, pr_number: int) -> PrHead:
    missing_ref_message = "couldn't find remote ref"
    try:
        git(repo, "fetch", "--quiet", origin, f"{PR_REF_PREFIX}{pr_number}/head")
    except PreflightError as exc:
        if missing_ref_message not in str(exc):
            raise
        raise PreflightError(
            f"PR #{pr_number} head ref was not found; ensure the PR exists and has a fetchable head"
        ) from exc
    sha = git(repo, "rev-parse", FETCH_HEAD).stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError(f"PR #{pr_number} did not resolve to a commit SHA")
    return PrHead(number=pr_number, sha=sha)


def parse_conflict_files(output: str) -> tuple[str, ...]:
    files: set[str] = set()
    for line in output.splitlines():
        match = CONFLICT_LINE_RE.match(line)
        if match is not None:
            files.add(match.group(1))
    if files:
        return tuple(sorted(files))
    fallback: set[str] = set()
    for line in output.splitlines():
        if line.startswith("CONFLICT ") and " in " in line:
            fallback.add(line.rsplit(" in ", 1)[1])
    return tuple(sorted(fallback))


def merge_tree(repo: pathlib.Path, left: str, right: str) -> MergeResult:
    result = git(repo, "merge-tree", "--write-tree", left, right, check=False)
    output = result.stdout + result.stderr
    if result.returncode == 0:
        tree = result.stdout.splitlines()[0].strip()
        if SHA_RE.fullmatch(tree) is None:
            raise PreflightError("git merge-tree returned an invalid tree SHA")
        return MergeResult(clean=True, tree=tree, files=(), raw=output)
    return MergeResult(
        clean=False,
        tree=None,
        files=parse_conflict_files(output),
        raw=output,
    )


def commit_tree(repo: pathlib.Path, tree: str, parents: Sequence[str], message: str) -> str:
    args = ["commit-tree", tree]
    for parent in parents:
        args.extend(["-p", parent])
    env = os.environ.copy()
    env.setdefault("GIT_AUTHOR_NAME", "merge-queue-preflight")
    env.setdefault("GIT_AUTHOR_EMAIL", "merge-queue-preflight@example.invalid")
    env.setdefault("GIT_COMMITTER_NAME", "merge-queue-preflight")
    env.setdefault("GIT_COMMITTER_EMAIL", "merge-queue-preflight@example.invalid")
    completed = subprocess.run(
        ["git", *args],
        cwd=repo,
        input=message,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        env=env,
    )
    if completed.returncode != 0:
        raise PreflightError(f"git commit-tree failed: {completed.stderr}{completed.stdout}")
    sha = completed.stdout.strip()
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError("git commit-tree returned an invalid commit SHA")
    return sha


def synthesize_merge(
    repo: pathlib.Path,
    left_commit: str,
    right_commit: str,
    prs: Sequence[int],
) -> SyntheticCommit | MergeResult:
    merged = merge_tree(repo, left_commit, right_commit)
    if not merged.clean or merged.tree is None:
        return merged
    message = "merge queue preflight: " + ",".join(f"#{pr}" for pr in prs)
    commit = commit_tree(repo, merged.tree, [left_commit, right_commit], message)
    return SyntheticCommit(commit=commit, prs=tuple(prs))


def run_verifier_commands(
    repo: pathlib.Path,
    commit: str,
    commands: Sequence[str],
) -> tuple[VerifierResult, ...]:
    if not commands:
        return ()
    results: list[VerifierResult] = []
    with tempfile.TemporaryDirectory(prefix="merge-queue-preflight-") as tmp:
        worktree = pathlib.Path(tmp) / "worktree"
        git(repo, "worktree", "add", "--quiet", "--detach", str(worktree), commit)
        try:
            for command in commands:
                parts = shlex.split(command)
                if not parts:
                    raise PreflightError("verifier command must not be empty")
                completed = run_command(parts, cwd=worktree, check=False)
                verifier_result = VerifierResult(
                    command=command,
                    returncode=completed.returncode,
                    stdout=completed.stdout,
                    stderr=completed.stderr,
                )
                results.append(verifier_result)
                if verifier_result.returncode != 0:
                    break
        finally:
            git(repo, "worktree", "remove", "--force", str(worktree), check=False)
    return tuple(results)


def first_failed_verifier(results: Sequence[VerifierResult]) -> VerifierResult | None:
    for result in results:
        if result.returncode != 0:
            return result
    return None


def verifier_block(pr: int, result: VerifierResult, output_policy: OutputPolicy) -> dict[str, object]:
    return {
        "pr": pr,
        "reason": f"verifier failed: {result.command}",
        "type": "verifier_failed",
        **result.as_public_json(output_policy),
    }


def gh_json(
    args: Sequence[str],
    *,
    allowed_returncodes: Sequence[int] = (0,),
) -> object:
    try:
        completed = subprocess.run(
            ["gh", *args],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except FileNotFoundError as exc:
        raise PreflightError("gh executable not found") from exc
    if completed.returncode not in allowed_returncodes:
        raise PreflightError(f"gh {' '.join(args)} failed: {completed.stderr}{completed.stdout}")
    try:
        return json.loads(completed.stdout or "[]")
    except json.JSONDecodeError as exc:
        raise PreflightError(f"gh {' '.join(args)} returned invalid JSON") from exc


def readiness_issues(
    payload: dict[str, object],
    checks: Sequence[object],
    *,
    expected_base: str | None,
    fetched_head: str | None,
) -> tuple[ReadinessIssue, ...]:
    expected_values = {
        "expected_base": expected_base,
        "fetched_head": fetched_head,
    }
    issues = [
        issue
        for rule in STATIC_READINESS_EXPECTATIONS
        if (issue := rule.evaluate(payload)) is not None
    ]
    issues.extend(
        issue
        for rule in DYNAMIC_READINESS_EXPECTATIONS
        if (issue := rule.evaluate(payload, expected_values)) is not None
    )
    for check in checks:
        if not isinstance(check, dict):
            continue
        bucket = check.get("bucket")
        issue_template = CHECK_BUCKET_ISSUES.get(bucket)
        if issue_template is None:
            continue
        code, message = issue_template
        issues.append(
            ReadinessIssue(
                code=code,
                message=message.format(name=check.get("name")),
            )
        )
    return tuple(issues)


def pr_readiness(
    pr_number: int,
    *,
    use_gh: bool,
    expected_base: str | None = None,
    fetched_head: str | None = None,
) -> dict[str, object]:
    if not use_gh:
        return {"pr": pr_number, "warnings": [], "warning_details": [], "checks": []}
    payload = gh_json(
        [
            "pr",
            "view",
            str(pr_number),
            "--json",
            "number,state,isDraft,mergeable,reviewDecision,headRefOid,baseRefName,labels,title,url",
        ]
    )
    if not isinstance(payload, dict):
        raise PreflightError(f"gh pr view {pr_number} did not return an object")
    checks = gh_json(
        [
            "pr",
            "checks",
            str(pr_number),
            "--required",
            "--json",
            "name,state,bucket,workflow",
        ],
        allowed_returncodes=GH_PR_CHECKS_JSON_RETURNCODES,
    )
    if not isinstance(checks, list):
        raise PreflightError(f"gh pr checks {pr_number} did not return a list")
    issues = readiness_issues(
        payload,
        checks,
        expected_base=expected_base,
        fetched_head=fetched_head,
    )
    return {
        "pr": pr_number,
        "warnings": [issue.message for issue in issues],
        "warning_details": [issue.as_json() for issue in issues],
        "metadata": payload,
        "checks": checks,
    }


def readiness_for_wave(
    pr_numbers: Sequence[int],
    *,
    use_gh: bool,
    base: str,
) -> tuple[list[dict[str, object]], list[str]]:
    if not use_gh:
        return [pr_readiness(pr, use_gh=False) for pr in pr_numbers], []
    readiness: list[dict[str, object]] = []
    metadata_warnings: list[str] = []
    for pr in pr_numbers:
        try:
            readiness.append(
                pr_readiness(
                    pr,
                    use_gh=True,
                    expected_base=base,
                )
            )
        except PreflightError as exc:
            warning = f"GitHub metadata unavailable for PR #{pr}; readiness checks skipped: {exc}"
            metadata_warnings.append(warning)
            readiness.append(
                {
                    "pr": pr,
                    "warnings": [],
                    "warning_details": [],
                    "checks": [],
                    "metadata_unavailable": True,
                    "metadata_error": str(exc),
                }
            )
    return readiness, metadata_warnings


def readiness_with_fetched_heads(
    readiness: Sequence[dict[str, object]],
    *,
    base: str,
    heads: Mapping[int, PrHead],
) -> list[dict[str, object]]:
    updated: list[dict[str, object]] = []
    for item in readiness:
        pr = int(item["pr"])
        head = heads.get(pr)
        metadata = item.get("metadata")
        if head is None or not isinstance(metadata, dict):
            updated.append(item)
            continue
        checks = item.get("checks")
        if not isinstance(checks, list):
            updated.append(item)
            continue
        issues = readiness_issues(
            metadata,
            checks,
            expected_base=base,
            fetched_head=head.sha,
        )
        updated.append(
            {
                **item,
                "warnings": [issue.message for issue in issues],
                "warning_details": [issue.as_json() for issue in issues],
            }
        )
    return updated


def fetch_available_pr_heads(
    *,
    repo: pathlib.Path,
    origin: str,
    requested: Sequence[int],
    blocked_numbers: set[int],
) -> tuple[dict[int, PrHead], list[dict[str, object]]]:
    heads: dict[int, PrHead] = {}
    blocks: list[dict[str, object]] = []
    missing_head_prefix = "PR #"
    missing_head_suffix = "head ref was not found"
    for pr in requested:
        if pr in blocked_numbers:
            continue
        try:
            heads[pr] = fetch_pr_head(repo, origin, pr)
        except PreflightError as exc:
            reason = str(exc)
            block_type = "head_unavailable"
            if not (reason.startswith(missing_head_prefix) and missing_head_suffix in reason):
                block_type = "head_fetch_failed"
            blocks.append(
                {
                    "pr": pr,
                    "reason": reason,
                    "type": block_type,
                }
            )
    return heads, blocks


def metadata_unavailable_block(readiness: dict[str, object]) -> dict[str, object] | None:
    if readiness.get("metadata_unavailable") is not True:
        return None
    reason = str(readiness.get("metadata_error", "GitHub metadata unavailable"))
    return {
        "pr": readiness["pr"],
        "reason": reason,
        "type": "metadata_unavailable",
    }


def readiness_warning_block(readiness: dict[str, object]) -> dict[str, object] | None:
    warnings = readiness.get("warnings", [])
    if not warnings:
        return None
    warning_details = readiness["warning_details"]
    issue_code = min(
        (str(detail["code"]) for detail in warning_details),
        key=READINESS_ISSUE_STATUS_RANKS.__getitem__,
    )
    return {
        "pr": readiness["pr"],
        "reason": "; ".join(str(warning) for warning in warnings),
        "type": READINESS_ISSUE_ARTIFACT_TYPES[issue_code],
    }


READINESS_BLOCK_CLASSIFIERS = (
    metadata_unavailable_block,
    readiness_warning_block,
)


def readiness_blocks(readiness: Sequence[dict[str, object]]) -> list[dict[str, object]]:
    blocks: list[dict[str, object]] = []
    for item in readiness:
        blocks.extend(
            block
            for classifier in READINESS_BLOCK_CLASSIFIERS
            if (block := classifier(item)) is not None
        )
    return blocks


def available_readiness_ready_findings(item: Mapping[str, object]) -> tuple[dict[str, object], ...]:
    metadata = dict(item["metadata"])
    pr = int(item["pr"])
    return (
        {
            "lane": LANE_READINESS,
            "scope": "pr",
            "status": STATUS_READY,
            "reason_code": "readiness_ready",
            "message": f"PR #{pr} has authoritative readiness metadata with no warnings",
            "evidence": {
                "pr": pr,
                "baseRefName": metadata["baseRefName"],
                "headRefOid": metadata["headRefOid"],
                "mergeable": metadata["mergeable"],
                "reviewDecision": metadata["reviewDecision"],
                "checks": list(item["checks"]),
            },
        },
    )


def no_readiness_ready_findings(item: Mapping[str, object]) -> tuple[dict[str, object], ...]:
    return ()


READINESS_READY_FINDING_BUILDERS = {
    True: available_readiness_ready_findings,
    False: no_readiness_ready_findings,
}


def readiness_ready_findings(readiness: Sequence[Mapping[str, object]]) -> tuple[dict[str, object], ...]:
    return tuple(
        finding
        for item in readiness
        for finding in READINESS_READY_FINDING_BUILDERS[
            "metadata" in item and not tuple(item["warning_details"])
        ](item)
    )


def preflight(
    *,
    repo: pathlib.Path,
    origin: str,
    base: str,
    expected_base_sha: str,
    expected_head_inputs: Sequence[ExpectedHead],
    pr_numbers: Sequence[int],
    verifier_commands: Sequence[str],
    output_policy: OutputPolicy,
    use_gh: bool,
) -> tuple[dict[str, object], int]:
    requested = unique_preserving_order(pr_numbers)
    expected_heads = expected_head_map(expected_head_inputs, requested)
    actual_base_sha = fetch_base(repo, origin, base)
    base_sha = expected_base_sha
    readiness, metadata_warnings = readiness_for_wave(
        requested,
        use_gh=use_gh,
        base=base,
    )
    initial_readiness_blocks = readiness_blocks(readiness)
    initial_blocked_numbers = {int(block["pr"]) for block in initial_readiness_blocks}
    heads, head_fetch_blocks = fetch_available_pr_heads(
        repo=repo,
        origin=origin,
        requested=requested,
        blocked_numbers=initial_blocked_numbers,
    )
    readiness = readiness_with_fetched_heads(
        readiness,
        base=base,
        heads=heads,
    )
    blocked_prs = [
        *head_fetch_blocks,
        *head_identity_blocks(expected_heads=expected_heads, actual_heads=heads),
        *readiness_blocks(readiness),
    ]
    blocked_numbers = {int(block["pr"]) for block in blocked_prs}
    base_commits: dict[int, SyntheticCommit] = {}
    base_verifiers: dict[int, tuple[VerifierResult, ...]] = {}
    for pr in requested:
        if pr in blocked_numbers:
            continue
        head = heads[pr]
        synthetic = synthesize_merge(repo, base_sha, head.sha, [pr])
        if isinstance(synthetic, MergeResult):
            blocked_prs.append(
                {
                    "pr": pr,
                    "reason": "conflicts with base",
                    "files": list(synthetic.files),
                    "type": "base_conflict",
                }
            )
            blocked_numbers.add(pr)
            continue
        verifier_results = run_verifier_commands(repo, synthetic.commit, verifier_commands)
        failed = first_failed_verifier(verifier_results)
        if failed is not None:
            blocked_prs.append(verifier_block(pr, failed, output_policy))
            blocked_numbers.add(pr)
            continue
        base_commits[pr] = synthetic
        base_verifiers[pr] = verifier_results

    conflicts: list[dict[str, object]] = []
    batches: list[Batch] = []
    current: SyntheticCommit | None = None
    current_verifiers: tuple[VerifierResult, ...] = ()
    batch_index = 1
    for pr in requested:
        if pr in blocked_numbers:
            continue
        pr_head = heads[pr]
        if current is None:
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_prs = [*current.prs, pr]
        synthetic = synthesize_merge(repo, current.commit, pr_head.sha, candidate_prs)
        if isinstance(synthetic, MergeResult):
            conflicts.append(
                {
                    "pr": pr,
                    "against_batch": list(current.prs),
                    "files": list(synthetic.files),
                    "type": "batch_conflict",
                }
            )
            batches.append(
                Batch(
                    index=batch_index,
                    commit=current.commit,
                    prs=current.prs,
                    verifiers=current_verifiers,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_verifiers = run_verifier_commands(repo, synthetic.commit, verifier_commands)
        failed = first_failed_verifier(candidate_verifiers)
        if failed is not None:
            conflicts.append(
                {
                    "pr": pr,
                    "against_batch": list(current.prs),
                    "type": "batch_verifier_failed",
                    **failed.as_public_json(output_policy),
                }
            )
            batches.append(
                Batch(
                    index=batch_index,
                    commit=current.commit,
                    prs=current.prs,
                    verifiers=current_verifiers,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        current = synthetic
        current_verifiers = candidate_verifiers
    if current is not None:
        batches.append(
            Batch(
                index=batch_index,
                commit=current.commit,
                prs=current.prs,
                verifiers=current_verifiers,
            )
        )
    contract_findings = (
        *base_identity_findings(
            expected_base_sha=expected_base_sha,
            actual_base_sha=actual_base_sha,
        ),
        *head_identity_findings(expected_heads=expected_heads, actual_heads=heads),
        *mergify_config_findings(repo=repo, base_sha=base_sha, readiness=readiness),
        *preflight_mode_findings(use_gh=use_gh),
        *readiness_ready_findings(readiness),
        *residual_risk_findings(),
        *integration_batch_ready_findings(batches),
        *verifier_batch_ready_findings(batches, output_policy),
    )
    contract_evaluation = evaluate_preflight_contract(
        ContractEvidence(
            findings=contract_findings,
            artifacts=(*blocked_prs, *conflicts),
            wave_status=mergify_wave_status(contract_findings),
        )
    )
    payload = {
        "base": base,
        "base_sha": base_sha,
        "actual_base_sha": actual_base_sha,
        "expected_base_sha": expected_base_sha,
        "expected_pr_heads": {str(number): sha for number, sha in expected_heads.items()},
        "requested_prs": list(requested),
        "pr_heads": {str(number): head.sha for number, head in heads.items()},
        "readiness": readiness,
        "metadata_warnings": metadata_warnings,
        "residual_risks": list(RESIDUAL_RISK_REASON_CODES),
        "batches": [batch.as_json(output_policy) for batch in batches],
        "blocked_prs": blocked_prs,
        "conflicts": conflicts,
        "contract_exit_code": contract_evaluation["exit_code"],
        "findings": contract_evaluation["findings"],
        "lane_statuses": contract_evaluation["lane_statuses"],
        "verdict": contract_evaluation["verdict"],
        "wave_status": contract_evaluation["wave_status"],
        "output_policy": output_policy.as_json(),
    }
    exit_code = int(contract_evaluation["exit_code"])
    return payload, exit_code


def output_policy_from_payload(payload: dict[str, object]) -> OutputPolicy:
    value = payload["output_policy"]
    if not isinstance(value, dict):
        raise PreflightError("payload output_policy must be an object")
    return OutputPolicy(
        verifier_stream_max_lines=int(value["verifier_stream_max_lines"]),
        verifier_stream_max_bytes=int(value["verifier_stream_max_bytes"]),
    )


def bounded_stream(output: str, output_policy: OutputPolicy) -> StreamPreview:
    encoded = output.encode("utf-8")
    byte_truncated = len(encoded) > output_policy.verifier_stream_max_bytes
    if byte_truncated:
        output = encoded[: output_policy.verifier_stream_max_bytes].decode(
            "utf-8",
            errors="ignore",
        )
    stream_lines = output.rstrip().splitlines()
    line_truncated = len(stream_lines) > output_policy.verifier_stream_max_lines
    text = "\n".join(stream_lines[: output_policy.verifier_stream_max_lines])
    return StreamPreview(text=text, truncated=byte_truncated or line_truncated)


def append_verifier_result(
    lines: list[str],
    verifier: dict[str, object],
    *,
    indent: str,
    output_policy: OutputPolicy,
) -> None:
    lines.append(
        "{indent}verifier {command}: exit {returncode}".format(
            indent=indent,
            command=verifier["command"],
            returncode=verifier["returncode"],
        )
    )
    if verifier["returncode"] == 0:
        return
    for stream in VERIFIER_STREAMS:
        preview = str(verifier.get(f"{stream}_preview", ""))
        truncated = bool(verifier.get(f"{stream}_truncated", False))
        if not preview and not truncated:
            continue
        lines.append(f"{indent}  {stream}:")
        lines.extend(f"{indent}    {line}" for line in preview.splitlines())
        if truncated:
            lines.append(f"{indent}    ... truncated by merge_queue_preflight output policy")


def plain_text(payload: dict[str, object]) -> str:
    output_policy = output_policy_from_payload(payload)
    lines = [
        f"base: {payload['base']} {payload['base_sha']}",
        "requested PRs: " + ", ".join(f"#{pr}" for pr in payload["requested_prs"]),
        "recommended batches:",
    ]
    for batch in payload["batches"]:
        lines.append("  batch {index}: {prs}".format(
            index=batch["index"],
            prs=", ".join(f"#{pr}" for pr in batch["prs"]),
        ))
        for verifier in batch["verifiers"]:
            append_verifier_result(
                lines,
                verifier,
                indent="    ",
                output_policy=output_policy,
            )
    if payload["blocked_prs"]:
        lines.append("blocked PRs:")
        for item in payload["blocked_prs"]:
            lines.append(f"  #{item['pr']}: {item['reason']}")
            if item.get("files"):
                lines.append("    files: " + ", ".join(item["files"]))
            if "command" in item:
                append_verifier_result(
                    lines,
                    item,
                    indent="    ",
                    output_policy=output_policy,
                )
    if payload["metadata_warnings"]:
        lines.append("metadata warnings:")
        for warning in payload["metadata_warnings"]:
            lines.append(f"  {warning}")
    if payload["conflicts"]:
        lines.append("conflicts:")
        for item in payload["conflicts"]:
            context = ", ".join(f"#{pr}" for pr in item.get("against_batch", []))
            lines.append(f"  #{item['pr']} vs [{context}]: {item['type']}")
            if item.get("files"):
                lines.append("    files: " + ", ".join(item["files"]))
            if "command" in item:
                append_verifier_result(
                    lines,
                    item,
                    indent="    ",
                    output_policy=output_policy,
                )
    lines.append("residual risks:")
    lines.extend(f"  {reason_code}" for reason_code in payload["residual_risks"])
    warnings = [
        (item["pr"], warning)
        for item in payload["readiness"]
        for warning in item.get("warnings", [])
    ]
    if warnings:
        lines.append("readiness warnings:")
        for pr, warning in warnings:
            lines.append(f"  #{pr}: {warning}")
    return "\n".join(lines)


def parser() -> argparse.ArgumentParser:
    root = PreflightArgumentParser(prog="merge_queue_preflight.py")
    root.add_argument("prs", nargs="+", type=positive_pr_number)
    root.add_argument("--base")
    root.add_argument("--expected-base-sha", required=True, type=commit_sha)
    root.add_argument("--expected-head-sha", action="append", required=True, type=expected_head_sha)
    root.add_argument("--origin")
    root.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    root.add_argument("--verifier-profile")
    root.add_argument("--run-verifier", action="append", default=[])
    root.add_argument("--no-gh", action="store_true")
    root.add_argument("--json", action="store_true")
    return root


def verifier_commands(config: PreflightConfig, profile: str | None, extra: Sequence[str]) -> tuple[str, ...]:
    selected = profile or config.default_verifier_profile
    if selected not in config.verifier_profiles:
        raise PreflightError(f"unknown verifier profile {selected!r}")
    return (*config.verifier_profiles[selected], *extra)


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        config = load_config(args.config)
        payload, exit_code = preflight(
            repo=pathlib.Path.cwd(),
            origin=args.origin or config.origin,
            base=args.base or config.base,
            expected_base_sha=args.expected_base_sha,
            expected_head_inputs=args.expected_head_sha,
            pr_numbers=args.prs,
            verifier_commands=verifier_commands(config, args.verifier_profile, args.run_verifier),
            output_policy=config.output_policy,
            use_gh=not args.no_gh,
        )
    except PreflightError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return PREFLIGHT_USAGE_EXIT_CODE
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(plain_text(payload))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
