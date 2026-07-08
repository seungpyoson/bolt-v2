"""Declarative contract rule tables for CI governance verifiers."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from ci_provenance import MERGIFY_CONFIG_EXPECTATIONS


@dataclass(frozen=True)
class ContractRule:
    rule_id: str
    kind: str
    selector: tuple[str, ...]
    expected: Any
    message_template: str


_EXPECTATIONS = MERGIFY_CONFIG_EXPECTATIONS
_REQUIRED_MERGE_CONDITIONS = {
    "required_reviewer": _EXPECTATIONS["required_reviewer"],
    "required_checks": _EXPECTATIONS["required_checks"],
}

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

MERGIFY_RULES = (
    ContractRule(
        "mergify.root.mapping",
        "mapping-EQ",
        ("root",),
        "mapping",
        "{config_name} root must be a mapping",
    ),
    ContractRule(
        "mergify.root.manual_queueing_only",
        "forbidden-unknown-key",
        ("root", "manual_queueing_only"),
        MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS,
        "{config_name} must keep manual queueing only; remove {key}",
    ),
    ContractRule(
        "mergify.root.supported_keys",
        "forbidden-unknown-key",
        ("root", "supported_keys"),
        MERGIFY_TOP_LEVEL_KEYS | MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS,
        "{config_name} must not define unsupported top-level key {key}",
    ),
    ContractRule(
        "mergify.merge_queue.supported_keys",
        "forbidden-unknown-key",
        ("merge_queue",),
        MERGIFY_MERGE_QUEUE_KEYS,
        "{config_name} merge_queue must not define unsupported key {key}",
    ),
    *(
        ContractRule(
            f"mergify.merge_queue.{key}",
            "scalar-EQ",
            ("merge_queue", key),
            _EXPECTATIONS["merge_queue"][key],
            "{config_name} merge_queue.{key} must be {expected_yaml}",
        )
        for key in ("max_parallel_checks", "reset_on_external_merge")
    ),
    ContractRule(
        "mergify.queue_rules.order",
        "required-rule-presence/order",
        ("queue_rules",),
        _EXPECTATIONS["queue_rule_order"],
        "{config_name} queue_rules must define exactly hotfix followed by default",
    ),
    *(
        ContractRule(
            f"mergify.queue_rules.{rule_name}.supported_keys",
            "forbidden-unknown-key",
            ("queue_rules", rule_name),
            MERGIFY_QUEUE_RULE_KEYS,
            "{config_name} {rule_name} must not define unsupported key {key}",
        )
        for rule_name in _EXPECTATIONS["queue_rule_order"]
    ),
    *(
        ContractRule(
            f"mergify.queue_rules.{rule_name}.queue_conditions",
            "mapping-EQ",
            ("queue_rules", rule_name, "queue_conditions"),
            _EXPECTATIONS["queue_rules"][rule_name]["queue_conditions"],
            "{config_name} {rule_name} queue_conditions must be {expected_list!r}",
        )
        for rule_name in _EXPECTATIONS["queue_rule_order"]
    ),
    *(
        ContractRule(
            f"mergify.queue_rules.{rule_name}.merge_conditions",
            "required-membership",
            ("queue_rules", rule_name, "merge_conditions"),
            _REQUIRED_MERGE_CONDITIONS,
            "{config_name} {rule_name} merge_conditions must require {required_reviewer} and all {required_check_count} gates",
        )
        for rule_name in _EXPECTATIONS["queue_rule_order"]
    ),
    *(
        ContractRule(
            f"mergify.queue_rules.{rule_name}.{key}",
            "scalar-EQ",
            ("queue_rules", rule_name, key),
            _EXPECTATIONS["queue_rules"][rule_name][key],
            "{config_name} {rule_name} {key} must be {expected_yaml}",
        )
        for rule_name in _EXPECTATIONS["queue_rule_order"]
        for key in (
            "branch_protection_injection_mode",
            "batch_max_wait_time",
            "batch_max_failure_resolution_attempts",
            "checks_timeout",
            "draft_bot_account",
            "merge_method",
        )
    ),
    ContractRule(
        "mergify.queue_rules.hotfix.batch_size",
        "scalar-EQ",
        ("queue_rules", "hotfix", "batch_size"),
        _EXPECTATIONS["queue_rules"]["hotfix"]["batch_size"],
        "{config_name} hotfix batch_size must be {expected_yaml}",
    ),
    ContractRule(
        "mergify.queue_rules.default.batch_size.supported_keys",
        "forbidden-unknown-key",
        ("queue_rules", "default", "batch_size"),
        MERGIFY_DYNAMIC_BATCH_KEYS,
        "{config_name} default batch_size must not define unsupported key {key}",
    ),
    ContractRule(
        "mergify.queue_rules.default.batch_size",
        "mapping-EQ",
        ("queue_rules", "default", "batch_size"),
        _EXPECTATIONS["queue_rules"]["default"]["batch_size"],
        "{config_name} default batch_size must be min {min} max {max}",
    ),
    ContractRule(
        "mergify.priority_rules.order",
        "required-rule-presence/order",
        ("priority_rules",),
        _EXPECTATIONS["priority_rule_order"],
        "{config_name} priority_rules must define exactly hotfix",
    ),
    ContractRule(
        "mergify.priority_rules.hotfix.supported_keys",
        "forbidden-unknown-key",
        ("priority_rules", "hotfix"),
        MERGIFY_PRIORITY_RULE_KEYS,
        "{config_name} hotfix priority must not define unsupported key {key}",
    ),
    ContractRule(
        "mergify.priority_rules.hotfix.conditions",
        "mapping-EQ",
        ("priority_rules", "hotfix", "conditions"),
        _EXPECTATIONS["priority_rules"]["hotfix"]["conditions"],
        "{config_name} hotfix priority conditions must be {expected_list!r}",
    ),
    ContractRule(
        "mergify.priority_rules.hotfix.priority",
        "scalar-EQ",
        ("priority_rules", "hotfix", "priority"),
        _EXPECTATIONS["priority_rules"]["hotfix"]["priority"],
        "{config_name} hotfix priority must be {expected_yaml}",
    ),
    ContractRule(
        "mergify.priority_rules.hotfix.allow_checks_interruption",
        "scalar-EQ",
        ("priority_rules", "hotfix", "allow_checks_interruption"),
        _EXPECTATIONS["priority_rules"]["hotfix"]["allow_checks_interruption"],
        "{config_name} hotfix allow_checks_interruption must be {expected_yaml}",
    ),
)

MERGIFY_LEGACY_ERROR_FAMILY_RULE_IDS = {
    "{config_name} requires Ruby/Psych to parse YAML": ("mergify.yaml.parse",),
    "{config_name} YAML parser timed out": ("mergify.yaml.parse",),
    "{config_name} YAML parser failed: {detail}": ("mergify.yaml.parse",),
    "{config_name} YAML parser returned invalid JSON: {exc}": ("mergify.yaml.parse",),
    "{config_name} YAML parser returned malformed errors": ("mergify.yaml.parse",),
    "{config_name} {error}": ("mergify.yaml.parse",),
    "{config_name} root must be a mapping": ("mergify.root.mapping",),
    "{config_name} must keep manual queueing only; remove {key}": (
        "mergify.root.manual_queueing_only",
    ),
    "{config_name} must not define unsupported top-level key {key}": (
        "mergify.root.supported_keys",
    ),
    "{config_name} must define {key}": (
        "mergify.merge_queue.supported_keys",
        "mergify.queue_rules.order",
        "mergify.priority_rules.order",
    ),
    "{config_name} merge_queue must be a mapping": (
        "mergify.merge_queue.supported_keys",
    ),
    "{config_name} merge_queue must not define unsupported key {key}": (
        "mergify.merge_queue.supported_keys",
    ),
    "{config_name} merge_queue.{key} must be {expected_yaml}": (
        "mergify.merge_queue.max_parallel_checks",
        "mergify.merge_queue.reset_on_external_merge",
    ),
    "{config_name} queue_rules must define exactly hotfix followed by default": (
        "mergify.queue_rules.order",
    ),
    "{config_name} must define {rule_name} {rule_kind} rule": (
        "mergify.queue_rules.order",
        "mergify.priority_rules.order",
    ),
    "{config_name} {path} must be a mapping": (
        "mergify.root.mapping",
        "mergify.queue_rules.default.batch_size",
    ),
    "{config_name} {path} must be a list": (
        "mergify.queue_rules.order",
        "mergify.queue_rules.hotfix.queue_conditions",
        "mergify.queue_rules.default.queue_conditions",
        "mergify.queue_rules.hotfix.merge_conditions",
        "mergify.queue_rules.default.merge_conditions",
        "mergify.priority_rules.order",
        "mergify.priority_rules.hotfix.conditions",
    ),
    "{config_name} {rule_name} must not define unsupported key {key}": (
        "mergify.queue_rules.hotfix.supported_keys",
        "mergify.queue_rules.default.supported_keys",
    ),
    "{config_name} {rule_name} queue_conditions must be {expected_list!r}": (
        "mergify.queue_rules.hotfix.queue_conditions",
        "mergify.queue_rules.default.queue_conditions",
    ),
    "{config_name} {rule_name} merge_conditions must require {required_reviewer} and all {required_check_count} gates": (
        "mergify.queue_rules.hotfix.merge_conditions",
        "mergify.queue_rules.default.merge_conditions",
    ),
    "{config_name} {rule_name} {key} must be {expected_yaml}": (
        "mergify.queue_rules.hotfix.branch_protection_injection_mode",
        "mergify.queue_rules.hotfix.batch_max_wait_time",
        "mergify.queue_rules.hotfix.batch_max_failure_resolution_attempts",
        "mergify.queue_rules.hotfix.checks_timeout",
        "mergify.queue_rules.hotfix.draft_bot_account",
        "mergify.queue_rules.hotfix.merge_method",
        "mergify.queue_rules.default.branch_protection_injection_mode",
        "mergify.queue_rules.default.batch_max_wait_time",
        "mergify.queue_rules.default.batch_max_failure_resolution_attempts",
        "mergify.queue_rules.default.checks_timeout",
        "mergify.queue_rules.default.draft_bot_account",
        "mergify.queue_rules.default.merge_method",
    ),
    "{config_name} hotfix batch_size must be {expected_yaml}": (
        "mergify.queue_rules.hotfix.batch_size",
    ),
    "{config_name} default batch_size must not define unsupported key {key}": (
        "mergify.queue_rules.default.batch_size.supported_keys",
    ),
    "{config_name} default batch_size must be min {min} max {max}": (
        "mergify.queue_rules.default.batch_size",
    ),
    "{config_name} priority_rules must define exactly hotfix": (
        "mergify.priority_rules.order",
    ),
    "{config_name} hotfix priority must not define unsupported key {key}": (
        "mergify.priority_rules.hotfix.supported_keys",
    ),
    "{config_name} hotfix priority conditions must be {expected_list!r}": (
        "mergify.priority_rules.hotfix.conditions",
    ),
    "{config_name} hotfix priority must be {expected_yaml}": (
        "mergify.priority_rules.hotfix.priority",
    ),
    "{config_name} hotfix allow_checks_interruption must be {expected_yaml}": (
        "mergify.priority_rules.hotfix.allow_checks_interruption",
    ),
}
