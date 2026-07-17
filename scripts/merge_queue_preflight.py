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
from git_remote_utils import (  # noqa: E402
    fetchable_remote_url,
    redact_remote_urls,
    remote_url_sha256,
    require_credential_free_remote_url as _require_credential_free_remote_url,
    require_remote_name as _require_remote_name,
)


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "rust-verification.toml"
MERGIFY_CONFIG_PATH = ".mergify.yml"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
EXPECTED_ORIGIN_URL_SHA256_ENV = "MERGE_QUEUE_PREFLIGHT_ORIGIN_URL_SHA256"
EXPECTED_HEAD_SHA_RE = re.compile(r"^(?P<pr>[1-9][0-9]*)=(?P<sha>[0-9a-f]{40})$")
MERGIFY_REQUIRED_REVIEWER_RE = re.compile(r"(?:^|\n)approved-reviews-by = (?P<reviewer>[^\n]+)")
MERGIFY_CHECK_SUCCESS_RE = re.compile(r"(?:^|\n)check-success = (?P<check>[^\n]+)")
MERGIFY_LABEL_CONDITION_RE = re.compile(r"^label = (?P<label>[^\n]+)$")
CONFLICT_LINE_RE = re.compile(r"^\d{6} [0-9a-f]{40} [123]\t(.+)$")
PR_REF_PREFIX = "refs/pull/"
PREFLIGHT_REF_PREFIX = "refs/preflight/merge_queue_preflight"
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

MERGIFY_REQUIRED_MERGE_CONDITIONS = frozenset(
    {
        f"approved-reviews-by = {MERGIFY_CONFIG_EXPECTATIONS['required_reviewer']}",
        *(
            f"check-success = {check_name}"
            for check_name in MERGIFY_CONFIG_EXPECTATIONS["required_checks"]
        ),
    }
)


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
MERGIFY_DYNAMIC_BATCH_KEYS = frozenset({"min", "max"})
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
    if set(values) != MERGIFY_REQUIRED_MERGE_CONDITIONS or len(values) != len(MERGIFY_REQUIRED_MERGE_CONDITIONS):
        errors.append(
            f"{config_name} {path} must require {MERGIFY_CONFIG_EXPECTATIONS['required_reviewer']} "
            f"and all {len(MERGIFY_CONFIG_EXPECTATIONS['required_checks'])} gates"
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
        if isinstance(expected_batch_size, dict):
            batch_size_mapping = mergify_mapping(batch_size, config_name, f"{rule_name} batch_size", errors)
            if batch_size_mapping is None:
                continue
            for key in unsupported_mapping_keys(batch_size_mapping, MERGIFY_DYNAMIC_BATCH_KEYS):
                errors.append(f"{config_name} {rule_name} batch_size must not define unsupported key {key}")
            if batch_size_mapping != expected_batch_size:
                errors.append(
                    f"{config_name} {rule_name} batch_size must be min {expected_batch_size['min']} max {expected_batch_size['max']}"
                )
        elif not scalar_equals(batch_size, expected_batch_size):
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
        "batch_verifier_failed",
        "mergify_queue_batch_above_max",
    }
)
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
    "batch_verifier_scope",
    "source_fence_test_phase_skipped",
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
RESIDUAL_RISK_MESSAGES = {
    "batch_verifier_scope": "verifier proof is batch-scoped for passing optimistic batches",
    "source_fence_test_phase_skipped": "source-fence fast path may skip fixture test suites for eligible diffs",
}
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
    "batch_verifier_timeout": (LANE_VERIFIER, "batch", STATUS_INCONCLUSIVE),
    "batch_verifier_unavailable": (LANE_VERIFIER, "batch", STATUS_INCONCLUSIVE),
    "base_mismatch": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_mismatch": (LANE_IDENTITY, "pr", STATUS_BLOCKED),
    "head_fetch_failed": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "head_unavailable": (LANE_IDENTITY, "pr", STATUS_INCONCLUSIVE),
    "metadata_unavailable": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_failed": (LANE_READINESS, "pr", STATUS_BLOCKED),
    "required_check_pending": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_skipped": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_missing": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_unknown": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_stale": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "required_check_wrong_workflow": (LANE_READINESS, "pr", STATUS_INCONCLUSIVE),
    "readiness_failed": (LANE_READINESS, "pr", STATUS_BLOCKED),
    "verifier_failed": (LANE_VERIFIER, "pr", STATUS_BLOCKED),
    "verifier_timeout": (LANE_VERIFIER, "pr", STATUS_INCONCLUSIVE),
    "verifier_unavailable": (LANE_VERIFIER, "pr", STATUS_INCONCLUSIVE),
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
CHECK_BUCKET_STATE_ALIASES = {
    "pass": "success",
    "success": "success",
    "fail": "failure",
    "failure": "failure",
    "cancel": "cancelled",
    "cancelled": "cancelled",
    "pending": "pending",
    "skipping": "skipped",
    "skipped": "skipped",
    "neutral": "neutral",
}
CHECK_STATE_ISSUE_MESSAGES = {
    "required_check_failed": "required check failed: {name}",
    "required_check_pending": "required check pending: {name}",
    "required_check_skipped": "required check skipped: {name}",
    "required_check_missing": "required check missing: {name}",
    "required_check_unknown": "required check state or bucket is unknown: {name}",
    "required_check_stale": "required check is stale: {name}",
}
VERIFIER_STREAMS = ("stdout", "stderr")
FENCES_ONLY_FLAG = "--fences-only"
SHELL_COMMAND_EXECUTABLES = frozenset({"bash", "dash", "fish", "ksh", "sh", "zsh"})
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


require_credential_free_remote_url = functools.partial(
    _require_credential_free_remote_url,
    error_cls=PreflightError,
)
require_remote_name = functools.partial(_require_remote_name, error_cls=PreflightError)


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


def mergify_required_check_names(rule: Mapping[str, object]) -> tuple[str, ...]:
    merge_conditions = [str(condition) for condition in tuple(rule["merge_conditions"])]
    return tuple(MERGIFY_CHECK_SUCCESS_RE.findall("\n".join(merge_conditions)))


