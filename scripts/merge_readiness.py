#!/usr/bin/env python3
"""Report and comment on PR merge-readiness progress."""

from __future__ import annotations

import argparse
import dataclasses
import datetime
import functools
import json
import os
import pathlib
import re
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv  # noqa: E402


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
GITHUB_API_HEADERS = {
    "Accept": "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
}
GITHUB_API_REDIRECT_HEADERS = {"authorization", "accept", "x-github-api-version"}
TERMINAL_STATES = {"passed", "failed", "stalled"}
NON_BLOCKING_CHECK_CONCLUSIONS = {"success", "skipped", "neutral"}
FAILING_CHECK_CONCLUSIONS = {
    "action_required",
    "cancelled",
    "failure",
    "stale",
    "startup_failure",
    "timed_out",
}
GATE_CONTEXT_VARIANT_KEYS = (
    ("gate_required", "backtester_required"),
    ("gate_iteration", "backtester_iteration"),
)
ACTIONS_BOT_LOGIN = "github-actions[bot]"
ACTIONS_BOT_TYPE = "Bot"


class MergeReadinessError(RuntimeError):
    """Raised when merge-readiness state cannot be resolved safely."""


class GitHubPermissionError(MergeReadinessError):
    """Raised when the token cannot access a required GitHub API operation."""


require_table = functools.partial(_cv.require_table, error_cls=MergeReadinessError)
require_string = functools.partial(_cv.require_string, error_cls=MergeReadinessError)
require_positive_int = functools.partial(_cv.require_positive_int, error_cls=MergeReadinessError)
as_text = _cv.as_text


@dataclasses.dataclass(frozen=True)
class MergeReadinessSettings:
    marker_name: str
    workflow_name: str
    workflow_path: str
    comments_per_page: int
    workflow_runs_per_page: int
    poll_seconds: int
    max_watch_seconds: int


@dataclasses.dataclass(frozen=True)
class RequiredCheckStatus:
    state: str
    completed: int
    total: int
    failed: tuple[str, ...]
    pending: tuple[str, ...]


@dataclasses.dataclass(frozen=True)
class CommentUpdateResult:
    posted: bool
    reason: str
    status: RequiredCheckStatus | None = None


@dataclasses.dataclass(frozen=True)
class GitHubApiPage:
    payload: object
    next_query: dict[str, str] | None


def load_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise MergeReadinessError(f"config missing: {path}") from exc
    except tomllib.TOMLDecodeError as exc:
        raise MergeReadinessError(f"config is invalid TOML: {exc}") from exc
    except OSError as exc:
        raise MergeReadinessError(f"config could not be read: {exc}") from exc
    if not isinstance(data, dict):
        raise MergeReadinessError("config must be a TOML table")
    return data


def require_bool(parent: dict[str, object], key: str, prefix: str) -> bool:
    value = parent.get(key)
    if type(value) is not bool:
        raise MergeReadinessError(f"{prefix}.{key} must be boolean")
    return value


def positive_int(value: object, field: str) -> int:
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return value
    if isinstance(value, str) and value.isdecimal() and int(value) > 0:
        return int(value)
    raise MergeReadinessError(f"{field} must be a positive integer")


def load_ci_provenance(path: pathlib.Path) -> dict[str, object]:
    data = load_toml(path)
    return require_table(data, "ci_provenance", "config")


def merge_settings(path: pathlib.Path) -> MergeReadinessSettings:
    ci_provenance = load_ci_provenance(path)
    api_limits = require_table(ci_provenance, "api_limits", "ci_provenance")
    readiness = require_table(
        ci_provenance, "merge_readiness", "ci_provenance"
    )
    return MergeReadinessSettings(
        marker_name=require_string(
            readiness, "comment_marker", "ci_provenance.merge_readiness"
        ),
        workflow_name=require_string(ci_provenance, "workflow_name", "ci_provenance"),
        workflow_path=require_string(ci_provenance, "workflow_path", "ci_provenance"),
        comments_per_page=require_positive_int(
            api_limits, "run_jobs_per_page", "ci_provenance.api_limits"
        ),
        workflow_runs_per_page=require_positive_int(
            api_limits, "workflow_runs_per_page", "ci_provenance.api_limits"
        ),
        poll_seconds=require_positive_int(
            readiness, "poll_seconds", "ci_provenance.merge_readiness"
        ),
        max_watch_seconds=require_positive_int(
            readiness, "max_watch_seconds", "ci_provenance.merge_readiness"
        ),
    )


