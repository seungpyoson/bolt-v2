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
import json
import subprocess
import sys
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
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
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


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except AuditError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(2) from exc