def check_name_matches(check: object, required_check: str) -> bool:
    return isinstance(check, Mapping) and check.get("name") == required_check


def missing_mergify_required_check_finding(
    *,
    required_check: str,
    actual_head: str,
) -> dict[str, object]:
    return classify_required_check_state(
        check_name=required_check,
        raw_state="missing",
        expected_head=actual_head,
        actual_head=actual_head,
        evidence={"workflow": None, "bucket": None},
    )


def duplicate_mergify_required_check_finding(
    *,
    required_check: str,
    actual_head: str,
) -> dict[str, object]:
    return classify_required_check_state(
        check_name=required_check,
        raw_state="unknown",
        expected_head=actual_head,
        actual_head=actual_head,
        evidence={"workflow": None, "bucket": None},
    )


def wrong_workflow_mergify_required_check_finding(
    *,
    required_check: str,
    expected_workflow: str,
    actual_workflow: object,
    actual_head: str,
) -> dict[str, object]:
    return {
        "lane": LANE_READINESS,
        "scope": "pr",
        "status": STATUS_INCONCLUSIVE,
        "reason_code": "required_check_wrong_workflow",
        "message": f"required check {required_check} came from unexpected workflow",
        "evidence": {
            "check_name": required_check,
            "expected_head": actual_head,
            "actual_head": actual_head,
            "workflow": actual_workflow,
            "expected_workflow": expected_workflow,
        },
    }


def with_merge_condition_check(
    finding: dict[str, object],
    *,
    merge_check: str,
    source_check: str,
) -> dict[str, object]:
    if merge_check == source_check:
        return finding
    return {
        **finding,
        "evidence": {
            **dict(finding.get("evidence", {})),
            "merge_condition_check": merge_check,
        },
    }


def source_check_evidence(readiness: Mapping[str, object]) -> tuple[object, ...]:
    if "source_checks" in readiness:
        raw_checks = readiness["source_checks"]
    else:
        raw_checks = readiness.get("checks", ())
    if raw_checks is None:
        return ()
    if isinstance(raw_checks, (str, bytes)) or not isinstance(raw_checks, Sequence):
        return ()
    return tuple(raw_checks)


def mergify_required_check_finding(
    *,
    merge_check: str,
    source_check: str,
    readiness: Mapping[str, object],
    expected_workflow: str | None,
) -> dict[str, object] | None:
    checks = source_check_evidence(readiness)
    metadata = dict(readiness.get("metadata", {}))
    actual_head = str(metadata.get("headRefOid", ""))
    matches = tuple(check for check in checks if check_name_matches(check, source_check))
    if not matches:
        finding = missing_mergify_required_check_finding(
            required_check=source_check,
            actual_head=actual_head,
        )
        return with_merge_condition_check(
            finding,
            merge_check=merge_check,
            source_check=source_check,
        )
    if len(matches) > 1:
        finding = duplicate_mergify_required_check_finding(
            required_check=source_check,
            actual_head=actual_head,
        )
        return with_merge_condition_check(
            finding,
            merge_check=merge_check,
            source_check=source_check,
        )
    if expected_workflow is None:
        finding = duplicate_mergify_required_check_finding(
            required_check=source_check,
            actual_head=actual_head,
        )
        return with_merge_condition_check(
            finding,
            merge_check=merge_check,
            source_check=source_check,
        )
    actual_workflow = matches[0].get("workflow")
    if actual_workflow != expected_workflow:
        finding = wrong_workflow_mergify_required_check_finding(
            required_check=source_check,
            expected_workflow=expected_workflow,
            actual_workflow=actual_workflow,
            actual_head=actual_head,
        )
        return with_merge_condition_check(
            finding,
            merge_check=merge_check,
            source_check=source_check,
        )
    finding = required_check_state_finding(
        check=matches[0],
        expected_head=actual_head,
        actual_head=actual_head,
    )
    if finding is None:
        return None
    return with_merge_condition_check(
        finding,
        merge_check=merge_check,
        source_check=source_check,
    )


def mergify_required_check_context_finding(
    finding: dict[str, object],
    *,
    pr: int,
    queue_rule: str,
) -> dict[str, object]:
    return {
        **finding,
        "evidence": {
            **dict(finding["evidence"]),
            "pr": pr,
            "queue_rule": queue_rule,
        },
    }


def selected_mergify_required_check_findings(
    *,
    config: Mapping[str, object],
    route_findings: Sequence[Mapping[str, object]],
    readiness: Sequence[Mapping[str, object]],
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
) -> tuple[dict[str, object], ...]:
    rules_by_name = mergify_queue_rules_by_name(config)
    readiness_by_pr = {int(item["pr"]): item for item in readiness}
    findings: list[dict[str, object]] = []
    for route_finding in route_findings:
        if route_finding["reason_code"] != "mergify_queue_route_selected":
            continue
        route_evidence = dict(route_finding["evidence"])
        pr = int(route_evidence["pr"])
        queue_rule = str(route_evidence["queue_rule"])
        rule = rules_by_name[queue_rule]
        item = readiness_by_pr[pr]
        for required_check in mergify_required_check_names(rule):
            source_check = source_check_aliases.get(required_check, required_check)
            finding = mergify_required_check_finding(
                merge_check=required_check,
                source_check=source_check,
                readiness=item,
                expected_workflow=required_check_workflows.get(source_check),
            )
            if finding is not None:
                findings.append(
                    mergify_required_check_context_finding(
                        finding,
                        pr=pr,
                        queue_rule=queue_rule,
                    )
                )
    return tuple(findings)


def mergify_queue_rules_by_name(config: Mapping[str, object]) -> dict[str, Mapping[str, object]]:
    return {
        str(rule["name"]): rule
        for rule in tuple(config["queue_rules"])
    }


def mergify_queue_batch_max(rule: Mapping[str, object]) -> int:
    batch_size = rule["batch_size"]
    if isinstance(batch_size, Mapping):
        return int(batch_size["max"])
    return int(batch_size)