def gate_context_variants(
    ci_provenance: dict[str, object],
) -> tuple[tuple[str, str], ...]:
    raw_gate_names = ci_provenance.get("gate_names")
    if raw_gate_names is None:
        return ()
    gate_names = require_table(ci_provenance, "gate_names", "ci_provenance")
    variants: list[tuple[str, str]] = []
    for gate_key, backtester_key in GATE_CONTEXT_VARIANT_KEYS:
        variants.append(
            (
                require_string(gate_names, gate_key, "ci_provenance.gate_names"),
                require_string(gate_names, backtester_key, "ci_provenance.gate_names"),
            )
        )
    return tuple(variants)


def active_gate_context_pair(
    ci_provenance: dict[str, object],
    check_runs: list[dict[str, object]] | None,
) -> tuple[str, str] | None:
    variants = gate_context_variants(ci_provenance)
    if not variants or check_runs is None:
        return None
    by_name = latest_check_runs_by_name(check_runs)

    def variant_key(pair: tuple[str, str]) -> tuple[tuple[datetime.datetime, datetime.datetime, int], int]:
        runs = [by_name[context] for context in pair if context in by_name]
        if not runs:
            timestamp_floor = datetime.datetime.min.replace(tzinfo=datetime.timezone.utc)
            return ((timestamp_floor, timestamp_floor, 0), 0)
        return (max(check_run_sort_key(run) for run in runs), len(runs))

    best_pair = max(variants, key=variant_key)
    if variant_key(best_pair)[1] == 0:
        return variants[0]
    return best_pair


def resolve_required_contexts(
    contexts: tuple[str, ...],
    ci_provenance: dict[str, object],
    check_runs: list[dict[str, object]] | None,
) -> tuple[str, ...]:
    active_pair = active_gate_context_pair(ci_provenance, check_runs)
    variants = gate_context_variants(ci_provenance)
    if active_pair is None or not variants or active_pair == variants[0]:
        return contexts
    required_pair = variants[0]
    replacements = {
        required_pair[0]: active_pair[0],
        required_pair[1]: active_pair[1],
    }
    return tuple(replacements.get(context, context) for context in contexts)


def required_contexts(
    path: pathlib.Path = DEFAULT_CONFIG,
    *,
    check_runs: list[dict[str, object]] | None = None,
) -> tuple[str, ...]:
    ci_provenance = load_ci_provenance(path)
    required_checks = require_table(
        ci_provenance, "required_checks", "ci_provenance"
    )
    contexts: list[str] = []
    for key, raw_entry in required_checks.items():
        if not isinstance(raw_entry, dict):
            raise MergeReadinessError(
                f"ci_provenance.required_checks.{key} must be a table"
            )
        prefix = f"ci_provenance.required_checks.{key}"
        if require_bool(raw_entry, "required", prefix):
            contexts.append(require_string(raw_entry, "context", prefix))
    if not contexts:
        raise MergeReadinessError("ci_provenance.required_checks has no required contexts")
    return resolve_required_contexts(tuple(contexts), ci_provenance, check_runs)


def optional_gate_name(gate_names: dict[str, object], key: str) -> str | None:
    value = gate_names.get(key)
    if value is None:
        return None
    return require_string(gate_names, key, "ci_provenance.gate_names")


def required_context_aliases(path: pathlib.Path = DEFAULT_CONFIG) -> dict[str, tuple[str, ...]]:
    ci_provenance = load_ci_provenance(path)
    gate_names = ci_provenance.get("gate_names")
    if gate_names is None:
        return {}
    if not isinstance(gate_names, dict):
        raise MergeReadinessError("ci_provenance.gate_names must be a table")

    aliases: dict[str, tuple[str, ...]] = {}
    for required_key, iteration_key in (
        ("gate_required", "gate_iteration"),
        ("backtester_required", "backtester_iteration"),
    ):
        required_name = optional_gate_name(gate_names, required_key)
        if required_name is None:
            continue
        names = [required_name]
        iteration_name = optional_gate_name(gate_names, iteration_key)
        if iteration_name is not None and iteration_name not in names:
            names.append(iteration_name)
        aliases[required_name] = tuple(names)
    return aliases


def parse_timestamp(value: object) -> datetime.datetime:
    if not isinstance(value, str) or not value:
        return datetime.datetime.min.replace(tzinfo=datetime.timezone.utc)
    try:
        parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return datetime.datetime.min.replace(tzinfo=datetime.timezone.utc)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=datetime.timezone.utc)
    return parsed.astimezone(datetime.timezone.utc)


