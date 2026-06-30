#!/usr/bin/env python3
"""Read-only GitHub Actions storage audit for the current repository.

The ``--json`` output is a stable downstream contract for #936 and the storage
tripwire: existing keys are append-only. ``required_checks.contexts`` preserves
the raw source shape, so rulesets entries are objects such as
``{"context": "...", "integration_id": ...}`` while legacy branch-protection
fallback entries may be strings or objects such as ``{"context": "...",
"app_id": ...}``.

``cache.count`` and ``artifacts.count`` are GitHub's ``total_count``. Missing or
malformed ``total_count`` is a contract failure, not an enumerated-row fallback.
``enumerated_count`` is derived from the paginated rows this audit actually read;
the two may differ under live CI churn.
"""

from __future__ import annotations

import argparse
import collections
import datetime as dt
import enum
import fnmatch
import json
import pathlib
import string
import subprocess
import sys
import tomllib
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
    artifact_metadata_unavailable_keep_reason: str
    active_run_keep_reason: str
    status_unavailable_keep_reason: str
    expiration_unknown_keep_reason: str
    not_expired_keep_reason: str
    billing_impact_unverifiable: str
    wait_and_remeasure: str
    protected_refs: tuple[str, ...]
    protected_ref_prefixes: tuple[str, ...]
    protected_ref_globs: tuple[str, ...]
    branch_ref_events: tuple[str, ...]
    active_run_statuses: tuple[str, ...]
    terminal_run_statuses: tuple[str, ...]
    workflow_run_fetch_limit: int
    billing_probe_paths: tuple[str, ...]
    classes: tuple[ArtifactClassRule, ...]


class CleanupAlertPolicy(NamedTuple):
    schema_version: int
    title: str
    clear_title: str
    candidate_count_error_threshold: int
    candidate_count_error_reason: str
    expected_reclaim_proxy_bytes_error_threshold: int
    expected_reclaim_proxy_bytes_error_reason: str
    unverified_candidate_count_warning_threshold: int
    unverified_candidate_count_warning_reason: str
    metadata_unavailable_count_warning_threshold: int
    metadata_unavailable_count_warning_reason: str


class CleanupAlertFinding(NamedTuple):
    level: str
    metric: str
    value: int
    threshold: int
    reason: str


class CleanupAlertAggregate(NamedTuple):
    key: str
    count: int
    size_bytes: int


class InputFailure(NamedTuple):
    field: str
    state: str
    code: str


class ClassifiedInt(NamedTuple):
    value: int | None
    failure: InputFailure | None


class ClassifiedText(NamedTuple):
    value: str | None
    failure: InputFailure | None


class ClassifiedBool(NamedTuple):
    value: bool | None
    failure: InputFailure | None


class CleanupDecision(NamedTuple):
    class_id: str
    decision: str
    reason_code: str
    reason: str
    metadata_failure: InputFailure | None


class WorkflowRunFetchResult(NamedTuple):
    payload: dict[str, Any] | None
    failure: InputFailure | None


class LabeledValue(NamedTuple):
    label: str
    value: str


class TextRule(NamedTuple):
    kind: FailureKind
    accepts: Callable[[Any], bool]
    message: str


class EventRefSpec(NamedTuple):
    base_ref_contract: str


_MISSING = object()


class FieldSpec(NamedTuple):
    source: str
    output: str
    parser: Callable[[Any, str], Any]
    default: Any = _MISSING


class PaginatedPayloadSpec(NamedTuple):
    label: str
    list_field: str
    scalar_fields: tuple[FieldSpec, ...]


class ContractInvariant(NamedTuple):
    field: str
    accepts: Callable[[dict[str, Any]], bool]
    message: str


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
KEEP_DECISION = "KEEP"
DELETE_CANDIDATE_DECISION = "DELETE-CANDIDATE"
STATE_ABSENT = "absent"
STATE_EMPTY = "empty"
STATE_INVALID = "invalid"
STATE_UNAVAILABLE = "unavailable"
STATE_TIMEOUT = "timeout"
FIELD_ARTIFACT_ID = "artifact_id"
FIELD_ARTIFACT_EXPIRED = "expired"
FIELD_ARTIFACT_REF = "workflow_run.ref"
FIELD_WORKFLOW_RUN_API = "workflow_run.api"
FIELD_WORKFLOW_RUN_ID = "workflow_run.id"
FIELD_WORKFLOW_STATUS = "workflow_run.status"
INPUT_FAILURE_CODES = {
    (FIELD_ARTIFACT_ID, STATE_ABSENT): "artifact_id_absent",
    (FIELD_ARTIFACT_ID, STATE_INVALID): "artifact_id_invalid",
    (FIELD_ARTIFACT_EXPIRED, STATE_ABSENT): "artifact_expired_absent",
    (FIELD_ARTIFACT_EXPIRED, STATE_EMPTY): "artifact_expired_empty",
    (FIELD_ARTIFACT_EXPIRED, STATE_INVALID): "artifact_expired_invalid",
    (FIELD_ARTIFACT_REF, STATE_ABSENT): "artifact_ref_absent",
    (FIELD_ARTIFACT_REF, STATE_EMPTY): "artifact_ref_empty",
    (FIELD_ARTIFACT_REF, STATE_INVALID): "artifact_ref_invalid",
    (FIELD_WORKFLOW_RUN_ID, STATE_ABSENT): "workflow_run_id_absent",
    (FIELD_WORKFLOW_RUN_ID, STATE_INVALID): "workflow_run_id_invalid",
    (FIELD_WORKFLOW_RUN_API, STATE_INVALID): "workflow_run_api_invalid",
    (FIELD_WORKFLOW_RUN_API, STATE_UNAVAILABLE): "workflow_run_api_unavailable",
    (FIELD_WORKFLOW_RUN_API, STATE_TIMEOUT): "workflow_run_api_timeout",
    (FIELD_WORKFLOW_STATUS, STATE_ABSENT): "workflow_status_absent",
    (FIELD_WORKFLOW_STATUS, STATE_EMPTY): "workflow_status_empty",
    (FIELD_WORKFLOW_STATUS, STATE_INVALID): "workflow_status_invalid",
}
WORKFLOW_RUN_ID_STATUS_SOURCES = {
    STATE_ABSENT: "workflow_run_id_absent",
    STATE_INVALID: "workflow_run_id_invalid",
}
REASON_DEFAULT_KEEP = "default_keep"
REASON_EXPIRATION_UNKNOWN = "expiration_status_unknown"
REASON_ARTIFACT_METADATA_UNAVAILABLE = "artifact_metadata_unavailable"
REASON_PROTECTED_REF = "protected_ref"
REASON_NOT_EXPIRED = "not_expired"
REASON_CLASS_KEEP = "class_keep"
REASON_ACTIVE_RUN = "active_run"
REASON_WORKFLOW_STATUS_UNAVAILABLE = "workflow_status_unavailable"
REASON_DELETE_CANDIDATE = "delete_candidate"


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

    object_pages: list[dict[str, Any]] = []
    list_items: list[Any] = []
    saw_list_page = False
    for index, page in enumerate(payload):
        if isinstance(page, list):
            if object_pages:
                raise AuditError("paginated payload mixed page shapes")
            saw_list_page = True
            list_items.extend(page)
            continue
        if not isinstance(page, dict):
            raise AuditError("paginated payload page is not an object or list")
        if saw_list_page:
            raise AuditError("paginated payload mixed page shapes")
        object_pages.append(page)
    if saw_list_page:
        return list_items
    if not object_pages:
        raise AuditError(
            "paginated payload must not be empty",
            kind=FailureKind.EMPTY,
            field="paginated",
        )
    return merge_paginated_object_pages(object_pages)


def paginated_payload_spec_for_pages(pages: list[dict[str, Any]]) -> PaginatedPayloadSpec:
    matches: list[PaginatedPayloadSpec] = []
    for page in pages:
        for spec in PAGINATED_PAYLOAD_SPECS.values():
            if spec.list_field in page and spec not in matches:
                matches.append(spec)
    if not matches:
        raise AuditError(
            "paginated object payload has no contract",
            kind=FailureKind.INVALID,
            field="paginated",
        )
    if len(matches) > 1:
        raise AuditError(
            "paginated object payload matches multiple contracts",
            kind=FailureKind.AMBIGUOUS,
            field="paginated",
        )
    return matches[0]


