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
import enum
import json
import subprocess
import sys
import urllib.parse
from collections.abc import Callable, Iterable
from typing import Any, NamedTuple


class FailureKind(enum.StrEnum):
    ABSENT = "absent"
    EMPTY = "empty"
    INVALID = "invalid"
    DUPLICATE = "duplicate"
    UNAVAILABLE = "unavailable"
    AMBIGUOUS = "ambiguous"


class AuditError(RuntimeError):
    def __init__(
        self,
        message: str,
        *,
        kind: FailureKind = FailureKind.INVALID,
        field: str = "audit",
    ) -> None:
        self.kind = kind
        self.field = field
        super().__init__(f"{kind.value} {field}: {message}")


class GhApiError(AuditError):
    def __init__(self, path: str, message: str) -> None:
        self.path = path
        self.message = message
        super().__init__(message, kind=FailureKind.UNAVAILABLE, field=path)


class CacheKeyProbeRequest(NamedTuple):
    label: str
    key: str


class LabeledValue(NamedTuple):
    label: str
    value: str


class TextRule(NamedTuple):
    kind: FailureKind
    accepts: Callable[[Any], bool]
    message: str


class EventRefSpec(NamedTuple):
    base_ref_contract: str


TEXT_RULES = (
    TextRule(FailureKind.ABSENT, lambda value: value is not None, "is required"),
    TextRule(FailureKind.INVALID, lambda value: isinstance(value, str), "must be text"),
    TextRule(FailureKind.EMPTY, lambda value: value != "", "must not be empty"),
    TextRule(FailureKind.INVALID, lambda value: value == value.strip(), "must not contain surrounding whitespace"),
)
REF_TEXT_RULES = TEXT_RULES + (
    TextRule(FailureKind.INVALID, lambda value: value.startswith("refs/"), "must be a refs/ value"),
)
BRANCH_TEXT_RULES = TEXT_RULES + (
    TextRule(FailureKind.INVALID, lambda value: not value.startswith("refs/"), "must be a branch name, not a refs/ value"),
)
TEXT_CONTRACTS = {
    "text": TEXT_RULES,
    "ref": REF_TEXT_RULES,
    "branch": BRANCH_TEXT_RULES,
}
GITHUB_CACHE_EVENT_SPECS = {
    "pull_request": EventRefSpec(base_ref_contract="required_branch"),
    "push": EventRefSpec(base_ref_contract="empty"),
    "workflow_dispatch": EventRefSpec(base_ref_contract="empty"),
    "merge_group": EventRefSpec(base_ref_contract="empty"),
}
EMPTY_BASE_REF_RULES = (
    TextRule(FailureKind.ABSENT, lambda value: value is not None, "--github-base-ref is required"),
    TextRule(FailureKind.INVALID, lambda value: value == "", "--github-base-ref must be empty outside pull_request"),
)
CACHE_PERSISTENCE_MISSING_WARNING = (
    "::warning::one or more root nextest cache keys are missing from the Actions cache inventory "
    "after save/restore; inspect cache save outcomes and repository cache usage above for quota/eviction context"
)


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
        cmd = ["gh", "api"]
        if paginate:
            cmd.extend(["--paginate", "--slurp"])
        cmd.extend(["--method", "GET", f"repos/{self.repo}/{path}"])
        for key, value in (params or {}).items():
            cmd.extend(["-f", f"{key}={value}"])
        result = subprocess.run(cmd, text=True, capture_output=True, check=False)
        if result.returncode != 0:
            raise GhApiError(path, result.stderr.strip() or "gh api failed")
        try:
            payload = json.loads(result.stdout)
        except json.JSONDecodeError as exc:
            raise GhApiError(path, f"invalid JSON: {exc}") from exc
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


def require_nonnegative_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise AuditError(f"{field} must be a non-negative integer", kind=FailureKind.INVALID, field=field)
    return value


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
        "ref": require_ref(raw.get("ref"), "actions/caches.ref"),
        "key": require_text(raw.get("key"), "actions/caches.key"),
        "last_accessed_at": optional_text(raw.get("last_accessed_at")),
        "size_bytes": require_nonnegative_int(raw.get("size_in_bytes"), "actions/caches.size_in_bytes"),
    }