def latest_check_runs_by_name(check_runs: list[dict[str, object]]) -> dict[str, dict[str, object]]:
    latest: dict[str, dict[str, object]] = {}
    for run in check_runs:
        name = run.get("name")
        if not isinstance(name, str) or not name:
            continue
        current = latest.get(name)
        if current is None or check_run_sort_key(run) > check_run_sort_key(current):
            latest[name] = run
    return latest


def check_run_sort_key(run: dict[str, object]) -> tuple[datetime.datetime, datetime.datetime, int]:
    return (
        parse_timestamp(run.get("started_at")),
        parse_timestamp(run.get("completed_at")),
        positive_int(run.get("id"), "check run id"),
    )


def latest_check_run_for_required_context(
    *,
    by_name: dict[str, dict[str, object]],
    context: str,
    context_aliases: dict[str, tuple[str, ...]],
) -> dict[str, object] | None:
    for name in context_aliases.get(context, (context,)):
        if name in by_name:
            return by_name[name]
    return None


def evaluate_required_checks(
    contexts: tuple[str, ...],
    check_runs: list[dict[str, object]],
    *,
    context_aliases: dict[str, tuple[str, ...]] | None = None,
) -> RequiredCheckStatus:
    by_name = latest_check_runs_by_name(check_runs)
    aliases = context_aliases or {}
    failed: list[str] = []
    pending: list[str] = []
    completed = 0
    for context in contexts:
        run = latest_check_run_for_required_context(
            by_name=by_name,
            context=context,
            context_aliases=aliases,
        )
        if run is None:
            pending.append(context)
            continue
        status = as_text(run.get("status"))
        conclusion = run.get("conclusion")
        if status != "completed":
            pending.append(context)
            continue
        conclusion_text = as_text(conclusion)
        if conclusion_text in NON_BLOCKING_CHECK_CONCLUSIONS:
            completed += 1
            continue
        if conclusion_text in FAILING_CHECK_CONCLUSIONS:
            failed.append(context)
            continue
        pending.append(context)
    if failed:
        state = "failed"
    elif pending:
        state = "running"
    else:
        state = "passed"
    return RequiredCheckStatus(
        state=state,
        completed=completed,
        total=len(contexts),
        failed=tuple(failed),
        pending=tuple(pending),
    )


def status_summary(status: RequiredCheckStatus) -> str:
    if status.state == "passed":
        return "passed"
    if status.state == "failed":
        return "failed: " + ", ".join(status.failed)
    if status.state == "stalled":
        return "stalled"
    return f"running ({status.completed}/{status.total} done)"


def normalized_redirect_port(parsed: urllib.parse.ParseResult) -> int | None:
    try:
        explicit_port = parsed.port
    except ValueError:
        return None
    if explicit_port is not None:
        return explicit_port
    if parsed.scheme == "https":
        return 443
    if parsed.scheme == "http":
        return 80
    return None


def redirect_preserves_github_api_headers(old_url: str, new_url: str) -> bool:
    old = urllib.parse.urlparse(old_url)
    new = urllib.parse.urlparse(new_url)
    old_host = (old.hostname or "").lower()
    new_host = (new.hostname or "").lower()
    return (
        old.scheme == new.scheme == "https"
        and old_host == new_host
        and normalized_redirect_port(old) == normalized_redirect_port(new)
        and old.username == new.username
        and old.password == new.password
    )


class SafeGitHubRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        if not redirect_preserves_github_api_headers(req.full_url, redirected.full_url):
            for header in tuple(redirected.headers):
                if header.lower() in GITHUB_API_REDIRECT_HEADERS:
                    redirected.remove_header(header)
        return redirected


def open_github_api_request(request: urllib.request.Request, *, timeout: int):
    opener = urllib.request.build_opener(SafeGitHubRedirectHandler())
    return opener.open(request, timeout=timeout)


def next_query_from_link_header(header: object) -> dict[str, str] | None:
    if not isinstance(header, str) or not header:
        return None
    for raw_part in header.split(","):
        parts = [part.strip() for part in raw_part.split(";")]
        if not parts or not parts[0].startswith("<") or not parts[0].endswith(">"):
            continue
        if not any(part.casefold() == 'rel="next"' for part in parts[1:]):
            continue
        parsed = urllib.parse.urlparse(parts[0][1:-1])
        query = dict(urllib.parse.parse_qsl(parsed.query, keep_blank_values=True))
        return query or None
    return None


def next_query_from_payload(payload: object) -> dict[str, str] | None:
    if not isinstance(payload, dict):
        return None
    raw_next = payload.get("next")
    if not isinstance(raw_next, dict):
        return None
    query: dict[str, str] = {}
    for key, value in raw_next.items():
        if isinstance(key, str):
            query[key] = as_text(value)
    return query or None


