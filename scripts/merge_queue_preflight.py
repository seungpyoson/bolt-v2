#!/usr/bin/env python3
"""Preflight candidate PR waves before queueing them through Mergify."""

from __future__ import annotations

import argparse
import dataclasses
import enum
import functools
import json
import os
import pathlib
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import tomllib
import uuid
from collections import Counter
from collections.abc import Mapping, Sequence
from typing import Any

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv  # noqa: E402
from ci_provenance import MERGIFY_CONFIG_EXPECTATIONS  # noqa: E402
from git_maintenance import GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG  # noqa: E402
from git_remote_utils import fetchable_origin_argument, fetchable_remote_url  # noqa: E402


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "rust-verification.toml"
MERGIFY_CONFIG_PATH = ".mergify.yml"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
EXPECTED_HEAD_SHA_RE = re.compile(r"^(?P<pr>[1-9][0-9]*)=(?P<sha>[0-9a-f]{40})$")
MERGIFY_REQUIRED_REVIEWER_RE = re.compile(r"(?:^|\n)approved-reviews-by = (?P<reviewer>[^\n]+)")
MERGIFY_LABEL_CONDITION_RE = re.compile(r"^label = (?P<label>[^\n]+)$")
CONFLICT_LINE_RE = re.compile(r"^\d{6} [0-9a-f]{40} [123]\t(.+)$")
PR_REF_PREFIX = "refs/pull/"
PREFLIGHT_REF_PREFIX = "refs/preflight/merge_queue_preflight"
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
CONTRACT_LANES = (
    LANE_MERGIFY_CONFIG,
    LANE_IDENTITY,
    LANE_READINESS,
    LANE_INTEGRATION,
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

MERGIFY_REQUIRED_QUEUE_RULES = MERGIFY_CONFIG_EXPECTATIONS["queue_rule_order"]
MERGIFY_REQUIRED_PRIORITY_RULES = MERGIFY_CONFIG_EXPECTATIONS["priority_rule_order"]
MERGIFY_TOP_LEVEL_KEYS = frozenset({"merge_queue", "queue_rules", "priority_rules"})
MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS = frozenset(
    {
        "auto_merge_conditions",
        "commands_restrictions",
        "defaults",
        "extends",
        "merge_protections",
        "merge_protections_settings",
        "pull_request_rules",
    }
)
MERGIFY_MERGE_QUEUE_KEYS = frozenset({"max_parallel_checks", "reset_on_external_merge"})
MERGIFY_QUEUE_RULE_KEYS = frozenset(
    {
        "name",
        "queue_conditions",
        "merge_conditions",
        "branch_protection_injection_mode",
        "batch_size",
        "batch_max_wait_time",
        "batch_max_failure_resolution_attempts",
        "checks_timeout",
        "draft_bot_account",
        "merge_method",
    }
)
MERGIFY_PRIORITY_RULE_KEYS = frozenset({"name", "conditions", "priority", "allow_checks_interruption"})
MERGIFY_YAML_PARSER_RUBY = r"""
require "yaml"
require "json"

input = STDIN.read
errors = []

def yaml_scalar_key(node)
  node.is_a?(Psych::Nodes::Scalar) ? node.value : nil
end

def walk_yaml(node, path, errors)
  case node
  when Psych::Nodes::Mapping
    seen = {}
    node.children.each_slice(2) do |key_node, value_node|
      key = yaml_scalar_key(key_node)
      if key.nil?
        errors << "#{path}: mapping keys must be scalars"
        key = "<non-scalar>"
      elsif seen.key?(key)
        errors << "#{path}: duplicate key #{key}"
      end
      seen[key] = true
      errors << "#{path}: YAML merge key is forbidden" if key == "<<"
      walk_yaml(value_node, "#{path}.#{key}", errors)
    end
  when Psych::Nodes::Sequence
    node.children.each_with_index do |child, index|
      walk_yaml(child, "#{path}[#{index}]", errors)
    end
  when Psych::Nodes::Alias
    errors << "#{path}: YAML aliases are forbidden"
  end
end

begin
  stream = Psych.parse_stream(input)
  documents = stream.children
  errors << "must contain exactly one YAML document" unless documents.length == 1
  documents.each do |document|
    walk_yaml(document.root, "$", errors) if document.root
  end
  data = nil
  if errors.empty?
    data = YAML.safe_load(input, permitted_classes: [], permitted_symbols: [], aliases: false)
  end
  puts JSON.generate({"errors" => errors, "data" => data})
rescue Psych::Exception => e
  puts JSON.generate({"errors" => ["YAML parse failed: #{e.message}"], "data" => nil})
end
"""
def parse_mergify_yaml(config_text: str, config_name: str) -> tuple[Any | None, list[str]]:
    try:
        result = subprocess.run(
            ["ruby", "-e", MERGIFY_YAML_PARSER_RUBY],
            input=config_text,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    except FileNotFoundError:
        return None, [f"{config_name} requires Ruby/Psych to parse YAML"]
    except subprocess.TimeoutExpired:
        return None, [f"{config_name} YAML parser timed out"]
    if result.returncode != 0:
        detail = result.stderr.strip().splitlines()[0] if result.stderr.strip() else f"exit {result.returncode}"
        return None, [f"{config_name} YAML parser failed: {detail}"]
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        return None, [f"{config_name} YAML parser returned invalid JSON: {exc}"]
    parse_errors = payload.get("errors")
    if not isinstance(parse_errors, list):
        return None, [f"{config_name} YAML parser returned malformed errors"]
    if parse_errors:
        return None, [f"{config_name} {error}" for error in parse_errors]
    return payload.get("data"), []


def scalar_equals(actual: Any, expected: Any) -> bool:
    return type(actual) is type(expected) and actual == expected


def yaml_display(value: Any) -> str:
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    return str(value)


def unsupported_mapping_keys(mapping: dict[str, Any], allowed: frozenset[str]) -> list[str]:
    return [key for key in mapping if key not in allowed]


def mergify_mapping(value: Any, config_name: str, path: str, errors: list[str]) -> dict[str, Any] | None:
    if isinstance(value, dict):
        return value
    errors.append(f"{config_name} {path} must be a mapping")
    return None


def mergify_list(value: Any, config_name: str, path: str, errors: list[str]) -> list[Any] | None:
    if isinstance(value, list):
        return value
    errors.append(f"{config_name} {path} must be a list")
    return None


def required_mergify_mapping(
    parent: dict[str, Any], key: str, config_name: str, errors: list[str]
) -> dict[str, Any] | None:
    if key not in parent:
        errors.append(f"{config_name} must define {key}")
        return None
    return mergify_mapping(parent[key], config_name, key, errors)


def required_mergify_list(parent: dict[str, Any], key: str, config_name: str, errors: list[str]) -> list[Any] | None:
    if key not in parent:
        errors.append(f"{config_name} must define {key}")
        return None
    return mergify_list(parent[key], config_name, key, errors)


def named_mergify_rules(
    parent: dict[str, Any],
    key: str,
    expected_names: tuple[str, ...],
    order_error: str,
    config_name: str,
    errors: list[str],
) -> dict[str, dict[str, Any]]:
    values = required_mergify_list(parent, key, config_name, errors)
    if values is None:
        return {}
    names: list[Any] = []
    by_name: dict[str, dict[str, Any]] = {}
    for index, value in enumerate(values):
        rule = mergify_mapping(value, config_name, f"{key}[{index}]", errors)
        name = rule.get("name") if rule is not None else None
        names.append(name)
        if isinstance(name, str) and rule is not None:
            by_name[name] = rule
    if tuple(names) != expected_names:
        errors.append(f"{config_name} {order_error}")
    return by_name


def mergify_condition_list(value: Any, expected: list[str], config_name: str, path: str, errors: list[str]) -> None:
    values = mergify_list(value, config_name, path, errors)
    if values is None:
        return
    if values != expected:
        errors.append(f"{config_name} {path} must be {expected!r}")


def mergify_required_conditions(value: Any, config_name: str, path: str, errors: list[str]) -> None:
    values = mergify_list(value, config_name, path, errors)
    if values is None:
        return
    expected = [f"approved-reviews-by = {MERGIFY_CONFIG_EXPECTATIONS['required_reviewer']}"]
    if values != expected:
        errors.append(
            f"{config_name} {path} must require only {MERGIFY_CONFIG_EXPECTATIONS['required_reviewer']}"
        )


def expect_scalar(value: Any, expected: Any, config_name: str, path: str, errors: list[str]) -> None:
    if not scalar_equals(value, expected):
        errors.append(f"{config_name} {path} must be {yaml_display(expected)}")


def verify_mergify_config(config_text: str, config_name: str = ".mergify.yml") -> list[str]:
    config, errors = parse_mergify_yaml(config_text, config_name)
    if errors:
        return errors
    root = mergify_mapping(config, config_name, "root", errors)
    if root is None:
        return errors
    for key in root:
        if key in MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS:
            errors.append(f"{config_name} must keep manual queueing only; remove {key}")
        elif key not in MERGIFY_TOP_LEVEL_KEYS:
            errors.append(f"{config_name} must not define unsupported top-level key {key}")

    merge_queue = required_mergify_mapping(root, "merge_queue", config_name, errors)
    if merge_queue is not None:
        for key in unsupported_mapping_keys(merge_queue, MERGIFY_MERGE_QUEUE_KEYS):
            errors.append(f"{config_name} merge_queue must not define unsupported key {key}")
        for key, expected in MERGIFY_CONFIG_EXPECTATIONS["merge_queue"].items():
            expect_scalar(merge_queue.get(key), expected, config_name, f"merge_queue.{key}", errors)

    rules_by_name = named_mergify_rules(
        root,
        "queue_rules",
        MERGIFY_REQUIRED_QUEUE_RULES,
        "queue_rules must define exactly hotfix followed by default",
        config_name,
        errors,
    )

    for rule_name in MERGIFY_REQUIRED_QUEUE_RULES:
        expectation = MERGIFY_CONFIG_EXPECTATIONS["queue_rules"][rule_name]
        rule = rules_by_name.get(rule_name)
        if rule is None:
            errors.append(f"{config_name} must define {rule_name} queue rule")
            continue
        for key in unsupported_mapping_keys(rule, MERGIFY_QUEUE_RULE_KEYS):
            errors.append(f"{config_name} {rule_name} must not define unsupported key {key}")
        mergify_condition_list(
            rule.get("queue_conditions"),
            list(expectation["queue_conditions"]),
            config_name,
            f"{rule_name} queue_conditions",
            errors,
        )
        mergify_required_conditions(
            rule.get("merge_conditions"),
            config_name,
            f"{rule_name} merge_conditions",
            errors,
        )
        for key in (
            "branch_protection_injection_mode",
            "batch_max_wait_time",
            "batch_max_failure_resolution_attempts",
            "checks_timeout",
            "draft_bot_account",
            "merge_method",
        ):
            expected = expectation[key]
            expect_scalar(rule.get(key), expected, config_name, f"{rule_name} {key}", errors)
        expected_batch_size = expectation["batch_size"]
        batch_size = rule.get("batch_size")
        if not scalar_equals(batch_size, expected_batch_size):
            errors.append(f"{config_name} {rule_name} batch_size must be {expected_batch_size}")

    priority_by_name = named_mergify_rules(
        root,
        "priority_rules",
        MERGIFY_REQUIRED_PRIORITY_RULES,
        "priority_rules must define exactly hotfix",
        config_name,
        errors,
    )

    hotfix_priority = priority_by_name.get("hotfix")
    if hotfix_priority is None:
        errors.append(f"{config_name} must define hotfix priority rule")
        return errors
    for key in unsupported_mapping_keys(hotfix_priority, MERGIFY_PRIORITY_RULE_KEYS):
        errors.append(f"{config_name} hotfix priority must not define unsupported key {key}")
    mergify_condition_list(
        hotfix_priority.get("conditions"),
        list(MERGIFY_CONFIG_EXPECTATIONS["priority_rules"]["hotfix"]["conditions"]),
        config_name,
        "hotfix priority conditions",
        errors,
    )
    expect_scalar(
        hotfix_priority.get("priority"),
        MERGIFY_CONFIG_EXPECTATIONS["priority_rules"]["hotfix"]["priority"],
        config_name,
        "hotfix priority",
        errors,
    )
    expect_scalar(
        hotfix_priority.get("allow_checks_interruption"),
        MERGIFY_CONFIG_EXPECTATIONS["priority_rules"]["hotfix"]["allow_checks_interruption"],
        config_name,
        "hotfix allow_checks_interruption",
        errors,
    )
    return errors

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
MERGIFY_QUEUE_WAVE_STATUSES = {
    False: STATUS_READY,
    True: VERDICT_SPLIT_ADVISED,
}
MERGIFY_SPLIT_REASON_CODES = frozenset(
    {
        "batch_conflict",
        "mergify_queue_batch_above_max",
    }
)
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
    "base_or_head_drift_after_preflight",
    "post_merge_config_or_workflow_changes",
    "queue_metadata_drift",
    "live_queue_ordering",
    "reset_on_external_merge",
    "max_parallel_checks_cost",
)
RESIDUAL_RISK_MESSAGES: dict[str, str] = {}
MERGIFY_CONFIG_FIELD_HANDLING = {
    "merge_queue.max_parallel_checks": "residual_cost_impact",
    "merge_queue.reset_on_external_merge": "residual_post_preflight_invalidation",
    "queue_rules[].name": "required_unique_queue_identity",
    "queue_rules[].queue_conditions": "effective_pr_to_queue_routing",
    "queue_rules[].merge_conditions": "required_reviewer_evidence",
    "queue_rules[].branch_protection_injection_mode": "explicit_support_or_inconclusive",
    "queue_rules[].batch_size": "scalar_single_pr_model",
    "queue_rules[].batch_max_wait_time": "explicit_support_or_inconclusive",
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
    "base_mismatch": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_mismatch": (LANE_IDENTITY, "pr", STATUS_BLOCKED),
    "head_fetch_failed": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_unavailable": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "metadata_unavailable": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "readiness_failed": (LANE_READINESS, "pr", STATUS_BLOCKED),
}
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


require_table = functools.partial(_cv.require_table, error_cls=PreflightError)
require_string = functools.partial(_cv.require_string, error_cls=PreflightError)
require_positive_int = functools.partial(_cv.require_positive_int, error_cls=PreflightError)


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


class MergifyQueueRuleMatchStatus(enum.Enum):
    MATCHED = enum.auto()
    NOT_MATCHED = enum.auto()
    UNSUPPORTED = enum.auto()


@dataclasses.dataclass(frozen=True)
class MergifyQueueRuleMatch:
    status: MergifyQueueRuleMatchStatus
    rule: Mapping[str, object] | None = None

    def stops_selection(self) -> bool:
        return self.status is not MergifyQueueRuleMatchStatus.NOT_MATCHED


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


def unavailable_base_identity_findings(
    *,
    expected_base_sha: str,
    base: str,
    reason: str,
) -> tuple[dict[str, object], ...]:
    return (
        {
            "lane": LANE_IDENTITY,
            "scope": "run",
            "status": STATUS_INCONCLUSIVE,
            "reason_code": "base_unavailable",
            "message": "base branch could not be fetched",
            "evidence": {
                "base": base,
                "expected_base_sha": expected_base_sha,
                "actual_base_sha": None,
                "reason": reason,
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
            "message": RESIDUAL_RISK_MESSAGES.get(reason_code, reason_code),
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


def mergify_config_snapshot_finding(
    *,
    repo: pathlib.Path,
    base_sha: str,
    input_timeout_seconds: int,
) -> dict[str, object]:
    result = git(
        repo,
        "rev-parse",
        f"{base_sha}:{MERGIFY_CONFIG_PATH}",
        check=False,
        timeout_seconds=input_timeout_seconds,
    )
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
    input_timeout_seconds: int,
) -> dict[str, object]:
    result = git(
        repo,
        "cat-file",
        "-p",
        blob_sha,
        check=False,
        timeout_seconds=input_timeout_seconds,
    )
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


def mergify_config_data(
    *,
    repo: pathlib.Path,
    blob_sha: str,
    input_timeout_seconds: int,
) -> Any:
    result = git(
        repo,
        "cat-file",
        "-p",
        blob_sha,
        check=False,
        timeout_seconds=input_timeout_seconds,
    )
    config, _ = parse_mergify_yaml(result.stdout, MERGIFY_CONFIG_PATH)
    return config


def readiness_label_names(readiness: Mapping[str, object]) -> tuple[str, ...]:
    metadata = dict(readiness["metadata"])
    labels = tuple(metadata["labels"])
    return tuple(sorted(str(dict(label)["name"]) for label in labels))


def mergify_queue_conditions(rule: Mapping[str, object]) -> tuple[object, ...] | None:
    value = rule.get("queue_conditions", ())
    if not isinstance(value, Sequence) or isinstance(value, (str, bytes)):
        return None
    return tuple(value)


def mergify_queue_condition_labels(rule: Mapping[str, object]) -> frozenset[str] | None:
    conditions = mergify_queue_conditions(rule)
    if conditions is None:
        return None
    labels: set[str] = set()
    for condition in conditions:
        if not isinstance(condition, str):
            return None
        match = MERGIFY_LABEL_CONDITION_RE.fullmatch(condition)
        if match is None:
            return None
        labels.add(match.group("label"))
    return frozenset(labels)


def mergify_queue_rule_match(rule: object, labels: frozenset[str]) -> MergifyQueueRuleMatch:
    if not isinstance(rule, Mapping):
        return MergifyQueueRuleMatch(MergifyQueueRuleMatchStatus.UNSUPPORTED)
    condition_labels = mergify_queue_condition_labels(rule)
    if condition_labels is None:
        return MergifyQueueRuleMatch(MergifyQueueRuleMatchStatus.UNSUPPORTED)
    if not condition_labels.issubset(labels):
        return MergifyQueueRuleMatch(MergifyQueueRuleMatchStatus.NOT_MATCHED)
    return MergifyQueueRuleMatch(MergifyQueueRuleMatchStatus.MATCHED, rule)


def selected_mergify_queue_rule(
    config: Mapping[str, object],
    labels: tuple[str, ...],
) -> Mapping[str, object] | None:
    label_set = frozenset(labels)
    first_terminal_match = next(
        (
            match
            for rule in tuple(config.get("queue_rules", ()))
            for match in (mergify_queue_rule_match(rule, label_set),)
            if match.stops_selection()
        ),
        MergifyQueueRuleMatch(MergifyQueueRuleMatchStatus.NOT_MATCHED),
    )
    return first_terminal_match.rule


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
            "max_batch_size": mergify_queue_batch_max(rule),
        },
    }


def mergify_queue_route_unavailable_finding(
    readiness: Mapping[str, object],
    labels: tuple[str, ...],
) -> dict[str, object]:
    pr = int(readiness["pr"])
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "pr",
        "status": STATUS_INCONCLUSIVE,
        "reason_code": "mergify_queue_route_unavailable",
        "message": f"PR #{pr} does not match a supported Mergify queue rule",
        "evidence": {
            "pr": pr,
            "labels": list(labels),
        },
    }


