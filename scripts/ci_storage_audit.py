#!/usr/bin/env python3
"""Read-only GitHub Actions storage audit for the current repository.

The ``--json`` output is a stable downstream contract for #936 and the storage
tripwire: existing keys are append-only. ``required_checks.contexts`` preserves
the raw source shape, so rulesets entries are objects such as
``{"context": "...", "integration_id": ...}`` while legacy branch-protection
fallback entries may be strings or objects such as ``{"context": "...",
"app_id": ...}``.

``cache.count`` and ``artifacts.count`` are GitHub's ``total_count`` when that
field is available. ``enumerated_count`` is derived from the paginated rows this
audit actually read; the two may differ under live CI churn.
"""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import fnmatch
import json
import pathlib
import subprocess
import sys
import tomllib
import urllib.parse
from typing import Any, NamedTuple


class AuditError(RuntimeError):
    pass


class GhApiError(AuditError):
    def __init__(self, path: str, message: str) -> None:
        self.path = path
        self.message = message
        super().__init__(f"{path}: {message}")


class CacheKeyProbeRequest(NamedTuple):
    label: str
    key: str


class ArtifactClassRule(NamedTuple):
    rule_id: str
    name_equals: tuple[str, ...]
    name_prefixes: tuple[str, ...]
    expired_decision: str
    candidate_reason: str | None
    keep_reason: str


class ArtifactCleanupPolicy(NamedTuple):
    schema_version: int
    default_class: str
    default_decision: str
    default_keep_reason: str
    protected_ref_keep_reason: str
    active_run_keep_reason: str
    status_unavailable_keep_reason: str
    expiration_unknown_keep_reason: str
    not_expired_keep_reason: str
    billing_impact_unverifiable: str
    wait_and_remeasure: str
    protected_refs: tuple[str, ...]
    protected_ref_prefixes: tuple[str, ...]
    protected_ref_globs: tuple[str, ...]
    active_run_statuses: tuple[str, ...]
    terminal_run_statuses: tuple[str, ...]
    workflow_run_fetch_limit: int
    billing_probe_paths: tuple[str, ...]
    classes: tuple[ArtifactClassRule, ...]


KEEP_DECISION = "KEEP"
DELETE_CANDIDATE_DECISION = "DELETE-CANDIDATE"
CLEANUP_POLICY_SECTION_MARKER = "[storage_audit.cleanup_feasibility]"


class GhClient:
    def __init__(self, repo: str) -> None:
        self.repo = repo

    def api(
        self,
        path: str,
        *,
        params: dict[str, str] | None = None,
        paginate: bool = False,
    ) -> Any:
        return self.api_global(
            f"repos/{self.repo}/{path}",
            params=params,
            paginate=paginate,
            error_path=path,
        )

    def api_global(
        self,
        path: str,
        *,
        params: dict[str, str] | None = None,
        paginate: bool = False,
        error_path: str | None = None,
    ) -> Any:
        cmd = ["gh", "api"]
        if paginate:
            cmd.extend(["--paginate", "--slurp"])
        cmd.extend(["--method", "GET", path])
        for key, value in (params or {}).items():
            cmd.extend(["-f", f"{key}={value}"])
        result = subprocess.run(cmd, text=True, capture_output=True, check=False)
        label = error_path or path
        if result.returncode != 0:
            raise GhApiError(label, result.stderr.strip() or "gh api failed")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise GhApiError(label, f"invalid JSON: {exc}") from exc
        return merge_paginated_payload(payload) if paginate else payload


def merge_paginated_payload(payload: Any) -> Any:
    if not isinstance(payload, list):
        return payload

    merged: dict[str, Any] = {}
    merged_items: list[Any] = []
    saw_list_page = False
    for page in payload:
        if isinstance(page, list):
            if merged:
                raise AuditError("paginated payload mixed page shapes")
            saw_list_page = True
            merged_items.extend(page)
            continue
        if not isinstance(page, dict):
            continue
        if saw_list_page:
            raise AuditError("paginated payload mixed page shapes")
        for key, value in page.items():
            if isinstance(value, list):
                merged.setdefault(key, [])
                if not isinstance(merged[key], list):
                    raise AuditError(f"paginated field changed type: {key}")
                merged[key].extend(value)
            else:
                merged[key] = value
    if saw_list_page and not merged:
        return merged_items
    return merged