def github_api_page(
    repo: str,
    token: str,
    path: str,
    query: dict[str, str] | None = None,
    *,
    method: str = "GET",
    data: object = None,
) -> GitHubApiPage:
    url = f"https://api.github.com/repos/{repo}/{path}"
    if query:
        url += "?" + urllib.parse.urlencode(query)
    body = None
    headers = {
        "Authorization": f"Bearer {token}",
        **GITHUB_API_HEADERS,
    }
    if data is not None:
        body = json.dumps(data, sort_keys=True).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=body, headers=headers, method=method)
    try:
        with open_github_api_request(request, timeout=30) as response:
            raw = response.read().decode("utf-8")
            next_query = next_query_from_link_header(response.headers.get("Link"))
    except urllib.error.HTTPError as exc:
        if exc.code in {403, 404}:
            raise GitHubPermissionError(
                f"GitHub API {method} {path} denied with HTTP {exc.code}"
            ) from exc
        raise MergeReadinessError(
            f"GitHub API request failed for {method} {path}: {exc}"
        ) from exc
    except (urllib.error.URLError, UnicodeDecodeError) as exc:
        raise MergeReadinessError(
            f"GitHub API request failed for {method} {path}: {exc}"
        ) from exc
    try:
        payload = json.loads(raw) if raw else {}
    except json.JSONDecodeError as exc:
        raise MergeReadinessError(
            f"GitHub API payload for {method} {path} is invalid JSON"
        ) from exc
    return GitHubApiPage(payload=payload, next_query=next_query)


def github_api_json(
    repo: str,
    token: str,
    path: str,
    query: dict[str, str] | None = None,
    *,
    method: str = "GET",
    data: object = None,
) -> object:
    return github_api_page(
        repo,
        token,
        path,
        query,
        method=method,
        data=data,
    ).payload


def require_env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise MergeReadinessError(f"{name} must be set")
    return value


def require_json_object(payload: object, label: str) -> dict[str, object]:
    if not isinstance(payload, dict):
        raise MergeReadinessError(f"{label} payload is malformed")
    return payload


def pull_request(
    *,
    repo: str,
    token: str,
    pr_number: int,
    api_json=github_api_json,
) -> dict[str, object]:
    return require_json_object(
        api_json(repo, token, f"pulls/{pr_number}"),
        "pull request",
    )


def pull_request_head_sha(pr: dict[str, object]) -> str:
    head = require_table(pr, "head", "pull request")
    sha = require_string(head, "sha", "pull request.head")
    if SHA_RE.fullmatch(sha) is None:
        raise MergeReadinessError("pull request head SHA is malformed")
    return sha


def pull_request_repo_names(pr: dict[str, object]) -> tuple[str, str]:
    head = require_table(pr, "head", "pull request")
    base = require_table(pr, "base", "pull request")
    head_repo = require_table(head, "repo", "pull request.head")
    base_repo = require_table(base, "repo", "pull request.base")
    return (
        require_string(head_repo, "full_name", "pull request.head.repo"),
        require_string(base_repo, "full_name", "pull request.base.repo"),
    )


def is_fork_pull_request(pr: dict[str, object]) -> bool:
    head_repo, base_repo = pull_request_repo_names(pr)
    return head_repo != base_repo


def check_runs_for_sha(
    *,
    repo: str,
    token: str,
    sha: str,
    settings: MergeReadinessSettings,
    api_json=github_api_json,
) -> list[dict[str, object]]:
    payload = require_json_object(
        api_json(
            repo,
            token,
            f"commits/{sha}/check-runs",
            {
                "per_page": str(settings.comments_per_page),
                "filter": "latest",
            },
        ),
        "check runs",
    )
    raw_runs = payload.get("check_runs")
    if not isinstance(raw_runs, list):
        raise MergeReadinessError("check runs payload is malformed")
    runs: list[dict[str, object]] = []
    for run in raw_runs:
        if not isinstance(run, dict):
            raise MergeReadinessError("check runs payload is malformed")
        runs.append(run)
    return runs


def sticky_comment_payloads(payload: object) -> list[dict[str, object]]:
    if isinstance(payload, list):
        raw_comments = payload
    elif isinstance(payload, dict):
        raw_comments = payload.get("comments")
    else:
        raw_comments = None
    if not isinstance(raw_comments, list):
        raise MergeReadinessError("issue comments payload is malformed")
    comments: list[dict[str, object]] = []
    for comment in raw_comments:
        if not isinstance(comment, dict):
            raise MergeReadinessError("issue comments payload is malformed")
        comments.append(comment)
    return comments