def mergify_route_queue_groups(route_findings: Sequence[Mapping[str, object]]) -> dict[str, list[int]]:
    groups: dict[str, list[int]] = {}
    for finding in route_findings:
        if finding["reason_code"] != "mergify_queue_route_selected":
            continue
        evidence = dict(finding["evidence"])
        groups.setdefault(str(evidence["queue_rule"]), []).append(int(evidence["pr"]))
    return groups


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


def approved_reviewers(readiness: Mapping[str, object]) -> tuple[str, ...]:
    metadata = dict(readiness.get("metadata", {}))
    reviews = metadata.get("reviews", ())
    if not isinstance(reviews, Sequence) or isinstance(reviews, (str, bytes)):
        return ()
    reviewers: set[str] = set()
    for review in reviews:
        if not isinstance(review, Mapping) or review.get("state") != "APPROVED":
            continue
        author = review.get("author")
        if not isinstance(author, Mapping):
            continue
        login = author.get("login")
        if isinstance(login, str) and login:
            reviewers.add(login)
    return tuple(sorted(reviewers))


def mergify_required_reviewer_identity_finding(
    *,
    pr: int,
    queue_rule: str,
    required_reviewers: Sequence[str],
    approved: Sequence[str],
) -> dict[str, object]:
    missing = tuple(reviewer for reviewer in required_reviewers if reviewer not in approved)
    status = STATUS_READY if not missing else STATUS_BLOCKED
    reason_code = (
        "mergify_required_reviewer_approved"
        if not missing
        else "mergify_required_reviewer_missing"
    )
    message = (
        f"PR #{pr} has approval from required Mergify reviewer"
        if not missing
        else f"PR #{pr} is missing approval from required Mergify reviewer"
    )
    return {
        "lane": LANE_READINESS,
        "scope": "pr",
        "status": status,
        "reason_code": reason_code,
        "message": message,
        "evidence": {
            "pr": pr,
            "queue_rule": queue_rule,
            "required_reviewers": list(required_reviewers),
            "approved_reviewers": list(approved),
            "missing_reviewers": list(missing),
        },
    }


