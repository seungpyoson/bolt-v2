#!/usr/bin/env python3
"""Fixed-threshold GitHub Actions storage tripwire.

This tool consumes the stable ``ci_storage_audit`` JSON contract. Live mode
imports ``ci_storage_audit`` directly so storage enumeration has one code path.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any, Protocol

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import ci_storage_audit


STORAGE_TRIPWIRE_TABLE_RE = re.compile(r"(?m)^\s*\[\[?\s*storage_tripwire(?:\s*\]|\.)")
WORKFLOW_KEY_RE = re.compile(r"^[A-Za-z0-9_.-]+$")


class TripwireError(RuntimeError):
    pass


class NoTripwirePolicyError(TripwireError):
    pass


class TripwirePolicyInventoryError(TripwireError):
    pass


@dataclass(frozen=True)
class MetricPolicy:
    metric_id: str
    label: str
    json_paths: tuple[str, ...]


@dataclass(frozen=True)
class ThresholdPolicy:
    threshold_id: str
    metric: str
    limit_bytes: int
    severity: str
    title: str


@dataclass(frozen=True)
class MarkerPolicy:
    prefix: str
    suffix: str


@dataclass(frozen=True)
class WorkflowPolicy:
    workflow_path: str
    job_id: str
    job_if: str
    top_level_keys: tuple[str, ...]
    job_keys: tuple[str, ...]
    runner_var: str
    schedule_cron: str
    concurrency_group: str
    cancel_in_progress: bool
    run_command: str
    triggers: tuple[str, ...]
    permissions: Mapping[str, str]
    required_fragments: tuple[str, ...]
    forbidden_fragments: tuple[str, ...]


@dataclass(frozen=True)
class IssueMatchPolicy:
    result_limit: int
    max_open_matches_per_marker: int
    max_page_size: int
    page_size: int


@dataclass(frozen=True)
class StorageTripwirePolicy:
    policy_id: str
    storage_cap_bytes: int
    cap_source: str
    owner: str
    escalation: str
    update_cadence: str
    issue_labels: tuple[str, ...]
    issue_match: IssueMatchPolicy
    marker: MarkerPolicy
    workflow: WorkflowPolicy
    metrics: Mapping[str, MetricPolicy]
    thresholds: tuple[ThresholdPolicy, ...]


@dataclass(frozen=True)
class CommandResult:
    evaluation: dict[str, Any]
    apply_result: dict[str, list[int]] | None
    exit_code: int


class IssueClient(Protocol):
    def find_open_issues_by_marker(
        self, *, marker: str, result_limit: int, page_size: int
    ) -> list[dict[str, Any]]:
        ...

    def create_issue(self, *, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        ...

    def edit_issue(self, *, number: int, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        ...


def require_table(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise TripwireError(f"{field} must be a TOML table")
    return value


def require_string(table: Mapping[str, Any], key: str, field: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value.strip():
        raise TripwireError(f"{field}.{key} must be a non-empty string")
    return value


def require_positive_int(table: Mapping[str, Any], key: str, field: str) -> int:
    value = table.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise TripwireError(f"{field}.{key} must be a positive integer")
    return value


def require_positive_int_at_most(
    table: Mapping[str, Any], key: str, field: str, maximum: int
) -> int:
    value = require_positive_int(table, key, field)
    if value > maximum:
        raise TripwireError(f"{field}.{key} must be no greater than {maximum}")
    return value


def require_bool(table: Mapping[str, Any], key: str, field: str) -> bool:
    value = table.get(key)
    if not isinstance(value, bool):
        raise TripwireError(f"{field}.{key} must be a boolean")
    return value


def require_string_list(table: Mapping[str, Any], key: str, field: str) -> tuple[str, ...]:
    value = table.get(key)
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item.strip() for item in value)
    ):
        raise TripwireError(f"{field}.{key} must be a non-empty string list")
    return tuple(value)


def require_unique_string_list(table: Mapping[str, Any], key: str, field: str) -> tuple[str, ...]:
    values = require_string_list(table, key, field)
    if len(values) != len(set(values)):
        raise TripwireError(f"{field}.{key} must not contain duplicates")
    return values


def require_workflow_key_list(table: Mapping[str, Any], key: str, field: str) -> tuple[str, ...]:
    values = require_unique_string_list(table, key, field)
    if any(WORKFLOW_KEY_RE.fullmatch(value) is None for value in values):
        raise TripwireError(f"{field}.{key} must contain valid YAML key identifiers")
    return values


def require_string_mapping(table: Mapping[str, Any], key: str, field: str) -> dict[str, str]:
    value = table.get(key)
    if (
        not isinstance(value, dict)
        or not value
        or any(not isinstance(k, str) or not isinstance(v, str) or not k.strip() or not v.strip() for k, v in value.items())
    ):
        raise TripwireError(f"{field}.{key} must be a non-empty string mapping")
    return dict(value)


def nested_value(payload: Mapping[str, Any], dotted_path: str, *, field: str) -> Any:
    if not dotted_path or any(not part for part in dotted_path.split(".")):
        raise TripwireError(f"{field} contains invalid JSON path {dotted_path!r}")
    current: Any = payload
    for part in dotted_path.split("."):
        if not isinstance(current, dict) or part not in current:
            raise TripwireError(f"{field} path {dotted_path!r} is missing")
        current = current[part]
    return current


def resolve_limit_bytes(raw: Mapping[str, Any], threshold: Mapping[str, Any], field: str) -> int:
    has_direct = "limit_bytes" in threshold
    has_ref = "limit_config_ref" in threshold
    if has_direct == has_ref:
        raise TripwireError(f"{field} must define exactly one of limit_bytes or limit_config_ref")
    if has_direct:
        return require_positive_int(threshold, "limit_bytes", field)
    ref = require_string(threshold, "limit_config_ref", field)
    value = nested_value(raw, ref, field=f"{field}.limit_config_ref")
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise TripwireError(f"{field}.limit_config_ref must resolve to a positive integer")
    return value


def load_policy_text(text: str, *, source: str) -> StorageTripwirePolicy:
    try:
        raw = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise TripwireError(f"{source}: invalid TOML: {exc}") from exc
    root = require_table(raw.get("storage_tripwire"), "storage_tripwire")
    schema_version = require_positive_int(root, "schema_version", "storage_tripwire")
    if schema_version != 1:
        raise TripwireError("storage_tripwire.schema_version must be 1")
    policy_id = require_string(root, "policy_id", "storage_tripwire")
    storage_cap_bytes = require_positive_int(root, "storage_cap_bytes", "storage_tripwire")
    cap_source = require_string(root, "cap_source", "storage_tripwire")
    owner = require_string(root, "owner", "storage_tripwire")
    escalation = require_string(root, "escalation", "storage_tripwire")
    update_cadence = require_string(root, "update_cadence", "storage_tripwire")
    issue_labels = require_string_list(root, "issue_labels", "storage_tripwire")

    issue_match_table = require_table(root.get("issue_match"), "storage_tripwire.issue_match")
    marker_table = require_table(root.get("marker"), "storage_tripwire.marker")
    workflow_table = require_table(root.get("workflow"), "storage_tripwire.workflow")
    metrics_table = require_table(root.get("metrics"), "storage_tripwire.metrics")
    max_page_size = require_positive_int(
        issue_match_table,
        "max_page_size",
        "storage_tripwire.issue_match",
    )
    issue_match = IssueMatchPolicy(
        result_limit=require_positive_int(
            issue_match_table, "result_limit", "storage_tripwire.issue_match"
        ),
        max_open_matches_per_marker=require_positive_int(
            issue_match_table,
            "max_open_matches_per_marker",
            "storage_tripwire.issue_match",
        ),
        max_page_size=max_page_size,
        page_size=require_positive_int_at_most(
            issue_match_table,
            "page_size",
            "storage_tripwire.issue_match",
            max_page_size,
        ),
    )
    if issue_match.result_limit <= issue_match.max_open_matches_per_marker:
        raise TripwireError(
            "storage_tripwire.issue_match.result_limit must be greater than "
            "storage_tripwire.issue_match.max_open_matches_per_marker"
        )
    workflow = WorkflowPolicy(
        workflow_path=require_string(workflow_table, "path", "storage_tripwire.workflow"),
        job_id=require_string(workflow_table, "job_id", "storage_tripwire.workflow"),
        job_if=require_string(workflow_table, "job_if", "storage_tripwire.workflow"),
        top_level_keys=require_workflow_key_list(workflow_table, "top_level_keys", "storage_tripwire.workflow"),
        job_keys=require_workflow_key_list(workflow_table, "job_keys", "storage_tripwire.workflow"),
        runner_var=require_string(workflow_table, "runner_var", "storage_tripwire.workflow"),
        schedule_cron=require_string(workflow_table, "schedule_cron", "storage_tripwire.workflow"),
        concurrency_group=require_string(workflow_table, "concurrency_group", "storage_tripwire.workflow"),
        cancel_in_progress=require_bool(workflow_table, "cancel_in_progress", "storage_tripwire.workflow"),
        run_command=require_string(workflow_table, "run_command", "storage_tripwire.workflow"),
        triggers=require_string_list(workflow_table, "triggers", "storage_tripwire.workflow"),
        permissions=require_string_mapping(workflow_table, "permissions", "storage_tripwire.workflow"),
        required_fragments=require_string_list(workflow_table, "required_fragments", "storage_tripwire.workflow"),
        forbidden_fragments=require_string_list(workflow_table, "forbidden_fragments", "storage_tripwire.workflow"),
    )

    metrics: dict[str, MetricPolicy] = {}
    for metric_id, raw_metric in metrics_table.items():
        metric_table = require_table(raw_metric, f"storage_tripwire.metrics.{metric_id}")
        metrics[metric_id] = MetricPolicy(
            metric_id=metric_id,
            label=require_string(metric_table, "label", f"storage_tripwire.metrics.{metric_id}"),
            json_paths=require_string_list(metric_table, "json_paths", f"storage_tripwire.metrics.{metric_id}"),
        )

    raw_thresholds = root.get("thresholds")
    if not isinstance(raw_thresholds, list) or not raw_thresholds:
        raise TripwireError("storage_tripwire.thresholds must be a non-empty array")
    threshold_ids: set[str] = set()
    thresholds: list[ThresholdPolicy] = []
    for index, raw_threshold in enumerate(raw_thresholds):
        field = f"storage_tripwire.thresholds[{index}]"
        threshold = require_table(raw_threshold, field)
        threshold_id = require_string(threshold, "id", field)
        if threshold_id in threshold_ids:
            raise TripwireError(f"{field}.id duplicates threshold id {threshold_id!r}")
        threshold_ids.add(threshold_id)
        metric = require_string(threshold, "metric", field)
        if metric not in metrics:
            raise TripwireError(f"{field}.metric references unknown metric {metric!r}")
        thresholds.append(
            ThresholdPolicy(
                threshold_id=threshold_id,
                metric=metric,
                limit_bytes=resolve_limit_bytes(raw, threshold, field),
                severity=require_string(threshold, "severity", field),
                title=require_string(threshold, "title", field),
            )
        )

    return StorageTripwirePolicy(
        policy_id=policy_id,
        storage_cap_bytes=storage_cap_bytes,
        cap_source=cap_source,
        owner=owner,
        escalation=escalation,
        update_cadence=update_cadence,
        issue_labels=issue_labels,
        issue_match=issue_match,
        marker=MarkerPolicy(
            prefix=require_string(marker_table, "prefix", "storage_tripwire.marker"),
            suffix=require_string(marker_table, "suffix", "storage_tripwire.marker"),
        ),
        workflow=workflow,
        metrics=metrics,
        thresholds=tuple(thresholds),
    )


def load_policy(path: pathlib.Path) -> StorageTripwirePolicy:
    return load_policy_text(path.read_text(encoding="utf-8"), source=str(path))


def repository_toml_paths(root: pathlib.Path) -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--cached", "*.toml"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "unknown git failure"
        raise TripwirePolicyInventoryError(f"git ls-files failed while discovering storage tripwire policy: {detail}")
    return [root / line for line in result.stdout.splitlines() if line.strip()]


def contains_storage_tripwire_table(text: str) -> bool:
    return STORAGE_TRIPWIRE_TABLE_RE.search(text) is not None


def discover_policy_path(root: pathlib.Path) -> pathlib.Path:
    matches: list[pathlib.Path] = []
    for path in repository_toml_paths(root):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            raise TripwireError(f"{path}: cannot inspect TOML while discovering storage tripwire policy: {exc}") from exc
        if not contains_storage_tripwire_table(text):
            continue
        try:
            parsed = tomllib.loads(text)
        except tomllib.TOMLDecodeError as exc:
            raise TripwireError(f"{path}: invalid TOML for storage tripwire policy: {exc}") from exc
        if isinstance(parsed, dict) and isinstance(parsed.get("storage_tripwire"), dict):
            matches.append(path)
    if not matches:
        raise NoTripwirePolicyError("no repository TOML file declares [storage_tripwire]")
    if len(matches) > 1:
        rels = ", ".join(str(path.relative_to(root)) for path in matches)
        raise TripwireError(f"multiple repository TOML files declare [storage_tripwire]: {rels}")
    return matches[0]


def load_repo_policy(root: pathlib.Path) -> StorageTripwirePolicy:
    return load_policy(discover_policy_path(root))


def load_audit_json(path: pathlib.Path) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise TripwireError(f"{path}: invalid JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise TripwireError(f"{path}: audit JSON must be an object")
    return payload


def metric_actual_bytes(metric: MetricPolicy, audit: Mapping[str, Any]) -> int:
    total = 0
    for json_path in metric.json_paths:
        value = nested_value(audit, json_path, field=f"metric {metric.metric_id}")
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise TripwireError(f"metric {metric.metric_id} path {json_path!r} must resolve to non-negative integer bytes")
        total += value
    return total


def evaluate_tripwire(policy: StorageTripwirePolicy, audit: Mapping[str, Any]) -> dict[str, Any]:
    metrics: dict[str, dict[str, Any]] = {}
    for metric_id, metric in policy.metrics.items():
        metrics[metric_id] = {
            "label": metric.label,
            "json_paths": list(metric.json_paths),
            "actual_bytes": metric_actual_bytes(metric, audit),
        }

    thresholds: list[dict[str, Any]] = []
    for threshold in policy.thresholds:
        metric = metrics[threshold.metric]
        actual_bytes = metric["actual_bytes"]
        thresholds.append(
            {
                "id": threshold.threshold_id,
                "metric": threshold.metric,
                "metric_label": metric["label"],
                "severity": threshold.severity,
                "title": threshold.title,
                "limit_bytes": threshold.limit_bytes,
                "actual_bytes": actual_bytes,
                "breached": actual_bytes >= threshold.limit_bytes,
            }
        )
    return {
        "policy_id": policy.policy_id,
        "storage_cap_bytes": policy.storage_cap_bytes,
        "cap_source": policy.cap_source,
        "snapshot_utc": str(audit.get("snapshot_utc", "")),
        "repo": str(audit.get("repo", "")),
        "metrics": metrics,
        "thresholds": thresholds,
        "breached": any(threshold["breached"] for threshold in thresholds),
        "data_source": "ci-storage-audit stable JSON",
    }


def issue_marker(policy: StorageTripwirePolicy, threshold_id: str) -> str:
    return f"{policy.marker.prefix}{threshold_id}{policy.marker.suffix}"


def body_has_marker_line(body: str, marker: str) -> bool:
    return any(line.strip() == marker for line in body.splitlines())


def render_issue_body(
    policy: StorageTripwirePolicy,
    evaluation: Mapping[str, Any],
    threshold: Mapping[str, Any],
) -> str:
    marker = issue_marker(policy, str(threshold["id"]))
    metric_lines = []
    for metric_id, metric in evaluation["metrics"].items():
        metric_lines.append(
            f"- {metric['label']}: {ci_storage_audit.human_bytes(metric['actual_bytes'])}"
        )
    return "\n".join(
        [
            marker,
            "",
            "## CI Storage Tripwire",
            "",
            f"- Policy: `{evaluation['policy_id']}`",
            f"- Repository: `{evaluation['repo']}`",
            f"- Snapshot: `{evaluation['snapshot_utc']}`",
            f"- Data source: {evaluation['data_source']}",
            f"- Cap source: {evaluation['cap_source']}",
            f"- Owner: {policy.owner}",
            f"- Escalation: {policy.escalation}",
            f"- Cadence: {policy.update_cadence}",
            "",
            "## Breach",
            "",
            f"- Threshold: `{threshold['id']}`",
            f"- Severity: `{threshold['severity']}`",
            f"- Metric: {threshold['metric_label']}",
            f"- Actual: {ci_storage_audit.human_bytes(threshold['actual_bytes'])}",
            f"- Limit: {ci_storage_audit.human_bytes(threshold['limit_bytes'])}",
            "",
            "## Current Metrics",
            "",
            *metric_lines,
            "",
            "No storage mutation was performed.",
        ]
    )


def existing_issue_by_marker(
    policy: StorageTripwirePolicy,
    issues: list[dict[str, Any]],
    marker: str,
) -> dict[str, Any] | None:
    if len(issues) > policy.issue_match.max_open_matches_per_marker:
        raise TripwireError(f"multiple open issues matched tripwire marker {marker!r}")
    return issues[0] if issues else None


def issue_label_names(issue: Mapping[str, Any]) -> frozenset[str]:
    raw_labels = issue.get("labels", [])
    if not isinstance(raw_labels, list):
        raise TripwireError("matched issue labels must be a list")
    names: set[str] = set()
    for label in raw_labels:
        if isinstance(label, str):
            name = label
        elif isinstance(label, dict):
            name = label.get("name")
        else:
            raise TripwireError("matched issue labels must be strings or objects with name")
        if not isinstance(name, str) or not name.strip():
            raise TripwireError("matched issue label names must be non-empty strings")
        names.add(name)
    return frozenset(names)


def issue_matches_desired_state(
    issue: Mapping[str, Any], *, title: str, body: str, labels: list[str]
) -> bool:
    return (
        issue.get("title") == title
        and issue.get("body") == body
        and issue_label_names(issue) == frozenset(labels)
    )


def require_issue_number(issue: Mapping[str, Any], context: str) -> int:
    value = issue.get("number")
    if isinstance(value, bool):
        raise TripwireError(f"{context} issue number must be a positive integer")
    try:
        number = int(value)
    except (TypeError, ValueError) as exc:
        raise TripwireError(f"{context} issue number must be a positive integer") from exc
    if number <= 0:
        raise TripwireError(f"{context} issue number must be a positive integer")
    return number


def apply_alerts(
    policy: StorageTripwirePolicy,
    evaluation: Mapping[str, Any],
    client: IssueClient,
) -> dict[str, list[int]]:
    breached = [threshold for threshold in evaluation["thresholds"] if threshold["breached"]]
    if not breached:
        return {"created": [], "updated": [], "unchanged": []}

    created: list[int] = []
    updated: list[int] = []
    unchanged: list[int] = []
    labels = list(policy.issue_labels)
    pending: list[dict[str, Any]] = []
    for threshold in breached:
        title = str(threshold["title"])
        body = render_issue_body(policy, evaluation, threshold)
        marker = issue_marker(policy, str(threshold["id"]))
        matches = client.find_open_issues_by_marker(
            marker=marker,
            result_limit=policy.issue_match.result_limit,
            page_size=policy.issue_match.page_size,
        )
        existing = existing_issue_by_marker(policy, matches, marker)
        if existing is None:
            pending.append({"action": "create", "title": title, "body": body})
            continue
        number = require_issue_number(existing, "matched")
        if issue_matches_desired_state(existing, title=title, body=body, labels=labels):
            pending.append({"action": "unchanged", "number": number})
            continue
        pending.append({"action": "update", "number": number, "title": title, "body": body})

    for alert in pending:
        action = alert["action"]
        if action == "create":
            created_issue = client.create_issue(title=alert["title"], body=alert["body"], labels=labels)
            created.append(require_issue_number(created_issue, "created"))
        elif action == "update":
            number = int(alert["number"])
            client.edit_issue(number=number, title=alert["title"], body=alert["body"], labels=labels)
            updated.append(number)
        elif action == "unchanged":
            unchanged.append(int(alert["number"]))
        else:
            raise TripwireError(f"unknown alert action {action!r}")
    return {"created": created, "updated": updated, "unchanged": unchanged}


class GhIssueClient:
    def __init__(self, repo: str) -> None:
        self.repo = repo

    def api(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | None = None,
        fields: Mapping[str, Any] | None = None,
        paginate: bool = False,
    ) -> Any:
        cmd = ["gh", "api", "--method", method, path]
        if paginate:
            cmd.extend(["--paginate", "--slurp"])
        for key, value in (fields or {}).items():
            cmd.extend(["--field", f"{key}={value}"])
        input_text = None
        if payload is not None:
            cmd.extend(["--input", "-"])
            input_text = json.dumps(payload)
        result = subprocess.run(cmd, input=input_text, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            raise TripwireError(result.stderr.strip() or "gh api failed")
        try:
            return json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise TripwireError(f"gh api returned invalid JSON: {exc}") from exc

    def find_open_issues_by_marker(
        self, *, marker: str, result_limit: int, page_size: int
    ) -> list[dict[str, Any]]:
        payload = self.api(
            "GET",
            f"repos/{self.repo}/issues",
            fields={
                "state": "open",
                "per_page": str(page_size),
            },
            paginate=True,
        )
        if not isinstance(payload, list):
            raise TripwireError("issue list response must be a paginated list")
        matches: list[dict[str, Any]] = []
        for page in payload:
            if not isinstance(page, list):
                raise TripwireError("issue list response pages must be lists")
            for issue in page:
                if not isinstance(issue, dict) or "pull_request" in issue:
                    continue
                body = issue.get("body")
                if isinstance(body, str) and body_has_marker_line(body, marker):
                    matches.append(issue)
                    if len(matches) >= result_limit:
                        return matches
        return matches

    def create_issue(self, *, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        payload = self.api(
            "POST",
            f"repos/{self.repo}/issues",
            {"title": title, "body": body, "labels": labels},
        )
        if not isinstance(payload, dict) or "number" not in payload:
            raise TripwireError("issue create response did not include number")
        return payload

    def edit_issue(self, *, number: int, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        payload = self.api(
            "PATCH",
            f"repos/{self.repo}/issues/{number}",
            {"title": title, "body": body, "labels": labels},
        )
        if not isinstance(payload, dict) or "number" not in payload:
            raise TripwireError("issue edit response did not include number")
        return payload


def build_live_audit(repo: str, branch: str) -> dict[str, Any]:
    client = ci_storage_audit.GhClient(repo)
    try:
        return ci_storage_audit.build_snapshot(
            client,
            repo=repo,
            branch=branch,
            snapshot_utc=ci_storage_audit.isoformat_utc(dt.datetime.now(dt.UTC)),
        )
    except ci_storage_audit.AuditError as exc:
        raise TripwireError(f"live audit failed: {exc}") from exc


def write_summary(text: str) -> None:
    raw_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not raw_path:
        return
    with pathlib.Path(raw_path).open("a", encoding="utf-8") as summary:
        summary.write(text + "\n")


def render_result(evaluation: Mapping[str, Any], apply_result: Mapping[str, list[int]] | None = None) -> str:
    lines = [
        f"CI storage tripwire for {evaluation['repo']}",
        f"Snapshot: {evaluation['snapshot_utc']}",
        f"Breached: {str(evaluation['breached']).lower()}",
        "",
        "Thresholds:",
    ]
    for threshold in evaluation["thresholds"]:
        state = "BREACH" if threshold["breached"] else "ok"
        lines.append(
            f"- {threshold['id']}: {state}; actual={ci_storage_audit.human_bytes(threshold['actual_bytes'])}; "
            f"limit={ci_storage_audit.human_bytes(threshold['limit_bytes'])}"
        )
    if apply_result is not None:
        lines.extend(
            [
                "",
                f"Created issues: {apply_result['created']}",
                f"Updated issues: {apply_result['updated']}",
                f"Unchanged issues: {apply_result['unchanged']}",
            ]
        )
    lines.extend(["", "No storage mutation was performed."])
    return "\n".join(lines)


def run_evaluate_command(
    args: argparse.Namespace, policy: StorageTripwirePolicy
) -> CommandResult:
    audit = load_audit_json(args.audit_json)
    evaluation = evaluate_tripwire(policy, audit)
    return CommandResult(
        evaluation=evaluation,
        apply_result=None,
        exit_code=1 if evaluation["breached"] else 0,
    )


def run_apply_command(
    args: argparse.Namespace, policy: StorageTripwirePolicy
) -> CommandResult:
    audit = load_audit_json(args.audit_json)
    repo = args.repo or str(audit.get("repo", ""))
    if not repo:
        raise TripwireError("repo is required for issue alerting")
    evaluation = evaluate_tripwire(policy, audit)
    return CommandResult(
        evaluation=evaluation,
        apply_result=apply_alerts(policy, evaluation, GhIssueClient(repo)),
        exit_code=0,
    )


def run_apply_live_command(
    args: argparse.Namespace, policy: StorageTripwirePolicy
) -> CommandResult:
    audit = build_live_audit(args.repo, args.branch)
    evaluation = evaluate_tripwire(policy, audit)
    return CommandResult(
        evaluation=evaluation,
        apply_result=apply_alerts(policy, evaluation, GhIssueClient(args.repo)),
        exit_code=0,
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--policy",
        type=pathlib.Path,
        help="Tripwire TOML policy. Defaults to the only tracked TOML file declaring [storage_tripwire].",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    evaluate = subparsers.add_parser("evaluate")
    evaluate.add_argument("--audit-json", required=True, type=pathlib.Path)
    evaluate.add_argument("--json", action="store_true")
    evaluate.set_defaults(command_runner=run_evaluate_command)

    apply = subparsers.add_parser("apply")
    apply.add_argument("--audit-json", required=True, type=pathlib.Path)
    apply.add_argument("--repo")
    apply.add_argument("--json", action="store_true")
    apply.set_defaults(command_runner=run_apply_command)

    live = subparsers.add_parser("apply-live")
    live.add_argument("--repo", required=True)
    live.add_argument("--branch", required=True)
    live.add_argument("--json", action="store_true")
    live.set_defaults(command_runner=run_apply_live_command)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    repo_root = pathlib.Path.cwd()
    policy = load_policy(args.policy) if args.policy is not None else load_repo_policy(repo_root)
    result = args.command_runner(args, policy)
    output: str
    if args.json:
        payload: dict[str, Any] = {"evaluation": result.evaluation}
        if result.apply_result is not None:
            payload["alerts"] = result.apply_result
        output = json.dumps(payload, indent=2, sort_keys=True)
    else:
        output = render_result(result.evaluation, result.apply_result)
    print(output)
    write_summary(output)
    return result.exit_code


def run_cli(argv: list[str]) -> int:
    try:
        return main(argv)
    except TripwireError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(run_cli(sys.argv[1:]))
