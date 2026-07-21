#!/usr/bin/env python3
"""Admit only exact-current-main advisory evidence and cancel stale runs."""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping, Sequence
from typing import Protocol


GITHUB_API_ORIGIN = "https://api.github.com"


@dataclasses.dataclass(frozen=True)
class Config:
    api_version: str
    branch: str
    workflow: str
    request_timeout_seconds: int
    runs_per_page: int
    cancel_poll_attempts: int
    cancel_poll_interval_seconds: int
    active_statuses: tuple[str, ...]


@dataclasses.dataclass(frozen=True)
class WorkflowRun:
    run_id: int
    head_sha: str
    event: str
    status: str


@dataclasses.dataclass(frozen=True)
class ReconcileResult:
    cancelled_run_ids: tuple[int, ...]


class SupersededRun(RuntimeError):
    """The invoking run no longer represents current main."""


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        request: urllib.request.Request,
        file_pointer: object,
        code: int,
        message: str,
        headers: object,
        new_url: str,
    ) -> None:
        return None


class ActionsClient(Protocol):
    def current_branch_sha(self) -> str: ...

    def active_push_runs(self) -> list[WorkflowRun]: ...

    def cancel_self(self, run_id: int) -> None: ...

    def cancel_and_confirm(self, run_id: int) -> None: ...


def _require_mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{label} must be a table")
    return value


def _require_string(document: Mapping[str, object], key: str) -> str:
    value = document.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{key} must be a non-empty string")
    return value


def load_config(path: pathlib.Path) -> Config:
    document = _require_mapping(
        tomllib.loads(path.read_text(encoding="utf-8")), "config"
    )
    expected = {
        "schema_version",
        "api_version",
        "branch",
        "workflow",
        "request_timeout_seconds",
        "runs_per_page",
        "cancel_poll_attempts",
        "cancel_poll_interval_seconds",
        "active_statuses",
    }
    unknown = set(document) - expected
    if unknown:
        raise ValueError(f"unknown config keys: {', '.join(sorted(unknown))}")
    schema_version = document.get("schema_version")
    if isinstance(schema_version, bool) or schema_version != 1:
        raise ValueError("schema_version must be 1")
    timeout = document.get("request_timeout_seconds")
    if isinstance(timeout, bool) or not isinstance(timeout, int) or timeout <= 0:
        raise ValueError("request_timeout_seconds must be a positive integer")
    runs_per_page = document.get("runs_per_page")
    if (
        isinstance(runs_per_page, bool)
        or not isinstance(runs_per_page, int)
        or not 1 <= runs_per_page <= 100
    ):
        raise ValueError("runs_per_page must be an integer from 1 through 100")
    cancel_poll_attempts = document.get("cancel_poll_attempts")
    if (
        isinstance(cancel_poll_attempts, bool)
        or not isinstance(cancel_poll_attempts, int)
        or cancel_poll_attempts <= 0
    ):
        raise ValueError("cancel_poll_attempts must be a positive integer")
    cancel_poll_interval_seconds = document.get("cancel_poll_interval_seconds")
    if (
        isinstance(cancel_poll_interval_seconds, bool)
        or not isinstance(cancel_poll_interval_seconds, int)
        or cancel_poll_interval_seconds <= 0
    ):
        raise ValueError("cancel_poll_interval_seconds must be a positive integer")
    statuses = document.get("active_statuses")
    if (
        not isinstance(statuses, list)
        or not statuses
        or any(not isinstance(status, str) or not status for status in statuses)
        or len(set(statuses)) != len(statuses)
    ):
        raise ValueError("active_statuses must contain unique non-empty strings")
    return Config(
        api_version=_require_string(document, "api_version"),
        branch=_require_string(document, "branch"),
        workflow=_require_string(document, "workflow"),
        request_timeout_seconds=timeout,
        runs_per_page=runs_per_page,
        cancel_poll_attempts=cancel_poll_attempts,
        cancel_poll_interval_seconds=cancel_poll_interval_seconds,
        active_statuses=tuple(str(status) for status in statuses),
    )