def selected_mergify_required_reviewer_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    readiness_by_pr = {int(item["pr"]): item for item in readiness}
    queue_findings = tuple(
        mergify_required_reviewer_finding(rules_by_name[queue_rule])
        for queue_rule in sorted(mergify_route_queue_rules(route_findings))
    )
    identity_findings = tuple(
        mergify_required_reviewer_identity_finding(
            pr=pr,
            queue_rule=queue_rule,
            required_reviewers=MERGIFY_REQUIRED_REVIEWER_RE.findall(
                "\n".join(str(condition) for condition in tuple(rules_by_name[queue_rule]["merge_conditions"]))
            ),
            approved=approved_reviewers(readiness_by_pr[pr]),
        )
        for route_finding in route_findings
        if route_finding["reason_code"] == "mergify_queue_route_selected"
        for route_evidence in (dict(route_finding["evidence"]),)
        for pr in (int(route_evidence["pr"]),)
        for queue_rule in (str(route_evidence["queue_rule"]),)
    )
    return (*queue_findings, *identity_findings)


def mergify_queue_rules_by_name(config: Mapping[str, object]) -> dict[str, Mapping[str, object]]:
    return {
        str(rule["name"]): rule
        for rule in tuple(config["queue_rules"])
    }


def mergify_queue_batch_max(rule: Mapping[str, object]) -> int:
    return int(rule["batch_size"])


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