def require_contract_text(value: Any, field: str, contract_name: str) -> str:
    for rule in TEXT_CONTRACTS[contract_name]:
        if not rule.accepts(value):
            raise AuditError(f"{field} {rule.message}", kind=rule.kind, field=field)
    return value


def require_text(value: Any, field: str) -> str:
    return require_contract_text(value, field, "text")


def require_ref(value: Any, field: str) -> str:
    return require_contract_text(value, field, "ref")


def require_branch(value: Any, field: str) -> str:
    return require_contract_text(value, field, "branch")


def require_labeled_pair(raw: str, field: str) -> tuple[str, str]:
    if "=" not in raw:
        raise AuditError(f"{field} must be LABEL=VALUE", kind=FailureKind.INVALID, field=field)
    return raw.split("=", 1)


def parse_cache_key_probe(raw: str) -> CacheKeyProbeRequest:
    label, key = require_labeled_pair(raw, "--cache-key")
    label = require_text(label, "--cache-key label")
    key = require_text(key, "--cache-key key")
    return CacheKeyProbeRequest(label=label, key=key)


def parse_labeled_value(raw: str, field: str) -> LabeledValue:
    label, value = require_labeled_pair(raw, field)
    label = require_text(label, f"{field} label")
    value = require_text(value, f"{field} value")
    return LabeledValue(label=label, value=value)


def provided_values(values: Iterable[str] | None, field: str) -> tuple[str, ...]:
    if values is None:
        return ()
    result = tuple(values)
    if not result:
        raise AuditError(f"{field} must not be empty", kind=FailureKind.EMPTY, field=field)
    return result


def append_unique_or_fail(refs: list[str], seen: set[str], ref: str) -> None:
    if ref in seen:
        raise AuditError(f"duplicate cache ref: {ref}", kind=FailureKind.DUPLICATE, field="cache_ref_filter")
    seen.add(ref)
    refs.append(ref)


def unique_ordered(refs: Iterable[str]) -> list[str]:
    result: list[str] = []
    seen: set[str] = set()
    for ref in refs:
        if ref not in seen:
            seen.add(ref)
            result.append(ref)
    return result


def normalize_cache_refs(cache_refs: list[str] | None) -> list[str]:
    refs: list[str] = []
    seen: set[str] = set()
    for raw in provided_values(cache_refs, "--cache-ref"):
        append_unique_or_fail(refs, seen, require_ref(raw, "--cache-ref"))
    return refs


def normalize_cache_ref_inputs(
    *,
    cache_refs: list[str] | None = None,
    cache_branches: list[str] | None = None,
) -> list[str]:
    refs = normalize_cache_refs(cache_refs)
    seen = set(refs)
    for raw in provided_values(cache_branches, "--cache-branch"):
        branch = require_branch(raw, "--cache-branch")
        append_unique_or_fail(refs, seen, f"refs/heads/{branch}")
    if not refs:
        raise AuditError(
            "cache key probes require at least one cache ref",
            kind=FailureKind.ABSENT,
            field="cache_ref_filter",
        )
    return refs


def resolve_github_cache_refs(
    *,
    github_event_name: str | None,
    github_ref: str | None,
    github_base_ref: str | None,
    github_default_branch: str | None,
) -> list[str]:
    event_name = require_text(github_event_name, "--github-event-name")
    event_spec = GITHUB_CACHE_EVENT_SPECS.get(event_name)
    if event_spec is None:
        raise AuditError(
            f"unsupported GitHub event for cache probes: {event_name}",
            kind=FailureKind.INVALID,
            field="--github-event-name",
        )
    current_ref = require_ref(github_ref, "--github-ref")
    default_branch = require_branch(github_default_branch, "--github-default-branch")
    base_refs = resolve_github_base_refs(github_base_ref, event_spec)
    return unique_ordered((current_ref, *base_refs, f"refs/heads/{default_branch}"))


def required_base_refs(github_base_ref: str | None) -> tuple[str, ...]:
    return (f"refs/heads/{require_branch(github_base_ref, '--github-base-ref')}",)


def empty_base_refs(github_base_ref: str | None) -> tuple[str, ...]:
    require_empty_base_ref(github_base_ref)
    return ()