class GitHubActionsClient:
    def __init__(
        self,
        *,
        config: Config,
        repository: str,
        token: str,
    ) -> None:
        repository_parts = repository.split("/")
        if len(repository_parts) != 2 or not all(repository_parts):
            raise ValueError("repository must use owner/name form")
        if not token:
            raise ValueError("GITHUB_TOKEN is required")
        self.config = config
        self.repository = repository
        self._headers = {
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": config.api_version,
        }
        self._opener = urllib.request.build_opener(_NoRedirectHandler())

    def _request_json(
        self,
        method: str,
        path_or_url: str,
        *,
        query: Mapping[str, str] | None = None,
        expected_statuses: Sequence[int] = (200,),
    ) -> tuple[int, object, Mapping[str, str]]:
        if path_or_url.startswith("https://"):
            url = path_or_url
        else:
            url = f"{GITHUB_API_ORIGIN}/{path_or_url.lstrip('/')}"
        if query:
            url = f"{url}?{urllib.parse.urlencode(query)}"
        if not url.startswith(f"{GITHUB_API_ORIGIN}/"):
            raise ValueError("refusing to send GITHUB_TOKEN outside the GitHub API")
        request = urllib.request.Request(url, headers=self._headers, method=method)
        try:
            with self._opener.open(
                request,
                timeout=self.config.request_timeout_seconds,
            ) as response:
                if response.status not in expected_statuses:
                    raise RuntimeError(
                        f"GitHub API returned HTTP {response.status} for {method} {url}"
                    )
                body = response.read()
                document = json.loads(body) if body else None
                return (
                    response.status,
                    document,
                    {key.lower(): value for key, value in response.headers.items()},
                )
        except urllib.error.HTTPError as error:
            if error.code in expected_statuses:
                return (
                    error.code,
                    None,
                    {key.lower(): value for key, value in error.headers.items()},
                )
            raise RuntimeError(
                f"GitHub API returned HTTP {error.code} for {method} {url}"
            ) from error

    def current_branch_sha(self) -> str:
        branch = urllib.parse.quote(self.config.branch, safe="")
        _, document, _ = self._request_json(
            "GET",
            f"repos/{self.repository}/git/ref/heads/{branch}",
        )
        root = _require_mapping(document, "branch response")
        obj = _require_mapping(root.get("object"), "branch response.object")
        return _require_string(obj, "sha")

    def active_push_runs(self) -> list[WorkflowRun]:
        workflow = urllib.parse.quote(self.config.workflow, safe="")
        runs: dict[int, WorkflowRun] = {}
        for status in self.config.active_statuses:
            next_url: str | None = (
                f"{GITHUB_API_ORIGIN}/repos/{self.repository}/actions/workflows/"
                f"{workflow}/runs?"
                + urllib.parse.urlencode(
                    {
                        "branch": self.config.branch,
                        "event": "push",
                        "status": status,
                        "per_page": str(self.config.runs_per_page),
                    }
                )
            )
            while next_url is not None:
                _, document, headers = self._request_json("GET", next_url)
                root = _require_mapping(document, "workflow runs response")
                raw_runs = root.get("workflow_runs")
                if not isinstance(raw_runs, list):
                    raise ValueError("workflow_runs must be an array")
                for raw_run in raw_runs:
                    item = _require_mapping(raw_run, "workflow run")
                    run_id = item.get("id")
                    if isinstance(run_id, bool) or not isinstance(run_id, int):
                        raise ValueError("workflow run id must be an integer")
                    runs[run_id] = WorkflowRun(
                        run_id=run_id,
                        head_sha=_require_string(item, "head_sha"),
                        event=_require_string(item, "event"),
                        status=_require_string(item, "status"),
                    )
                next_url = _next_link(headers.get("link"))
        return [runs[run_id] for run_id in sorted(runs)]

    def _run_status(self, run_id: int) -> str:
        _, document, _ = self._request_json(
            "GET",
            f"repos/{self.repository}/actions/runs/{run_id}",
        )
        root = _require_mapping(document, "workflow run response")
        return _require_string(root, "status")

    def _request_cancel(self, run_id: int, *, force: bool) -> int:
        operation = "force-cancel" if force else "cancel"
        status, _, _ = self._request_json(
            "POST",
            f"repos/{self.repository}/actions/runs/{run_id}/{operation}",
            expected_statuses=(202, 409),
        )
        if status == 409:
            print(f"::warning::GitHub returned 409 while cancelling run {run_id}")
        return status

    def _wait_until_terminal(self, run_id: int) -> bool:
        for attempt in range(self.config.cancel_poll_attempts):
            if self._run_status(run_id) not in self.config.active_statuses:
                return True
            if attempt + 1 < self.config.cancel_poll_attempts:
                time.sleep(self.config.cancel_poll_interval_seconds)
        return False

    def cancel_self(self, run_id: int) -> None:
        self._request_cancel(run_id, force=False)

    def cancel_and_confirm(self, run_id: int) -> None:
        self._request_cancel(run_id, force=False)
        if self._wait_until_terminal(run_id):
            return
        self._request_cancel(run_id, force=True)
        if self._wait_until_terminal(run_id):
            return
        raise RuntimeError(f"run {run_id} remained active after force-cancellation")