def infer_repo() -> str:
    result = subprocess.run(
        ["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise AuditError("could not infer repo; pass --repo OWNER/REPO")
    repo = result.stdout.strip()
    if "/" not in repo:
        raise AuditError("could not infer repo; pass --repo OWNER/REPO")
    return repo


def infer_default_branch() -> str:
    result = subprocess.run(
        ["gh", "repo", "view", "--json", "defaultBranchRef", "-q", ".defaultBranchRef.name"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise AuditError("could not infer default branch; pass --branch BRANCH")
    branch = result.stdout.strip()
    if not branch:
        raise AuditError("could not infer default branch; pass --branch BRANCH")
    return branch


def isoformat_utc(value: dt.datetime) -> str:
    return value.astimezone(dt.UTC).replace(microsecond=0).isoformat()


def human_bytes(size: int) -> str:
    if size < 0:
        raise ValueError("byte count must be non-negative")
    if size < 1024:
        return f"{size} B"
    value = float(size)
    for unit in ("KiB", "MiB", "GiB", "TiB", "PiB"):
        value /= 1024.0
        if value < 1024.0 or unit == "PiB":
            return f"{value:.1f} {unit}"
    raise AssertionError("unreachable")


def require_object(payload: Any, label: str) -> dict[str, Any]:
    if not isinstance(payload, dict):
        raise AuditError(f"{label} payload is not an object")
    return payload


def list_field(payload: dict[str, Any], field: str, label: str) -> list[Any]:
    value = payload.get(field)
    if not isinstance(value, list):
        raise AuditError(f"{label}.{field} is not a list")
    return value


def optional_text(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def nonnegative_int(value: Any, *, default: int = 0) -> int:
    if isinstance(value, bool):
        return default
    if isinstance(value, int) and value >= 0:
        return value
    return default


def optional_nonnegative_int(value: Any) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int) and value >= 0:
        return value
    return None


def require_policy_table(payload: dict[str, Any], key: str, label: str) -> dict[str, Any]:
    value = payload.get(key)
    if not isinstance(value, dict):
        raise AuditError(f"{label}.{key} must be a table")
    return value


def require_policy_string(payload: dict[str, Any], key: str, label: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        raise AuditError(f"{label}.{key} must be a non-empty string")
    return value


def optional_policy_string(payload: dict[str, Any], key: str, label: str) -> str | None:
    value = payload.get(key)
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise AuditError(f"{label}.{key} must be a non-empty string when present")
    return value


def optional_policy_string_list(payload: dict[str, Any], key: str, label: str) -> tuple[str, ...]:
    value = payload.get(key, [])
    if not isinstance(value, list):
        raise AuditError(f"{label}.{key} must be a string list")
    result: list[str] = []
    for index, item in enumerate(value):
        if not isinstance(item, str) or not item:
            raise AuditError(f"{label}.{key}[{index}] must be a non-empty string")
        result.append(item)
    if len(set(result)) != len(result):
        raise AuditError(f"{label}.{key} must not contain duplicates")
    return tuple(result)


def require_policy_string_list(payload: dict[str, Any], key: str, label: str) -> tuple[str, ...]:
    result = optional_policy_string_list(payload, key, label)
    if not result:
        raise AuditError(f"{label}.{key} must not be empty")
    return result


def require_policy_positive_int(payload: dict[str, Any], key: str, label: str) -> int:
    value = payload.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise AuditError(f"{label}.{key} must be a positive integer")
    return value


def require_policy_decision(payload: dict[str, Any], key: str, label: str) -> str:
    value = require_policy_string(payload, key, label)
    if value not in (KEEP_DECISION, DELETE_CANDIDATE_DECISION):
        raise AuditError(f"{label}.{key} must be {KEEP_DECISION} or {DELETE_CANDIDATE_DECISION}")
    return value


def resolve_string_ref(document: dict[str, Any], dotted_ref: str, label: str) -> str:
    current: Any = document
    for part in dotted_ref.split("."):
        if not isinstance(current, dict) or part not in current:
            raise AuditError(f"{label} reference {dotted_ref!r} is missing")
        current = current[part]
    if not isinstance(current, str) or not current:
        raise AuditError(f"{label} reference {dotted_ref!r} must resolve to a non-empty string")
    return current


def template_prefix(template: str, label: str) -> str:
    prefix = template.split("{", 1)[0]
    if not prefix:
        raise AuditError(f"{label} template must have a literal prefix before the first placeholder")
    return prefix


def referenced_strings(
    document: dict[str, Any],
    table: dict[str, Any],
    key: str,
    label: str,
    *,
    prefix_from_template: bool = False,
) -> tuple[str, ...]:
    refs = optional_policy_string_list(table, key, label)
    values: list[str] = []
    for ref in refs:
        value = resolve_string_ref(document, ref, f"{label}.{key}")
        values.append(template_prefix(value, f"{label}.{key}") if prefix_from_template else value)
    return tuple(values)


def parse_cleanup_class_rule(
    document: dict[str, Any],
    raw: Any,
    *,
    index: int,
) -> ArtifactClassRule:
    if not isinstance(raw, dict):
        raise AuditError(f"storage_audit.cleanup_feasibility.classes[{index}] must be a table")
    label = f"storage_audit.cleanup_feasibility.classes[{index}]"
    rule_id = require_policy_string(raw, "id", label)
    name_equals = (
        optional_policy_string_list(raw, "name_equals", label)
        + referenced_strings(document, raw, "name_equals_from", label)
    )
    name_prefixes = (
        optional_policy_string_list(raw, "name_prefixes", label)
        + referenced_strings(document, raw, "name_prefixes_from", label)
        + referenced_strings(document, raw, "name_prefixes_from_templates", label, prefix_from_template=True)
    )
    if not name_equals and not name_prefixes:
        raise AuditError(f"{label} must declare at least one artifact name matcher")
    if len(set(name_equals)) != len(name_equals):
        raise AuditError(f"{label}.name_equals must not contain duplicates after reference resolution")
    if len(set(name_prefixes)) != len(name_prefixes):
        raise AuditError(f"{label}.name_prefixes must not contain duplicates after reference resolution")
    expired_decision = require_policy_decision(raw, "expired_decision", label)
    candidate_reason = optional_policy_string(raw, "candidate_reason", label)
    if expired_decision == DELETE_CANDIDATE_DECISION and candidate_reason is None:
        raise AuditError(f"{label}.candidate_reason is required for DELETE-CANDIDATE classes")
    if expired_decision == KEEP_DECISION and candidate_reason is not None:
        raise AuditError(f"{label}.candidate_reason must be omitted for KEEP classes")
    return ArtifactClassRule(
        rule_id=rule_id,
        name_equals=name_equals,
        name_prefixes=name_prefixes,
        expired_decision=expired_decision,
        candidate_reason=candidate_reason,
        keep_reason=require_policy_string(raw, "keep_reason", label),
    )


def load_cleanup_policy_text(raw: str, *, label: str) -> ArtifactCleanupPolicy:
    try:
        document = tomllib.loads(raw)
    except tomllib.TOMLDecodeError as exc:
        raise AuditError(f"{label}: invalid TOML: {exc}") from exc
    if not isinstance(document, dict):
        raise AuditError(f"{label}: TOML root must be a table")
    storage_audit = require_policy_table(document, "storage_audit", label)
    table = require_policy_table(storage_audit, "cleanup_feasibility", "storage_audit")
    if table.get("schema_version") != 1:
        raise AuditError("storage_audit.cleanup_feasibility.schema_version must be 1")
    default_decision = require_policy_decision(table, "default_decision", "storage_audit.cleanup_feasibility")
    if default_decision != KEEP_DECISION:
        raise AuditError("storage_audit.cleanup_feasibility.default_decision must be KEEP")
    raw_classes = table.get("classes")
    if not isinstance(raw_classes, list) or not raw_classes:
        raise AuditError("storage_audit.cleanup_feasibility.classes must not be empty")
    classes = tuple(
        parse_cleanup_class_rule(document, raw_class, index=index)
        for index, raw_class in enumerate(raw_classes)
    )
    class_ids = [rule.rule_id for rule in classes]
    if len(set(class_ids)) != len(class_ids):
        raise AuditError("storage_audit.cleanup_feasibility.classes ids must be unique")
    exact_matchers: dict[str, str] = {}
    prefix_matchers: dict[str, str] = {}
    for rule in classes:
        for name in rule.name_equals:
            previous = exact_matchers.setdefault(name, rule.rule_id)
            if previous != rule.rule_id:
                raise AuditError(f"cleanup artifact exact matcher {name!r} is declared by multiple classes")
        for prefix in rule.name_prefixes:
            previous = prefix_matchers.setdefault(prefix, rule.rule_id)
            if previous != rule.rule_id:
                raise AuditError(f"cleanup artifact prefix matcher {prefix!r} is declared by multiple classes")
    for name, exact_rule in exact_matchers.items():
        for prefix, prefix_rule in prefix_matchers.items():
            if exact_rule != prefix_rule and name.startswith(prefix):
                raise AuditError(
                    f"cleanup artifact exact matcher {name!r} overlaps prefix matcher {prefix!r}"
                )
    prefix_items = list(prefix_matchers.items())
    for index, (left_prefix, left_rule) in enumerate(prefix_items):
        for right_prefix, right_rule in prefix_items[index + 1:]:
            if left_rule == right_rule:
                continue
            if left_prefix.startswith(right_prefix) or right_prefix.startswith(left_prefix):
                raise AuditError(
                    f"cleanup artifact prefix matcher {left_prefix!r} overlaps {right_prefix!r}"
                )
    return ArtifactCleanupPolicy(
        schema_version=1,
        default_class=require_policy_string(table, "default_class", "storage_audit.cleanup_feasibility"),
        default_decision=default_decision,
        default_keep_reason=require_policy_string(table, "default_keep_reason", "storage_audit.cleanup_feasibility"),
        protected_ref_keep_reason=require_policy_string(
            table,
            "protected_ref_keep_reason",
            "storage_audit.cleanup_feasibility",
        ),
        active_run_keep_reason=require_policy_string(table, "active_run_keep_reason", "storage_audit.cleanup_feasibility"),
        status_unavailable_keep_reason=require_policy_string(
            table,
            "status_unavailable_keep_reason",
            "storage_audit.cleanup_feasibility",
        ),
        expiration_unknown_keep_reason=require_policy_string(
            table,
            "expiration_unknown_keep_reason",
            "storage_audit.cleanup_feasibility",
        ),
        not_expired_keep_reason=require_policy_string(table, "not_expired_keep_reason", "storage_audit.cleanup_feasibility"),
        billing_impact_unverifiable=require_policy_string(
            table,
            "billing_impact_unverifiable",
            "storage_audit.cleanup_feasibility",
        ),
        wait_and_remeasure=require_policy_string(table, "wait_and_remeasure", "storage_audit.cleanup_feasibility"),
        protected_refs=optional_policy_string_list(table, "protected_refs", "storage_audit.cleanup_feasibility"),
        protected_ref_prefixes=optional_policy_string_list(
            table,
            "protected_ref_prefixes",
            "storage_audit.cleanup_feasibility",
        ),
        protected_ref_globs=optional_policy_string_list(
            table,
            "protected_ref_globs",
            "storage_audit.cleanup_feasibility",
        ),
        active_run_statuses=require_policy_string_list(
            table,
            "active_run_statuses",
            "storage_audit.cleanup_feasibility",
        ),
        terminal_run_statuses=require_policy_string_list(
            table,
            "terminal_run_statuses",
            "storage_audit.cleanup_feasibility",
        ),
        workflow_run_fetch_limit=require_policy_positive_int(
            table,
            "workflow_run_fetch_limit",
            "storage_audit.cleanup_feasibility",
        ),
        billing_probe_paths=optional_policy_string_list(
            table,
            "billing_probe_paths",
            "storage_audit.cleanup_feasibility",
        ),
        classes=classes,
    )


def load_cleanup_policy_path(path: pathlib.Path) -> ArtifactCleanupPolicy:
    try:
        return load_cleanup_policy_text(path.read_text(), label=str(path))
    except OSError as exc:
        raise AuditError(f"{path}: could not read cleanup policy: {exc}") from exc


def repository_toml_paths() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files", "*.toml"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise AuditError(result.stderr.strip() or "git ls-files failed while discovering cleanup policy")
    return [pathlib.Path(line) for line in result.stdout.splitlines() if line]


def discover_cleanup_policy_path() -> pathlib.Path:
    candidates: list[pathlib.Path] = []
    for path in repository_toml_paths():
        try:
            text = path.read_text()
        except OSError as exc:
            raise AuditError(f"{path}: could not inspect TOML during cleanup policy discovery: {exc}") from exc
        if CLEANUP_POLICY_SECTION_MARKER in text:
            candidates.append(path)
    if not candidates:
        raise AuditError("no tracked TOML file declares [storage_audit.cleanup_feasibility]")
    if len(candidates) > 1:
        joined = ", ".join(str(path) for path in candidates)
        raise AuditError(f"multiple cleanup feasibility policies found: {joined}")
    return candidates[0]


def count_with_source(payload: dict[str, Any], *, fallback: int) -> tuple[int, str]:
    value = payload.get("total_count")
    if isinstance(value, bool):
        return fallback, "enumerated_count_fallback"
    if isinstance(value, int) and value >= 0:
        return value, "github_total_count"
    return fallback, "enumerated_count_fallback"


def cache_entry_from_raw(raw: dict[str, Any]) -> dict[str, Any]:
    return {
        "cache_id": raw.get("id"),
        "ref": optional_text(raw.get("ref")),
        "key": optional_text(raw.get("key")),
        "last_accessed_at": optional_text(raw.get("last_accessed_at")),
        "size_bytes": nonnegative_int(raw.get("size_in_bytes")),
    }


def workflow_run_from_raw(raw: Any) -> dict[str, Any]:
    data = raw if isinstance(raw, dict) else {}
    head_branch = optional_text(data.get("head_branch"))
    head_sha = optional_text(data.get("head_sha"))
    ref = optional_text(data.get("ref")) or head_branch
    status = optional_text(data.get("status"))
    return {
        "id": optional_nonnegative_int(data.get("id")),
        "status": status,
        "conclusion": optional_text(data.get("conclusion")),
        "ref": ref,
        "head_branch": head_branch,
        "head_sha": head_sha,
        "event": optional_text(data.get("event")),
        "status_source": "artifact_payload" if status is not None else "not_fetched",
    }


def merge_workflow_run_metadata(entry: dict[str, Any], payload: dict[str, Any]) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    refreshed = workflow_run_from_raw(payload)
    for key, value in refreshed.items():
        if value is not None and key != "status_source":
            workflow_run[key] = value
    workflow_run["status_source"] = "run_api"


def set_workflow_run_status_source(entry: dict[str, Any], source: str) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    workflow_run["status_source"] = source


def artifact_entry_from_raw(raw: dict[str, Any]) -> dict[str, Any]:
    expired = raw.get("expired")
    return {
        "artifact_id": optional_nonnegative_int(raw.get("id")),
        "name": optional_text(raw.get("name")) or "",
        "size_bytes": nonnegative_int(raw.get("size_in_bytes")),
        "created_at": optional_text(raw.get("created_at")),
        "expires_at": optional_text(raw.get("expires_at")),
        "expired": expired if isinstance(expired, bool) else None,
        "workflow_run": workflow_run_from_raw(raw.get("workflow_run")),
    }


def empty_artifact_group() -> dict[str, int]:
    return {
        "total_bytes": 0,
        "count": 0,
        "expired_bytes": 0,
        "expired_count": 0,
        "non_expired_bytes": 0,
        "non_expired_count": 0,
        "unknown_expiration_bytes": 0,
        "unknown_expiration_count": 0,
    }


def add_artifact_expiration_totals(bucket: dict[str, int], *, size_bytes: int, expired: bool | None) -> None:
    bucket["total_bytes"] += size_bytes
    bucket["count"] += 1
    if expired is True:
        bucket["expired_bytes"] += size_bytes
        bucket["expired_count"] += 1
    elif expired is False:
        bucket["non_expired_bytes"] += size_bytes
        bucket["non_expired_count"] += 1
    else:
        bucket["unknown_expiration_bytes"] += size_bytes
        bucket["unknown_expiration_count"] += 1


def parse_cache_key_probe(raw: str) -> CacheKeyProbeRequest:
    if "=" not in raw:
        raise AuditError("--cache-key must be LABEL=KEY")
    label, key = raw.split("=", 1)
    label = label.strip()
    key = key.strip()
    if not label:
        raise AuditError("--cache-key label must not be empty")
    if not key:
        raise AuditError("--cache-key key must not be empty")
    return CacheKeyProbeRequest(label=label, key=key)


def fetch_cache_key_probes(
    client: GhClient,
    requests: list[CacheKeyProbeRequest],
) -> list[dict[str, Any]]:
    probes: list[dict[str, Any]] = []
    for request in requests:
        payload = require_object(
            client.api(
                "actions/caches",
                params={"key": request.key, "per_page": "100"},
                paginate=True,
            ),
            "actions/caches",
        )
        raw_entries = list_field(payload, "actions_caches", "actions/caches")
        prefix_entries = [
            cache_entry_from_raw(raw)
            for raw in raw_entries
            if isinstance(raw, dict)
        ]
        exact_entries = [
            entry for entry in prefix_entries
            if entry.get("key") == request.key
        ]
        api_prefix_count, count_source = count_with_source(payload, fallback=len(prefix_entries))
        probes.append(
            {
                "label": request.label,
                "key": request.key,
                "present": bool(exact_entries),
                "exact_count": len(exact_entries),
                "api_prefix_count": api_prefix_count,
                "api_prefix_count_source": count_source,
                "api_prefix_enumerated_count": len(prefix_entries),
                "prefix_only_count": max(0, len(prefix_entries) - len(exact_entries)),
                "entries": exact_entries,
            }
        )
    return probes


def fetch_cache(client: GhClient) -> dict[str, Any]:
    payload = require_object(
        client.api("actions/caches", params={"per_page": "100"}, paginate=True),
        "actions/caches",
    )
    raw_entries = list_field(payload, "actions_caches", "actions/caches")
    entries: list[dict[str, Any]] = []
    total_bytes = 0
    for raw in raw_entries:
        if not isinstance(raw, dict):
            continue
        entry = cache_entry_from_raw(raw)
        total_bytes += entry["size_bytes"]
        entries.append(entry)

    count, count_source = count_with_source(payload, fallback=len(entries))
    return {
        "total_bytes": total_bytes,
        "count": count,
        "count_source": count_source,
        "enumerated_count": len(entries),
        "enumeration_consistency": "live_churn_possible",
        "entries": entries,
    }


def fetch_artifacts(client: GhClient, *, include_entries: bool = False) -> dict[str, Any]:
    payload = require_object(
        client.api("actions/artifacts", params={"per_page": "100"}, paginate=True),
        "actions/artifacts",
    )
    raw_artifacts = list_field(payload, "artifacts", "actions/artifacts")
    by_name: dict[str, dict[str, int]] = collections.defaultdict(empty_artifact_group)
    totals = empty_artifact_group()
    entries: list[dict[str, Any]] = []

    for raw in raw_artifacts:
        if not isinstance(raw, dict):
            continue
        entry = artifact_entry_from_raw(raw)
        add_artifact_expiration_totals(
            totals,
            size_bytes=entry["size_bytes"],
            expired=entry["expired"],
        )
        add_artifact_expiration_totals(
            by_name[entry["name"]],
            size_bytes=entry["size_bytes"],
            expired=entry["expired"],
        )
        if include_entries:
            entries.append(entry)

    grouped = [
        {
            "name": name,
            "total_bytes": values["total_bytes"],
            "count": values["count"],
            "expired_bytes": values["expired_bytes"],
            "expired_count": values["expired_count"],
            "non_expired_bytes": values["non_expired_bytes"],
            "non_expired_count": values["non_expired_count"],
            "unknown_expiration_bytes": values["unknown_expiration_bytes"],
            "unknown_expiration_count": values["unknown_expiration_count"],
        }
        for name, values in by_name.items()
    ]
    grouped.sort(key=lambda entry: (-entry["total_bytes"], entry["name"]))
    count, count_source = count_with_source(payload, fallback=totals["count"])
    result: dict[str, Any] = {
        "total_bytes": totals["total_bytes"],
        "count": count,
        "count_source": count_source,
        "enumerated_count": totals["count"],
        "enumeration_consistency": "live_churn_possible",
        "expired_bytes": totals["expired_bytes"],
        "expired_count": totals["expired_count"],
        "non_expired_bytes": totals["non_expired_bytes"],
        "non_expired_count": totals["non_expired_count"],
        "unknown_expiration_bytes": totals["unknown_expiration_bytes"],
        "unknown_expiration_count": totals["unknown_expiration_count"],
        "by_name": grouped,
    }
    if include_entries:
        result["entries"] = entries
    return result


def fetch_retention_setting(client: GhClient) -> dict[str, Any]:
    try:
        payload = require_object(
            client.api("actions/permissions/artifact-and-log-retention"),
            "actions/permissions/artifact-and-log-retention",
        )
    except GhApiError:
        return {"artifact_and_log_days": None, "source": "unavailable"}
    except AuditError:
        return {"artifact_and_log_days": None, "source": "unavailable"}

    days = payload.get("days")
    if isinstance(days, int) and not isinstance(days, bool):
        return {"artifact_and_log_days": days, "source": "rest"}
    return {"artifact_and_log_days": None, "source": "settings-ui-only"}


def branch_path_segment(branch: str) -> str:
    return urllib.parse.quote(branch, safe="")


def required_checks_from_rulesets(payload: Any) -> list[Any]:
    if not isinstance(payload, list):
        raise AuditError("rulesets payload is not a list")

    contexts: list[Any] = []
    for rule in payload:
        if not isinstance(rule, dict):
            continue
        if rule.get("type") != "required_status_checks":
            continue
        parameters = rule.get("parameters")
        if not isinstance(parameters, dict):
            continue
        checks = parameters.get("required_status_checks")
        if isinstance(checks, list):
            contexts.extend(checks)
    return contexts


def required_checks_from_branch_protection(payload: Any) -> list[Any]:
    data = require_object(payload, "branch protection required status checks")
    checks = data.get("checks")
    if isinstance(checks, list) and checks:
        return checks
    contexts = data.get("contexts")
    if isinstance(contexts, list):
        return contexts
    return []


def fetch_required_checks(client: GhClient, branch: str) -> dict[str, Any]:
    encoded_branch = branch_path_segment(branch)

    try:
        ruleset_contexts = required_checks_from_rulesets(client.api(f"rules/branches/{encoded_branch}"))
    except (GhApiError, AuditError):
        return {"available": False, "source": "unavailable", "contexts": []}

    if ruleset_contexts:
        return {"available": True, "source": "rulesets", "contexts": ruleset_contexts}

    try:
        branch_contexts = required_checks_from_branch_protection(
            client.api(f"branches/{encoded_branch}/protection/required_status_checks")
        )
    except (GhApiError, AuditError):
        return {"available": True, "source": "rulesets", "contexts": ruleset_contexts}

    if branch_contexts:
        return {"available": True, "source": "branch-protection", "contexts": branch_contexts}
    return {"available": True, "source": "rulesets", "contexts": ruleset_contexts}


def cleanup_rule_for_name(policy: ArtifactCleanupPolicy, name: str) -> ArtifactClassRule | None:
    matches = [
        rule for rule in policy.classes
        if name in rule.name_equals or any(name.startswith(prefix) for prefix in rule.name_prefixes)
    ]
    if len(matches) == 1:
        return matches[0]
    return None


def artifact_ref(entry: dict[str, Any]) -> str | None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    return optional_text(workflow_run.get("ref")) or optional_text(workflow_run.get("head_branch"))


def protected_ref_forms(ref: str) -> tuple[str, ...]:
    forms = [ref]
    if ref.startswith("refs/heads/"):
        branch = ref.removeprefix("refs/heads/")
        forms.append(branch)
    elif ref.startswith("heads/"):
        branch = ref.removeprefix("heads/")
        forms.extend((branch, f"refs/heads/{branch}"))
    elif ref.startswith("refs/tags/"):
        tag = ref.removeprefix("refs/tags/")
        forms.extend((tag, f"tags/{tag}"))
    elif ref.startswith("tags/"):
        tag = ref.removeprefix("tags/")
        forms.extend((tag, f"refs/tags/{tag}"))
    else:
        forms.append(f"refs/heads/{ref}")
    return tuple(dict.fromkeys(forms))


def ref_is_protected(policy: ArtifactCleanupPolicy, ref: str | None) -> bool:
    if ref is None:
        return False
    forms = protected_ref_forms(ref)
    return (
        any(form in policy.protected_refs for form in forms)
        or any(form.startswith(prefix) for form in forms for prefix in policy.protected_ref_prefixes)
        or any(fnmatch.fnmatchcase(form, pattern) for form in forms for pattern in policy.protected_ref_globs)
    )


def should_fetch_workflow_run(
    policy: ArtifactCleanupPolicy,
    rule: ArtifactClassRule | None,
    entry: dict[str, Any],
) -> bool:
    if rule is None:
        return False
    if rule.expired_decision != DELETE_CANDIDATE_DECISION:
        return False
    if entry.get("expired") is not True:
        return False
    if ref_is_protected(policy, artifact_ref(entry)):
        return False
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    status = optional_text(workflow_run.get("status"))
    return status is None


def workflow_run_id_from_entry(entry: dict[str, Any]) -> int | None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    run_id = workflow_run.get("id")
    return run_id if isinstance(run_id, int) else None


def fetch_workflow_run_metadata_payload(client: GhClient, run_id: int) -> dict[str, Any] | None:
    try:
        return require_object(client.api(f"actions/runs/{run_id}"), f"actions/runs/{run_id}")
    except (GhApiError, AuditError):
        return None


def cleanup_decision_for_entry(
    policy: ArtifactCleanupPolicy,
    rule: ArtifactClassRule | None,
    entry: dict[str, Any],
) -> tuple[str, str, str]:
    if rule is None:
        return policy.default_class, policy.default_decision, policy.default_keep_reason
    class_id = rule.rule_id
    expired = entry.get("expired")
    if expired is None:
        return class_id, KEEP_DECISION, policy.expiration_unknown_keep_reason
    if expired is False:
        return class_id, KEEP_DECISION, rule.keep_reason or policy.not_expired_keep_reason
    ref = artifact_ref(entry)
    if ref_is_protected(policy, ref):
        return class_id, KEEP_DECISION, policy.protected_ref_keep_reason
    if rule.expired_decision == KEEP_DECISION:
        return class_id, KEEP_DECISION, rule.keep_reason
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    status = optional_text(workflow_run.get("status"))
    if status in policy.active_run_statuses:
        return class_id, KEEP_DECISION, policy.active_run_keep_reason
    if status not in policy.terminal_run_statuses:
        return class_id, KEEP_DECISION, policy.status_unavailable_keep_reason
    if rule.candidate_reason is None:
        raise AuditError(f"cleanup class {rule.rule_id} has no candidate reason")
    return class_id, DELETE_CANDIDATE_DECISION, rule.candidate_reason


def row_from_entry(
    entry: dict[str, Any],
    *,
    class_id: str,
    decision: str,
    reason: str,
) -> dict[str, Any]:
    return {
        "artifact_id": entry.get("artifact_id"),
        "name": entry.get("name"),
        "class": class_id,
        "size_bytes": entry.get("size_bytes"),
        "created_at": entry.get("created_at"),
        "expires_at": entry.get("expires_at"),
        "expired": entry.get("expired"),
        "workflow_run": entry.get("workflow_run"),
        "decision": decision,
        "reason": reason,
    }


def parse_github_timestamp(value: Any) -> dt.datetime | None:
    if not isinstance(value, str) or not value:
        return None
    try:
        return dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def cleanup_self_clear_horizon(entries: list[dict[str, Any]]) -> dict[str, Any]:
    best_entry: dict[str, Any] | None = None
    best_timestamp: dt.datetime | None = None
    for entry in entries:
        if entry.get("expired") is not False:
            continue
        timestamp = parse_github_timestamp(entry.get("expires_at"))
        if timestamp is None:
            continue
        if best_timestamp is None or timestamp > best_timestamp:
            best_timestamp = timestamp
            best_entry = entry
    if best_entry is None:
        return {"expires_at": None, "source": "no_non_expired_artifact_expiry"}
    return {
        "expires_at": best_entry.get("expires_at"),
        "source": "max_non_expired_artifact_expires_at",
    }


def format_global_api_path(template: str, repo: str) -> str:
    if "/" not in repo:
        raise AuditError("repo must be OWNER/REPO for billing endpoint path expansion")
    owner, repo_name = repo.split("/", 1)
    try:
        return template.format(owner=owner, repo=repo_name, owner_repo=repo)
    except KeyError as exc:
        raise AuditError(f"unsupported billing endpoint placeholder: {exc}") from exc


def api_response_summary(payload: Any) -> dict[str, Any]:
    if isinstance(payload, dict):
        return {"type": "object", "keys": sorted(str(key) for key in payload)}
    if isinstance(payload, list):
        return {"type": "list", "count": len(payload)}
    return {"type": type(payload).__name__}


def probe_billing_endpoint(
    client: GhClient,
    *,
    repo: str,
    policy: ArtifactCleanupPolicy,
) -> dict[str, Any]:
    probes: list[dict[str, Any]] = []
    for template in policy.billing_probe_paths:
        path = format_global_api_path(template, repo)
        try:
            payload = client.api_global(path)
        except (GhApiError, AuditError) as exc:
            probes.append({"path": path, "status": "unavailable", "error": str(exc)})
            continue
        return {
            "status": "available",
            "message": "billing endpoint reachable",
            "source": path,
            "probes": [*probes, {"path": path, "status": "available"}],
            "response": api_response_summary(payload),
        }
    return {
        "status": "unavailable",
        "message": policy.billing_impact_unverifiable,
        "source": None,
        "probes": probes,
    }


def build_artifact_cleanup_feasibility(
    client: GhClient,
    *,
    repo: str,
    artifacts: dict[str, Any],
    policy: ArtifactCleanupPolicy,
) -> dict[str, Any]:
    entries = list_field(artifacts, "entries", "artifacts")
    rows: list[dict[str, Any]] = []
    workflow_run_fetches = 0
    workflow_run_fetch_limit_reached = False
    workflow_run_metadata_cache: dict[int, dict[str, Any] | None] = {}

    for entry in entries:
        if not isinstance(entry, dict):
            continue
        rule = cleanup_rule_for_name(policy, str(entry.get("name") or ""))
        if should_fetch_workflow_run(policy, rule, entry):
            run_id = workflow_run_id_from_entry(entry)
            if run_id is None:
                set_workflow_run_status_source(entry, "missing_run_id")
            elif run_id in workflow_run_metadata_cache:
                payload = workflow_run_metadata_cache[run_id]
                if payload is not None:
                    merge_workflow_run_metadata(entry, payload)
                else:
                    set_workflow_run_status_source(entry, "run_api_unavailable")
            elif workflow_run_fetches < policy.workflow_run_fetch_limit:
                workflow_run_fetches += 1
                payload = fetch_workflow_run_metadata_payload(client, run_id)
                workflow_run_metadata_cache[run_id] = payload
                if payload is not None:
                    merge_workflow_run_metadata(entry, payload)
                else:
                    set_workflow_run_status_source(entry, "run_api_unavailable")
            else:
                workflow_run_fetch_limit_reached = True
                set_workflow_run_status_source(entry, "fetch_limit")
        class_id, decision, reason = cleanup_decision_for_entry(policy, rule, entry)
        rows.append(row_from_entry(entry, class_id=class_id, decision=decision, reason=reason))

    candidate_bytes = sum(
        nonnegative_int(row.get("size_bytes"))
        for row in rows
        if row.get("decision") == DELETE_CANDIDATE_DECISION
    )
    unverified_candidate_bytes = sum(
        nonnegative_int(row.get("size_bytes"))
        for row in rows
        if row.get("decision") == KEEP_DECISION
        and row.get("reason") == policy.status_unavailable_keep_reason
    )
    billing = probe_billing_endpoint(client, repo=repo, policy=policy)
    measured_billed_reclaim_bytes = None if billing.get("status") != "available" else None
    return {
        "listed_bytes": artifacts["total_bytes"],
        "expired_bytes": artifacts["expired_bytes"],
        "non_expired_bytes": artifacts["non_expired_bytes"],
        "unknown_expiration_bytes": artifacts["unknown_expiration_bytes"],
        "candidate_bytes": candidate_bytes,
        "candidate_count": sum(1 for row in rows if row.get("decision") == DELETE_CANDIDATE_DECISION),
        "unverified_candidate_bytes": unverified_candidate_bytes,
        "unverified_candidate_count": sum(
            1 for row in rows
            if row.get("decision") == KEEP_DECISION
            and row.get("reason") == policy.status_unavailable_keep_reason
        ),
        "expected_reclaim_proxy_bytes": candidate_bytes,
        "measured_billed_reclaim_bytes": measured_billed_reclaim_bytes,
        "reclaim_basis": "listed_artifact_bytes_proxy",
        "billing": billing,
        "self_clear_horizon": cleanup_self_clear_horizon([entry for entry in entries if isinstance(entry, dict)]),
        "wait_and_remeasure": policy.wait_and_remeasure,
        "workflow_run_metadata": {
            "fetches": workflow_run_fetches,
            "fetch_limit": policy.workflow_run_fetch_limit,
            "fetch_limit_reached": workflow_run_fetch_limit_reached,
        },
        "rows": rows,
    }


def build_cache_key_probe_snapshot(
    client: GhClient,
    *,
    repo: str,
    snapshot_utc: str,
    requests: list[CacheKeyProbeRequest],
) -> dict[str, Any]:
    return {
        "snapshot_utc": snapshot_utc,
        "repo": repo,
        "cache_key_probes": fetch_cache_key_probes(client, requests),
    }


def render_cache_key_probe_text(snapshot: dict[str, Any]) -> str:
    probes = list_field(snapshot, "cache_key_probes", "cache_key_probes")
    lines = [
        f"CI cache key probe for {snapshot['repo']}",
        f"Snapshot: {snapshot['snapshot_utc']}",
        "",
        "Cache key probes:",
    ]
    for raw in probes:
        if not isinstance(raw, dict):
            continue
        status = "present" if raw.get("present") else "missing"
        lines.append(
            f"  - {raw.get('label')}: {status}; "
            f"exact_count={raw.get('exact_count')} "
            f"api_prefix_count={raw.get('api_prefix_count')} "
            f"api_prefix_enumerated={raw.get('api_prefix_enumerated_count')} "
            f"key={raw.get('key')}"
        )
        if raw.get("exact_count") == 0 and raw.get("api_prefix_enumerated_count", 0) > 0:
            lines.append("      note=API returned prefix matches, but no exact key matched")
        entries = raw.get("entries")
        if isinstance(entries, list):
            for entry in entries:
                if not isinstance(entry, dict):
                    continue
                lines.append(
                    f"      id={entry.get('cache_id')} ref={entry.get('ref')} "
                    f"size={human_bytes(nonnegative_int(entry.get('size_bytes')))} "
                    f"last_accessed_at={entry.get('last_accessed_at')}"
                )
    return "\n".join(lines)


def build_snapshot(
    client: GhClient,
    *,
    repo: str,
    branch: str,
    snapshot_utc: str,
    cleanup_policy: ArtifactCleanupPolicy | None = None,
) -> dict[str, Any]:
    cache = fetch_cache(client)
    artifacts = fetch_artifacts(client, include_entries=cleanup_policy is not None)
    snapshot = {
        "snapshot_utc": snapshot_utc,
        "repo": repo,
        "cache": cache,
        "artifacts": artifacts,
        "retention_setting": fetch_retention_setting(client),
        "required_checks": fetch_required_checks(client, branch),
    }
    if cleanup_policy is not None:
        snapshot["artifact_cleanup_feasibility"] = build_artifact_cleanup_feasibility(
            client,
            repo=repo,
            artifacts=artifacts,
            policy=cleanup_policy,
        )
    return snapshot


def check_label(entry: Any) -> str:
    if isinstance(entry, dict):
        context = entry.get("context")
        if context is None:
            return json.dumps(entry, sort_keys=True)
        details = []
        if entry.get("integration_id") is not None:
            details.append(f"integration_id={entry['integration_id']}")
        if entry.get("app_id") is not None:
            details.append(f"app_id={entry['app_id']}")
        if not details:
            return str(context)
        return f"{context} ({', '.join(details)})"
    return str(entry)


def render_text(snapshot: dict[str, Any], *, artifact_name_limit: int = 10) -> str:
    cache = require_object(snapshot["cache"], "cache")
    artifacts = require_object(snapshot["artifacts"], "artifacts")
    retention = require_object(snapshot["retention_setting"], "retention_setting")
    required_checks = require_object(snapshot["required_checks"], "required_checks")
    artifact_groups = list_field(artifacts, "by_name", "artifacts")
    contexts = list_field(required_checks, "contexts", "required_checks")

    lines = [
        f"CI storage audit for {snapshot['repo']}",
        f"Snapshot: {snapshot['snapshot_utc']}",
        "",
        f"Actions cache: {cache['count']} entries, {human_bytes(cache['total_bytes'])}",
        (
            f"Actions artifacts: {artifacts['count']} artifacts, "
            f"{human_bytes(artifacts['total_bytes'])} across {len(artifact_groups)} names"
        ),
        (
            "Artifact expiry: "
            f"expired={human_bytes(nonnegative_int(artifacts.get('expired_bytes')))} "
            f"non_expired={human_bytes(nonnegative_int(artifacts.get('non_expired_bytes')))} "
            f"unknown={human_bytes(nonnegative_int(artifacts.get('unknown_expiration_bytes')))}"
        ),
        (
            "Retention setting: "
            f"{retention['artifact_and_log_days']} days (source: {retention['source']})"
            if retention.get("artifact_and_log_days") is not None
            else f"Retention setting: unavailable in audit (source: {retention['source']})"
        ),
        (
            "Required checks: "
            f"{len(contexts)} contexts (source: {required_checks['source']}, "
            f"available: {str(required_checks['available']).lower()})"
        ),
    ]

    if contexts:
        lines.extend(["", "Required check contexts:"])
        for entry in contexts:
            lines.append(f"  - {check_label(entry)}")

    if artifact_groups:
        lines.extend(["", "Artifacts by name:"])
        for entry in artifact_groups[:artifact_name_limit]:
            if not isinstance(entry, dict):
                continue
            lines.append(
                f"  - {entry['name']}: {human_bytes(entry['total_bytes'])} "
                f"({entry['count']} artifacts)"
            )
        remaining = len(artifact_groups) - artifact_name_limit
        if remaining > 0:
            lines.append(f"  - ... {remaining} additional artifact names in --json")

    cleanup = snapshot.get("artifact_cleanup_feasibility")
    if isinstance(cleanup, dict):
        billing = cleanup.get("billing") if isinstance(cleanup.get("billing"), dict) else {}
        horizon = cleanup.get("self_clear_horizon") if isinstance(cleanup.get("self_clear_horizon"), dict) else {}
        lines.extend(
            [
                "",
                "Artifact cleanup feasibility:",
                (
                    f"  - listed={human_bytes(nonnegative_int(cleanup.get('listed_bytes')))} "
                    f"expired={human_bytes(nonnegative_int(cleanup.get('expired_bytes')))} "
                    f"non_expired={human_bytes(nonnegative_int(cleanup.get('non_expired_bytes')))}"
                ),
                (
                    f"  - delete_candidates={cleanup.get('candidate_count')} "
                    f"proxy_reclaim={human_bytes(nonnegative_int(cleanup.get('expected_reclaim_proxy_bytes')))}"
                ),
                (
                    f"  - unverified_candidate_rows={cleanup.get('unverified_candidate_count')} "
                    f"unverified_bytes={human_bytes(nonnegative_int(cleanup.get('unverified_candidate_bytes')))}"
                ),
                f"  - billing={billing.get('status')} ({billing.get('message')})",
                f"  - self_clear_horizon={horizon.get('expires_at')}",
                f"  - wait_and_remeasure={cleanup.get('wait_and_remeasure')}",
            ]
        )

    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="GitHub repository as OWNER/REPO. Defaults to gh repo view.")
    parser.add_argument("--branch", help="Branch for required-check lookup. Defaults to the repo default branch.")
    parser.add_argument("--json", action="store_true", help="Print the stable JSON contract only.")
    parser.add_argument(
        "--cleanup-feasibility",
        action="store_true",
        help="Include the read-only artifact cleanup feasibility report.",
    )
    parser.add_argument(
        "--cleanup-policy",
        type=pathlib.Path,
        help="TOML policy path for --cleanup-feasibility. Defaults to tracked TOML discovery.",
    )
    parser.add_argument(
        "--cache-key",
        action="append",
        default=[],
        metavar="LABEL=KEY",
        help="Probe an exact Actions cache key. Repeat to probe multiple keys.",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.cache_key and args.cleanup_feasibility:
        raise AuditError("--cleanup-feasibility cannot be combined with --cache-key")
    repo = args.repo or infer_repo()
    client = GhClient(repo)
    snapshot_utc = isoformat_utc(dt.datetime.now(dt.UTC))
    if args.cache_key:
        snapshot = build_cache_key_probe_snapshot(
            client,
            repo=repo,
            snapshot_utc=snapshot_utc,
            requests=[parse_cache_key_probe(raw) for raw in args.cache_key],
        )
        if args.json:
            print(json.dumps(snapshot, indent=2, sort_keys=True))
        else:
            print(render_cache_key_probe_text(snapshot))
        return 0
    branch = args.branch or infer_default_branch()
    cleanup_policy = None
    if args.cleanup_feasibility:
        cleanup_policy_path = args.cleanup_policy or discover_cleanup_policy_path()
        cleanup_policy = load_cleanup_policy_path(cleanup_policy_path)
    snapshot = build_snapshot(
        client,
        repo=repo,
        branch=branch,
        snapshot_utc=snapshot_utc,
        cleanup_policy=cleanup_policy,
    )
    if args.json:
        print(json.dumps(snapshot, indent=2, sort_keys=True))
    else:
        print(render_text(snapshot))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except AuditError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