BASE_REF_CONTRACT_RESOLVERS = {
    "required_branch": required_base_refs,
    "empty": empty_base_refs,
}


def resolve_github_base_refs(github_base_ref: str | None, event_spec: EventRefSpec) -> tuple[str, ...]:
    resolver = BASE_REF_CONTRACT_RESOLVERS.get(event_spec.base_ref_contract)
    if resolver is None:
        raise AuditError(
            f"unsupported base ref contract: {event_spec.base_ref_contract}",
            kind=FailureKind.INVALID,
            field="--github-event-name",
        )
    return resolver(github_base_ref)


def require_empty_base_ref(github_base_ref: str | None) -> None:
    for rule in EMPTY_BASE_REF_RULES:
        if not rule.accepts(github_base_ref):
            raise AuditError(rule.message, kind=rule.kind, field="--github-base-ref")


def resolve_cache_ref_inputs(
    *,
    cache_refs: list[str] | None = None,
    cache_branches: list[str] | None = None,
    github_event_name: str | None = None,
    github_ref: str | None = None,
    github_base_ref: str | None = None,
    github_default_branch: str | None = None,
) -> list[str]:
    explicit_inputs_present = cache_refs is not None or cache_branches is not None
    github_inputs_present = any(
        value is not None
        for value in (github_event_name, github_ref, github_base_ref, github_default_branch)
    )
    if explicit_inputs_present and github_inputs_present:
        raise AuditError(
            "use either explicit cache refs or GitHub context, not both",
            kind=FailureKind.AMBIGUOUS,
            field="cache_ref_filter",
        )
    if github_inputs_present:
        return resolve_github_cache_refs(
            github_event_name=github_event_name,
            github_ref=github_ref,
            github_base_ref=github_base_ref,
            github_default_branch=github_default_branch,
        )
    return normalize_cache_ref_inputs(cache_refs=cache_refs, cache_branches=cache_branches)


def fetch_cache_key_probes(
    client: GhClient,
    requests: list[CacheKeyProbeRequest],
    *,
    cache_refs: list[str] | None = None,
    cache_branches: list[str] | None = None,
) -> list[dict[str, Any]]:
    probes: list[dict[str, Any]] = []
    ref_filter = normalize_cache_ref_inputs(cache_refs=cache_refs, cache_branches=cache_branches)
    ref_filter_set = set(ref_filter)
    for request in requests:
        try:
            payload = require_object(
                client.api(
                    "actions/caches",
                    params={"key": request.key, "per_page": "100"},
                    paginate=True,
                ),
                "actions/caches",
            )
            raw_entries = list_field(payload, "actions_caches", "actions/caches")
        except GhApiError:
            raise
        except AuditError as exc:
            raise AuditError(str(exc), kind=FailureKind.UNAVAILABLE, field="actions/caches") from exc
        prefix_entries = [
            cache_entry_from_raw(raw)
            for raw in raw_entries
            if isinstance(raw, dict)
        ]
        accessible_prefix_entries = [
            entry
            for entry in prefix_entries
            if not ref_filter_set or entry.get("ref") in ref_filter_set
        ]
        exact_entries = [
            entry for entry in accessible_prefix_entries
            if entry.get("key") == request.key
        ]
        api_prefix_count, count_source = count_with_source(payload, fallback=len(prefix_entries))
        probes.append(
            {
                "label": request.label,
                "key": request.key,
                "available": True,
                "present": bool(exact_entries),
                "exact_count": len(exact_entries),
                "api_prefix_count": api_prefix_count,
                "api_prefix_count_source": count_source,
                "api_prefix_enumerated_count": len(prefix_entries),
                "ref_filtered_prefix_enumerated_count": len(accessible_prefix_entries),
                "prefix_only_count": max(0, len(accessible_prefix_entries) - len(exact_entries)),
                "entries": exact_entries,
                "ref_filter": ref_filter,
            }
        )
    return probes