def mergify_queue_batch_size_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    groups = mergify_route_queue_groups(route_findings)
    findings: list[dict[str, object]] = []
    for queue_rule, prs in groups.items():
        rule = rules_by_name[queue_rule]
        max_batch_size = mergify_queue_batch_max(rule)
        if len(prs) > max_batch_size:
            findings.append(
                mergify_queue_batch_above_max_finding(
                    queue_rule=queue_rule,
                    prs=prs,
                    max_batch_size=max_batch_size,
                )
            )
    return tuple(findings)


def mergify_batch_limits(
    findings: Sequence[Mapping[str, object]],
) -> dict[int, int]:
    limits: dict[int, int] = {}
    for finding in findings:
        if finding["reason_code"] != "mergify_queue_route_selected":
            continue
        evidence = dict(finding["evidence"])
        limits[int(evidence["pr"])] = int(evidence["max_batch_size"])
    return limits


def batch_max_size(prs: Sequence[int], limits: Mapping[int, int]) -> int | None:
    candidates = tuple(limits[pr] for pr in prs if pr in limits)
    if not candidates:
        return None
    return min(candidates)


def batch_would_exceed_max(prs: Sequence[int], limits: Mapping[int, int]) -> bool:
    max_size = batch_max_size(prs, limits)
    return max_size is not None and len(prs) > max_size