def _next_link(header: str | None) -> str | None:
    if header is None:
        return None
    for entry in header.split(","):
        parts = [part.strip() for part in entry.split(";")]
        if len(parts) == 2 and parts[1] == 'rel="next"':
            return parts[0].removeprefix("<").removesuffix(">")
    return None


def reconcile(
    client: ActionsClient,
    *,
    run_id: int,
    run_sha: str,
) -> ReconcileResult:
    def supersede(message: str) -> None:
        client.cancel_self(run_id)
        raise SupersededRun(message)

    current_sha = client.current_branch_sha()
    if run_sha != current_sha:
        supersede(f"run {run_id} is not exact-current main")

    active_runs = client.active_push_runs()
    if client.current_branch_sha() != run_sha:
        supersede(f"run {run_id} ceased to be exact-current main")

    cancelled: list[int] = []
    for run in active_runs:
        if (
            run.run_id == run_id
            or run.event != "push"
            or run.status == "completed"
            or run.head_sha == current_sha
        ):
            continue
        if client.current_branch_sha() != run_sha:
            supersede(f"run {run_id} ceased to be exact-current main")
        client.cancel_and_confirm(run.run_id)
        cancelled.append(run.run_id)

    if client.current_branch_sha() != run_sha:
        supersede(f"run {run_id} ceased to be exact-current main")

    return ReconcileResult(cancelled_run_ids=tuple(cancelled))


def cancel_superseded_target(
    client: ActionsClient,
    *,
    run_id: int,
    run_sha: str,
) -> ReconcileResult:
    current_sha = client.current_branch_sha()
    if run_sha == current_sha:
        return ReconcileResult(cancelled_run_ids=())
    if client.current_branch_sha() != current_sha:
        raise RuntimeError("main moved while the watchdog was evaluating a rerun")
    client.cancel_and_confirm(run_id)
    return ReconcileResult(cancelled_run_ids=(run_id,))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True, type=pathlib.Path)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--run-id", required=True, type=int)
    parser.add_argument("--run-sha", required=True)
    parser.add_argument("--watch-only", action="store_true")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        config = load_config(args.config)
        client = GitHubActionsClient(
            config=config,
            repository=args.repository,
            token=os.environ.get("GITHUB_TOKEN", ""),
        )
        operation = cancel_superseded_target if args.watch_only else reconcile
        result = operation(client, run_id=args.run_id, run_sha=args.run_sha)
    except SupersededRun as error:
        print(f"::notice::{error}")
        return 78
    except (OSError, RuntimeError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    print(json.dumps(dataclasses.asdict(result), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