def is_actions_bot_comment(comment: dict[str, object]) -> bool:
    user = comment.get("user")
    return (
        isinstance(user, dict)
        and user.get("login") == ACTIONS_BOT_LOGIN
        and user.get("type") == ACTIONS_BOT_TYPE
    )


def comment_marker(
    *,
    marker_name: str,
    head_sha: str,
    workflow: str,
    run_id: int,
    run_attempt: int,
    state: str,
) -> str:
    payload = {
        "head_sha": head_sha,
        "run_attempt": run_attempt,
        "run_id": run_id,
        "state": state,
        "workflow": workflow,
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":"))
    return f"<!-- {marker_name}: {encoded} -->"


def marker_pattern(marker_name: str) -> re.Pattern[str]:
    return re.compile(rf"<!--\s*{re.escape(marker_name)}:\s*(\{{.*?\}})\s*-->")


def parse_marker(body: object, marker_name: str) -> dict[str, object] | None:
    if not isinstance(body, str):
        return None
    match = marker_pattern(marker_name).search(body)
    if match is None:
        return None
    try:
        payload = json.loads(match.group(1))
    except json.JSONDecodeError:
        return None
    return payload if isinstance(payload, dict) else None


def find_sticky_comment(
    comments: list[dict[str, object]],
    marker_name: str,
) -> tuple[dict[str, object], dict[str, object]] | None:
    for comment in comments:
        if not is_actions_bot_comment(comment):
            continue
        metadata = parse_marker(comment.get("body"), marker_name)
        if metadata is not None:
            return comment, metadata
    return None


def comment_body_for_status(
    status: RequiredCheckStatus,
    *,
    marker_name: str,
    head_sha: str,
    workflow: str,
    run_id: int,
    run_attempt: int,
) -> str:
    marker = comment_marker(
        marker_name=marker_name,
        head_sha=head_sha,
        workflow=workflow,
        run_id=run_id,
        run_attempt=run_attempt,
        state=status.state,
    )
    if status.state == "passed":
        headline = "✅ all required checks passed — safe to merge"
    elif status.state == "failed":
        headline = "❌ failed: " + ", ".join(status.failed)
    elif status.state == "stalled":
        headline = "⚠️ CI stalled"
    else:
        headline = f"⏳ CI running — {status.completed}/{status.total} checks done"
    return f"{marker}\n{headline}\n"


def list_issue_comments(
    *,
    repo: str,
    token: str,
    pr_number: int,
    settings: MergeReadinessSettings,
    api_json=github_api_json,
) -> list[dict[str, object]]:
    path = f"issues/{pr_number}/comments"
    query = {"per_page": str(settings.comments_per_page)}
    comments: list[dict[str, object]] = []
    while True:
        if api_json is github_api_json:
            page = github_api_page(repo, token, path, query)
            payload = page.payload
            next_query = page.next_query
        else:
            payload = api_json(repo, token, path, query)
            next_query = next_query_from_payload(payload)
        comments.extend(sticky_comment_payloads(payload))
        if next_query is None:
            return comments
        query = next_query


def upsert_sticky_comment(
    *,
    repo: str,
    token: str,
    pr_number: int,
    settings: MergeReadinessSettings,
    body: str,
    api_json=github_api_json,
) -> None:
    comments = list_issue_comments(
        repo=repo,
        token=token,
        pr_number=pr_number,
        settings=settings,
        api_json=api_json,
    )
    existing = find_sticky_comment(comments, settings.marker_name)
    if existing is None:
        api_json(
            repo,
            token,
            f"issues/{pr_number}/comments",
            method="POST",
            data={"body": body},
        )
        return
    comment, _metadata = existing
    comment_id = positive_int(comment.get("id"), "issue comment id")
    api_json(
        repo,
        token,
        f"issues/comments/{comment_id}",
        method="PATCH",
        data={"body": body},
    )


def stalled_status() -> RequiredCheckStatus:
    return RequiredCheckStatus(
        state="stalled",
        completed=0,
        total=0,
        failed=(),
        pending=(),
    )


