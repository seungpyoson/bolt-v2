"""Contract rule evaluator for declarative verifier tables."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class ContractFinding:
    rule_id: str
    message: str


MISSING_PARENT = object()
MISSING_KEY = object()


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


def rule_field(rule: object, name: str) -> Any:
    return getattr(rule, name)


def rule_selector(rule: object) -> tuple[str, ...]:
    selector = rule_field(rule, "selector")
    return tuple(selector)


def rule_message(rule: object, config_name: str, **values: Any) -> str:
    expected = rule_field(rule, "expected")
    if "expected_yaml" not in values:
        values["expected_yaml"] = yaml_display(expected)
    if "expected_list" not in values and isinstance(expected, tuple):
        values["expected_list"] = list(expected)
    if isinstance(expected, dict):
        values.setdefault("min", expected.get("min"))
        values.setdefault("max", expected.get("max"))
        values.setdefault("required_reviewer", expected.get("required_reviewer"))
        required_checks = expected.get("required_checks")
        if isinstance(required_checks, tuple):
            values.setdefault("required_check_count", len(required_checks))
    return str(rule_field(rule, "message_template")).format(config_name=config_name, **values)


def finding(rule: object, config_name: str, **values: Any) -> ContractFinding:
    return ContractFinding(
        rule_id=str(rule_field(rule, "rule_id")),
        message=rule_message(rule, config_name, **values),
    )


def fallback_finding(rule: object, message: str) -> ContractFinding:
    return ContractFinding(rule_id=str(rule_field(rule, "rule_id")), message=message)


def mapping_value(
    value: Any,
    *,
    path: str,
    rule: object,
    config_name: str,
) -> tuple[dict[str, Any] | None, ContractFinding | None]:
    if isinstance(value, dict):
        return value, None
    return None, fallback_finding(rule, f"{config_name} {path} must be a mapping")


def list_value(
    value: Any,
    *,
    path: str,
    rule: object,
    config_name: str,
) -> tuple[list[Any] | None, ContractFinding | None]:
    if isinstance(value, list):
        return value, None
    return None, fallback_finding(rule, f"{config_name} {path} must be a list")


def unsupported_mapping_keys(mapping: dict[str, Any], allowed: frozenset[str]) -> list[str]:
    return [key for key in mapping if key not in allowed]


def expected_required_conditions(expected: dict[str, object]) -> frozenset[str]:
    checks = expected["required_checks"]
    if not isinstance(checks, tuple):
        raise TypeError("required_checks must be a tuple")
    reviewer = expected["required_reviewer"]
    return frozenset(
        {
            f"approved-reviews-by = {reviewer}",
            *(f"check-success = {check_name}" for check_name in checks),
        }
    )


def named_rules(
    *,
    root: dict[str, Any],
    key: str,
    rule_kind: str,
    order_rule: object,
    config_name: str,
) -> tuple[dict[str, dict[str, Any]], tuple[ContractFinding, ...]]:
    findings: list[ContractFinding] = []
    expected_names = tuple(rule_field(order_rule, "expected"))
    if key not in root:
        findings.append(fallback_finding(order_rule, f"{config_name} must define {key}"))
        for expected_name in expected_names:
            findings.append(
                fallback_finding(
                    order_rule,
                    f"{config_name} must define {expected_name} {rule_kind} rule",
                )
            )
        return {}, tuple(findings)
    values, type_finding = list_value(root[key], path=key, rule=order_rule, config_name=config_name)
    if type_finding is not None:
        findings.append(type_finding)
    if values is None:
        for expected_name in expected_names:
            findings.append(
                fallback_finding(
                    order_rule,
                    f"{config_name} must define {expected_name} {rule_kind} rule",
                )
            )
        return {}, tuple(findings)

    names: list[Any] = []
    by_name: dict[str, dict[str, Any]] = {}
    for index, value in enumerate(values):
        entry, entry_finding = mapping_value(
            value,
            path=f"{key}[{index}]",
            rule=order_rule,
            config_name=config_name,
        )
        if entry_finding is not None:
            findings.append(entry_finding)
        name = entry.get("name") if entry is not None else None
        names.append(name)
        if isinstance(name, str) and entry is not None:
            by_name[name] = entry

    if tuple(names) != expected_names:
        findings.append(finding(order_rule, config_name))
    for expected_name in expected_names:
        if expected_name not in by_name:
            findings.append(
                fallback_finding(
                    order_rule,
                    f"{config_name} must define {expected_name} {rule_kind} rule",
                )
            )
    return by_name, tuple(findings)


def queue_rule_context(
    *,
    root: dict[str, Any],
    rules: tuple[object, ...],
    config_name: str,
) -> tuple[dict[str, dict[str, Any]], dict[str, dict[str, Any]], tuple[ContractFinding, ...]]:
    findings: list[ContractFinding] = []
    order_rules = {rule_selector(rule): rule for rule in rules if rule_field(rule, "kind") == "required-rule-presence/order"}

    queue_rules: dict[str, dict[str, Any]] = {}
    priority_rules: dict[str, dict[str, Any]] = {}
    queue_order_rule = order_rules.get(("queue_rules",))
    if queue_order_rule is not None:
        queue_rules, queue_findings = named_rules(
            root=root,
            key="queue_rules",
            rule_kind="queue",
            order_rule=queue_order_rule,
            config_name=config_name,
        )
        findings.extend(queue_findings)
    priority_order_rule = order_rules.get(("priority_rules",))
    if priority_order_rule is not None:
        priority_rules, priority_findings = named_rules(
            root=root,
            key="priority_rules",
            rule_kind="priority",
            order_rule=priority_order_rule,
            config_name=config_name,
        )
        findings.extend(priority_findings)
    return queue_rules, priority_rules, tuple(findings)


def selector_parent(
    *,
    selector: tuple[str, ...],
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
) -> dict[str, Any] | None:
    if selector[0] == "merge_queue":
        value = root.get("merge_queue")
        return value if isinstance(value, dict) else None
    if selector[0] == "queue_rules":
        parent = queue_rules.get(selector[1])
        if parent is not None and len(selector) >= 3 and selector[2] == "batch_size":
            value = parent.get("batch_size")
            return value if isinstance(value, dict) else None
        return parent
    if selector[0] == "priority_rules":
        return priority_rules.get(selector[1])
    return root


def evaluate_forbidden_unknown_key(
    *,
    rule: object,
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
    config_name: str,
) -> tuple[ContractFinding, ...]:
    selector = rule_selector(rule)
    expected = rule_field(rule, "expected")
    if not isinstance(expected, frozenset):
        raise TypeError(f"{rule_field(rule, 'rule_id')} expected key set must be a frozenset")

    if selector == ("root", "manual_queueing_only"):
        return tuple(
            finding(rule, config_name, key=key)
            for key in root
            if key in expected
        )
    if selector == ("root", "supported_keys"):
        return tuple(
            finding(rule, config_name, key=key)
            for key in root
            if key not in expected
        )

    if selector == ("merge_queue",):
        if "merge_queue" not in root:
            return (fallback_finding(rule, f"{config_name} must define merge_queue"),)
        merge_queue = root["merge_queue"]
        if not isinstance(merge_queue, dict):
            return (fallback_finding(rule, f"{config_name} merge_queue must be a mapping"),)

    parent = selector_parent(
        selector=selector,
        root=root,
        queue_rules=queue_rules,
        priority_rules=priority_rules,
    )
    if parent is None:
        return ()
    context = {"rule_name": selector[1]} if selector[0] in {"queue_rules", "priority_rules"} else {}
    return tuple(
        finding(rule, config_name, key=key, **context)
        for key in unsupported_mapping_keys(parent, expected)
    )


def selected_value(
    *,
    rule: object,
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
    config_name: str,
) -> tuple[Any, ContractFinding | None]:
    selector = rule_selector(rule)
    if selector[0] == "merge_queue":
        if "merge_queue" not in root or not isinstance(root["merge_queue"], dict):
            return MISSING_PARENT, None
        merge_queue = root["merge_queue"]
        if len(selector) == 1:
            return merge_queue, None
        if selector[1] not in merge_queue:
            return MISSING_KEY, None
        return merge_queue[selector[1]], None
    if selector[0] == "queue_rules":
        rule_name = selector[1]
        parent = queue_rules.get(rule_name)
        if parent is None:
            return MISSING_PARENT, None
        if len(selector) == 2:
            return parent, None
        if selector[2] not in parent:
            return MISSING_KEY, None
        return parent[selector[2]], None
    if selector[0] == "priority_rules":
        rule_name = selector[1]
        parent = priority_rules.get(rule_name)
        if parent is None:
            return MISSING_PARENT, None
        if len(selector) == 2:
            return parent, None
        if selector[2] not in parent:
            return MISSING_KEY, None
        return parent[selector[2]], None
    return root, None


def evaluate_scalar_eq(
    *,
    rule: object,
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
    config_name: str,
) -> tuple[ContractFinding, ...]:
    actual, selection_finding = selected_value(
        rule=rule,
        root=root,
        queue_rules=queue_rules,
        priority_rules=priority_rules,
        config_name=config_name,
    )
    if selection_finding is not None:
        return (selection_finding,)
    if actual is MISSING_PARENT:
        return ()
    expected = rule_field(rule, "expected")
    if actual is MISSING_KEY or not scalar_equals(actual, expected):
        selector = rule_selector(rule)
        return (finding(rule, config_name, rule_name=selector[1] if len(selector) > 2 else "", key=selector[-1]),)
    return ()


def evaluate_mapping_eq(
    *,
    rule: object,
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
    config_name: str,
) -> tuple[ContractFinding, ...]:
    selector = rule_selector(rule)
    if selector == ("root",):
        if not isinstance(root, dict):
            return (finding(rule, config_name),)
        return ()
    actual, selection_finding = selected_value(
        rule=rule,
        root=root,
        queue_rules=queue_rules,
        priority_rules=priority_rules,
        config_name=config_name,
    )
    if selection_finding is not None:
        return (selection_finding,)
    if actual is MISSING_PARENT:
        return ()
    expected = rule_field(rule, "expected")
    if isinstance(expected, tuple):
        values, type_finding = list_value(
            None if actual is MISSING_KEY else actual,
            path=f"{selector[1]} {selector[2]}" if selector[0] == "queue_rules" else "hotfix priority conditions",
            rule=rule,
            config_name=config_name,
        )
        if type_finding is not None:
            return (type_finding,)
        if tuple(values) != expected:
            return (finding(rule, config_name, rule_name=selector[1]),)
        return ()
    if isinstance(expected, dict):
        values, type_finding = mapping_value(
            None if actual is MISSING_KEY else actual,
            path=f"{selector[1]} {selector[2]}",
            rule=rule,
            config_name=config_name,
        )
        if type_finding is not None:
            return (type_finding,)
        if values != expected:
            return (finding(rule, config_name),)
        return ()
    raise TypeError(f"unsupported mapping-EQ expected value: {expected!r}")


def evaluate_required_membership(
    *,
    rule: object,
    root: dict[str, Any],
    queue_rules: dict[str, dict[str, Any]],
    priority_rules: dict[str, dict[str, Any]],
    config_name: str,
) -> tuple[ContractFinding, ...]:
    selector = rule_selector(rule)
    actual, selection_finding = selected_value(
        rule=rule,
        root=root,
        queue_rules=queue_rules,
        priority_rules=priority_rules,
        config_name=config_name,
    )
    if selection_finding is not None:
        return (selection_finding,)
    if actual is MISSING_PARENT:
        return ()
    values, type_finding = list_value(
        None if actual is MISSING_KEY else actual,
        path=f"{selector[1]} {selector[2]}",
        rule=rule,
        config_name=config_name,
    )
    if type_finding is not None:
        return (type_finding,)
    expected = rule_field(rule, "expected")
    required = expected_required_conditions(expected)
    if set(values) != required or len(values) != len(required):
        return (finding(rule, config_name, rule_name=selector[1]),)
    return ()


def evaluate(
    rules: tuple[object, ...],
    parsed: Any,
    *,
    config_name: str = ".mergify.yml",
) -> tuple[ContractFinding, ...]:
    root_rule = next((rule for rule in rules if rule_selector(rule) == ("root",)), None)
    if not isinstance(parsed, dict):
        if root_rule is None:
            return ()
        return (finding(root_rule, config_name),)

    queue_rules, priority_rules, order_findings = queue_rule_context(
        root=parsed,
        rules=rules,
        config_name=config_name,
    )
    findings: list[ContractFinding] = list(order_findings)

    for rule in rules:
        kind = rule_field(rule, "kind")
        selector = rule_selector(rule)
        if kind == "required-rule-presence/order":
            continue
        if kind == "mapping-EQ" and selector == ("root",):
            continue
        if kind == "forbidden-unknown-key":
            findings.extend(
                evaluate_forbidden_unknown_key(
                    rule=rule,
                    root=parsed,
                    queue_rules=queue_rules,
                    priority_rules=priority_rules,
                    config_name=config_name,
                )
            )
            continue
        if kind == "scalar-EQ":
            findings.extend(
                evaluate_scalar_eq(
                    rule=rule,
                    root=parsed,
                    queue_rules=queue_rules,
                    priority_rules=priority_rules,
                    config_name=config_name,
                )
            )
            continue
        if kind == "mapping-EQ":
            findings.extend(
                evaluate_mapping_eq(
                    rule=rule,
                    root=parsed,
                    queue_rules=queue_rules,
                    priority_rules=priority_rules,
                    config_name=config_name,
                )
            )
            continue
        if kind == "required-membership":
            findings.extend(
                evaluate_required_membership(
                    rule=rule,
                    root=parsed,
                    queue_rules=queue_rules,
                    priority_rules=priority_rules,
                    config_name=config_name,
                )
            )
            continue
        raise ValueError(f"unsupported contract rule kind {kind!r}")

    return tuple(findings)
