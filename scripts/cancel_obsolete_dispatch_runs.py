#!/usr/bin/env python3
"""Cancel older active same-branch manual CI dispatch runs.

This script is intended for the default-branch workflow_run watchdog. It only
uses GitHub workflow-run metadata and never checks out or executes code from
the branch whose CI run triggered the watchdog.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import functools
import json
import os
import pathlib
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterable
from typing import Any


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import config_validators as _cv  # noqa: E402


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
GITHUB_API_HEADERS = {
    "Accept": "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
}


class DispatchCancelError(RuntimeError):
    """Raised when cancellation cannot be evaluated safely."""


require_string = functools.partial(_cv.require_string, error_cls=DispatchCancelError)


class GitHubApiError(DispatchCancelError):
    def __init__(self, *, method: str, path: str, code: int, body: str) -> None:
        self.method = method
        self.path = path
        self.code = code
        super().__init__(f"GitHub API {method} {path} failed with HTTP {code}: {body}")


@dataclasses.dataclass(frozen=True)
class DispatchCancelConfig:
    workflow_name: str
    workflow_path: str
    workflow_event: str
    run_name_full: str
    run_name_iteration: str
    active_statuses: frozenset[str]
    workflow_runs_per_page: int
    max_pages: int


@dataclasses.dataclass(frozen=True)
class CurrentRun:
    run_id: int
    branch: str
    created_at: dt.datetime
    run_class: str


class GitHubClient:
    def __init__(self, *, repo: str, token: str) -> None:
        self.repo = repo
        self.token = token

    def get_json(self, path: str, params: dict[str, str]) -> dict[str, Any]:
        return self._request_json("GET", path, params=params)

    def cancel_run(self, run_id: int) -> str:
        try:
            self._request_json("POST", f"actions/runs/{run_id}/force-cancel", params={})
        except GitHubApiError as exc:
            if exc.code in (409, 404, 422):
                return "conflict"
            raise
        return "cancelled"

    def _request_json(self, method: str, path: str, *, params: dict[str, str]) -> dict[str, Any]:
        query = urllib.parse.urlencode(params)
        url = f"https://api.github.com/repos/{self.repo}/{path}"
        if query:
            url = f"{url}?{query}"
        request = urllib.request.Request(
            url,
            data=b"" if method == "POST" else None,
            method=method,
            headers={
                **GITHUB_API_HEADERS,
                "Authorization": f"Bearer {self.token}",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                payload = response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", errors="replace")[:500]
            raise GitHubApiError(method=method, path=path, code=exc.code, body=body) from exc
        except (urllib.error.URLError, OSError) as exc:
            raise DispatchCancelError(f"GitHub API {method} {path} network error: {exc}") from exc
        if not payload:
            return {}
        try:
            parsed = json.loads(payload)
        except json.JSONDecodeError as exc:
            raise DispatchCancelError(f"GitHub API {method} {path} returned invalid JSON") from exc
        if not isinstance(parsed, dict):
            raise DispatchCancelError(f"GitHub API {method} {path} returned a non-object payload")
        return parsed


def as_text(value: object) -> str:
    return value if isinstance(value, str) else ""


def require_table(data: dict[str, object], key: str, section: str) -> dict[str, object]:
    value = data.get(key)
    if not isinstance(value, dict):
        raise DispatchCancelError(f"{section} must define [{key}]")
    return value


def require_positive_int(data: dict[str, object], key: str, section: str) -> int:
    value = data.get(key)
    if not isinstance(value, int) or value <= 0:
        raise DispatchCancelError(f"{section}.{key} must be a positive integer")
    return value


def load_config(path: pathlib.Path = DEFAULT_CONFIG) -> DispatchCancelConfig:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    ci_provenance = require_table(data, "ci_provenance", "ci/github-actions-runners.toml")
    dispatch_cancel = require_table(data, "dispatch_cancel", "ci/github-actions-runners.toml")
    workflow_name = require_string(ci_provenance, "workflow_name", "ci_provenance")
    workflow_path = require_string(ci_provenance, "workflow_path", "ci_provenance")
    workflow_event = require_string(dispatch_cancel, "workflow_event", "dispatch_cancel")
    dispatch = require_table(ci_provenance, "dispatch", "ci_provenance")
    run_name_full = require_string(dispatch, "run_name_full", "ci_provenance.dispatch")
    run_name_iteration = require_string(dispatch, "run_name_iteration", "ci_provenance.dispatch")
    if run_name_full == run_name_iteration:
        raise DispatchCancelError("ci_provenance.dispatch run_name_full and run_name_iteration must differ")
    active_statuses_raw = dispatch_cancel.get("active_statuses")
    if (
        not isinstance(active_statuses_raw, list)
        or not active_statuses_raw
        or not all(isinstance(status, str) and status for status in active_statuses_raw)
    ):
        raise DispatchCancelError("dispatch_cancel.active_statuses must be a non-empty string list")
    return DispatchCancelConfig(
        workflow_name=workflow_name,
        workflow_path=workflow_path,
        workflow_event=workflow_event,
        run_name_full=run_name_full,
        run_name_iteration=run_name_iteration,
        active_statuses=frozenset(active_statuses_raw),
        workflow_runs_per_page=require_positive_int(
            dispatch_cancel, "workflow_runs_per_page", "dispatch_cancel"
        ),
        max_pages=require_positive_int(dispatch_cancel, "max_pages", "dispatch_cancel"),
    )


def parse_timestamp(value: str, field: str) -> dt.datetime:
    if not value:
        raise DispatchCancelError(f"workflow_run.{field} is missing")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise DispatchCancelError(f"workflow_run.{field} is not an ISO timestamp: {value!r}") from exc
    if parsed.tzinfo is None:
        raise DispatchCancelError(f"workflow_run.{field} must include a timezone")
    return parsed


def workflow_path_matches(run: dict[str, object], config: DispatchCancelConfig) -> bool:
    path = as_text(run.get("path"))
    return path == config.workflow_path


def run_display_title(run: dict[str, object]) -> str:
    title = as_text(run.get("displayTitle"))
    if title:
        return title
    return as_text(run.get("display_title"))


def dispatch_run_class(run: dict[str, object], config: DispatchCancelConfig) -> str | None:
    title = run_display_title(run)
    if title == config.run_name_full:
        return "full"
    if title == config.run_name_iteration:
        return "iteration"
    return None


def current_run_from_payload(
    payload: dict[str, object], config: DispatchCancelConfig
) -> tuple[CurrentRun | None, str]:
    run = payload.get("workflow_run")
    if not isinstance(run, dict):
        raise DispatchCancelError("event payload must contain workflow_run object")
    if as_text(run.get("event")) != config.workflow_event:
        return None, "not configured workflow event"
    if not workflow_path_matches(run, config):
        return None, "not configured workflow path"
    branch = as_text(run.get("head_branch"))
    if not branch:
        return None, "workflow run has no branch"
    run_id = run.get("id")
    if not isinstance(run_id, int) or isinstance(run_id, bool):
        raise DispatchCancelError("workflow_run.id must be an integer")
    created_at = parse_timestamp(as_text(run.get("created_at")), "created_at")
    return CurrentRun(run_id=run_id, branch=branch, created_at=created_at, run_class=""), "selected"


def candidate_runs(
    client: GitHubClient,
    config: DispatchCancelConfig,
    branch: str,
) -> Iterable[dict[str, object]]:
    for page in range(1, config.max_pages + 1):
        payload = client.get_json(
            "actions/runs",
            {
                "branch": branch,
                "event": config.workflow_event,
                "per_page": str(config.workflow_runs_per_page),
                "page": str(page),
            },
        )
        runs = payload.get("workflow_runs")
        if not isinstance(runs, list):
            raise DispatchCancelError("actions/runs response missing workflow_runs list")
        final_page_was_full = len(runs) >= config.workflow_runs_per_page
        for run in runs:
            if isinstance(run, dict):
                yield run
        if not final_page_was_full:
            break
        if page == config.max_pages:
            print(
                "warning: dispatch cancel scan reached "
                f"max_pages={config.max_pages} with a full final page; "
                "older active runs may remain uncancelled",
                file=sys.stderr,
            )


def obsolete_run_ids(
    runs: Iterable[dict[str, object]],
    *,
    current: CurrentRun,
    config: DispatchCancelConfig,
) -> list[int]:
    obsolete: list[tuple[dt.datetime, int]] = []
    for run in runs:
        run_id = run.get("id")
        if not isinstance(run_id, int) or isinstance(run_id, bool) or run_id == current.run_id:
            continue
        if as_text(run.get("event")) != config.workflow_event:
            continue
        if not workflow_path_matches(run, config):
            continue
        if dispatch_run_class(run, config) != current.run_class:
            continue
        if as_text(run.get("head_branch")) != current.branch:
            continue
        if as_text(run.get("status")) not in config.active_statuses:
            continue
        if run.get("conclusion") not in (None, ""):
            continue
        created_at = parse_timestamp(as_text(run.get("created_at")), "created_at")
        if created_at > current.created_at:
            continue
        if created_at == current.created_at and run_id >= current.run_id:
            continue
        obsolete.append((created_at, run_id))
    obsolete.sort()
    return [run_id for _, run_id in obsolete]


def handle_payload(
    payload: dict[str, object],
    *,
    config: DispatchCancelConfig,
    client: GitHubClient,
    dry_run: bool,
) -> dict[str, object]:
    current, reason = current_run_from_payload(payload, config)
    if current is None:
        return {"ignored": True, "reason": reason}
    try:
        current_run = client.get_json(f"actions/runs/{current.run_id}", {})
    except (DispatchCancelError, GitHubApiError) as exc:
        print(f"warning: could not rehydrate current workflow run {current.run_id}: {exc}", file=sys.stderr)
        return {"ignored": True, "reason": "could not rehydrate current workflow run"}
    if as_text(current_run.get("event")) != config.workflow_event:
        return {"ignored": True, "reason": "rehydrated run is not configured workflow event"}
    if not workflow_path_matches(current_run, config):
        return {"ignored": True, "reason": "rehydrated run is not configured workflow path"}
    current_class = dispatch_run_class(current_run, config)
    if current_class is None:
        return {"ignored": True, "reason": "current dispatch run has no configured class marker"}
    current = dataclasses.replace(current, run_class=current_class)

    stale_ids = obsolete_run_ids(candidate_runs(client, config, current.branch), current=current, config=config)
    cancelled: list[int] = []
    conflicts: list[int] = []
    for run_id in stale_ids:
        if dry_run:
            continue
        result = client.cancel_run(run_id)
        if result == "cancelled":
            cancelled.append(run_id)
        elif result == "conflict":
            conflicts.append(run_id)
        else:
            raise DispatchCancelError(f"unexpected cancellation result for run {run_id}: {result}")

    return {
        "ignored": False,
        "branch": current.branch,
        "current_run_id": current.run_id,
        "obsolete_run_ids": stale_ids,
        "cancelled_run_ids": cancelled,
        "conflict_run_ids": conflicts,
        "dry_run": dry_run,
    }


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    parser.add_argument("--event-path", type=pathlib.Path, default=os.environ.get("GITHUB_EVENT_PATH"))
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    parser.add_argument("--token-env", default="GITHUB_TOKEN")
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.event_path is None:
        raise DispatchCancelError("GITHUB_EVENT_PATH is required")
    if not args.repo:
        raise DispatchCancelError("GITHUB_REPOSITORY is required")
    token = os.environ.get(args.token_env)
    if not token and not args.dry_run:
        raise DispatchCancelError(f"{args.token_env} is required")
    config = load_config(args.config)
    try:
        payload = json.loads(args.event_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        raise DispatchCancelError("event payload is invalid JSON") from exc
    if not isinstance(payload, dict):
        raise DispatchCancelError("event payload must be a JSON object")
    client = GitHubClient(repo=args.repo, token=token or "")
    summary = handle_payload(payload, config=config, client=client, dry_run=args.dry_run)
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except DispatchCancelError as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