def resolve_status(
    *,
    repo: str,
    token: str,
    pr_number: int,
    config_path: pathlib.Path,
    head_sha: str | None = None,
    include_sticky_state: bool = False,
    api_json=github_api_json,
) -> RequiredCheckStatus:
    settings = merge_settings(config_path)
    pr = pull_request(repo=repo, token=token, pr_number=pr_number, api_json=api_json)
    current_head_sha = pull_request_head_sha(pr)
    sha = head_sha or current_head_sha
    if SHA_RE.fullmatch(sha) is None:
        raise MergeReadinessError("head SHA is malformed")
    if sha != current_head_sha:
        raise MergeReadinessError("stale head SHA does not match current PR head")
    check_runs = check_runs_for_sha(
        repo=repo,
        token=token,
        sha=sha,
        settings=settings,
        api_json=api_json,
    )
    status = evaluate_required_checks(
        required_contexts(config_path, check_runs=check_runs),
        check_runs,
    )
    if status.state != "running" or not include_sticky_state:
        return status
    try:
        comments = list_issue_comments(
            repo=repo,
            token=token,
            pr_number=pr_number,
            settings=settings,
            api_json=api_json,
        )
    except GitHubPermissionError:
        return status
    existing = find_sticky_comment(comments, settings.marker_name)
    if existing is None:
        return status
    _comment, metadata = existing
    if metadata.get("head_sha") == sha and metadata.get("state") == "stalled":
        return stalled_status()
    return status


def update_progress_comment(
    *,
    repo: str,
    token: str,
    pr_number: int,
    config_path: pathlib.Path,
    head_sha: str,
    workflow: str,
    run_id: int,
    run_attempt: int,
    api_json=github_api_json,
) -> CommentUpdateResult:
    settings = merge_settings(config_path)
    pr = pull_request(repo=repo, token=token, pr_number=pr_number, api_json=api_json)
    current_head_sha = pull_request_head_sha(pr)
    if head_sha != current_head_sha:
        return CommentUpdateResult(False, "stale head SHA; PR advanced", stalled_status())
    check_runs = check_runs_for_sha(
        repo=repo,
        token=token,
        sha=head_sha,
        settings=settings,
        api_json=api_json,
    )
    status = evaluate_required_checks(
        required_contexts(config_path, check_runs=check_runs),
        check_runs,
    )
    if is_fork_pull_request(pr):
        return CommentUpdateResult(
            False,
            "fork PR; fallback to merge_readiness.py status",
            status,
        )
    body = comment_body_for_status(
        status,
        marker_name=settings.marker_name,
        head_sha=head_sha,
        workflow=workflow,
        run_id=run_id,
        run_attempt=run_attempt,
    )
    try:
        upsert_sticky_comment(
            repo=repo,
            token=token,
            pr_number=pr_number,
            settings=settings,
            body=body,
            api_json=api_json,
        )
    except (GitHubPermissionError, PermissionError):
        return CommentUpdateResult(
            False,
            "pull-requests: write unavailable; fallback to merge_readiness.py status",
            status,
        )
    return CommentUpdateResult(True, status_summary(status), status)


def watch_progress_comment(
    *,
    repo: str,
    token: str,
    pr_number: int,
    config_path: pathlib.Path,
    head_sha: str,
    workflow: str,
    run_id: int,
    run_attempt: int,
    api_json=github_api_json,
) -> CommentUpdateResult:
    settings = merge_settings(config_path)
    deadline = time.monotonic() + settings.max_watch_seconds
    last_result: CommentUpdateResult | None = None
    while True:
        last_result = update_progress_comment(
            repo=repo,
            token=token,
            pr_number=pr_number,
            config_path=config_path,
            head_sha=head_sha,
            workflow=workflow,
            run_id=run_id,
            run_attempt=run_attempt,
            api_json=api_json,
        )
        if not last_result.posted:
            return last_result
        if last_result.status is None or last_result.status.state in TERMINAL_STATES:
            return last_result
        if time.monotonic() >= deadline:
            return last_result
        time.sleep(settings.poll_seconds)


def workflow_runs_path(workflow_path: str) -> str:
    workflow_file = pathlib.PurePosixPath(workflow_path).name
    return f"actions/workflows/{workflow_file}/runs"