def merge_paginated_object_pages(pages: list[dict[str, Any]]) -> dict[str, Any]:
    spec = paginated_payload_spec_for_pages(pages)
    merged: dict[str, Any] = {spec.list_field: []}
    scalar_names = {field.source for field in spec.scalar_fields}
    for page in pages:
        parsed_scalars = parse_contract_object(page, spec.label, spec.scalar_fields)
        page_items = list_field(page, spec.list_field, spec.label)
        for key in page:
            if key == spec.list_field or key in scalar_names:
                continue
            raise AuditError(
                f"{spec.label}.{key} is not part of the paginated payload contract",
                kind=FailureKind.INVALID,
                field=f"{spec.label}.{key}",
            )
        for key, value in parsed_scalars.items():
            if key in merged and merged[key] != value:
                raise AuditError(
                    f"{spec.label}.{key} differs across paginated pages",
                    kind=FailureKind.AMBIGUOUS,
                    field=f"{spec.label}.{key}",
                )
            merged[key] = value
        merged[spec.list_field].extend(page_items)
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
        raise AuditError(f"{label} payload is not an object", kind=FailureKind.INVALID, field=label)
    return payload


def require_field(payload: dict[str, Any], key: str, field: str) -> Any:
    if key not in payload:
        raise AuditError(f"{field} is required", kind=FailureKind.ABSENT, field=field)
    return payload[key]


def require_contract_list(value: Any, field: str) -> list[Any]:
    if not isinstance(value, list):
        raise AuditError(f"{field} must be a list", kind=FailureKind.INVALID, field=field)
    return value


def list_field(payload: dict[str, Any], field: str, label: str) -> list[Any]:
    return require_contract_list(require_field(payload, field, f"{label}.{field}"), f"{label}.{field}")


def object_list_field(payload: dict[str, Any], field: str, label: str) -> list[dict[str, Any]]:
    items = list_field(payload, field, label)
    objects: list[dict[str, Any]] = []
    for index, item in enumerate(items):
        if not isinstance(item, dict):
            item_field = f"{label}.{field}[{index}]"
            raise AuditError(f"{item_field} must be an object", kind=FailureKind.INVALID, field=item_field)
        objects.append(item)
    return objects


def require_bool(value: Any, field: str) -> bool:
    if not isinstance(value, bool):
        raise AuditError(f"{field} must be a boolean", kind=FailureKind.INVALID, field=field)
    return value