def fetch_cache_usage(client: GhClient) -> dict[str, Any]:
    try:
        payload = require_object(client.api("actions/cache/usage"), "actions/cache/usage")
    except GhApiError:
        raise
    except AuditError as exc:
        raise AuditError(str(exc), kind=FailureKind.UNAVAILABLE, field="actions/cache/usage") from exc
    return {
        "available": True,
        "active_caches_count": require_nonnegative_int(
            payload.get("active_caches_count"),
            "actions/cache/usage",
        ),
        "active_caches_size_in_bytes": require_nonnegative_int(
            payload.get("active_caches_size_in_bytes"),
            "actions/cache/usage",
        ),
        "source": "rest",
    }


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


def fetch_artifacts(client: GhClient) -> dict[str, Any]:
    payload = require_object(
        client.api("actions/artifacts", params={"per_page": "100"}, paginate=True),
        "actions/artifacts",
    )
    raw_artifacts = list_field(payload, "artifacts", "actions/artifacts")
    by_name: dict[str, dict[str, int]] = collections.defaultdict(lambda: {"total_bytes": 0, "count": 0})
    total_bytes = 0
    artifact_count = 0

    for raw in raw_artifacts:
        if not isinstance(raw, dict):
            continue
        name = optional_text(raw.get("name")) or ""
        size_bytes = nonnegative_int(raw.get("size_in_bytes"))
        total_bytes += size_bytes
        artifact_count += 1
        by_name[name]["total_bytes"] += size_bytes
        by_name[name]["count"] += 1

    grouped = [
        {"name": name, "total_bytes": values["total_bytes"], "count": values["count"]}
        for name, values in by_name.items()
    ]
    grouped.sort(key=lambda entry: (-entry["total_bytes"], entry["name"]))
    count, count_source = count_with_source(payload, fallback=artifact_count)
    return {
        "total_bytes": total_bytes,
        "count": count,
        "count_source": count_source,
        "enumerated_count": artifact_count,
        "enumeration_consistency": "live_churn_possible",
        "by_name": grouped,
    }


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


def build_cache_key_probe_snapshot(
    client: GhClient,
    *,
    repo: str,
    snapshot_utc: str,
    requests: list[CacheKeyProbeRequest],
    cache_refs: list[str] | None = None,
    cache_branches: list[str] | None = None,
) -> dict[str, Any]:
    normalized_cache_refs = normalize_cache_ref_inputs(
        cache_refs=cache_refs,
        cache_branches=cache_branches,
    )
    return {
        "snapshot_utc": snapshot_utc,
        "repo": repo,
        "cache_key_probes": fetch_cache_key_probes(
            client,
            requests,
            cache_refs=normalized_cache_refs,
        ),
        "cache_refs": normalized_cache_refs,
        "cache_usage": fetch_cache_usage(client),
    }


def append_step_summary(path: str, text: str) -> None:
    summary_path = require_text(path, "--github-step-summary")
    with open(summary_path, "a", encoding="utf-8") as summary:
        summary.write(text)
        summary.write("\n")


def render_cache_key_probe_text(snapshot: dict[str, Any]) -> str:
    probes = list_field(snapshot, "cache_key_probes", "cache_key_probes")
    lines = [
        f"CI cache key probe for {snapshot['repo']}",
        f"Snapshot: {snapshot['snapshot_utc']}",
        "",
    ]
    cache_refs = snapshot.get("cache_refs")
    if isinstance(cache_refs, list) and cache_refs:
        lines.append(f"Cache refs: {', '.join(str(ref) for ref in cache_refs)}")
        lines.append("")
    usage = snapshot.get("cache_usage")
    if isinstance(usage, dict):
        if usage.get("available"):
            lines.append(
                "Cache usage: "
                f"{usage.get('active_caches_count')} active caches, "
                f"{human_bytes(nonnegative_int(usage.get('active_caches_size_in_bytes')))} "
                f"(source: {usage.get('source')})"
            )
        else:
            raise AuditError(
                "cache usage is unavailable in a successful probe snapshot",
                kind=FailureKind.UNAVAILABLE,
                field="cache_usage",
            )
        lines.append("")
    lines.append("Cache key probes:")
    for raw in probes:
        if not isinstance(raw, dict):
            continue
        if raw.get("available") is False:
            raise AuditError(
                "cache key probe is unavailable in a successful probe snapshot",
                kind=FailureKind.UNAVAILABLE,
                field="cache_key_probes",
            )
        status = "present" if raw.get("present") else "missing"
        reason = optional_text(raw.get("reason"))
        reason_fragment = f" reason={reason}" if reason else ""
        ref_filtered_count = raw.get("ref_filtered_prefix_enumerated_count")
        ref_filtered_fragment = ""
        if isinstance(ref_filtered_count, int) and not isinstance(ref_filtered_count, bool):
            ref_filtered_fragment = f" ref_filtered_prefix_enumerated={ref_filtered_count}"
        ref_filter = raw.get("ref_filter")
        ref_fragment = ""
        if isinstance(ref_filter, list) and ref_filter:
            ref_fragment = f" ref_filter={','.join(str(ref) for ref in ref_filter)}"
        lines.append(
            f"  - {raw.get('label')}: {status}; "
            f"exact_count={raw.get('exact_count')} "
            f"api_prefix_count={raw.get('api_prefix_count')} "
            f"api_prefix_enumerated={raw.get('api_prefix_enumerated_count')} "
            f"key={raw.get('key')}"
            f"{ref_filtered_fragment}"
            f"{ref_fragment}"
            f"{reason_fragment}"
        )
        if (
            raw.get("exact_count") == 0
            and raw.get("ref_filter")
            and raw.get("api_prefix_enumerated_count", 0)
            > raw.get("ref_filtered_prefix_enumerated_count", 0)
        ):
            lines.append("      note=API returned matches outside the configured cache refs")
        elif raw.get("exact_count") == 0 and raw.get("api_prefix_enumerated_count", 0) > 0:
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