def mergify_queue_batch_min(rule: Mapping[str, object]) -> int:
    batch_size = rule["batch_size"]
    if isinstance(batch_size, Mapping):
        return int(batch_size["min"])
    return int(batch_size)


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


def mergify_queue_batch_below_min_finding(
    *,
    queue_rule: str,
    prs: Sequence[int],
    min_batch_size: int,
    batch_max_wait_time: object,
) -> dict[str, object]:
    return {
        "lane": LANE_MERGIFY_CONFIG,
        "scope": "queue",
        "status": STATUS_READY,
        "reason_code": "mergify_queue_batch_below_min_wait",
        "message": f"Mergify queue rule {queue_rule} selected {len(prs)} PRs below min batch size {min_batch_size}",
        "evidence": {
            "queue_rule": queue_rule,
            "prs": list(prs),
            "selected_count": len(prs),
            "min_batch_size": min_batch_size,
            "batch_max_wait_time": batch_max_wait_time,
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
        min_batch_size = mergify_queue_batch_min(rule)
        max_batch_size = mergify_queue_batch_max(rule)
        if len(prs) > max_batch_size:
            findings.append(
                mergify_queue_batch_above_max_finding(
                    queue_rule=queue_rule,
                    prs=prs,
                    max_batch_size=max_batch_size,
                )
            )
        if len(prs) < min_batch_size:
            findings.append(
                mergify_queue_batch_below_min_finding(
                    queue_rule=queue_rule,
                    prs=prs,
                    min_batch_size=min_batch_size,
                    batch_max_wait_time=rule["batch_max_wait_time"],
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
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
) -> tuple[dict[str, object], ...]:
    route_findings = available_mergify_queue_route_findings(config=config, readiness=readiness)
    return (
        *route_findings,
        *selected_mergify_required_check_findings(
            config=config,
            route_findings=route_findings,
            readiness=readiness,
            required_check_workflows=required_check_workflows,
            source_check_aliases=source_check_aliases,
        ),
        *selected_mergify_queue_proof_source_findings(
            config=config,
            route_findings=route_findings,
        ),
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
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
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
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
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
            required_check_workflows=required_check_workflows,
            source_check_aliases=source_check_aliases,
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


def check_bucket_state(raw_bucket: object) -> str | None:
    if not isinstance(raw_bucket, str):
        return None
    return CHECK_BUCKET_STATE_ALIASES.get(normalize_check_state(raw_bucket))


def required_check_state_finding(
    *,
    check: Mapping[str, object],
    expected_head: str,
    actual_head: str,
) -> dict[str, object] | None:
    raw_name = check.get("name")
    raw_workflow = check.get("workflow")
    valid_name = isinstance(raw_name, str) and bool(raw_name)
    valid_workflow = isinstance(raw_workflow, str) and bool(raw_workflow)
    check_name = raw_name if valid_name else "<unknown>"
    if not valid_name or not valid_workflow:
        return classify_required_check_state(
            check_name=check_name,
            raw_state="unknown",
            expected_head=expected_head,
            actual_head=actual_head,
            evidence={"workflow": raw_workflow, "bucket": check.get("bucket")},
        )
    state_finding = classify_required_check_state(
        check_name=check_name,
        raw_state=str(check.get("state", "missing")),
        expected_head=expected_head,
        actual_head=actual_head,
        evidence={"workflow": check.get("workflow"), "bucket": check.get("bucket")},
    )
    candidates = [state_finding]
    bucket_state = check_bucket_state(check.get("bucket"))
    if bucket_state is None:
        candidates.append(
            classify_required_check_state(
                check_name=check_name,
                raw_state="unknown",
                expected_head=expected_head,
                actual_head=actual_head,
                evidence={"workflow": check.get("workflow"), "bucket": check.get("bucket")},
            )
        )
    else:
        candidates.append(
            classify_required_check_state(
                check_name=check_name,
                raw_state=bucket_state,
                expected_head=expected_head,
                actual_head=actual_head,
                evidence={"workflow": check.get("workflow"), "bucket": check.get("bucket")},
            )
        )
    finding = min(
        candidates,
        key=lambda candidate: CONTRACT_STATUS_RANK[str(candidate["status"])],
    )
    if finding["status"] == STATUS_READY:
        return None
    return finding


def required_check_readiness_issue(
    check: object,
    *,
    expected_head: str,
    actual_head: str,
) -> ReadinessIssue | None:
    if not isinstance(check, Mapping):
        return ReadinessIssue(
            code="required_check_unknown",
            message="required check metadata is malformed",
        )
    finding = required_check_state_finding(
        check=check,
        expected_head=expected_head,
        actual_head=actual_head,
    )
    if finding is None:
        return None
    code = str(finding["reason_code"])
    evidence = dict(finding["evidence"])
    name = str(evidence["check_name"])
    return ReadinessIssue(
        code=code,
        message=CHECK_STATE_ISSUE_MESSAGES[code].format(name=name),
    )


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
        require_remote_name(origin)
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
            raise PreflightError("configured Git remote did not resolve to a URL")
        remote_url = fetchable_remote_url(remote_url, self.source_repo)
        remote_url = require_credential_free_remote_url(remote_url)
        self.remotes[origin] = remote_url
        return remote_url

    def fetch_sha(self, origin: str, source: str, name: str) -> str:
        if not self.git_repo.is_dir():
            raise PreflightError(f"private Git repository directory {self.git_repo} does not exist")
        ref = f"{self.namespace}/{name}"
        remote_url = self.fetch_origin(origin)
        git(
            self.git_repo,
            "fetch",
            "--quiet",
            "--no-write-fetch-head",
            "--no-tags",
            remote_url,
            f"{source}:{ref}",
            redact_values=(remote_url,),
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
class VerifierResult:
    command: str
    returncode: int
    stdout: str
    stderr: str
    classification: str = "verifier_failed"

    def as_public_json(self, output_policy: OutputPolicy) -> dict[str, object]:
        payload: dict[str, object] = {
            "command": self.command,
            "returncode": self.returncode,
        }
        if self.returncode != 0:
            payload["classification"] = self.classification
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
    source_fence_full_profile_pathspecs: tuple[str, ...]
    source_fence_fences_only_rewrites: dict[str, str]
    required_check_workflows: dict[str, str]
    source_check_aliases: dict[str, str]
    input_timeout_seconds: int
    verifier_timeout_seconds: int
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
READINESS_ISSUE_ARTIFACT_TYPES = {
    "base_mismatch": "base_mismatch",
    "draft": "readiness_failed",
    "head_mismatch": "head_mismatch",
    "not_mergeable": "readiness_failed",
    "not_open": "readiness_failed",
    "required_check_failed": "required_check_failed",
    "required_check_pending": "required_check_pending",
    "required_check_skipped": "required_check_skipped",
    "required_check_missing": "required_check_missing",
    "required_check_unknown": "required_check_unknown",
    "required_check_stale": "required_check_stale",
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
    redact_values: Sequence[str] = (),
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
        rendered = " ".join(
            shlex.quote(redact_remote_urls(part, redact_values)) for part in result.args
        )
        raise PreflightError(
            "command failed ({returncode}): {rendered}\n{stderr}{stdout}".format(
                returncode=result.returncode,
                rendered=rendered,
                stderr=redact_remote_urls(result.stderr, redact_values),
                stdout=redact_remote_urls(result.stdout, redact_values),
            )
        )
    return result


def git(
    repo: pathlib.Path,
    *args: str,
    check: bool = True,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout_seconds: int | None = None,
    redact_values: Sequence[str] = (),
) -> CommandResult:
    return run_command(
        ["git", *args],
        cwd=repo,
        check=check,
        env=env,
        input_text=input_text,
        timeout_seconds=timeout_seconds,
        redact_values=redact_values,
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


def require_string_map(
    parent: dict[str, object],
    key: str,
    prefix: str,
    *,
    allow_empty: bool = False,
) -> dict[str, str]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise PreflightError(f"{prefix}.{key} must be a table")
    if not value and not allow_empty:
        raise PreflightError(f"{prefix}.{key} must be a non-empty table")
    result: dict[str, str] = {}
    for raw_key, raw_value in value.items():
        if not isinstance(raw_key, str) or not raw_key:
            raise PreflightError(f"{prefix}.{key} keys must be non-empty strings")
        if not isinstance(raw_value, str) or not raw_value:
            raise PreflightError(f"{prefix}.{key}.{raw_key} must be a non-empty string")
        result[raw_key] = raw_value
    return result


def require_string_tuple(parent: dict[str, object], key: str, prefix: str) -> tuple[str, ...]:
    value = parent.get(key)
    if not isinstance(value, list) or not value:
        raise PreflightError(f"{prefix}.{key} must be a non-empty string array")
    result: list[str] = []
    for index, item in enumerate(value):
        if not isinstance(item, str) or not item:
            raise PreflightError(f"{prefix}.{key}[{index}] must be a non-empty string")
        result.append(item)
    return tuple(result)


def validate_source_check_aliases(
    source_check_aliases: Mapping[str, str],
    required_check_workflows: Mapping[str, str],
) -> None:
    for merge_check, source_check in source_check_aliases.items():
        if source_check not in required_check_workflows:
            raise PreflightError(
                "config.merge_queue_preflight.source_check_aliases."
                f"{merge_check} target {source_check!r} must exist in "
                "config.merge_queue_preflight.required_check_workflows"
            )


def cheap_local_gate_labels(root: Mapping[str, object]) -> frozenset[str]:
    lane_policy = require_table(root, "local_lane_policy", "config")
    labels = lane_policy.get("cheap_lane_labels")
    if not isinstance(labels, list) or not labels:
        raise PreflightError("config.local_lane_policy.cheap_lane_labels must be a non-empty string array")
    result: set[str] = set()
    for index, label in enumerate(labels):
        if not isinstance(label, str) or not label:
            raise PreflightError(f"config.local_lane_policy.cheap_lane_labels[{index}] must be a non-empty string")
        if label.startswith("local-gate:"):
            result.add(label)
    if not result:
        raise PreflightError("config.local_lane_policy.cheap_lane_labels must declare at least one local-gate label")
    return frozenset(result)


def just_recipe_from_rewrite_command(command: str, *, field: str) -> str:
    try:
        parts = shlex.split(command)
    except ValueError as exc:
        raise PreflightError(
            "config.merge_queue_preflight.source_fence_fences_only_rewrites "
            f"{field} contains an invalid shell command: {exc}"
        ) from exc
    if len(parts) != 2 or parts[0] != "just" or not parts[1]:
        raise PreflightError(
            "config.merge_queue_preflight.source_fence_fences_only_rewrites "
            f"{field} must be exactly 'just <public-recipe>'"
        )
    return parts[1]


def validate_source_fence_fences_only_rewrites(
    rewrites: Mapping[str, str],
    cheap_gate_labels: frozenset[str],
) -> None:
    for source, target in rewrites.items():
        source_recipe = just_recipe_from_rewrite_command(source, field="source")
        if f"local-gate:{source_recipe}" not in cheap_gate_labels:
            raise PreflightError(
                "config.merge_queue_preflight.source_fence_fences_only_rewrites "
                f"source {source!r} must route through a configured public local-gate label"
            )
        target_recipe = just_recipe_from_rewrite_command(target, field="target")
        if f"local-gate:{target_recipe}" not in cheap_gate_labels:
            raise PreflightError(
                "config.merge_queue_preflight.source_fence_fences_only_rewrites "
                f"target {target!r} must route through a configured public local-gate label"
            )


def parsed_shell_command(command: str) -> tuple[str, ...] | None:
    try:
        return tuple(shlex.split(command))
    except ValueError:
        return None


def invokes_reduced_source_fence_command(
    parsed: tuple[str, ...],
    rewrite_targets: frozenset[tuple[str, ...]],
    rewrite_target_recipes: frozenset[str],
) -> bool:
    if (
        any(
            len(token) > 2 and token.startswith("--") and FENCES_ONLY_FLAG.startswith(token)
            for token in parsed
        )
        or parsed in rewrite_targets
    ):
        return True
    for index, part in enumerate(parsed):
        if pathlib.PurePath(part).name == "just" and any(
            token in rewrite_target_recipes for token in parsed[index + 1 :]
        ):
            return True
    return False


def uses_shell_wrapper_syntax(parsed: tuple[str, ...]) -> bool:
    for part in parsed:
        if any(marker in part for marker in (" ", "\t", "'", '"')):
            return True
    for index, part in enumerate(parsed):
        if pathlib.PurePath(part).name not in SHELL_COMMAND_EXECUTABLES:
            continue
        for token in parsed[index + 1 :]:
            if token == "--":
                break
            if token.startswith("-") and "c" in token[1:]:
                return True
    return False


def validate_verifier_commands(
    field: str,
    commands: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
) -> None:
    rewrite_targets = frozenset(
        parsed
        for target in source_fence_fences_only_rewrites.values()
        if (parsed := parsed_shell_command(target)) is not None
    )
    rewrite_target_recipes = frozenset(
        recipe
        for target in rewrite_targets
        if len(target) == 2 and pathlib.PurePath(target[0]).name == "just"
        for recipe in (target[1], f"{target[1]}-inner")
    )
    for command in commands:
        parsed = parsed_shell_command(command)
        if parsed is None:
            raise PreflightError(f"{field} contains an invalid shell command {command!r}")
        if uses_shell_wrapper_syntax(parsed):
            raise PreflightError(
                f"{field} must not use shell wrapper syntax {command!r}; "
                "use direct verifier commands"
            )
        if invokes_reduced_source_fence_command(
            parsed,
            rewrite_targets,
            rewrite_target_recipes,
        ):
            raise PreflightError(
                f"{field} must not use reduced-profile rewrite target {command!r}; "
                "use the configured rewrite source"
            )


def validate_verifier_profile_commands(
    profile_name: str,
    commands: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
) -> None:
    validate_verifier_commands(
        f"config.merge_queue_preflight.verifier_profiles.{profile_name}.commands",
        commands,
        source_fence_fences_only_rewrites,
    )


def load_config(path: pathlib.Path) -> PreflightConfig:
    root = load_toml(path)
    settings = require_table(root, "merge_queue_preflight", "config")
    cheap_gate_labels = cheap_local_gate_labels(root)
    origin = require_string(settings, "origin", "config.merge_queue_preflight")
    base = require_string(settings, "base", "config.merge_queue_preflight")
    default_profile = require_string(
        settings, "default_verifier_profile", "config.merge_queue_preflight"
    )
    profiles_root = require_table(
        settings, "verifier_profiles", "config.merge_queue_preflight"
    )
    source_fence_full_profile_pathspecs = require_string_tuple(
        settings,
        "source_fence_full_profile_pathspecs",
        "config.merge_queue_preflight",
    )
    source_fence_fences_only_rewrites = require_string_map(
        settings,
        "source_fence_fences_only_rewrites",
        "config.merge_queue_preflight",
    )
    validate_source_fence_fences_only_rewrites(source_fence_fences_only_rewrites, cheap_gate_labels)
    timeout_settings = require_table(settings, "timeouts", "config.merge_queue_preflight")
    verifier_timeout_seconds = require_positive_int(
        timeout_settings,
        "verifier_seconds",
        "config.merge_queue_preflight.timeouts",
    )
    input_timeout_seconds = require_positive_int(
        timeout_settings,
        "input_seconds",
        "config.merge_queue_preflight.timeouts",
    )
    required_check_workflows = require_string_map(
        settings,
        "required_check_workflows",
        "config.merge_queue_preflight",
    )
    source_check_aliases = require_string_map(
        settings,
        "source_check_aliases",
        "config.merge_queue_preflight",
        allow_empty=True,
    )
    validate_source_check_aliases(source_check_aliases, required_check_workflows)
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
        validate_verifier_profile_commands(
            profile_name,
            raw_commands,
            source_fence_fences_only_rewrites,
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
        source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
        source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
        required_check_workflows=required_check_workflows,
        source_check_aliases=source_check_aliases,
        input_timeout_seconds=input_timeout_seconds,
        verifier_timeout_seconds=verifier_timeout_seconds,
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


def fetch_base(fetch_refs: PrivateFetchRefs, origin: str, base: str) -> str:
    sha = fetch_refs.fetch_sha(origin, f"refs/heads/{base}", f"base-{base}")
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


def configure_verifier_worktree_origin(
    worktree: pathlib.Path,
    origin_url: str,
    input_timeout_seconds: int,
) -> None:
    origin_url = require_credential_free_remote_url(origin_url)
    completed = git(
        worktree,
        "config",
        "--worktree",
        "remote.origin.url",
        origin_url,
        check=False,
        redact_values=(origin_url,),
        timeout_seconds=input_timeout_seconds,
    )
    if completed.returncode != 0:
        details = redact_remote_urls(f"{completed.stderr}{completed.stdout}".strip(), (origin_url,))
        suffix = f": {details}" if details else ""
        raise PreflightError(f"failed to configure verifier origin{suffix}")


def run_verifier_commands(
    repo: pathlib.Path,
    commit: str,
    commands: Sequence[str],
    timeout_seconds: int,
    input_timeout_seconds: int,
    origin_url: str,
    alternate_object_dir: str | None = None,
) -> tuple[VerifierResult, ...]:
    if not commands:
        return ()
    results: list[VerifierResult] = []
    with tempfile.TemporaryDirectory(prefix="merge-queue-preflight-") as tmp:
        worktree = pathlib.Path(tmp) / "worktree"
        git(
            repo,
            "config",
            "extensions.worktreeConfig",
            "true",
            timeout_seconds=input_timeout_seconds,
        )
        git(
            repo,
            "worktree",
            "add",
            "--quiet",
            "--detach",
            str(worktree),
            commit,
            timeout_seconds=input_timeout_seconds,
        )
        try:
            configure_verifier_worktree_origin(
                worktree,
                origin_url,
                input_timeout_seconds,
            )
            for command in commands:
                parts = shlex.split(command)
                if not parts:
                    raise PreflightError("verifier command must not be empty")
                public_command = redact_remote_urls(command, (origin_url,))
                env = None
                if alternate_object_dir:
                    env = os.environ.copy()
                    existing_alternates = env.get("GIT_ALTERNATE_OBJECT_DIRECTORIES")
                    env["GIT_ALTERNATE_OBJECT_DIRECTORIES"] = (
                        alternate_object_dir
                        if not existing_alternates
                        else f"{alternate_object_dir}{os.pathsep}{existing_alternates}"
                    )
                print(
                    f"merge_queue_preflight: verifier running: {public_command}",
                    file=sys.stderr,
                    flush=True,
                )
                completed = run_command(
                    parts,
                    cwd=worktree,
                    check=False,
                    env=env,
                    timeout_seconds=timeout_seconds,
                    process_group=True,
                )
                verifier_result = VerifierResult(
                    command=public_command,
                    returncode=completed.returncode,
                    stdout=redact_remote_urls(completed.stdout, (origin_url,)),
                    stderr=redact_remote_urls(completed.stderr, (origin_url,)),
                    classification=verifier_failure_classification(completed.failure_type),
                )
                results.append(verifier_result)
                status = "passed" if verifier_result.returncode == 0 else "failed"
                print(
                    "merge_queue_preflight: verifier "
                    f"{status}: {public_command} (exit {verifier_result.returncode})",
                    file=sys.stderr,
                    flush=True,
                )
                if verifier_result.returncode != 0:
                    break
        finally:
            git(
                repo,
                "worktree",
                "remove",
                "--force",
                str(worktree),
                check=False,
                timeout_seconds=input_timeout_seconds,
            )
    return tuple(results)


def verifier_failure_classification(failure_type: str | None) -> str:
    if failure_type == "timeout":
        return "verifier_timeout"
    if failure_type == "unavailable":
        return "verifier_unavailable"
    return "verifier_failed"


def batch_verifier_artifact_type(result: VerifierResult) -> str:
    if result.classification == "verifier_timeout":
        return "batch_verifier_timeout"
    if result.classification == "verifier_unavailable":
        return "batch_verifier_unavailable"
    return "batch_verifier_failed"


def first_failed_verifier(results: Sequence[VerifierResult]) -> VerifierResult | None:
    for result in results:
        if result.returncode != 0:
            return result
    return None


def verifier_block(pr: int, result: VerifierResult, output_policy: OutputPolicy) -> dict[str, object]:
    return {
        "pr": pr,
        "reason": f"verifier failed: {result.command}",
        "type": result.classification,
        **result.as_public_json(output_policy),
    }


def source_fence_fences_only_command(
    command: str,
    source_fence_fences_only_rewrites: Mapping[str, str],
) -> str:
    """Rewrite configured source-fence commands to their governed reduced-profile recipes."""
    try:
        parts = shlex.split(command)
    except ValueError:
        return command
    if not parts or FENCES_ONLY_FLAG in parts:
        return command
    for source, target in source_fence_fences_only_rewrites.items():
        if tuple(parts) == tuple(shlex.split(source)):
            return target
    return command


def commit_touches_source_fence_full_profile_path(
    repo: pathlib.Path,
    base_sha: str,
    commit: str,
    pathspecs: Sequence[str],
    input_timeout_seconds: int,
) -> bool:
    """Return True when the commit changes source-fence governance, failing closed."""
    completed = git(
        repo,
        "diff",
        "--name-only",
        base_sha,
        commit,
        "--",
        *pathspecs,
        check=False,
        timeout_seconds=input_timeout_seconds,
    )
    if completed.returncode != 0 or completed.failure_type == "timeout":
        return True
    return any(line.strip() for line in completed.stdout.splitlines())


def verifier_commands_for_commit(
    *,
    repo: pathlib.Path,
    base_sha: str,
    commit: str,
    commands: Sequence[str],
    source_fence_full_profile_pathspecs: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
    input_timeout_seconds: int,
) -> tuple[str, ...]:
    """Select full or fences-only verifier commands for a synthetic commit."""
    if not commands:
        return ()
    if commit_touches_source_fence_full_profile_path(
        repo,
        base_sha,
        commit,
        source_fence_full_profile_pathspecs,
        input_timeout_seconds,
    ):
        return tuple(commands)
    return tuple(
        source_fence_fences_only_command(command, source_fence_fences_only_rewrites)
        for command in commands
    )


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
    """Build optimistic merge batches before running expensive verifier commands."""
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
                    verifiers=(),
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
                    verifiers=(),
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
                verifiers=(),
            )
        )
    return conflicts, batches


@dataclasses.dataclass(frozen=True)
class VerifiedBatchFallback:
    blocked_prs: tuple[dict[str, object], ...]
    conflicts: tuple[dict[str, object], ...]
    batches: tuple[Batch, ...]


def remaining_batch_prs(candidate_batches: Sequence[Batch], start_index: int) -> tuple[int, ...]:
    return tuple(pr for batch in candidate_batches[start_index:] for pr in batch.prs)


def conflict_against_batch_intersects_prs(conflict: Mapping[str, object], prs: set[int]) -> bool:
    against_batch = conflict.get("against_batch", ())
    if not isinstance(against_batch, Sequence) or isinstance(against_batch, str):
        return False
    return any(int(pr) in prs for pr in against_batch)


def verified_fallback_batches(
    *,
    repo: pathlib.Path,
    base_sha: str,
    prs: Sequence[int],
    heads: Mapping[int, ExpectedHead],
    base_commits: Mapping[int, SyntheticCommit],
    batch_max_limits: Mapping[str, int],
    verifier_commands: Sequence[str],
    source_fence_full_profile_pathspecs: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
    verifier_timeout_seconds: int,
    input_timeout_seconds: int,
    output_policy: OutputPolicy,
    alternate_object_dir: str | None,
    origin_url: str,
    start_index: int,
) -> VerifiedBatchFallback:
    """Recover from a failed optimistic batch by verifying each PR, then rebuilding batches."""
    blocked_prs: list[dict[str, object]] = []
    blocked_numbers: set[int] = set()
    base_verifiers: dict[int, tuple[VerifierResult, ...]] = {}
    for pr in prs:
        synthetic = base_commits[pr]
        verifier_results = run_verifier_commands(
            repo,
            synthetic.commit,
            verifier_commands_for_commit(
                repo=repo,
                base_sha=base_sha,
                commit=synthetic.commit,
                commands=verifier_commands,
                source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
                source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
                input_timeout_seconds=input_timeout_seconds,
            ),
            verifier_timeout_seconds,
            input_timeout_seconds,
            origin_url,
            alternate_object_dir,
        )
        failed = first_failed_verifier(verifier_results)
        if failed is not None:
            blocked_prs.append(verifier_block(pr, failed, output_policy))
            blocked_numbers.add(pr)
            continue
        base_verifiers[pr] = verifier_results

    conflicts: list[dict[str, object]] = []
    batches: list[Batch] = []
    current: SyntheticCommit | None = None
    current_verifiers: tuple[VerifierResult, ...] = ()
    batch_index = start_index
    for pr in prs:
        if pr in blocked_numbers:
            continue
        pr_head = heads[pr]
        if current is None:
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_prs = [*current.prs, pr]
        if batch_would_exceed_max(candidate_prs, batch_max_limits):
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
                    verifiers=current_verifiers,
                )
            )
            batch_index += 1
            current = base_commits[pr]
            current_verifiers = base_verifiers[pr]
            continue
        candidate_verifiers = run_verifier_commands(
            repo,
            synthetic.commit,
            verifier_commands_for_commit(
                repo=repo,
                base_sha=base_sha,
                commit=synthetic.commit,
                commands=verifier_commands,
                source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
                source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
                input_timeout_seconds=input_timeout_seconds,
            ),
            verifier_timeout_seconds,
            input_timeout_seconds,
            origin_url,
            alternate_object_dir,
        )
        failed = first_failed_verifier(candidate_verifiers)
        if failed is not None:
            conflicts.append(
                {
                    "pr": pr,
                    "against_batch": list(current.prs),
                    "type": batch_verifier_artifact_type(failed),
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
    return VerifiedBatchFallback(
        blocked_prs=tuple(blocked_prs),
        conflicts=tuple(conflicts),
        batches=tuple(batches),
    )


def verify_final_batches_with_fallback(
    *,
    repo: pathlib.Path,
    base_sha: str,
    candidate_batches: Sequence[Batch],
    heads: Mapping[int, ExpectedHead],
    base_commits: Mapping[int, SyntheticCommit],
    batch_max_limits: Mapping[str, int],
    verifier_commands: Sequence[str],
    source_fence_full_profile_pathspecs: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
    verifier_timeout_seconds: int,
    input_timeout_seconds: int,
    output_policy: OutputPolicy,
    alternate_object_dir: str | None,
    origin_url: str,
) -> tuple[list[dict[str, object]], list[dict[str, object]], list[Batch], set[int]]:
    """Verify optimistic batches, falling back over the remaining suffix after the first failure."""
    blocked_prs: list[dict[str, object]] = []
    conflicts: list[dict[str, object]] = []
    batches: list[Batch] = []
    fallback_suffix_prs: set[int] = set()
    batch_index = 1
    for candidate_index, batch in enumerate(candidate_batches):
        verifier_results = run_verifier_commands(
            repo,
            batch.commit,
            verifier_commands_for_commit(
                repo=repo,
                base_sha=base_sha,
                commit=batch.commit,
                commands=verifier_commands,
                source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
                source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
                input_timeout_seconds=input_timeout_seconds,
            ),
            verifier_timeout_seconds,
            input_timeout_seconds,
            origin_url,
            alternate_object_dir,
        )
        failed = first_failed_verifier(verifier_results)
        if failed is None:
            batches.append(
                Batch(
                    index=batch_index,
                    commit=batch.commit,
                    prs=batch.prs,
                    verifiers=verifier_results,
                )
            )
            batch_index += 1
            continue
        suffix_prs = remaining_batch_prs(candidate_batches, candidate_index)
        fallback_suffix_prs = set(suffix_prs)
        fallback = verified_fallback_batches(
            repo=repo,
            base_sha=base_sha,
            prs=suffix_prs,
            heads=heads,
            base_commits=base_commits,
            batch_max_limits=batch_max_limits,
            verifier_commands=verifier_commands,
            source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
            source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
            verifier_timeout_seconds=verifier_timeout_seconds,
            input_timeout_seconds=input_timeout_seconds,
            output_policy=output_policy,
            alternate_object_dir=alternate_object_dir,
            origin_url=origin_url,
            start_index=batch_index,
        )
        blocked_prs.extend(fallback.blocked_prs)
        conflicts.extend(fallback.conflicts)
        batches.extend(fallback.batches)
        batch_index += len(fallback.batches)
        break
    return blocked_prs, conflicts, batches, fallback_suffix_prs


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
    metadata_head = payload.get("headRefOid")
    actual_check_head = str(metadata_head) if isinstance(metadata_head, str) else ""
    expected_check_head = fetched_head or actual_check_head
    if not checks:
        issues.append(
            ReadinessIssue(
                code="required_check_missing",
                message="required check evidence is unavailable",
            )
        )
    for check in checks:
        issue = required_check_readiness_issue(
            check,
            expected_head=expected_check_head,
            actual_head=actual_check_head,
        )
        if issue is not None:
            issues.append(issue)
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
        return {"pr": pr_number, "warnings": [], "warning_details": [], "checks": [], "source_checks": []}
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
        timeout_seconds=input_timeout_seconds,
    )
    if not isinstance(checks, list):
        raise PreflightError(f"gh pr checks {pr_number} did not return a list")
    source_checks = gh_json(
        [
            "pr",
            "checks",
            str(pr_number),
            "--json",
            "name,state,bucket,workflow",
        ],
        allowed_returncodes=GH_PR_CHECKS_JSON_RETURNCODES,
        timeout_seconds=input_timeout_seconds,
    )
    if not isinstance(source_checks, list):
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
        "source_checks": source_checks,
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
                    "checks": [],
                    "source_checks": [],
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
    output_policy: OutputPolicy,
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
        "output_policy": output_policy.as_json(),
    }
    return payload, int(contract_evaluation["exit_code"])


def preflight(
    *,
    repo: pathlib.Path,
    origin: str,
    base: str,
    expected_base_sha: str,
    expected_origin_url_sha256: str,
    expected_head_inputs: Sequence[ExpectedHead],
    pr_numbers: Sequence[int],
    verifier_commands: Sequence[str],
    source_fence_full_profile_pathspecs: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
    input_timeout_seconds: int,
    verifier_timeout_seconds: int,
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
    output_policy: OutputPolicy,
    use_gh: bool,
) -> tuple[dict[str, object], int]:
    fetch_refs = PrivateFetchRefs.create(repo, input_timeout_seconds)
    try:
        return preflight_with_fetch_refs(
            origin=origin,
            base=base,
            expected_base_sha=expected_base_sha,
            expected_origin_url_sha256=expected_origin_url_sha256,
            expected_head_inputs=expected_head_inputs,
            pr_numbers=pr_numbers,
            verifier_commands=verifier_commands,
            source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
            source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
            input_timeout_seconds=input_timeout_seconds,
            verifier_timeout_seconds=verifier_timeout_seconds,
            required_check_workflows=required_check_workflows,
            source_check_aliases=source_check_aliases,
            output_policy=output_policy,
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
    expected_origin_url_sha256: str,
    expected_head_inputs: Sequence[ExpectedHead],
    pr_numbers: Sequence[int],
    verifier_commands: Sequence[str],
    source_fence_full_profile_pathspecs: Sequence[str],
    source_fence_fences_only_rewrites: Mapping[str, str],
    input_timeout_seconds: int,
    verifier_timeout_seconds: int,
    required_check_workflows: Mapping[str, str],
    source_check_aliases: Mapping[str, str],
    output_policy: OutputPolicy,
    use_gh: bool,
    fetch_refs: PrivateFetchRefs,
) -> tuple[dict[str, object], int]:
    requested = unique_preserving_order(pr_numbers)
    expected_heads = expected_head_map(expected_head_inputs, requested)
    git_repo = fetch_refs.git_repo
    try:
        origin_url = fetch_refs.fetch_origin(origin)
        if remote_url_sha256(origin_url) != expected_origin_url_sha256:
            raise PreflightError("configured Git remote identity changed during merge queue preflight")
        actual_base_sha = fetch_base(fetch_refs, origin, base)
    except PreflightError as exc:
        return unavailable_base_payload(
            base=base,
            expected_base_sha=expected_base_sha,
            expected_heads=expected_heads,
            requested=requested,
            output_policy=output_policy,
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
        required_check_workflows=required_check_workflows,
        source_check_aliases=source_check_aliases,
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
    conflicts, candidate_batches = unverified_batches_for_ready_prs(
        repo=git_repo,
        requested=requested,
        blocked_numbers=blocked_numbers,
        heads=heads,
        base_commits=base_commits,
        batch_max_limits=batch_max_limits,
        input_timeout_seconds=input_timeout_seconds,
    )
    fallback_blocked_prs, fallback_conflicts, batches, fallback_suffix_prs = verify_final_batches_with_fallback(
        repo=git_repo,
        base_sha=base_sha,
        candidate_batches=candidate_batches,
        heads=heads,
        base_commits=base_commits,
        batch_max_limits=batch_max_limits,
        verifier_commands=verifier_commands,
        source_fence_full_profile_pathspecs=source_fence_full_profile_pathspecs,
        source_fence_fences_only_rewrites=source_fence_fences_only_rewrites,
        verifier_timeout_seconds=verifier_timeout_seconds,
        input_timeout_seconds=input_timeout_seconds,
        output_policy=output_policy,
        alternate_object_dir=fetch_refs.source_objects,
        origin_url=origin_url,
    )
    conflicts = [
        conflict
        for conflict in conflicts
        if not conflict_against_batch_intersects_prs(conflict, fallback_suffix_prs)
    ]
    blocked_prs.extend(fallback_blocked_prs)
    conflicts.extend(fallback_conflicts)
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
        *verifier_batch_ready_findings(batches, output_policy),
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
    root.add_argument("--expected-base-sha", required=True, type=commit_sha)
    root.add_argument("--expected-head-sha", action="append", required=True, type=expected_head_sha)
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
    validate_verifier_commands(
        "--run-verifier",
        extra,
        config.source_fence_fences_only_rewrites,
    )
    return (*config.verifier_profiles[selected], *extra)


def expected_origin_url_sha256(environ: Mapping[str, str] | None = None) -> str:
    source = os.environ if environ is None else environ
    value = source.get(EXPECTED_ORIGIN_URL_SHA256_ENV, "")
    if SHA256_RE.fullmatch(value) is None:
        raise PreflightError(f"{EXPECTED_ORIGIN_URL_SHA256_ENV} must contain one SHA-256 digest")
    return value


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        config = load_config(args.config)
        payload, exit_code = preflight(
            repo=pathlib.Path.cwd(),
            origin=config.origin,
            base=config.base,
            expected_base_sha=args.expected_base_sha,
            expected_origin_url_sha256=expected_origin_url_sha256(),
            expected_head_inputs=args.expected_head_sha,
            pr_numbers=args.prs,
            verifier_commands=verifier_commands(config, args.verifier_profile, args.run_verifier),
            source_fence_full_profile_pathspecs=config.source_fence_full_profile_pathspecs,
            source_fence_fences_only_rewrites=config.source_fence_fences_only_rewrites,
            input_timeout_seconds=config.input_timeout_seconds,
            verifier_timeout_seconds=config.verifier_timeout_seconds,
            required_check_workflows=config.required_check_workflows,
            source_check_aliases=config.source_check_aliases,
            output_policy=config.output_policy,
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