def require_nonnegative_int(value: Any, field: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise AuditError(f"{field} must be a non-negative integer", kind=FailureKind.INVALID, field=field)
    return value


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


def require_optional_text(value: Any, field: str) -> str | None:
    if value is None:
        return None
    return require_text(value, field)


def require_source_value(value: Any, field: str, allowed: tuple[str, ...]) -> str:
    source = require_text(value, field)
    if source not in allowed:
        allowed_text = ", ".join(allowed)
        raise AuditError(f"{field} must be one of: {allowed_text}", kind=FailureKind.INVALID, field=field)
    return source


def require_probe_count_source(value: Any, field: str) -> str:
    return require_source_value(value, field, ("github_total_count", "unavailable"))


def require_ref_list(value: Any, field: str) -> list[str]:
    refs = require_contract_list(value, field)
    return [require_ref(raw, f"{field}[{index}]") for index, raw in enumerate(refs)]


def parse_contract_object(raw: Any, label: str, specs: tuple[FieldSpec, ...]) -> dict[str, Any]:
    payload = require_object(raw, label)
    parsed: dict[str, Any] = {}
    for spec in specs:
        field = f"{label}.{spec.source}"
        value = payload[spec.source] if spec.source in payload else spec.default
        if value is _MISSING:
            raise AuditError(f"{field} is required", kind=FailureKind.ABSENT, field=field)
        parsed[spec.output] = spec.parser(value, field)
    return parsed


def parse_contract_items(
    raw_items: list[Any],
    label: str,
    parser: Callable[[Any, str], dict[str, Any]],
) -> list[dict[str, Any]]:
    parsed: list[dict[str, Any]] = []
    for index, raw in enumerate(raw_items):
        if not isinstance(raw, dict):
            field = f"{label}[{index}]"
            raise AuditError(f"{field} must be an object", kind=FailureKind.INVALID, field=field)
        parsed.append(parser(raw, label))
    return parsed


def require_total_count(payload: dict[str, Any], label: str) -> int:
    return require_nonnegative_int(require_field(payload, "total_count", f"{label}.total_count"), f"{label}.total_count")


def optional_text(value: Any) -> str | None:
    if value is None:
        return None
    if value == "":
        return None
    return require_text(value, "optional_text")


def nonnegative_int(value: Any, *, default: int = 0) -> int:
    if isinstance(value, bool):
        return default
    if isinstance(value, int) and value >= 0:
        return value
    return default


def require_nonempty_text(value: Any, field: str) -> str:
    return require_text(value, field)


def input_failure(field: str, state: str) -> InputFailure:
    code = INPUT_FAILURE_CODES.get((field, state))
    if code is None:
        raise AssertionError(f"unregistered input failure: {field} {state}")
    return InputFailure(field=field, state=state, code=code)


def serialized_failure(failure: InputFailure | None) -> dict[str, str] | None:
    return failure._asdict() if failure is not None else None


def parsed_failure(value: Any, label: str) -> InputFailure | None:
    if value is None:
        return None
    data = require_object(value, label)
    failure = InputFailure(
        field=require_nonempty_text(data.get("field"), f"{label}.field"),
        state=require_nonempty_text(data.get("state"), f"{label}.state"),
        code=require_nonempty_text(data.get("code"), f"{label}.code"),
    )
    expected_code = INPUT_FAILURE_CODES.get((failure.field, failure.state))
    if expected_code != failure.code:
        raise AuditError(f"{label} has inconsistent failure code")
    return failure


def classify_positive_int(value: Any, field: str) -> ClassifiedInt:
    if value is None:
        return ClassifiedInt(value=None, failure=input_failure(field, STATE_ABSENT))
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        return ClassifiedInt(value=None, failure=input_failure(field, STATE_INVALID))
    return ClassifiedInt(value=value, failure=None)


def classify_optional_text(value: Any, field: str) -> ClassifiedText:
    if value is None:
        return ClassifiedText(value=None, failure=None)
    if not isinstance(value, str):
        return ClassifiedText(value=None, failure=input_failure(field, STATE_INVALID))
    if not value:
        return ClassifiedText(value=None, failure=input_failure(field, STATE_EMPTY))
    if value != value.strip():
        return ClassifiedText(value=None, failure=input_failure(field, STATE_INVALID))
    return ClassifiedText(value=value, failure=None)


def classify_required_bool(value: Any, field: str) -> ClassifiedBool:
    if value is None:
        return ClassifiedBool(value=None, failure=input_failure(field, STATE_ABSENT))
    if isinstance(value, bool):
        return ClassifiedBool(value=value, failure=None)
    if value == "":
        return ClassifiedBool(value=None, failure=input_failure(field, STATE_EMPTY))
    return ClassifiedBool(value=None, failure=input_failure(field, STATE_INVALID))


def classify_api_unavailable_failure(exc: GhApiError, field: str) -> InputFailure:
    message = exc.message.lower()
    state = STATE_TIMEOUT if "timeout" in message or "timed out" in message else STATE_UNAVAILABLE
    return input_failure(field, state)


def require_policy_table(payload: dict[str, Any], key: str, label: str) -> dict[str, Any]:
    value = payload.get(key)
    if not isinstance(value, dict):
        raise AuditError(f"{label}.{key} must be a table")
    return value


def require_policy_string(payload: dict[str, Any], key: str, label: str) -> str:
    value = payload.get(key)
    return require_text(value, f"{label}.{key}")


def optional_policy_string(payload: dict[str, Any], key: str, label: str) -> str | None:
    value = payload.get(key)
    if value is None:
        return None
    return require_text(value, f"{label}.{key}")


def optional_policy_string_list(payload: dict[str, Any], key: str, label: str) -> tuple[str, ...]:
    value = payload.get(key, [])
    if not isinstance(value, list):
        raise AuditError(f"{label}.{key} must be a string list")
    result: list[str] = []
    for index, item in enumerate(value):
        result.append(require_text(item, f"{label}.{key}[{index}]"))
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
    return require_text(current, f"{label} reference {dotted_ref!r}")


def template_prefix(template: str, label: str) -> str:
    prefix = template.split("{", 1)[0]
    if not prefix:
        raise AuditError(f"{label} template must have a literal prefix before the first placeholder")
    return require_text(prefix, f"{label} template prefix")


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
        values.append(
            template_prefix(value, f"{label}.{key} reference {ref!r}")
            if prefix_from_template
            else value
        )
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
    schema_version = table.get("schema_version")
    if isinstance(schema_version, bool) or schema_version != 1:
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
    billing_probe_paths = optional_policy_string_list(
        table,
        "billing_probe_paths",
        "storage_audit.cleanup_feasibility",
    )
    for template in billing_probe_paths:
        validate_global_api_path_template(
            template,
            "storage_audit.cleanup_feasibility.billing_probe_paths",
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
        artifact_metadata_unavailable_keep_reason=require_policy_string(
            table,
            "artifact_metadata_unavailable_keep_reason",
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
        branch_ref_events=optional_policy_string_list(
            table,
            "branch_ref_events",
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
        billing_probe_paths=billing_probe_paths,
        classes=classes,
    )


def load_cleanup_policy_path(path: pathlib.Path) -> ArtifactCleanupPolicy:
    try:
        return load_cleanup_policy_text(path.read_text(), label=str(path))
    except OSError as exc:
        raise AuditError(f"{path}: could not read cleanup policy: {exc}") from exc


def load_cleanup_alert_policy_text(raw: str, *, label: str) -> CleanupAlertPolicy:
    try:
        document = tomllib.loads(raw)
    except tomllib.TOMLDecodeError as exc:
        raise AuditError(f"{label}: invalid TOML: {exc}") from exc
    if not isinstance(document, dict):
        raise AuditError(f"{label}: TOML root must be a table")
    storage_audit = require_policy_table(document, "storage_audit", label)
    table = require_policy_table(storage_audit, "cleanup_feasibility_alert", "storage_audit")
    schema_version = table.get("schema_version")
    if isinstance(schema_version, bool) or schema_version != 1:
        raise AuditError("storage_audit.cleanup_feasibility_alert.schema_version must be 1")
    label_name = "storage_audit.cleanup_feasibility_alert"
    return CleanupAlertPolicy(
        schema_version=1,
        title=require_policy_string(table, "title", label_name),
        clear_title=require_policy_string(table, "clear_title", label_name),
        candidate_count_error_threshold=require_policy_positive_int(
            table,
            "candidate_count_error_threshold",
            label_name,
        ),
        candidate_count_error_reason=require_policy_string(
            table,
            "candidate_count_error_reason",
            label_name,
        ),
        expected_reclaim_proxy_bytes_error_threshold=require_policy_positive_int(
            table,
            "expected_reclaim_proxy_bytes_error_threshold",
            label_name,
        ),
        expected_reclaim_proxy_bytes_error_reason=require_policy_string(
            table,
            "expected_reclaim_proxy_bytes_error_reason",
            label_name,
        ),
        unverified_candidate_count_warning_threshold=require_policy_positive_int(
            table,
            "unverified_candidate_count_warning_threshold",
            label_name,
        ),
        unverified_candidate_count_warning_reason=require_policy_string(
            table,
            "unverified_candidate_count_warning_reason",
            label_name,
        ),
        metadata_unavailable_count_warning_threshold=require_policy_positive_int(
            table,
            "metadata_unavailable_count_warning_threshold",
            label_name,
        ),
        metadata_unavailable_count_warning_reason=require_policy_string(
            table,
            "metadata_unavailable_count_warning_reason",
            label_name,
        ),
    )


def load_cleanup_alert_policy_path(path: pathlib.Path) -> CleanupAlertPolicy:
    try:
        return load_cleanup_alert_policy_text(path.read_text(), label=str(path))
    except OSError as exc:
        raise AuditError(f"{path}: could not read cleanup alert policy: {exc}") from exc


def has_cleanup_policy_table(path: pathlib.Path) -> bool:
    try:
        document = tomllib.loads(path.read_text())
    except OSError as exc:
        raise AuditError(f"{path}: could not inspect TOML during cleanup policy discovery: {exc}") from exc
    except tomllib.TOMLDecodeError:
        return False
    storage_audit = document.get("storage_audit")
    return isinstance(storage_audit, dict) and isinstance(storage_audit.get("cleanup_feasibility"), dict)


def repository_root_path() -> pathlib.Path:
    result = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise AuditError(result.stderr.strip() or "git rev-parse failed while discovering repository root")
    return pathlib.Path(require_text(result.stdout.strip(), "repository_root"))


def repository_toml_paths() -> list[pathlib.Path]:
    root = repository_root_path()
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", ":(top)*.toml"],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise AuditError(result.stderr.strip() or "git ls-files failed while discovering cleanup policy")
    return [root / line for line in result.stdout.splitlines() if line]


def discover_cleanup_policy_path() -> pathlib.Path:
    candidates: list[pathlib.Path] = []
    for path in repository_toml_paths():
        if has_cleanup_policy_table(path):
            candidates.append(path)
    if not candidates:
        raise AuditError("no tracked TOML file declares [storage_audit.cleanup_feasibility]")
    if len(candidates) > 1:
        joined = ", ".join(str(path) for path in candidates)
        raise AuditError(f"multiple cleanup feasibility policies found: {joined}")
    return candidates[0]


PAGINATED_PAYLOAD_SPECS = {
    "actions/caches": PaginatedPayloadSpec(
        label="actions/caches",
        list_field="actions_caches",
        scalar_fields=(FieldSpec("total_count", "total_count", require_nonnegative_int),),
    ),
    "actions/artifacts": PaginatedPayloadSpec(
        label="actions/artifacts",
        list_field="artifacts",
        scalar_fields=(FieldSpec("total_count", "total_count", require_nonnegative_int),),
    ),
}


CACHE_API_ENTRY_CONTRACT = (
    FieldSpec("id", "cache_id", require_nonnegative_int),
    FieldSpec("ref", "ref", require_ref),
    FieldSpec("key", "key", require_text),
    FieldSpec("last_accessed_at", "last_accessed_at", require_optional_text, None),
    FieldSpec("size_in_bytes", "size_bytes", require_nonnegative_int),
)
CACHE_SNAPSHOT_ENTRY_CONTRACT = (
    FieldSpec("cache_id", "cache_id", require_nonnegative_int),
    FieldSpec("ref", "ref", require_ref),
    FieldSpec("key", "key", require_text),
    FieldSpec("last_accessed_at", "last_accessed_at", require_optional_text, None),
    FieldSpec("size_bytes", "size_bytes", require_nonnegative_int),
)
CACHE_USAGE_CONTRACT = (
    FieldSpec("available", "available", require_bool),
    FieldSpec("active_caches_count", "active_caches_count", require_nonnegative_int),
    FieldSpec("active_caches_size_in_bytes", "active_caches_size_in_bytes", require_nonnegative_int),
    FieldSpec("source", "source", require_text),
)
CACHE_KEY_PROBE_CONTRACT = (
    FieldSpec("label", "label", require_text),
    FieldSpec("key", "key", require_text),
    FieldSpec("available", "available", require_bool),
    FieldSpec("present", "present", require_bool),
    FieldSpec("exact_count", "exact_count", require_nonnegative_int),
    FieldSpec("api_prefix_count", "api_prefix_count", require_nonnegative_int),
    FieldSpec("api_prefix_count_source", "api_prefix_count_source", require_probe_count_source),
    FieldSpec("api_prefix_enumerated_count", "api_prefix_enumerated_count", require_nonnegative_int),
    FieldSpec("ref_filtered_prefix_enumerated_count", "ref_filtered_prefix_enumerated_count", require_nonnegative_int),
    FieldSpec("prefix_only_count", "prefix_only_count", require_nonnegative_int),
    FieldSpec("entries", "entries", require_contract_list),
    FieldSpec("ref_filter", "ref_filter", require_ref_list),
    FieldSpec("reason", "reason", require_optional_text, None),
)


def cache_entry_from_raw(raw: Any, label: str = "actions/caches") -> dict[str, Any]:
    return parse_contract_object(raw, label, CACHE_API_ENTRY_CONTRACT)


def cache_snapshot_entry_from_raw(raw: Any, label: str = "cache_key_probes.entries") -> dict[str, Any]:
    return parse_contract_object(raw, label, CACHE_SNAPSHOT_ENTRY_CONTRACT)


def cache_usage_from_raw(raw: Any, label: str = "cache_usage") -> dict[str, Any]:
    return parse_contract_object(raw, label, CACHE_USAGE_CONTRACT)


CACHE_KEY_PROBE_INVARIANTS = (
    ContractInvariant(
        "api_prefix_count_source",
        lambda probe: not probe["available"] or probe["api_prefix_count_source"] == "github_total_count",
        "available probe must use the GitHub total_count source",
    ),
    ContractInvariant(
        "api_prefix_count_source",
        lambda probe: probe["available"] or probe["api_prefix_count_source"] == "unavailable",
        "unavailable probe must use the unavailable count source",
    ),
    ContractInvariant(
        "present",
        lambda probe: probe["available"] or not probe["present"],
        "unavailable probe must not report a present key",
    ),
    ContractInvariant(
        "exact_count",
        lambda probe: probe["available"] or probe["exact_count"] == 0,
        "unavailable probe must not report exact entries",
    ),
    ContractInvariant(
        "entries",
        lambda probe: probe["available"] or not probe["entries"],
        "unavailable probe must not include entries",
    ),
    ContractInvariant(
        "present",
        lambda probe: probe["present"] == (probe["exact_count"] > 0),
        "probe present flag must match exact_count",
    ),
    ContractInvariant(
        "entries",
        lambda probe: len(probe["entries"]) == probe["exact_count"],
        "probe entries length must match exact_count",
    ),
    ContractInvariant(
        "ref_filtered_prefix_enumerated_count",
        lambda probe: probe["exact_count"] <= probe["ref_filtered_prefix_enumerated_count"],
        "exact_count must not exceed ref-filtered prefix enumeration",
    ),
    ContractInvariant(
        "ref_filtered_prefix_enumerated_count",
        lambda probe: probe["ref_filtered_prefix_enumerated_count"] <= probe["api_prefix_enumerated_count"],
        "ref-filtered prefix enumeration must not exceed API prefix enumeration",
    ),
    ContractInvariant(
        "api_prefix_enumerated_count",
        lambda probe: probe["api_prefix_enumerated_count"] <= probe["api_prefix_count"],
        "API prefix enumeration must not exceed API prefix count",
    ),
    ContractInvariant(
        "prefix_only_count",
        lambda probe: probe["prefix_only_count"] == probe["ref_filtered_prefix_enumerated_count"] - probe["exact_count"],
        "prefix_only_count must match ref-filtered prefix entries minus exact entries",
    ),
    ContractInvariant(
        "entries",
        lambda probe: all(entry["key"] == probe["key"] for entry in probe["entries"]),
        "probe entries must match the exact probe key",
    ),
    ContractInvariant(
        "entries",
        lambda probe: not probe["ref_filter"] or all(entry["ref"] in probe["ref_filter"] for entry in probe["entries"]),
        "probe entries must match the configured ref filter",
    ),
)


def validate_contract_invariants(
    payload: dict[str, Any],
    label: str,
    invariants: tuple[ContractInvariant, ...],
) -> dict[str, Any]:
    for invariant in invariants:
        if not invariant.accepts(payload):
            raise AuditError(invariant.message, kind=FailureKind.INVALID, field=f"{label}.{invariant.field}")
    return payload


def cache_key_probe_from_raw(raw: Any, label: str = "cache_key_probes") -> dict[str, Any]:
    probe = parse_contract_object(raw, label, CACHE_KEY_PROBE_CONTRACT)
    probe["entries"] = parse_contract_items(probe["entries"], f"{label}.entries", cache_snapshot_entry_from_raw)
    return validate_contract_invariants(probe, label, CACHE_KEY_PROBE_INVARIANTS)


def cache_key_probe_list_from_raw(value: Any, field: str) -> list[dict[str, Any]]:
    return parse_contract_items(require_contract_list(value, field), field, cache_key_probe_from_raw)


CACHE_KEY_PROBE_SNAPSHOT_CONTRACT = (
    FieldSpec("snapshot_utc", "snapshot_utc", require_text),
    FieldSpec("repo", "repo", require_text),
    FieldSpec("cache_refs", "cache_refs", require_ref_list),
    FieldSpec("cache_usage", "cache_usage", cache_usage_from_raw),
    FieldSpec("cache_key_probes", "cache_key_probes", cache_key_probe_list_from_raw),
)


def cache_key_probe_snapshot_from_raw(raw: Any) -> dict[str, Any]:
    return parse_contract_object(raw, "cache_key_probe_snapshot", CACHE_KEY_PROBE_SNAPSHOT_CONTRACT)


def require_labeled_pair(raw: str, field: str) -> tuple[str, str]:
    if "=" not in raw:
        raise AuditError(f"{field} must be LABEL=VALUE", kind=FailureKind.INVALID, field=field)
    return raw.split("=", 1)


def classify_cleanup_ref(value: Any, field: str, *, allow_canonical_refs: bool) -> ClassifiedText:
    ref = classify_optional_text(value, field)
    if ref.failure is not None or ref.value is None:
        return ref
    if ref.value.startswith(("heads/", "tags/")):
        return ClassifiedText(value=None, failure=input_failure(field, STATE_INVALID))
    if ref.value.startswith("refs/"):
        if allow_canonical_refs:
            if ref.value.startswith("refs/heads/") and ref.value.removeprefix("refs/heads/"):
                return ref
            if ref.value.startswith("refs/tags/") and ref.value.removeprefix("refs/tags/"):
                return ref
        return ClassifiedText(value=None, failure=input_failure(field, STATE_INVALID))
    return ref


def cleanup_ref_has_known_namespace(ref: str | None) -> bool:
    return ref is not None and ref.startswith(("refs/heads/", "refs/tags/"))


def classify_workflow_ref(
    workflow_run: dict[str, Any],
    *,
    branch_ref_events: tuple[str, ...] = (),
) -> ClassifiedText:
    ref = classify_cleanup_ref(workflow_run.get("ref"), FIELD_ARTIFACT_REF, allow_canonical_refs=True)
    if ref.failure is not None or cleanup_ref_has_known_namespace(ref.value):
        return ref
    head_branch = classify_cleanup_ref(workflow_run.get("head_branch"), FIELD_ARTIFACT_REF, allow_canonical_refs=False)
    if head_branch.failure is not None or head_branch.value is not None:
        if head_branch.value is not None and workflow_run.get("event") in branch_ref_events:
            return ClassifiedText(value=f"refs/heads/{head_branch.value}", failure=None)
        return head_branch
    if ref.value is not None:
        return ref
    return ClassifiedText(value=None, failure=input_failure(FIELD_ARTIFACT_REF, STATE_ABSENT))


def workflow_run_from_raw(
    raw: Any,
    *,
    label: str = "actions/artifacts.workflow_run",
    require_status: bool = False,
) -> dict[str, Any]:
    data = require_object(raw, label)
    run_id = classify_positive_int(data.get("id"), FIELD_WORKFLOW_RUN_ID)
    status = classify_optional_text(data.get("status"), FIELD_WORKFLOW_STATUS)
    if require_status and status.value is None and status.failure is None:
        status = ClassifiedText(value=None, failure=input_failure(FIELD_WORKFLOW_STATUS, STATE_ABSENT))
    ref = classify_workflow_ref(data)
    head_branch = classify_cleanup_ref(data.get("head_branch"), FIELD_ARTIFACT_REF, allow_canonical_refs=False)
    head_sha = optional_text(data.get("head_sha"))
    return {
        "id": run_id.value,
        "id_failure": serialized_failure(run_id.failure),
        "status": status.value,
        "status_failure": serialized_failure(status.failure),
        "conclusion": optional_text(data.get("conclusion")),
        "ref": ref.value,
        "ref_failure": serialized_failure(ref.failure),
        "head_branch": head_branch.value,
        "head_sha": head_sha,
        "event": optional_text(data.get("event")),
        "status_source": "artifact_payload" if status.value is not None else "not_fetched",
    }


def merge_workflow_run_metadata(entry: dict[str, Any], payload: dict[str, Any]) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    refreshed = workflow_run_from_raw(
        payload,
        label="actions/runs workflow_run",
        require_status=True,
    )
    for key, value in refreshed.items():
        if key != "status_source":
            workflow_run[key] = value
    workflow_run["status_source"] = "run_api"


def set_workflow_run_status_source(entry: dict[str, Any], source: str) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    workflow_run["status_source"] = source


def artifact_base_entry_from_raw(raw: dict[str, Any], label: str = "actions/artifacts") -> dict[str, Any]:
    expired = classify_required_bool(raw.get("expired"), FIELD_ARTIFACT_EXPIRED)
    return {
        "name": require_nonempty_text(raw.get("name"), f"{label}.name"),
        "size_bytes": require_nonnegative_int(raw.get("size_in_bytes"), f"{label}.size_in_bytes"),
        "created_at": optional_text(raw.get("created_at")),
        "expires_at": optional_text(raw.get("expires_at")),
        "expired": expired.value,
        "expiration_failure": serialized_failure(expired.failure),
    }


def artifact_entry_from_raw(raw: dict[str, Any], label: str = "actions/artifacts") -> dict[str, Any]:
    artifact_id = classify_positive_int(raw.get("id"), FIELD_ARTIFACT_ID)
    return {
        "artifact_id": artifact_id.value,
        "artifact_id_failure": serialized_failure(artifact_id.failure),
        **artifact_base_entry_from_raw(raw, label),
        "workflow_run": workflow_run_from_raw(raw.get("workflow_run"), label=f"{label}.workflow_run"),
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
        payload = require_object(
            client.api(
                "actions/caches",
                params={"key": request.key, "per_page": "100"},
                paginate=True,
            ),
            "actions/caches",
        )
        raw_entries = list_field(payload, "actions_caches", "actions/caches")
        prefix_entries = parse_contract_items(raw_entries, "actions/caches", cache_entry_from_raw)
        accessible_prefix_entries = [
            entry
            for entry in prefix_entries
            if not ref_filter_set or entry["ref"] in ref_filter_set
        ]
        exact_entries = [
            entry for entry in accessible_prefix_entries
            if entry["key"] == request.key
        ]
        api_prefix_count = require_total_count(payload, "actions/caches")
        probes.append(
            {
                "label": request.label,
                "key": request.key,
                "available": True,
                "present": bool(exact_entries),
                "exact_count": len(exact_entries),
                "api_prefix_count": api_prefix_count,
                "api_prefix_count_source": "github_total_count",
                "api_prefix_enumerated_count": len(prefix_entries),
                "ref_filtered_prefix_enumerated_count": len(accessible_prefix_entries),
                "prefix_only_count": max(0, len(accessible_prefix_entries) - len(exact_entries)),
                "entries": exact_entries,
                "ref_filter": ref_filter,
            }
        )
    return probes


def fetch_cache_usage(client: GhClient) -> dict[str, Any]:
    payload = require_object(client.api("actions/cache/usage"), "actions/cache/usage")
    contract_payload = dict(payload)
    contract_payload["available"] = True
    contract_payload["source"] = "rest"
    return cache_usage_from_raw(contract_payload, "actions/cache/usage")


def fetch_cache(client: GhClient) -> dict[str, Any]:
    payload = require_object(
        client.api("actions/caches", params={"per_page": "100"}, paginate=True),
        "actions/caches",
    )
    raw_entries = list_field(payload, "actions_caches", "actions/caches")
    entries = parse_contract_items(raw_entries, "actions/caches", cache_entry_from_raw)
    total_bytes = 0
    for entry in entries:
        total_bytes += entry["size_bytes"]

    return {
        "total_bytes": total_bytes,
        "count": require_total_count(payload, "actions/caches"),
        "count_source": "github_total_count",
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
    parser = artifact_entry_from_raw if include_entries else artifact_base_entry_from_raw
    artifacts = parse_contract_items(raw_artifacts, "actions/artifacts", parser)
    by_name: dict[str, dict[str, int]] = collections.defaultdict(empty_artifact_group)
    totals = empty_artifact_group()
    entries: list[dict[str, Any]] = []

    for artifact in artifacts:
        name = artifact["name"]
        size_bytes = artifact["size_bytes"]
        add_artifact_expiration_totals(
            totals,
            size_bytes=size_bytes,
            expired=artifact["expired"],
        )
        add_artifact_expiration_totals(
            by_name[name],
            size_bytes=size_bytes,
            expired=artifact["expired"],
        )
        if include_entries:
            entries.append(artifact)

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
    result = {
        "total_bytes": totals["total_bytes"],
        "count": require_total_count(payload, "actions/artifacts"),
        "count_source": "github_total_count",
        "enumerated_count": len(artifacts),
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
    for rule in policy.classes:
        if name in rule.name_equals or any(name.startswith(prefix) for prefix in rule.name_prefixes):
            return rule
    return None


def artifact_ref(policy: ArtifactCleanupPolicy, entry: dict[str, Any]) -> str | None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    return classify_workflow_ref(workflow_run, branch_ref_events=policy.branch_ref_events).value


def apply_cleanup_policy_ref_resolution(entry: dict[str, Any], policy: ArtifactCleanupPolicy) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    if parsed_failure(workflow_run.get("ref_failure"), "artifact workflow_run.ref_failure") is not None:
        return
    ref = classify_workflow_ref(workflow_run, branch_ref_events=policy.branch_ref_events)
    if ref.failure is not None:
        workflow_run["ref"] = None
        workflow_run["ref_failure"] = serialized_failure(ref.failure)
        return
    workflow_run["ref"] = ref.value
    workflow_run["ref_failure"] = None


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
        forms.append(f"tags/{tag}")
    else:
        forms.append(f"refs/heads/{ref}")
    return tuple(dict.fromkeys(forms))


def ref_is_protected(policy: ArtifactCleanupPolicy, ref: str | None) -> bool:
    if ref is None:
        return False
    if ref in policy.protected_refs:
        return True
    if not cleanup_ref_has_known_namespace(ref):
        return False
    forms = protected_ref_forms(ref)
    return (
        any(form in policy.protected_refs for form in forms)
        or any(form.startswith(prefix) for form in forms for prefix in policy.protected_ref_prefixes)
        or any(fnmatch.fnmatchcase(form, pattern) for form in forms for pattern in policy.protected_ref_globs)
    )


def cleanup_ref_namespace_failure(ref: str | None) -> InputFailure | None:
    if ref is None:
        return None
    if cleanup_ref_has_known_namespace(ref):
        return None
    return input_failure(FIELD_ARTIFACT_REF, STATE_INVALID)


def artifact_identity_failure(entry: dict[str, Any]) -> InputFailure | None:
    stored_failure = parsed_failure(entry.get("artifact_id_failure"), "artifact artifact_id_failure")
    if stored_failure is not None:
        return stored_failure
    return classify_positive_int(entry.get("artifact_id"), FIELD_ARTIFACT_ID).failure


def artifact_metadata_failure(policy: ArtifactCleanupPolicy, entry: dict[str, Any]) -> InputFailure | None:
    identity_failure = artifact_identity_failure(entry)
    if identity_failure is not None:
        return identity_failure
    return artifact_ref_failure(policy, entry)


def artifact_ref_failure(policy: ArtifactCleanupPolicy, entry: dict[str, Any]) -> InputFailure | None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    stored_ref_failure = parsed_failure(workflow_run.get("ref_failure"), "artifact workflow_run.ref_failure")
    if stored_ref_failure is not None:
        return stored_ref_failure
    return classify_workflow_ref(workflow_run, branch_ref_events=policy.branch_ref_events).failure


def workflow_status_failure(workflow_run: dict[str, Any]) -> InputFailure | None:
    stored_failure = parsed_failure(workflow_run.get("status_failure"), "artifact workflow_run.status_failure")
    if stored_failure is not None:
        return stored_failure
    return classify_optional_text(workflow_run.get("status"), FIELD_WORKFLOW_STATUS).failure


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
    if artifact_identity_failure(entry) is not None:
        return False
    ref_failure = artifact_ref_failure(policy, entry)
    ref_namespace_failure = None
    if ref_failure is None:
        ref = artifact_ref(policy, entry)
        if ref_is_protected(policy, ref):
            return False
        ref_namespace_failure = cleanup_ref_namespace_failure(ref)
    elif ref_failure.state != STATE_ABSENT:
        return False
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    if workflow_status_failure(workflow_run) is not None:
        return False
    status = classify_optional_text(workflow_run.get("status"), FIELD_WORKFLOW_STATUS)
    return status.value is None or ref_failure is not None or ref_namespace_failure is not None


def workflow_run_id_from_entry(entry: dict[str, Any]) -> ClassifiedInt:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    stored_failure = parsed_failure(workflow_run.get("id_failure"), "artifact workflow_run.id_failure")
    if stored_failure is not None:
        return ClassifiedInt(value=None, failure=stored_failure)
    return classify_positive_int(workflow_run.get("id"), FIELD_WORKFLOW_RUN_ID)


def fetch_workflow_run_metadata_payload(client: GhClient, run_id: int) -> WorkflowRunFetchResult:
    try:
        payload = client.api(f"actions/runs/{run_id}")
    except GhApiError as exc:
        return WorkflowRunFetchResult(
            payload=None,
            failure=classify_api_unavailable_failure(exc, FIELD_WORKFLOW_RUN_API),
        )
    if not isinstance(payload, dict):
        return WorkflowRunFetchResult(
            payload=None,
            failure=input_failure(FIELD_WORKFLOW_RUN_API, STATE_INVALID),
        )
    fetched_run_id = classify_positive_int(payload.get("id"), FIELD_WORKFLOW_RUN_ID)
    if fetched_run_id.failure is not None or fetched_run_id.value != run_id:
        return WorkflowRunFetchResult(
            payload=None,
            failure=input_failure(FIELD_WORKFLOW_RUN_API, STATE_INVALID),
        )
    return WorkflowRunFetchResult(payload=payload, failure=None)


def set_workflow_run_status_failure(entry: dict[str, Any], failure: InputFailure) -> None:
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    workflow_run["status_failure"] = serialized_failure(failure)
    workflow_run["status_source"] = failure.code


def apply_workflow_run_fetch_result(entry: dict[str, Any], result: WorkflowRunFetchResult) -> None:
    if result.failure is not None:
        set_workflow_run_status_failure(entry, result.failure)
        return
    if result.payload is None:
        raise AssertionError("workflow run fetch result has neither payload nor failure")
    merge_workflow_run_metadata(entry, result.payload)


def cleanup_decision_for_entry(
    policy: ArtifactCleanupPolicy,
    rule: ArtifactClassRule | None,
    entry: dict[str, Any],
) -> CleanupDecision:
    if rule is None:
        return CleanupDecision(
            class_id=policy.default_class,
            decision=policy.default_decision,
            reason_code=REASON_DEFAULT_KEEP,
            reason=policy.default_keep_reason,
            metadata_failure=None,
        )
    class_id = rule.rule_id
    expired = entry.get("expired")
    expiration_failure = parsed_failure(entry.get("expiration_failure"), "artifact expiration_failure")
    if expiration_failure is not None or expired is None:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_EXPIRATION_UNKNOWN,
            reason=policy.expiration_unknown_keep_reason,
            metadata_failure=None,
        )
    if expired is False:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_NOT_EXPIRED,
            reason=policy.not_expired_keep_reason,
            metadata_failure=None,
        )
    metadata_failure = artifact_metadata_failure(policy, entry)
    if rule.expired_decision == DELETE_CANDIDATE_DECISION and metadata_failure is not None:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_ARTIFACT_METADATA_UNAVAILABLE,
            reason=policy.artifact_metadata_unavailable_keep_reason,
            metadata_failure=metadata_failure,
        )
    ref = artifact_ref(policy, entry)
    if ref_is_protected(policy, ref):
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_PROTECTED_REF,
            reason=policy.protected_ref_keep_reason,
            metadata_failure=None,
        )
    ref_namespace_failure = cleanup_ref_namespace_failure(ref)
    if rule.expired_decision == DELETE_CANDIDATE_DECISION and ref_namespace_failure is not None:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_ARTIFACT_METADATA_UNAVAILABLE,
            reason=policy.artifact_metadata_unavailable_keep_reason,
            metadata_failure=ref_namespace_failure,
        )
    if rule.expired_decision == KEEP_DECISION:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_CLASS_KEEP,
            reason=rule.keep_reason,
            metadata_failure=None,
        )
    workflow_run = require_object(entry["workflow_run"], "artifact workflow_run")
    if workflow_status_failure(workflow_run) is not None:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_WORKFLOW_STATUS_UNAVAILABLE,
            reason=policy.status_unavailable_keep_reason,
            metadata_failure=None,
        )
    status = classify_optional_text(workflow_run.get("status"), FIELD_WORKFLOW_STATUS)
    if status.value in policy.active_run_statuses:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_ACTIVE_RUN,
            reason=policy.active_run_keep_reason,
            metadata_failure=None,
        )
    if status.value not in policy.terminal_run_statuses:
        return CleanupDecision(
            class_id=class_id,
            decision=KEEP_DECISION,
            reason_code=REASON_WORKFLOW_STATUS_UNAVAILABLE,
            reason=policy.status_unavailable_keep_reason,
            metadata_failure=None,
        )
    if rule.candidate_reason is None:
        raise AuditError(f"cleanup class {rule.rule_id} has no candidate reason")
    return CleanupDecision(
        class_id=class_id,
        decision=DELETE_CANDIDATE_DECISION,
        reason_code=REASON_DELETE_CANDIDATE,
        reason=rule.candidate_reason,
        metadata_failure=None,
    )