def available_mergify_queue_route_findings(
    *,
    config: Mapping[str, object],
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    return tuple(
        mergify_queue_route_finding(item, rule, labels)
        if rule is not None
        else mergify_queue_route_unavailable_finding(item, labels)
        for item in filter(lambda candidate: "metadata" in candidate, readiness)
        for labels in (readiness_label_names(item),)
        for rule in (selected_mergify_queue_rule(config, labels),)
    )


def available_mergify_config_route_and_batch_findings(
    *,
    config: Mapping[str, object],
    readiness: Sequence[Mapping[str, object]],
) -> tuple[dict[str, object], ...]:
    route_findings = available_mergify_queue_route_findings(config=config, readiness=readiness)
    return (
        *route_findings,
        *selected_mergify_required_reviewer_findings(
            config=config,
            route_findings=route_findings,
            readiness=readiness,
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
    input_timeout_seconds: int,
) -> tuple[dict[str, object], ...]:
    snapshot = mergify_config_snapshot_finding(
        repo=repo,
        base_sha=base_sha,
        input_timeout_seconds=input_timeout_seconds,
    )
    if snapshot["status"] != STATUS_READY:
        return (snapshot,)
    validation = mergify_config_validation_finding(
        repo=repo,
        base_sha=base_sha,
        blob_sha=str(snapshot["evidence"]["blob_sha"]),
        input_timeout_seconds=input_timeout_seconds,
    )
    if validation["status"] != STATUS_READY:
        return (snapshot, validation)
    config = mergify_config_data(
        repo=repo,
        blob_sha=str(snapshot["evidence"]["blob_sha"]),
        input_timeout_seconds=input_timeout_seconds,
    )
    return (
        snapshot,
        validation,
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


def mergify_wave_status(
    findings: Sequence[Mapping[str, object]],
    artifacts: Sequence[Mapping[str, object]] = (),
) -> str:
    split_reasons = frozenset(str(finding["reason_code"]) for finding in findings) | frozenset(
        str(artifact["type"])
        for artifact in artifacts
    )
    return MERGIFY_QUEUE_WAVE_STATUSES[
        len(mergify_route_queue_rules(findings)) > 1
        or bool(MERGIFY_SPLIT_REASON_CODES & split_reasons)
    ]


@dataclasses.dataclass(frozen=True)
class CommandResult:
    args: tuple[str, ...]
    returncode: int
    stdout: str
    stderr: str
    failure_type: str | None = None


@dataclasses.dataclass
class PrivateFetchRefs:
    source_repo: pathlib.Path
    git_repo: pathlib.Path
    source_objects: str
    input_timeout_seconds: int
    namespace: str
    temp_dir: tempfile.TemporaryDirectory
    refs: list[str] = dataclasses.field(default_factory=list)
    remotes: dict[str, str] = dataclasses.field(default_factory=dict)

    @classmethod
    def create(cls, repo: pathlib.Path, input_timeout_seconds: int) -> "PrivateFetchRefs":
        temp_dir = tempfile.TemporaryDirectory(prefix="merge-queue-preflight-git-")
        git_repo = pathlib.Path(temp_dir.name) / "repo.git"
        try:
            run_command(
                ["git", "init", "--bare", str(git_repo)],
                cwd=pathlib.Path(temp_dir.name),
                check=True,
                timeout_seconds=input_timeout_seconds,
            )
            for key, value in GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
                git(
                    git_repo,
                    "config",
                    "--local",
                    key,
                    value,
                    timeout_seconds=input_timeout_seconds,
                )
            source_objects = git(
                repo,
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
                timeout_seconds=input_timeout_seconds,
            ).stdout.strip()
            if not source_objects:
                raise PreflightError("source Git object directory did not resolve")
        except Exception:
            temp_dir.cleanup()
            raise
        return cls(
            source_repo=repo,
            git_repo=git_repo,
            source_objects=source_objects,
            input_timeout_seconds=input_timeout_seconds,
            namespace=f"{PREFLIGHT_REF_PREFIX}/{uuid.uuid4().hex}",
            temp_dir=temp_dir,
        )

    def fetch_origin(self, origin: str) -> str:
        cached = self.remotes.get(origin)
        if cached is not None:
            return cached
        if not self.source_repo.is_dir():
            raise PreflightError(f"source repository directory {self.source_repo} does not exist")
        result = git(
            self.source_repo,
            "remote",
            "get-url",
            origin,
            check=False,
            timeout_seconds=self.input_timeout_seconds,
        )
        remote_url = result.stdout.strip()
        if result.returncode != 0 or not remote_url:
            remote_url = fetchable_origin_argument(origin, self.source_repo)
            self.remotes[origin] = remote_url
            return remote_url
        remote_url = fetchable_remote_url(remote_url, self.source_repo)
        self.remotes[origin] = remote_url
        return remote_url

    def fetch_sha(self, origin: str, source: str, name: str) -> str:
        if not self.git_repo.is_dir():
            raise PreflightError(f"private Git repository directory {self.git_repo} does not exist")
        ref = f"{self.namespace}/{name}"
        git(
            self.git_repo,
            "fetch",
            "--quiet",
            "--no-write-fetch-head",
            "--no-tags",
            self.fetch_origin(origin),
            f"{source}:{ref}",
            timeout_seconds=self.input_timeout_seconds,
        )
        self.refs.append(ref)
        return git(
            self.git_repo,
            "rev-parse",
            ref,
            timeout_seconds=self.input_timeout_seconds,
        ).stdout.strip()

    def cleanup(self) -> None:
        for ref in reversed(self.refs):
            git(
                self.git_repo,
                "update-ref",
                "-d",
                ref,
                check=False,
                timeout_seconds=self.input_timeout_seconds,
            )
        self.temp_dir.cleanup()


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

    def as_json(self) -> dict[str, object]:
        return {
            "index": self.index,
            "prs": list(self.prs),
            "status": STATUS_READY,
        }


@dataclasses.dataclass(frozen=True)
class PreflightConfig:
    origin: str
    base: str
    input_timeout_seconds: int


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
READINESS_ISSUE_ARTIFACT_TYPES = {
    "base_mismatch": "base_mismatch",
    "draft": "readiness_failed",
    "head_mismatch": "head_mismatch",
    "not_mergeable": "readiness_failed",
    "not_open": "readiness_failed",
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
    input_text: str | None = None,
    timeout_seconds: int | None = None,
    process_group: bool = False,
) -> CommandResult:
    command_args = list(args)
    stdin = subprocess.PIPE if input_text is not None else subprocess.DEVNULL
    try:
        process = subprocess.Popen(
            command_args,
            cwd=cwd,
            text=True,
            stdin=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            start_new_session=process_group,
        )
    except FileNotFoundError:
        result = CommandResult(
            args=tuple(args),
            returncode=127,
            stdout="",
            stderr=f"executable not found: {command_args[0]}\n",
            failure_type="unavailable",
        )
        if check:
            raise PreflightError(result.stderr.strip())
        return result
    except OSError as exc:
        result = CommandResult(
            args=tuple(args),
            returncode=127,
            stdout="",
            stderr=f"executable could not start: {command_args[0]}: {exc}\n",
            failure_type="unavailable",
        )
        if check:
            raise PreflightError(result.stderr.strip())
        return result
    try:
        stdout, stderr = process.communicate(input=input_text, timeout=timeout_seconds)
        returncode = process.returncode
        failure_type = None
    except subprocess.TimeoutExpired:
        if process_group:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            process.kill()
        stdout, stderr = process.communicate()
        returncode = -signal.SIGKILL
        timeout_message = f"command timed out after {timeout_seconds} seconds\n"
        stderr = f"{stderr or ''}{timeout_message}"
        failure_type = "timeout"
    result = CommandResult(
        args=tuple(args),
        returncode=returncode,
        stdout=stdout or "",
        stderr=stderr or "",
        failure_type=failure_type,
    )
    if check and result.returncode != 0:
        rendered = " ".join(shlex.quote(part) for part in result.args)
        raise PreflightError(
            f"command failed ({result.returncode}): {rendered}\n{result.stderr}{result.stdout}"
        )
    return result


def git(
    repo: pathlib.Path,
    *args: str,
    check: bool = True,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout_seconds: int | None = None,
) -> CommandResult:
    return run_command(
        ["git", *args],
        cwd=repo,
        check=check,
        env=env,
        input_text=input_text,
        timeout_seconds=timeout_seconds,
    )


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


def load_config(path: pathlib.Path) -> PreflightConfig:
    root = load_toml(path)
    settings = require_table(root, "merge_queue_preflight", "config")
    origin = require_string(settings, "origin", "config.merge_queue_preflight")
    base = require_string(settings, "base", "config.merge_queue_preflight")
    timeout_settings = require_table(settings, "timeouts", "config.merge_queue_preflight")
    input_timeout_seconds = require_positive_int(
        timeout_settings,
        "input_seconds",
        "config.merge_queue_preflight.timeouts",
    )
    return PreflightConfig(
        origin=origin,
        base=base,
        input_timeout_seconds=input_timeout_seconds,
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


def fetch_base(fetch_refs: PrivateFetchRefs, origin: str, base: str) -> str:
    sha = fetch_refs.fetch_sha(origin, base, "base")
    if SHA_RE.fullmatch(sha) is None:
        raise PreflightError(f"base {base!r} did not resolve to a commit SHA")
    return sha


def fetch_pr_head(fetch_refs: PrivateFetchRefs, origin: str, pr_number: int) -> PrHead:
    missing_ref_message = "couldn't find remote ref"
    try:
        sha = fetch_refs.fetch_sha(origin, f"{PR_REF_PREFIX}{pr_number}/head", f"pr-{pr_number}")
    except PreflightError as exc:
        if missing_ref_message not in str(exc):
            raise
        raise PreflightError(
            f"PR #{pr_number} head ref was not found; ensure the PR exists and has a fetchable head"
        ) from exc
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


def merge_tree(
    repo: pathlib.Path,
    left: str,
    right: str,
    input_timeout_seconds: int,
) -> MergeResult:
    result = git(
        repo,
        "merge-tree",
        "--write-tree",
        left,
        right,
        check=False,
        timeout_seconds=input_timeout_seconds,
    )
    output = result.stdout + result.stderr
    if result.failure_type == "timeout":
        raise PreflightError(f"git merge-tree timed out after {input_timeout_seconds} seconds")
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


def commit_tree(
    repo: pathlib.Path,
    tree: str,
    parents: Sequence[str],
    message: str,
    input_timeout_seconds: int,
) -> str:
    args = ["commit-tree", tree]
    for parent in parents:
        args.extend(["-p", parent])
    env = os.environ.copy()
    env.setdefault("GIT_AUTHOR_NAME", "merge-queue-preflight")
    env.setdefault("GIT_AUTHOR_EMAIL", "merge-queue-preflight@example.invalid")
    env.setdefault("GIT_COMMITTER_NAME", "merge-queue-preflight")
    env.setdefault("GIT_COMMITTER_EMAIL", "merge-queue-preflight@example.invalid")
    completed = git(
        repo,
        *args,
        check=False,
        env=env,
        input_text=message,
        timeout_seconds=input_timeout_seconds,
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
    input_timeout_seconds: int,
) -> SyntheticCommit | MergeResult:
    merged = merge_tree(repo, left_commit, right_commit, input_timeout_seconds)
    if not merged.clean or merged.tree is None:
        return merged
    message = "merge queue preflight: " + ",".join(f"#{pr}" for pr in prs)
    commit = commit_tree(
        repo,
        merged.tree,
        [left_commit, right_commit],
        message,
        input_timeout_seconds,
    )
    return SyntheticCommit(commit=commit, prs=tuple(prs))


def unverified_batches_for_ready_prs(
    *,
    repo: pathlib.Path,
    requested: Sequence[int],
    blocked_numbers: set[int],
    heads: Mapping[int, ExpectedHead],
    base_commits: Mapping[int, SyntheticCommit],
    batch_max_limits: Mapping[str, int],
    input_timeout_seconds: int,
) -> tuple[list[dict[str, object]], list[Batch]]:
    """Build merge-conflict batches for ready pull requests."""
    conflicts: list[dict[str, object]] = []
    batches: list[Batch] = []
    current: SyntheticCommit | None = None
    batch_index = 1
    for pr in requested:
        if pr in blocked_numbers:
            continue
        pr_head = heads[pr]
        if current is None:
            current = base_commits[pr]
            continue
        candidate_prs = [*current.prs, pr]
        if batch_would_exceed_max(candidate_prs, batch_max_limits):
            batches.append(
                Batch(
                    index=batch_index,
                    commit=current.commit,
                    prs=current.prs,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            continue
        synthetic = synthesize_merge(
            repo,
            current.commit,
            pr_head.sha,
            candidate_prs,
            input_timeout_seconds,
        )
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
                )
            )
            batch_index += 1
            current = base_commits[pr]
            continue
        current = synthetic
    if current is not None:
        batches.append(
            Batch(
                index=batch_index,
                commit=current.commit,
                prs=current.prs,
            )
        )
    return conflicts, batches


def gh_json(
    args: Sequence[str],
    *,
    allowed_returncodes: Sequence[int] = (0,),
    timeout_seconds: int,
) -> object:
    completed = run_command(
        ["gh", *args],
        cwd=pathlib.Path.cwd(),
        check=False,
        timeout_seconds=timeout_seconds,
        process_group=True,
    )
    if completed.failure_type == "unavailable":
        raise PreflightError("gh executable not found")
    if completed.failure_type == "timeout":
        raise PreflightError(f"gh {' '.join(args)} timed out after {timeout_seconds} seconds")
    if completed.returncode not in allowed_returncodes:
        raise PreflightError(f"gh {' '.join(args)} failed: {completed.stderr}{completed.stdout}")
    try:
        return json.loads(completed.stdout or "[]")
    except json.JSONDecodeError as exc:
        raise PreflightError(f"gh {' '.join(args)} returned invalid JSON") from exc


def readiness_issues(
    payload: dict[str, object],
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
    return tuple(issues)


def pr_readiness(
    pr_number: int,
    *,
    use_gh: bool,
    input_timeout_seconds: int,
    expected_base: str | None = None,
    fetched_head: str | None = None,
) -> dict[str, object]:
    if not use_gh:
        return {"pr": pr_number, "warnings": [], "warning_details": []}
    payload = gh_json(
        [
            "pr",
            "view",
            str(pr_number),
            "--json",
            "number,state,isDraft,mergeable,reviewDecision,headRefOid,baseRefName,labels,reviews,title,url",
        ],
        timeout_seconds=input_timeout_seconds,
    )
    if not isinstance(payload, dict):
        raise PreflightError(f"gh pr view {pr_number} did not return an object")
    issues = readiness_issues(
        payload,
        expected_base=expected_base,
        fetched_head=fetched_head,
    )
    return {
        "pr": pr_number,
        "warnings": [issue.message for issue in issues],
        "warning_details": [issue.as_json() for issue in issues],
        "metadata": payload,
    }


def readiness_for_wave(
    pr_numbers: Sequence[int],
    *,
    use_gh: bool,
    base: str,
    input_timeout_seconds: int,
) -> tuple[list[dict[str, object]], list[str]]:
    if not use_gh:
        return [
            pr_readiness(pr, use_gh=False, input_timeout_seconds=input_timeout_seconds)
            for pr in pr_numbers
        ], []
    readiness: list[dict[str, object]] = []
    metadata_warnings: list[str] = []
    for pr in pr_numbers:
        try:
            readiness.append(
                pr_readiness(
                    pr,
                    use_gh=True,
                    input_timeout_seconds=input_timeout_seconds,
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
        issues = readiness_issues(
            metadata,
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
    fetch_refs: PrivateFetchRefs,
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
            heads[pr] = fetch_pr_head(fetch_refs, origin, pr)
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
            },
        },
    )


def no_readiness_ready_findings(item: Mapping[str, object]) -> tuple[dict[str, object], ...]:
    return ()


READINESS_READY_FINDING_BUILDERS = {
    True: available_readiness_ready_findings,
    False: no_readiness_ready_findings,
}


def non_ready_readiness_prs(findings: Sequence[Mapping[str, object]]) -> frozenset[int]:
    prs: set[int] = set()
    for finding in findings:
        if finding["lane"] != LANE_READINESS or finding["scope"] != "pr":
            continue
        if finding["status"] == STATUS_READY:
            continue
        evidence = finding.get("evidence")
        if not isinstance(evidence, Mapping) or "pr" not in evidence:
            continue
        prs.add(int(evidence["pr"]))
    return frozenset(prs)


def readiness_ready_findings(
    readiness: Sequence[Mapping[str, object]],
    *,
    related_findings: Sequence[Mapping[str, object]] = (),
) -> tuple[dict[str, object], ...]:
    suppressed_prs = non_ready_readiness_prs(related_findings)
    return tuple(
        finding
        for item in readiness
        if int(item["pr"]) not in suppressed_prs
        for finding in READINESS_READY_FINDING_BUILDERS[
            "metadata" in item and not tuple(item["warning_details"])
        ](item)
    )


def unavailable_base_payload(
    *,
    base: str,
    expected_base_sha: str,
    expected_heads: Mapping[int, str],
    requested: Sequence[int],
    use_gh: bool,
    reason: str,
) -> tuple[dict[str, object], int]:
    contract_findings = (
        *unavailable_base_identity_findings(
            expected_base_sha=expected_base_sha,
            base=base,
            reason=reason,
        ),
        *preflight_mode_findings(use_gh=use_gh),
        *residual_risk_findings(),
    )
    contract_evaluation = evaluate_preflight_contract(
        ContractEvidence(
            findings=contract_findings,
            artifacts=(),
            wave_status=mergify_wave_status(contract_findings),
        )
    )
    payload = {
        "base": base,
        "base_sha": expected_base_sha,
        "actual_base_sha": None,
        "expected_base_sha": expected_base_sha,
        "expected_pr_heads": {str(number): sha for number, sha in expected_heads.items()},
        "requested_prs": list(requested),
        "pr_heads": {},
        "readiness": [],
        "metadata_warnings": [],
        "residual_risks": list(RESIDUAL_RISK_REASON_CODES),
        "batches": [],
        "blocked_prs": [],
        "conflicts": [],
        "contract_exit_code": contract_evaluation["exit_code"],
        "findings": contract_evaluation["findings"],
        "lane_statuses": contract_evaluation["lane_statuses"],
        "verdict": contract_evaluation["verdict"],
        "wave_status": contract_evaluation["wave_status"],
    }
    return payload, int(contract_evaluation["exit_code"])


def preflight(
    *,
    repo: pathlib.Path,
    origin: str,
    base: str,
    expected_base_sha: str,
    expected_head_inputs: Sequence[ExpectedHead],
    pr_numbers: Sequence[int],
    input_timeout_seconds: int,
    use_gh: bool,
) -> tuple[dict[str, object], int]:
    fetch_refs = PrivateFetchRefs.create(repo, input_timeout_seconds)
    try:
        return preflight_with_fetch_refs(
            origin=origin,
            base=base,
            expected_base_sha=expected_base_sha,
            expected_head_inputs=expected_head_inputs,
            pr_numbers=pr_numbers,
            input_timeout_seconds=input_timeout_seconds,
            use_gh=use_gh,
            fetch_refs=fetch_refs,
        )
    finally:
        fetch_refs.cleanup()


def preflight_with_fetch_refs(
    *,
    origin: str,
    base: str,
    expected_base_sha: str,
    expected_head_inputs: Sequence[ExpectedHead],
    pr_numbers: Sequence[int],
    input_timeout_seconds: int,
    use_gh: bool,
    fetch_refs: PrivateFetchRefs,
) -> tuple[dict[str, object], int]:
    requested = unique_preserving_order(pr_numbers)
    expected_heads = expected_head_map(expected_head_inputs, requested)
    git_repo = fetch_refs.git_repo
    try:
        actual_base_sha = fetch_base(fetch_refs, origin, base)
    except PreflightError as exc:
        return unavailable_base_payload(
            base=base,
            expected_base_sha=expected_base_sha,
            expected_heads=expected_heads,
            requested=requested,
            use_gh=use_gh,
            reason=str(exc),
        )
    base_sha = expected_base_sha
    readiness, metadata_warnings = readiness_for_wave(
        requested,
        use_gh=use_gh,
        base=base,
        input_timeout_seconds=input_timeout_seconds,
    )
    initial_readiness_blocks = readiness_blocks(readiness)
    initial_blocked_numbers = {
        int(block["pr"])
        for block in initial_readiness_blocks
        if PREFLIGHT_ARTIFACT_CLASSIFICATIONS[str(block["type"])][2] == STATUS_BLOCKED
    }
    heads, head_fetch_blocks = fetch_available_pr_heads(
        fetch_refs=fetch_refs,
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
    mergify_findings = mergify_config_findings(
        repo=git_repo,
        base_sha=base_sha,
        readiness=readiness,
        input_timeout_seconds=input_timeout_seconds,
    )
    batch_max_limits = mergify_batch_limits(mergify_findings)
    base_commits: dict[int, SyntheticCommit] = {}
    for pr in requested:
        if pr in blocked_numbers:
            continue
        head = heads[pr]
        synthetic = synthesize_merge(git_repo, base_sha, head.sha, [pr], input_timeout_seconds)
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
        base_commits[pr] = synthetic
    conflicts, batches = unverified_batches_for_ready_prs(
        repo=git_repo,
        requested=requested,
        blocked_numbers=blocked_numbers,
        heads=heads,
        base_commits=base_commits,
        batch_max_limits=batch_max_limits,
        input_timeout_seconds=input_timeout_seconds,
    )
    contract_findings = (
        *base_identity_findings(
            expected_base_sha=expected_base_sha,
            actual_base_sha=actual_base_sha,
        ),
        *head_identity_findings(expected_heads=expected_heads, actual_heads=heads),
        *mergify_findings,
        *preflight_mode_findings(use_gh=use_gh),
        *readiness_ready_findings(readiness, related_findings=mergify_findings),
        *residual_risk_findings(),
        *integration_batch_ready_findings(batches),
    )
    contract_evaluation = evaluate_preflight_contract(
        ContractEvidence(
            findings=contract_findings,
            artifacts=(*blocked_prs, *conflicts),
            wave_status=mergify_wave_status(contract_findings, (*blocked_prs, *conflicts)),
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
        "batches": [batch.as_json() for batch in batches],
        "blocked_prs": blocked_prs,
        "conflicts": conflicts,
        "contract_exit_code": contract_evaluation["exit_code"],
        "findings": contract_evaluation["findings"],
        "lane_statuses": contract_evaluation["lane_statuses"],
        "verdict": contract_evaluation["verdict"],
        "wave_status": contract_evaluation["wave_status"],
    }
    exit_code = int(contract_evaluation["exit_code"])
    return payload, exit_code


def plain_text(payload: dict[str, object]) -> str:
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
    if payload["blocked_prs"]:
        lines.append("blocked PRs:")
        for item in payload["blocked_prs"]:
            lines.append(f"  #{item['pr']}: {item['reason']}")
            if item.get("files"):
                lines.append("    files: " + ", ".join(item["files"]))
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
    root.add_argument("--no-gh", action="store_true")
    root.add_argument("--json", action="store_true")
    return root


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
            input_timeout_seconds=config.input_timeout_seconds,
            use_gh=not args.no_gh,
        )
    except PreflightError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return PREFLIGHT_USAGE_EXIT_CODE
    except Exception as exc:
        print(f"error: internal preflight failure: {exc}", file=sys.stderr)
        return PREFLIGHT_USAGE_EXIT_CODE
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(plain_text(payload))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