def latest_run_guard_passes(
    *,
    repo: str,
    token: str,
    workflow_run: dict[str, object],
    settings: MergeReadinessSettings,
    api_json=github_api_json,
) -> tuple[bool, str]:
    head_sha = require_string(workflow_run, "head_sha", "workflow_run")
    run_id = positive_int(workflow_run.get("id"), "workflow_run id")
    run_attempt = positive_int(workflow_run.get("run_attempt"), "workflow_run run_attempt")
    payload = require_json_object(
        api_json(
            repo,
            token,
            workflow_runs_path(settings.workflow_path),
            {
                "event": "pull_request",
                "head_sha": head_sha,
                "per_page": str(settings.workflow_runs_per_page),
            },
        ),
        "workflow runs",
    )
    raw_runs = payload.get("workflow_runs")
    if not isinstance(raw_runs, list):
        raise MergeReadinessError("workflow runs payload is malformed")
    for raw_run in raw_runs:
        if not isinstance(raw_run, dict):
            raise MergeReadinessError("workflow runs payload is malformed")
        if as_text(raw_run.get("head_sha")) != head_sha:
            continue
        if as_text(raw_run.get("path")) != settings.workflow_path:
            continue
        candidate_id = positive_int(raw_run.get("id"), "workflow run id")
        candidate_attempt = positive_int(
            raw_run.get("run_attempt"), "workflow run run_attempt"
        )
        if candidate_id > run_id:
            return False, f"newer run {candidate_id} exists for head"
        if candidate_id == run_id and candidate_attempt > run_attempt:
            return False, f"newer run attempt {candidate_attempt} exists for head"
    return True, "latest run for head"


def workflow_run_pr_numbers(workflow_run: dict[str, object]) -> tuple[int, ...]:
    raw_prs = workflow_run.get("pull_requests")
    if not isinstance(raw_prs, list):
        return ()
    numbers: list[int] = []
    for raw_pr in raw_prs:
        if not isinstance(raw_pr, dict):
            continue
        value = raw_pr.get("number")
        if isinstance(value, int) and value > 0:
            numbers.append(value)
    return tuple(numbers)


def marker_matches_workflow_run(
    metadata: dict[str, object],
    workflow_run: dict[str, object],
    *,
    settings: MergeReadinessSettings,
) -> bool:
    return (
        metadata.get("head_sha") == workflow_run.get("head_sha")
        and metadata.get("workflow") == settings.workflow_path
        and metadata.get("run_id") == positive_int(workflow_run.get("id"), "workflow_run id")
        and metadata.get("run_attempt")
        == positive_int(workflow_run.get("run_attempt"), "workflow_run run_attempt")
    )


def finalize_stalled_comment(
    *,
    repo: str,
    token: str,
    workflow_run: dict[str, object],
    config_path: pathlib.Path,
    api_json=github_api_json,
) -> CommentUpdateResult:
    settings = merge_settings(config_path)
    if as_text(workflow_run.get("path")) != settings.workflow_path:
        return CommentUpdateResult(False, "non-CI workflow run", stalled_status())
    if as_text(workflow_run.get("event")) != "pull_request":
        return CommentUpdateResult(False, "non-PR workflow run", stalled_status())
    pr_numbers = workflow_run_pr_numbers(workflow_run)
    if not pr_numbers:
        return CommentUpdateResult(False, "workflow run has no pull request", stalled_status())
    pr_number = pr_numbers[0]
    pr = pull_request(repo=repo, token=token, pr_number=pr_number, api_json=api_json)
    current_head_sha = pull_request_head_sha(pr)
    if current_head_sha != workflow_run.get("head_sha"):
        return CommentUpdateResult(False, "stale head SHA; PR advanced", stalled_status())
    try:
        latest, reason = latest_run_guard_passes(
            repo=repo,
            token=token,
            workflow_run=workflow_run,
            settings=settings,
            api_json=api_json,
        )
    except GitHubPermissionError as exc:
        return CommentUpdateResult(False, str(exc), stalled_status())
    if not latest:
        return CommentUpdateResult(False, reason, stalled_status())
    comments = list_issue_comments(
        repo=repo,
        token=token,
        pr_number=pr_number,
        settings=settings,
        api_json=api_json,
    )
    existing = find_sticky_comment(comments, settings.marker_name)
    if existing is None:
        return CommentUpdateResult(False, "sticky comment missing", stalled_status())
    comment, metadata = existing
    if not marker_matches_workflow_run(metadata, workflow_run, settings=settings):
        return CommentUpdateResult(
            False,
            "sticky marker does not match completed run",
            stalled_status(),
        )
    if metadata.get("state") != "running":
        return CommentUpdateResult(False, "sticky comment already terminal", stalled_status())
    body = comment_body_for_status(
        stalled_status(),
        marker_name=settings.marker_name,
        head_sha=current_head_sha,
        workflow=settings.workflow_path,
        run_id=positive_int(workflow_run.get("id"), "workflow_run id"),
        run_attempt=positive_int(workflow_run.get("run_attempt"), "workflow_run run_attempt"),
    )
    comment_id = positive_int(comment.get("id"), "issue comment id")
    try:
        api_json(
            repo,
            token,
            f"issues/comments/{comment_id}",
            method="PATCH",
            data={"body": body},
        )
    except (GitHubPermissionError, PermissionError):
        return CommentUpdateResult(
            False,
            "pull-requests: write unavailable; fallback to merge_readiness.py status",
            stalled_status(),
        )
    return CommentUpdateResult(True, "stalled", stalled_status())