def render_cache_persistence_audit_text(
    snapshot: dict[str, Any],
    *,
    restore_hits: list[LabeledValue],
    save_outcomes: list[LabeledValue],
) -> str:
    lines = ["### Cache persistence audit", ""]
    for entry in restore_hits:
        lines.append(f"- {entry.label} restore hit: `{entry.value}`")
    for entry in save_outcomes:
        lines.append(f"- {entry.label} save outcome: `{entry.value}`")
    lines.extend(["", "```text", render_cache_key_probe_text(snapshot), "```"])
    return "\n".join(lines)


def cache_persistence_annotations(snapshot: dict[str, Any]) -> list[str]:
    probes = list_field(snapshot, "cache_key_probes", "cache_key_probes")
    if any(isinstance(raw, dict) and raw.get("available") is False for raw in probes):
        raise AuditError(
            "cache key probe is unavailable in a successful probe snapshot",
            kind=FailureKind.UNAVAILABLE,
            field="cache_key_probes",
        )
    has_missing = any(
        isinstance(raw, dict) and raw.get("available") is not False and not raw.get("present")
        for raw in probes
    )
    annotations: list[str] = []
    if has_missing:
        annotations.append(CACHE_PERSISTENCE_MISSING_WARNING)
    return annotations


def render_cache_persistence_failure_text(error: AuditError) -> str:
    return "\n".join(
        [
            "### Cache persistence audit",
            "",
            f"- contract failure kind: `{error.kind.value}`",
            f"- contract failure field: `{error.field}`",
            "",
            "```text",
            f"ERROR: {error}",
            "```",
        ]
    )