def row_from_entry(
    entry: dict[str, Any],
    *,
    cleanup_decision: CleanupDecision,
) -> dict[str, Any]:
    return {
        "artifact_id": entry.get("artifact_id"),
        "name": entry.get("name"),
        "class": cleanup_decision.class_id,
        "size_bytes": entry.get("size_bytes"),
        "created_at": entry.get("created_at"),
        "expires_at": entry.get("expires_at"),
        "expired": entry.get("expired"),
        "expiration_failure": entry.get("expiration_failure"),
        "workflow_run": entry.get("workflow_run"),
        "decision": cleanup_decision.decision,
        "reason_code": cleanup_decision.reason_code,
        "reason": cleanup_decision.reason,
        "metadata_failure": serialized_failure(cleanup_decision.metadata_failure),
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
    saw_non_expired = False
    for entry in entries:
        expired = entry.get("expired")
        if expired is None:
            return {"expires_at": None, "source": "artifact_expiration_status_unknown"}
        if expired is True:
            continue
        if expired is not False:
            return {"expires_at": None, "source": "artifact_expiration_status_unknown"}
        saw_non_expired = True
        timestamp = parse_github_timestamp(entry.get("expires_at"))
        if timestamp is None:
            return {"expires_at": None, "source": "non_expired_artifact_expiry_unknown"}
        if best_timestamp is None or timestamp > best_timestamp:
            best_timestamp = timestamp
            best_entry = entry
    if not saw_non_expired:
        return {"expires_at": None, "source": "no_non_expired_artifact_expiry"}
    if best_entry is None:
        raise AssertionError("unreachable non-expired horizon state")
    return {
        "expires_at": best_entry.get("expires_at"),
        "source": "max_non_expired_artifact_expires_at",
    }


def validate_global_api_path_template(template: str, label: str) -> None:
    allowed_fields = frozenset(("owner", "repo", "owner_repo"))
    try:
        parsed = tuple(string.Formatter().parse(template))
    except ValueError as exc:
        raise AuditError(f"{label} template is invalid: {exc}") from exc
    for _, field_name, _, _ in parsed:
        if field_name is None:
            continue
        if field_name not in allowed_fields:
            raise AuditError(f"{label} has unsupported placeholder: {field_name!r}")


def format_global_api_path(template: str, repo: str) -> str:
    if "/" not in repo:
        raise AuditError("repo must be OWNER/REPO for billing endpoint path expansion")
    owner, repo_name = repo.split("/", 1)
    validate_global_api_path_template(template, "billing endpoint path")
    try:
        return template.format(owner=owner, repo=repo_name, owner_repo=repo)
    except (IndexError, KeyError, ValueError) as exc:
        raise AuditError(f"billing endpoint path template is invalid: {exc}") from exc


def api_response_summary(payload: Any) -> dict[str, Any]:
    if isinstance(payload, dict):
        return {"type": "object", "keys": sorted(str(key) for key in payload)}
    if isinstance(payload, list):
        return {"type": "list", "count": len(payload)}
    return {"type": type(payload).__name__}


def sanitized_probe_error(exc: AuditError) -> str:
    return exc.kind.value


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
            probes.append({"path": path, "status": "unavailable", "error": sanitized_probe_error(exc)})
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
    entries = object_list_field(artifacts, "entries", "artifacts")
    rows: list[dict[str, Any]] = []
    workflow_run_fetches = 0
    workflow_run_fetch_limit_reached = False
    workflow_run_metadata_cache: dict[int, WorkflowRunFetchResult] = {}

    for entry in entries:
        artifact_name = require_nonempty_text(entry.get("name"), "artifacts.entries.name")
        rule = cleanup_rule_for_name(policy, artifact_name)
        if should_fetch_workflow_run(policy, rule, entry):
            run_id = workflow_run_id_from_entry(entry)
            if run_id.failure is not None:
                set_workflow_run_status_source(entry, WORKFLOW_RUN_ID_STATUS_SOURCES[run_id.failure.state])
            elif run_id.value in workflow_run_metadata_cache:
                apply_workflow_run_fetch_result(entry, workflow_run_metadata_cache[run_id.value])
            elif workflow_run_fetches < policy.workflow_run_fetch_limit:
                workflow_run_fetches += 1
                if run_id.value is None:
                    raise AssertionError("workflow run id classifier returned no value without failure")
                result = fetch_workflow_run_metadata_payload(client, run_id.value)
                workflow_run_metadata_cache[run_id.value] = result
                apply_workflow_run_fetch_result(entry, result)
            else:
                workflow_run_fetch_limit_reached = True
                set_workflow_run_status_source(entry, "fetch_limit")
        apply_cleanup_policy_ref_resolution(entry, policy)
        cleanup_decision = cleanup_decision_for_entry(policy, rule, entry)
        rows.append(row_from_entry(entry, cleanup_decision=cleanup_decision))

    candidate_bytes = sum(
        nonnegative_int(row.get("size_bytes"))
        for row in rows
        if row.get("decision") == DELETE_CANDIDATE_DECISION
    )
    unverified_candidate_bytes = sum(
        nonnegative_int(row.get("size_bytes"))
        for row in rows
        if row.get("decision") == KEEP_DECISION
        and row.get("reason_code") == REASON_WORKFLOW_STATUS_UNAVAILABLE
    )
    metadata_unavailable_bytes = sum(
        nonnegative_int(row.get("size_bytes"))
        for row in rows
        if row.get("decision") == KEEP_DECISION
        and row.get("reason_code") == REASON_ARTIFACT_METADATA_UNAVAILABLE
    )
    billing = probe_billing_endpoint(client, repo=repo, policy=policy)
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
            and row.get("reason_code") == REASON_WORKFLOW_STATUS_UNAVAILABLE
        ),
        "metadata_unavailable_bytes": metadata_unavailable_bytes,
        "metadata_unavailable_count": sum(
            1 for row in rows
            if row.get("decision") == KEEP_DECISION
            and row.get("reason_code") == REASON_ARTIFACT_METADATA_UNAVAILABLE
        ),
        "expected_reclaim_proxy_bytes": candidate_bytes,
        # GitHub billing probes expose aggregate usage, not per-candidate reclaim.
        "measured_billed_reclaim_bytes": None,
        "reclaim_basis": "listed_artifact_bytes_proxy",
        "billing": billing,
        "self_clear_horizon": cleanup_self_clear_horizon(rows),
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
    cache_refs: list[str] | None = None,
    cache_branches: list[str] | None = None,
) -> dict[str, Any]:
    normalized_cache_refs = normalize_cache_ref_inputs(
        cache_refs=cache_refs,
        cache_branches=cache_branches,
    )
    snapshot = {
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
    return cache_key_probe_snapshot_from_raw(snapshot)


def append_step_summary(path: str, text: str) -> None:
    summary_path = require_text(path, "--github-step-summary")
    with open(summary_path, "a", encoding="utf-8") as summary:
        summary.write(text)
        summary.write("\n")


def write_json_snapshot(path: pathlib.Path, snapshot: dict[str, Any]) -> None:
    with open(path, "w", encoding="utf-8") as output:
        json.dump(snapshot, output, indent=2, sort_keys=True)
        output.write("\n")


def cleanup_alert_metric(cleanup: dict[str, Any], metric: str) -> int:
    return require_nonnegative_int(
        require_field(cleanup, metric, f"artifact_cleanup_feasibility.{metric}"),
        f"artifact_cleanup_feasibility.{metric}",
    )


def cleanup_alert_findings(
    snapshot: dict[str, Any],
    policy: CleanupAlertPolicy,
) -> list[CleanupAlertFinding]:
    cleanup = require_object(
        require_field(
            snapshot,
            "artifact_cleanup_feasibility",
            "artifact_cleanup_feasibility",
        ),
        "artifact_cleanup_feasibility",
    )
    checks = (
        (
            "error",
            "candidate_count",
            policy.candidate_count_error_threshold,
            policy.candidate_count_error_reason,
        ),
        (
            "error",
            "expected_reclaim_proxy_bytes",
            policy.expected_reclaim_proxy_bytes_error_threshold,
            policy.expected_reclaim_proxy_bytes_error_reason,
        ),
        (
            "warning",
            "unverified_candidate_count",
            policy.unverified_candidate_count_warning_threshold,
            policy.unverified_candidate_count_warning_reason,
        ),
        (
            "warning",
            "metadata_unavailable_count",
            policy.metadata_unavailable_count_warning_threshold,
            policy.metadata_unavailable_count_warning_reason,
        ),
    )
    findings: list[CleanupAlertFinding] = []
    for level, metric, threshold, reason in checks:
        value = cleanup_alert_metric(cleanup, metric)
        if value >= threshold:
            findings.append(
                CleanupAlertFinding(
                    level=level,
                    metric=metric,
                    value=value,
                    threshold=threshold,
                    reason=reason,
                )
            )
    return findings


def cleanup_alert_has_errors(findings: Iterable[CleanupAlertFinding]) -> bool:
    return any(finding.level == "error" for finding in findings)


def cleanup_alert_annotations(findings: Iterable[CleanupAlertFinding]) -> list[str]:
    annotations: list[str] = []
    for finding in findings:
        annotations.append(
            f"::{finding.level}::cleanup feasibility {finding.metric}={finding.value} "
            f"crossed threshold={finding.threshold}: {finding.reason}"
        )
    return annotations


def cleanup_alert_row_aggregates(
    cleanup: dict[str, Any],
    *,
    decision: str,
    field: str,
) -> list[CleanupAlertAggregate]:
    rows = object_list_field(cleanup, "rows", "artifact_cleanup_feasibility")
    counts: dict[str, int] = collections.defaultdict(int)
    sizes: dict[str, int] = collections.defaultdict(int)
    for index, row in enumerate(rows):
        row_label = f"artifact_cleanup_feasibility.rows[{index}]"
        row_decision = require_text(
            require_field(row, "decision", f"{row_label}.decision"),
            f"{row_label}.decision",
        )
        if row_decision != decision:
            continue
        key = require_text(
            require_field(row, field, f"{row_label}.{field}"),
            f"{row_label}.{field}",
        )
        size_bytes = require_nonnegative_int(
            require_field(row, "size_bytes", f"{row_label}.size_bytes"),
            f"{row_label}.size_bytes",
        )
        counts[key] += 1
        sizes[key] += size_bytes
    return [
        CleanupAlertAggregate(key=key, count=counts[key], size_bytes=sizes[key])
        for key in sorted(counts, key=lambda item: (-counts[item], item))
    ]


def append_cleanup_alert_aggregates(lines: list[str], title: str, aggregates: list[CleanupAlertAggregate]) -> None:
    if not aggregates:
        return
    lines.extend(["", f"{title}:"])
    for aggregate in aggregates:
        lines.append(
            f"- `{aggregate.key}`: `{aggregate.count}` rows, `{human_bytes(aggregate.size_bytes)}`"
        )


def render_cleanup_alert_summary(snapshot: dict[str, Any], policy: CleanupAlertPolicy) -> str:
    cleanup = require_object(
        require_field(
            snapshot,
            "artifact_cleanup_feasibility",
            "artifact_cleanup_feasibility",
        ),
        "artifact_cleanup_feasibility",
    )
    findings = cleanup_alert_findings(snapshot, policy)
    billing = cleanup.get("billing") if isinstance(cleanup.get("billing"), dict) else {}
    measured_billed = cleanup.get("measured_billed_reclaim_bytes")
    measured_billed_text = (
        human_bytes(measured_billed)
        if isinstance(measured_billed, int) and not isinstance(measured_billed, bool)
        else "unavailable"
    )
    title = policy.title if findings else policy.clear_title
    lines = [
        f"### {title}",
        "",
        f"- repo: `{require_text(snapshot.get('repo'), 'repo')}`",
        f"- snapshot: `{require_text(snapshot.get('snapshot_utc'), 'snapshot_utc')}`",
        f"- delete candidates: `{cleanup_alert_metric(cleanup, 'candidate_count')}`",
        (
            "- proxy reclaim: "
            f"`{human_bytes(cleanup_alert_metric(cleanup, 'expected_reclaim_proxy_bytes'))}`"
        ),
        f"- unverified candidate rows: `{cleanup_alert_metric(cleanup, 'unverified_candidate_count')}`",
        f"- metadata-unavailable rows: `{cleanup_alert_metric(cleanup, 'metadata_unavailable_count')}`",
        f"- measured billed reclaim: `{measured_billed_text}`",
        f"- reclaim basis: `{require_text(cleanup.get('reclaim_basis'), 'artifact_cleanup_feasibility.reclaim_basis')}`",
        f"- billing: `{billing.get('status')}` ({billing.get('message')})",
    ]
    append_cleanup_alert_aggregates(
        lines,
        "Candidate classes",
        cleanup_alert_row_aggregates(cleanup, decision="DELETE-CANDIDATE", field="class_id"),
    )
    append_cleanup_alert_aggregates(
        lines,
        "Keep reason codes",
        cleanup_alert_row_aggregates(cleanup, decision="KEEP", field="reason_code"),
    )
    if not findings:
        lines.extend(["", "No configured cleanup alert thresholds were crossed."])
        return "\n".join(lines)
    lines.extend(["", "Findings:"])
    for finding in findings:
        value_text = (
            human_bytes(finding.value)
            if finding.metric.endswith("_bytes")
            else str(finding.value)
        )
        threshold_text = (
            human_bytes(finding.threshold)
            if finding.metric.endswith("_bytes")
            else str(finding.threshold)
        )
        lines.append(
            f"- {finding.level.upper()} `{finding.metric}` value `{value_text}` "
            f"crossed threshold `{threshold_text}`: {finding.reason}"
        )
    return "\n".join(lines)


def render_cache_key_probe_text(snapshot: dict[str, Any]) -> str:
    parsed = cache_key_probe_snapshot_from_raw(snapshot)
    probes = parsed["cache_key_probes"]
    lines = [
        f"CI cache key probe for {parsed['repo']}",
        f"Snapshot: {parsed['snapshot_utc']}",
        "",
    ]
    cache_refs = parsed["cache_refs"]
    if cache_refs:
        lines.append(f"Cache refs: {', '.join(str(ref) for ref in cache_refs)}")
        lines.append("")
    usage = parsed["cache_usage"]
    if not usage["available"]:
        raise AuditError(
            "cache usage is unavailable in a successful probe snapshot",
            kind=FailureKind.UNAVAILABLE,
            field="cache_usage",
        )
    lines.append(
        "Cache usage: "
        f"{usage['active_caches_count']} active caches, "
        f"{human_bytes(usage['active_caches_size_in_bytes'])} "
        f"(source: {usage['source']})"
    )
    lines.append("")
    lines.append("Cache key probes:")
    for raw in probes:
        if not raw["available"]:
            raise AuditError(
                "cache key probe is unavailable in a successful probe snapshot",
                kind=FailureKind.UNAVAILABLE,
                field="cache_key_probes",
            )
        status = "present" if raw["present"] else "missing"
        reason = raw["reason"]
        reason_fragment = f" reason={reason}" if reason else ""
        ref_filtered_fragment = f" ref_filtered_prefix_enumerated={raw['ref_filtered_prefix_enumerated_count']}"
        ref_filter = raw["ref_filter"]
        ref_fragment = ""
        if ref_filter:
            ref_fragment = f" ref_filter={','.join(str(ref) for ref in ref_filter)}"
        lines.append(
            f"  - {raw['label']}: {status}; "
            f"exact_count={raw['exact_count']} "
            f"api_prefix_count={raw['api_prefix_count']} "
            f"api_prefix_enumerated={raw['api_prefix_enumerated_count']} "
            f"key={raw['key']}"
            f"{ref_filtered_fragment}"
            f"{ref_fragment}"
            f"{reason_fragment}"
        )
        if (
            raw["exact_count"] == 0
            and raw["ref_filter"]
            and raw["api_prefix_enumerated_count"]
            > raw["ref_filtered_prefix_enumerated_count"]
        ):
            lines.append("      note=API returned matches outside the configured cache refs")
        elif raw["exact_count"] == 0 and raw["api_prefix_enumerated_count"] > 0:
            lines.append("      note=API returned prefix matches, but no exact key matched")
        for entry in raw["entries"]:
            lines.append(
                f"      id={entry['cache_id']} ref={entry['ref']} "
                f"size={human_bytes(entry['size_bytes'])} "
                f"last_accessed_at={entry['last_accessed_at']}"
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
    probes = cache_key_probe_snapshot_from_raw(snapshot)["cache_key_probes"]
    if any(not raw["available"] for raw in probes):
        raise AuditError(
            "cache key probe is unavailable in a successful probe snapshot",
            kind=FailureKind.UNAVAILABLE,
            field="cache_key_probes",
        )
    has_missing = any(not raw["present"] for raw in probes)
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
                (
                    f"  - metadata_unavailable_rows={cleanup.get('metadata_unavailable_count')} "
                    f"metadata_unavailable_bytes={human_bytes(nonnegative_int(cleanup.get('metadata_unavailable_bytes')))}"
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
        "--cleanup-alert",
        action="store_true",
        help="Evaluate configured cleanup-feasibility alert thresholds.",
    )
    parser.add_argument(
        "--cleanup-json-output",
        type=pathlib.Path,
        help="Write the stable cleanup-feasibility JSON snapshot to this path.",
    )
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
    if args.cleanup_alert and not args.cleanup_feasibility:
        raise AuditError(
            "--cleanup-alert requires --cleanup-feasibility",
            kind=FailureKind.ABSENT,
            field="--cleanup-feasibility",
        )
    if args.cleanup_json_output is not None and not args.cleanup_feasibility:
        raise AuditError(
            "--cleanup-json-output requires --cleanup-feasibility",
            kind=FailureKind.ABSENT,
            field="--cleanup-feasibility",
        )
    if args.cleanup_json_output is not None:
        require_text(str(args.cleanup_json_output), "--cleanup-json-output")


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
    cleanup_policy = None
    cleanup_alert_policy = None
    if args.cleanup_feasibility:
        cleanup_policy_path = args.cleanup_policy or discover_cleanup_policy_path()
        cleanup_policy = load_cleanup_policy_path(cleanup_policy_path)
        if args.cleanup_alert:
            cleanup_alert_policy = load_cleanup_alert_policy_path(cleanup_policy_path)
    snapshot = build_snapshot(
        client,
        repo=repo,
        branch=branch,
        snapshot_utc=snapshot_utc,
        cleanup_policy=cleanup_policy,
    )
    if args.cleanup_json_output is not None:
        write_json_snapshot(args.cleanup_json_output, snapshot)
    if args.json:
        print(json.dumps(snapshot, indent=2, sort_keys=True))
    else:
        print(render_text(snapshot))
    if cleanup_alert_policy is not None:
        summary = render_cleanup_alert_summary(snapshot, cleanup_alert_policy)
        findings = cleanup_alert_findings(snapshot, cleanup_alert_policy)
        if args.github_step_summary is not None:
            append_step_summary(args.github_step_summary, summary)
        elif not args.json:
            print()
            print(summary)
        if args.github_annotations:
            for annotation in cleanup_alert_annotations(findings):
                print(annotation)
        if cleanup_alert_has_errors(findings):
            return 1
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