def load_event(path: pathlib.Path) -> dict[str, object]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise MergeReadinessError(f"event payload missing: {path}") from exc
    except json.JSONDecodeError as exc:
        raise MergeReadinessError(f"event payload is invalid JSON: {exc}") from exc
    except OSError as exc:
        raise MergeReadinessError(f"event payload could not be read: {exc}") from exc
    if not isinstance(payload, dict):
        raise MergeReadinessError("event payload must be a JSON object")
    return payload


def workflow_run_from_event(path: pathlib.Path) -> dict[str, object]:
    event = load_event(path)
    return require_table(event, "workflow_run", "event")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(prog="merge_readiness.py")
    subparsers = root.add_subparsers(dest="mode")

    def add_common(target: argparse.ArgumentParser) -> None:
        target.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
        target.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
        target.add_argument("--token", default=os.environ.get("GITHUB_TOKEN"))

    status = subparsers.add_parser("status")
    status.add_argument("pr", type=int)
    add_common(status)

    comment = subparsers.add_parser("comment")
    comment.add_argument("pr", type=int)
    comment.add_argument("--head-sha", default=os.environ.get("GITHUB_SHA"))
    comment.add_argument("--workflow")
    comment.add_argument("--run-id", default=os.environ.get("GITHUB_RUN_ID"))
    comment.add_argument("--run-attempt", default=os.environ.get("GITHUB_RUN_ATTEMPT"))
    comment.add_argument("--watch", action="store_true")
    add_common(comment)

    finalizer = subparsers.add_parser("finalize-stalled")
    finalizer.add_argument(
        "--event-path",
        type=pathlib.Path,
        default=pathlib.Path(os.environ.get("GITHUB_EVENT_PATH", "")),
    )
    add_common(finalizer)
    return root


def normalize_argv(argv: list[str]) -> list[str]:
    if argv and argv[0].isdecimal():
        return ["status", *argv]
    return argv


def require_repo_and_token(args: argparse.Namespace) -> tuple[str, str]:
    repo = args.repo
    token = args.token
    if not repo:
        repo = require_env("GITHUB_REPOSITORY")
    if not token:
        token = require_env("GITHUB_TOKEN")
    return repo, token


def run_status(args: argparse.Namespace) -> int:
    repo, token = require_repo_and_token(args)
    status = resolve_status(
        repo=repo,
        token=token,
        pr_number=args.pr,
        config_path=args.config,
        include_sticky_state=True,
    )
    print(status_summary(status))
    return 0


def run_comment(args: argparse.Namespace) -> int:
    repo, token = require_repo_and_token(args)
    settings = merge_settings(args.config)
    head_sha = args.head_sha
    if not head_sha or SHA_RE.fullmatch(head_sha) is None:
        raise MergeReadinessError("--head-sha must be a 40-character lowercase hex SHA")
    run_id = positive_int(args.run_id, "run id")
    run_attempt = positive_int(args.run_attempt, "run attempt")
    workflow = args.workflow or settings.workflow_path
    updater = watch_progress_comment if args.watch else update_progress_comment
    result = updater(
        repo=repo,
        token=token,
        pr_number=args.pr,
        config_path=args.config,
        head_sha=head_sha,
        workflow=workflow,
        run_id=run_id,
        run_attempt=run_attempt,
    )
    if result.status is not None:
        print(status_summary(result.status))
    if not result.posted:
        print(f"comment_skipped={result.reason}")
    return 0


def run_finalize_stalled(args: argparse.Namespace) -> int:
    repo, token = require_repo_and_token(args)
    if not str(args.event_path):
        raise MergeReadinessError("GITHUB_EVENT_PATH must be set")
    result = finalize_stalled_comment(
        repo=repo,
        token=token,
        workflow_run=workflow_run_from_event(args.event_path),
        config_path=args.config,
    )
    print(result.reason)
    return 0


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv[1:]
    args = parser().parse_args(normalize_argv(argv))
    try:
        if args.mode == "status":
            return run_status(args)
        if args.mode == "comment":
            return run_comment(args)
        if args.mode == "finalize-stalled":
            return run_finalize_stalled(args)
        parser().print_help(sys.stderr)
        return 2
    except (MergeReadinessError, PermissionError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