def build_snapshot(client: GhClient, *, repo: str, branch: str, snapshot_utc: str) -> dict[str, Any]:
    return {
        "snapshot_utc": snapshot_utc,
        "repo": repo,
        "cache": fetch_cache(client),
        "artifacts": fetch_artifacts(client),
        "retention_setting": fetch_retention_setting(client),
        "required_checks": fetch_required_checks(client, branch),
    }


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

    return "\n".join(lines)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="GitHub repository as OWNER/REPO. Defaults to gh repo view.")
    parser.add_argument("--branch", help="Branch for required-check lookup. Defaults to the repo default branch.")
    parser.add_argument("--json", action="store_true", help="Print the stable JSON contract only.")
    parser.add_argument(
        "--cache-key",
        action="append",
        default=[],
        metavar="LABEL=KEY",
        help="Probe an exact Actions cache key. Repeat to probe multiple keys.",
    )
    parser.add_argument(
        "--cache-ref",
        action="append",
        default=None,
        metavar="REF",
        help="Limit exact-key presence to cache refs restorable by this run. Repeat for multiple refs.",
    )
    parser.add_argument(
        "--cache-branch",
        action="append",
        default=None,
        metavar="BRANCH",
        help="Limit exact-key presence to a branch ref restorable by this run. Repeat for multiple branches.",
    )
    parser.add_argument("--github-event-name", help="GitHub event name for cache ref resolution.")
    parser.add_argument("--github-ref", help="GitHub ref for the current workflow run.")
    parser.add_argument("--github-base-ref", help="GitHub base branch for pull_request cache ref resolution.")
    parser.add_argument("--github-default-branch", help="GitHub repository default branch.")
    parser.add_argument("--github-step-summary", help="Append the cache persistence audit summary to this path.")
    parser.add_argument("--github-annotations", action="store_true", help="Emit GitHub workflow annotations.")
    parser.add_argument(
        "--restore-hit",
        action="append",
        default=None,
        metavar="LABEL=VALUE",
        help="Restore-hit evidence for cache persistence summaries.",
    )
    parser.add_argument(
        "--save-outcome",
        action="append",
        default=None,
        metavar="LABEL=VALUE",
        help="Save outcome evidence for cache persistence summaries.",
    )
    return parser.parse_args(argv)


def validate_args(args: argparse.Namespace) -> None:
    if args.github_step_summary is not None:
        require_text(args.github_step_summary, "--github-step-summary")


def run(args: argparse.Namespace) -> int:
    repo = args.repo or infer_repo()
    client = GhClient(repo)
    snapshot_utc = isoformat_utc(dt.datetime.now(dt.UTC))
    if args.cache_key:
        cache_refs = resolve_cache_ref_inputs(
            cache_refs=args.cache_ref,
            cache_branches=args.cache_branch,
            github_event_name=args.github_event_name,
            github_ref=args.github_ref,
            github_base_ref=args.github_base_ref,
            github_default_branch=args.github_default_branch,
        )
        snapshot = build_cache_key_probe_snapshot(
            client,
            repo=repo,
            snapshot_utc=snapshot_utc,
            requests=[parse_cache_key_probe(raw) for raw in args.cache_key],
            cache_refs=cache_refs,
        )
        restore_hits = [
            parse_labeled_value(raw, "--restore-hit")
            for raw in provided_values(args.restore_hit, "--restore-hit")
        ]
        save_outcomes = [
            parse_labeled_value(raw, "--save-outcome")
            for raw in provided_values(args.save_outcome, "--save-outcome")
        ]
        if args.github_step_summary is not None:
            if not restore_hits:
                raise AuditError("--restore-hit is required", kind=FailureKind.ABSENT, field="--restore-hit")
            if not save_outcomes:
                raise AuditError("--save-outcome is required", kind=FailureKind.ABSENT, field="--save-outcome")
            append_step_summary(
                args.github_step_summary,
                render_cache_persistence_audit_text(
                    snapshot,
                    restore_hits=restore_hits,
                    save_outcomes=save_outcomes,
                ),
            )
        elif args.json:
            print(json.dumps(snapshot, indent=2, sort_keys=True))
        elif restore_hits or save_outcomes:
            print(
                render_cache_persistence_audit_text(
                    snapshot,
                    restore_hits=restore_hits,
                    save_outcomes=save_outcomes,
                )
            )
        else:
            print(render_cache_key_probe_text(snapshot))
        if args.github_annotations:
            for annotation in cache_persistence_annotations(snapshot):
                print(annotation)
        return 0
    branch = args.branch or infer_default_branch()
    snapshot = build_snapshot(
        client,
        repo=repo,
        branch=branch,
        snapshot_utc=snapshot_utc,
    )
    if args.json:
        print(json.dumps(snapshot, indent=2, sort_keys=True))
    else:
        print(render_text(snapshot))
    return 0


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    try:
        validate_args(args)
    except AuditError as exc:
        if getattr(args, "github_annotations", False):
            print(f"::error::cache persistence audit contract failed: {exc}")
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    try:
        return run(args)
    except AuditError as exc:
        if getattr(args, "github_step_summary", None) is not None:
            append_step_summary(args.github_step_summary, render_cache_persistence_failure_text(exc))
        if getattr(args, "github_annotations", False):
            print(f"::error::cache persistence audit contract failed: {exc}")
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
