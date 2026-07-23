#!/usr/bin/env python3
"""Admit only exact-current-main advisory evidence and cancel stale runs."""

from __future__ import annotations

import argparse
import dataclasses
import datetime
import email.utils
import json
import math
import os
import pathlib
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from typing import Protocol


GITHUB_API_ORIGIN = "https://api.github.com"


@dataclasses.dataclass(frozen=True)
class Config:
    api_version: str
    branch: str
    workflow: str
    event: str
    request_timeout_seconds: int
    runs_per_page: int
    sweep_interval_seconds: int
    reconciliation_timeout_seconds: int
    api_rate_limit_reserve: int
    secondary_read_points: int
    secondary_mutation_points: int
    max_secondary_points: int
    workflow_run_lifetime_days: int
    rerun_request_window_days: int
    created_lookback_days: int
    search_result_limit: int
    max_search_results: int
    discovery_stable_sweeps: int
    discovery_max_sweeps: int
    max_cancellation_targets: int
    max_total_requests: int
    max_reconciliation_rounds: int
    cancel_poll_attempts: int
    cancel_poll_interval_seconds: int
    terminal_status: str


@dataclasses.dataclass(frozen=True)
class ReconciliationTopology:
    requests: int
    secondary_points: int
    minimum_pacing_seconds: int


@dataclasses.dataclass(frozen=True)
class EpisodeCapacity:
    requests: int
    secondary_points: int
    pacing_seconds: int

    def __add__(self, other: EpisodeCapacity) -> EpisodeCapacity:
        return EpisodeCapacity(
            requests=self.requests + other.requests,
            secondary_points=self.secondary_points + other.secondary_points,
            pacing_seconds=self.pacing_seconds + other.pacing_seconds,
        )


def cancellation_episode_capacity(config: Config) -> EpisodeCapacity:
    reads = 4 + (2 * config.cancel_poll_attempts)
    mutations = 2
    return EpisodeCapacity(
        requests=reads + mutations,
        secondary_points=(
            reads * config.secondary_read_points
            + mutations * config.secondary_mutation_points
        ),
        pacing_seconds=(
            2
            * (config.cancel_poll_attempts - 1)
            * config.cancel_poll_interval_seconds
        ),
    )


def reconciliation_topology(config: Config) -> ReconciliationTopology:
    pages_per_sweep = math.ceil(
        config.max_search_results / config.runs_per_page
    )
    census_count = config.max_reconciliation_rounds + 1
    sweep_count = census_count * config.discovery_max_sweeps
    sweep_reads = pages_per_sweep + 2
    episode = cancellation_episode_capacity(config)
    requests = (
        1
        + (sweep_count * sweep_reads)
        + (config.max_cancellation_targets * episode.requests)
        + 1
    )
    secondary_points = (
        config.secondary_read_points
        + (
            sweep_count
            * sweep_reads
            * config.secondary_read_points
        )
        + (
            config.max_cancellation_targets
            * episode.secondary_points
        )
        + max(config.secondary_read_points, config.secondary_mutation_points)
    )
    minimum_pacing_seconds = (
        census_count
        * (config.discovery_max_sweeps - 1)
        * config.sweep_interval_seconds
        + config.max_cancellation_targets
        * episode.pacing_seconds
    )
    return ReconciliationTopology(
        requests=requests,
        secondary_points=secondary_points,
        minimum_pacing_seconds=minimum_pacing_seconds,
    )


@dataclasses.dataclass(frozen=True)
class WorkflowRun:
    run_id: int
    head_sha: str
    event: str
    status: str
    run_attempt: int = 1
    created_at: str = "1970-01-01T00:00:00Z"
    head_branch: str = "main"


@dataclasses.dataclass(frozen=True)
class ReconciliationContext:
    invoking_run: WorkflowRun
    repository_id: int
    repository_full_name: str
    github_now: datetime.datetime
    created_filter: str
    deadline: float


@dataclasses.dataclass(frozen=True)
class CensusObservation:
    total_count: int
    fetched_count: int
    page_count: int


@dataclasses.dataclass(frozen=True)
class ReconciliationEvidence:
    census: CensusObservation | None
    computed_request_budget: int
    remaining_primary_rate_limit: int | None
    requests_used: int
    secondary_points_used: int
    cancellation_episodes_used: int
    reservations_released: int
    reservations_consumed: int
    reconciliation_rounds_used: int


@dataclasses.dataclass(frozen=True)
class SuccessorReservation:
    current: tuple[int, int]
    successor: tuple[int, int]


@dataclasses.dataclass
class ReconciliationLedger:
    config: Config
    deadline: float
    monotonic_now: Callable[[], float] = time.monotonic
    requests: int = 0
    secondary_points: int = 0
    mutations: int = 0
    rounds: int = 0
    reservations_released: int = 0
    reservations_consumed: int = 0
    poisoned: bool = False
    latest_primary_remaining: int | None = None
    episodes: set[tuple[int, int]] = dataclasses.field(default_factory=set)
    pending_reservation: SuccessorReservation | None = None

    def remaining_seconds(self) -> float:
        remaining = self.deadline - self.monotonic_now()
        if remaining <= 0:
            raise RuntimeError("reconciliation deadline exhausted")
        return remaining

    def charge_request(self, method: str) -> None:
        if self.poisoned:
            raise RuntimeError("reconciliation authority is poisoned")
        self.remaining_seconds()
        if method == "GET":
            points = self.config.secondary_read_points
        elif method == "POST":
            points = self.config.secondary_mutation_points
        else:
            raise ValueError(f"unsupported GitHub request method: {method}")
        if self.requests + 1 > self.config.max_total_requests:
            raise RuntimeError("GitHub request budget exhausted")
        if self.secondary_points + points > self.config.max_secondary_points:
            raise RuntimeError("GitHub secondary-point budget exhausted")
        self.requests += 1
        self.secondary_points += points
        if method == "POST":
            self.mutations += 1

    def observe_headers(self, headers: Mapping[str, str]) -> None:
        raw_remaining = headers.get("x-ratelimit-remaining")
        if raw_remaining is None:
            self.latest_primary_remaining = None
            return
        try:
            remaining = int(raw_remaining)
        except ValueError as error:
            raise RuntimeError(
                "GitHub x-ratelimit-remaining header is malformed"
            ) from error
        if remaining < 0:
            raise RuntimeError("GitHub x-ratelimit-remaining header is negative")
        self.latest_primary_remaining = remaining

    def begin_episode(self, run_id: int, run_attempt: int) -> tuple[int, int]:
        if self.poisoned:
            raise RuntimeError("reconciliation authority is poisoned")
        if (
            isinstance(run_id, bool)
            or not isinstance(run_id, int)
            or run_id <= 0
            or isinstance(run_attempt, bool)
            or not isinstance(run_attempt, int)
            or run_attempt <= 0
        ):
            raise ValueError("cancellation episode identity must be positive")
        episode = (run_id, run_attempt)
        if episode in self.episodes:
            return episode
        occupied = len(self.episodes) + int(self.pending_reservation is not None)
        if occupied + 1 > self.config.max_cancellation_targets:
            raise RuntimeError("cancellation-episode budget exhausted")
        self.episodes.add(episode)
        return episode

    def reserve_successor(
        self,
        episode: tuple[int, int],
        current_remaining: EpisodeCapacity,
    ) -> SuccessorReservation:
        if self.poisoned:
            raise RuntimeError("reconciliation authority is poisoned")
        if episode not in self.episodes:
            raise RuntimeError("cannot reserve for an uncharged cancellation episode")
        if self.pending_reservation is not None:
            raise RuntimeError("an immediate-successor reservation is already pending")
        successor = (episode[0], episode[1] + 1)
        if successor in self.episodes:
            raise RuntimeError("immediate successor was already observed")
        if len(self.episodes) + 1 > self.config.max_cancellation_targets:
            raise RuntimeError("immediate-successor reservation budget exhausted")
        required = current_remaining + cancellation_episode_capacity(self.config)
        if self.requests + required.requests > self.config.max_total_requests:
            raise RuntimeError("request capacity cannot cover immediate successor")
        if (
            self.secondary_points + required.secondary_points
            > self.config.max_secondary_points
        ):
            raise RuntimeError(
                "secondary-point capacity cannot cover immediate successor"
            )
        if self.latest_primary_remaining is None:
            raise RuntimeError("primary rate-limit authority is unavailable")
        if (
            self.latest_primary_remaining - required.requests
            < self.config.api_rate_limit_reserve
        ):
            raise RuntimeError("primary rate-limit reserve cannot cover cancellation")
        if self.monotonic_now() + required.pacing_seconds > self.deadline:
            raise RuntimeError("deadline cannot cover immediate successor")
        reservation = SuccessorReservation(
            current=episode,
            successor=successor,
        )
        self.pending_reservation = reservation
        return reservation

    def bind_first_observation(self, run_attempt: int) -> tuple[int, int]:
        reservation = self.pending_reservation
        if reservation is None:
            raise RuntimeError("no immediate-successor reservation is pending")
        if (
            isinstance(run_attempt, bool)
            or not isinstance(run_attempt, int)
            or run_attempt <= 0
        ):
            self.consume_ambiguous_reservation()
            raise RuntimeError("first post-mutation attempt is malformed")
        if run_attempt == reservation.current[1]:
            self.pending_reservation = None
            self.reservations_released += 1
            return reservation.current
        if run_attempt == reservation.successor[1]:
            self.pending_reservation = None
            self.reservations_consumed += 1
            self.episodes.add(reservation.successor)
            return reservation.successor
        self.consume_ambiguous_reservation()
        raise RuntimeError(
            "first post-mutation observation was not the bound attempt or immediate successor"
        )

    def consume_ambiguous_reservation(self) -> None:
        if self.pending_reservation is not None:
            self.pending_reservation = None
            self.reservations_consumed += 1
        self.poisoned = True


@dataclasses.dataclass(frozen=True)
class ReconcileResult:
    cancelled_run_ids: tuple[int, ...]
    evidence: ReconciliationEvidence | None = None


class SupersededRun(RuntimeError):
    """The invoking run no longer represents current main."""


class IncompleteCensus(RuntimeError):
    """A workflow-run sweep cannot prove a complete bounded census."""


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
    def capture_context(
        self,
        *,
        run_id: int,
        run_sha: str,
        monotonic_now: Callable[[], float] = time.monotonic,
    ) -> ReconciliationContext: ...

    def current_branch_sha(self) -> str: ...

    def active_push_runs(
        self,
        context: ReconciliationContext,
        freshness_guard: Callable[[], None],
    ) -> list[WorkflowRun]: ...

    def begin_reconciliation_round(self) -> None: ...

    def reconciliation_evidence(self) -> ReconciliationEvidence | None: ...

    def cancel_invoking_run(self, run_id: int) -> None: ...

    def cancel_and_confirm(
        self,
        target: WorkflowRun,
        context: ReconciliationContext,
        freshness_guard: Callable[[], None],
    ) -> None: ...


def _require_mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{label} must be a table")
    return value


def _require_string(document: Mapping[str, object], key: str) -> str:
    value = document.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{key} must be a non-empty string")
    return value


def _require_positive_integer(document: Mapping[str, object], key: str) -> int:
    value = document.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"{key} must be a positive integer")
    return value


def _require_timestamp(document: Mapping[str, object], key: str) -> datetime.datetime:
    value = _require_string(document, key)
    try:
        parsed = datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError(f"{key} must be an ISO-8601 timestamp") from error
    if parsed.tzinfo is None:
        raise ValueError(f"{key} must include a timezone")
    return parsed.astimezone(datetime.UTC)


def _canonical_timestamp(value: datetime.datetime) -> str:
    timespec = "microseconds" if value.microsecond else "seconds"
    return value.isoformat(timespec=timespec).replace("+00:00", "Z")


def load_config(path: pathlib.Path) -> Config:
    document = _require_mapping(
        tomllib.loads(path.read_text(encoding="utf-8")), "config"
    )
    expected = {
        "schema_version",
        "api_version",
        "branch",
        "workflow",
        "event",
        "request_timeout_seconds",
        "runs_per_page",
        "sweep_interval_seconds",
        "reconciliation_timeout_seconds",
        "api_rate_limit_reserve",
        "secondary_read_points",
        "secondary_mutation_points",
        "max_secondary_points",
        "workflow_run_lifetime_days",
        "rerun_request_window_days",
        "created_lookback_days",
        "search_result_limit",
        "max_search_results",
        "discovery_stable_sweeps",
        "discovery_max_sweeps",
        "max_cancellation_targets",
        "max_total_requests",
        "max_reconciliation_rounds",
        "cancel_poll_attempts",
        "cancel_poll_interval_seconds",
        "terminal_status",
    }
    unknown = set(document) - expected
    if unknown:
        raise ValueError(f"unknown config keys: {', '.join(sorted(unknown))}")
    schema_version = document.get("schema_version")
    if isinstance(schema_version, bool) or schema_version != 1:
        raise ValueError("schema_version must be 1")
    values = {
        key: _require_positive_integer(document, key)
        for key in expected
        if key
        not in {
            "schema_version",
            "api_version",
            "branch",
            "workflow",
            "event",
            "terminal_status",
        }
    }
    runs_per_page = values["runs_per_page"]
    if runs_per_page != 100:
        raise ValueError("runs_per_page must be exactly 100")
    discovery_stable_sweeps = values["discovery_stable_sweeps"]
    if discovery_stable_sweeps < 2:
        raise ValueError("discovery_stable_sweeps must be an integer of at least 2")
    discovery_max_sweeps = values["discovery_max_sweeps"]
    if discovery_max_sweeps < discovery_stable_sweeps:
        raise ValueError(
            "discovery_max_sweeps must be at least discovery_stable_sweeps"
        )
    if values["created_lookback_days"] <= (
        values["workflow_run_lifetime_days"]
        + values["rerun_request_window_days"]
    ):
        raise ValueError(
            "created_lookback_days must exceed the workflow lifetime plus rerun window"
        )
    if values["max_search_results"] >= values["search_result_limit"]:
        raise ValueError("max_search_results must be below search_result_limit")
    if values["secondary_read_points"] != 1:
        raise ValueError("secondary_read_points must be exactly 1")
    if values["secondary_mutation_points"] != 5:
        raise ValueError("secondary_mutation_points must be exactly 5")
    if values["max_cancellation_targets"] < 2:
        raise ValueError(
            "max_cancellation_targets must fit an episode and successor reservation"
        )
    event = _require_string(document, "event")
    if event != "push":
        raise ValueError("event must be push")
    terminal_status = _require_string(document, "terminal_status")
    if terminal_status != "completed":
        raise ValueError("terminal_status must be completed")
    config = Config(
        api_version=_require_string(document, "api_version"),
        branch=_require_string(document, "branch"),
        workflow=_require_string(document, "workflow"),
        event=event,
        request_timeout_seconds=values["request_timeout_seconds"],
        runs_per_page=runs_per_page,
        sweep_interval_seconds=values["sweep_interval_seconds"],
        reconciliation_timeout_seconds=values["reconciliation_timeout_seconds"],
        api_rate_limit_reserve=values["api_rate_limit_reserve"],
        secondary_read_points=values["secondary_read_points"],
        secondary_mutation_points=values["secondary_mutation_points"],
        max_secondary_points=values["max_secondary_points"],
        workflow_run_lifetime_days=values["workflow_run_lifetime_days"],
        rerun_request_window_days=values["rerun_request_window_days"],
        created_lookback_days=values["created_lookback_days"],
        search_result_limit=values["search_result_limit"],
        max_search_results=values["max_search_results"],
        discovery_stable_sweeps=discovery_stable_sweeps,
        discovery_max_sweeps=discovery_max_sweeps,
        max_cancellation_targets=values["max_cancellation_targets"],
        max_total_requests=values["max_total_requests"],
        max_reconciliation_rounds=values["max_reconciliation_rounds"],
        cancel_poll_attempts=values["cancel_poll_attempts"],
        cancel_poll_interval_seconds=values["cancel_poll_interval_seconds"],
        terminal_status=terminal_status,
    )
    topology = reconciliation_topology(config)
    if config.max_total_requests < topology.requests:
        raise ValueError(
            "max_total_requests cannot cover the configured reconciliation topology"
        )
    if config.max_secondary_points < topology.secondary_points:
        raise ValueError(
            "max_secondary_points cannot cover the configured reconciliation topology"
        )
    if config.reconciliation_timeout_seconds < topology.minimum_pacing_seconds:
        raise ValueError(
            "reconciliation_timeout_seconds cannot cover configured pacing"
        )
    return config


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
        self._sleep = time.sleep
        self._ledger: ReconciliationLedger | None = None
        self._last_census: CensusObservation | None = None

    def capture_context(
        self,
        *,
        run_id: int,
        run_sha: str,
        monotonic_now: Callable[[], float] = time.monotonic,
    ) -> ReconciliationContext:
        if run_id <= 0:
            raise ValueError("run_id must be positive")
        if getattr(self, "_ledger", None) is not None:
            raise RuntimeError("reconciliation context was already captured")
        deadline = monotonic_now() + self.config.reconciliation_timeout_seconds
        self._ledger = ReconciliationLedger(
            config=self.config,
            deadline=deadline,
            monotonic_now=monotonic_now,
        )
        _, document, headers = self._request_json(
            "GET",
            f"repos/{self.repository}/actions/runs/{run_id}",
        )
        root = _require_mapping(document, "exact workflow run response")
        observed_run_id = root.get("id")
        if (
            isinstance(observed_run_id, bool)
            or not isinstance(observed_run_id, int)
            or observed_run_id != run_id
        ):
            raise ValueError("exact workflow run id does not match requested run")
        run_attempt = root.get("run_attempt")
        if (
            isinstance(run_attempt, bool)
            or not isinstance(run_attempt, int)
            or run_attempt <= 0
        ):
            raise ValueError("exact workflow run attempt must be positive")
        repository = _require_mapping(
            root.get("repository"), "exact workflow run repository"
        )
        repository_id = repository.get("id")
        if (
            isinstance(repository_id, bool)
            or not isinstance(repository_id, int)
            or repository_id <= 0
        ):
            raise ValueError("exact workflow run repository id must be positive")
        repository_full_name = _require_string(repository, "full_name")
        if repository_full_name != self.repository:
            raise ValueError("exact workflow run repository does not match request")
        event = _require_string(root, "event")
        if event != self.config.event:
            raise ValueError("exact workflow run event does not match config")
        head_branch = _require_string(root, "head_branch")
        if head_branch != self.config.branch:
            raise ValueError("exact workflow run branch does not match config")
        head_sha = _require_string(root, "head_sha")
        if head_sha != run_sha:
            raise ValueError("exact workflow run SHA does not match invocation")
        workflow_path = _require_string(root, "path").split("@", maxsplit=1)[0]
        expected_workflow_path = f".github/workflows/{self.config.workflow}"
        if workflow_path != expected_workflow_path:
            raise ValueError("exact workflow run workflow does not match config")
        created_at = _require_timestamp(root, "created_at")
        run_started_at = _require_timestamp(root, "run_started_at")
        raw_date = headers.get("date")
        if raw_date is None:
            raise ValueError("exact workflow run response is missing HTTP Date")
        try:
            github_now = email.utils.parsedate_to_datetime(raw_date)
        except (TypeError, ValueError) as error:
            raise ValueError(
                "exact workflow run response has malformed HTTP Date"
            ) from error
        if github_now is None or github_now.tzinfo is None:
            raise ValueError("exact workflow run response has malformed HTTP Date")
        github_now = github_now.astimezone(datetime.UTC)
        if github_now < created_at or github_now < run_started_at:
            raise ValueError("GitHub HTTP Date precedes exact workflow run timestamps")
        cutoff = github_now - datetime.timedelta(
            days=self.config.created_lookback_days
        )
        created_filter = f">={cutoff.strftime('%Y-%m-%dT%H:%M:%SZ')}"
        return ReconciliationContext(
            invoking_run=WorkflowRun(
                run_id=run_id,
                head_sha=head_sha,
                event=event,
                status=_require_string(root, "status"),
                run_attempt=run_attempt,
                created_at=_canonical_timestamp(created_at),
                head_branch=head_branch,
            ),
            repository_id=repository_id,
            repository_full_name=repository_full_name,
            github_now=github_now,
            created_filter=created_filter,
            deadline=deadline,
        )

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
        ledger = getattr(self, "_ledger", None)
        if ledger is not None:
            ledger.charge_request(method)
            timeout = min(
                self.config.request_timeout_seconds,
                ledger.remaining_seconds(),
            )
        else:
            timeout = self.config.request_timeout_seconds
        request = urllib.request.Request(url, headers=self._headers, method=method)
        try:
            with self._opener.open(
                request,
                timeout=timeout,
            ) as response:
                if response.status not in expected_statuses:
                    raise RuntimeError(
                        f"GitHub API returned HTTP {response.status} for {method} {url}"
                    )
                body = response.read()
                document = json.loads(body) if body else None
                response_headers = {
                    key.lower(): value for key, value in response.headers.items()
                }
                if ledger is not None:
                    ledger.remaining_seconds()
                    ledger.observe_headers(response_headers)
                return (
                    response.status,
                    document,
                    response_headers,
                )
        except urllib.error.HTTPError as error:
            error_headers = {
                key.lower(): value for key, value in error.headers.items()
            }
            if ledger is not None:
                ledger.remaining_seconds()
                ledger.observe_headers(error_headers)
            if error.code in expected_statuses:
                return (
                    error.code,
                    None,
                    error_headers,
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

    def _workflow_run_sweep(
        self, context: ReconciliationContext
    ) -> list[WorkflowRun]:
        workflow = urllib.parse.quote(self.config.workflow, safe="")
        runs: list[WorkflowRun] = []
        seen_run_ids: set[int] = set()
        seen_urls: set[str] = set()
        seen_pages: set[int] = set()
        expected_total: int | None = None
        sentinel_seen = False
        max_pages = math.ceil(
            self.config.max_search_results / self.config.runs_per_page
        )
        next_url: str | None = (
            f"{GITHUB_API_ORIGIN}/repos/{self.repository}/actions/workflows/"
            f"{workflow}/runs?"
            + urllib.parse.urlencode(
                {
                    "branch": self.config.branch,
                    "event": self.config.event,
                    "created": context.created_filter,
                    "per_page": str(self.config.runs_per_page),
                }
            )
        )
        while next_url is not None:
            if len(seen_urls) >= max_pages:
                raise IncompleteCensus("workflow-run census exceeded page budget")
            page = self._validate_census_url(next_url, context)
            if next_url in seen_urls or page in seen_pages:
                raise IncompleteCensus("workflow-run pagination repeated a page")
            seen_urls.add(next_url)
            seen_pages.add(page)
            _, document, headers = self._request_json("GET", next_url)
            root = _require_mapping(document, "workflow runs response")
            total_count = root.get("total_count")
            if (
                isinstance(total_count, bool)
                or not isinstance(total_count, int)
                or total_count < 0
            ):
                raise IncompleteCensus("workflow-run total_count must be non-negative")
            if total_count >= self.config.max_search_results:
                raise IncompleteCensus(
                    "workflow-run total_count reached the governed search threshold"
                )
            if expected_total is None:
                expected_total = total_count
            elif total_count != expected_total:
                raise IncompleteCensus("workflow-run total_count changed between pages")
            raw_runs = root.get("workflow_runs")
            if not isinstance(raw_runs, list):
                raise IncompleteCensus("workflow_runs must be an array")
            if not raw_runs and _next_link(headers.get("link")) is not None:
                raise IncompleteCensus("empty workflow-run page has a continuation")
            for raw_run in raw_runs:
                item = _require_mapping(raw_run, "workflow run")
                run_id = item.get("id")
                if (
                    isinstance(run_id, bool)
                    or not isinstance(run_id, int)
                    or run_id <= 0
                ):
                    raise IncompleteCensus("workflow run id must be positive")
                if run_id in seen_run_ids:
                    raise IncompleteCensus("workflow-run census contains a duplicate id")
                seen_run_ids.add(run_id)
                run_attempt = item.get("run_attempt")
                if (
                    isinstance(run_attempt, bool)
                    or not isinstance(run_attempt, int)
                    or run_attempt <= 0
                ):
                    raise IncompleteCensus("workflow run attempt must be positive")
                event = _require_string(item, "event")
                if event != self.config.event:
                    raise IncompleteCensus("workflow run event does not match config")
                head_branch = _require_string(item, "head_branch")
                if head_branch != self.config.branch:
                    raise IncompleteCensus("workflow run branch does not match config")
                created_at = _canonical_timestamp(
                    _require_timestamp(item, "created_at")
                )
                run = WorkflowRun(
                    run_id=run_id,
                    head_sha=_require_string(item, "head_sha"),
                    event=event,
                    status=_require_string(item, "status"),
                    run_attempt=run_attempt,
                    created_at=created_at,
                    head_branch=head_branch,
                )
                if run.run_id == context.invoking_run.run_id:
                    if (
                        run.run_attempt != context.invoking_run.run_attempt
                        or run.head_sha != context.invoking_run.head_sha
                    ):
                        raise IncompleteCensus(
                            "workflow-run sentinel identity changed in census"
                        )
                    sentinel_seen = True
                if run.status != self.config.terminal_status:
                    runs.append(run)
            if expected_total is None:
                raise AssertionError("total_count was not initialized")
            fetched = len(seen_run_ids)
            continuation = _next_link(headers.get("link"))
            if fetched < expected_total and continuation is None:
                raise IncompleteCensus(
                    "workflow-run census ended before total_count records"
                )
            if fetched == expected_total and continuation is not None:
                raise IncompleteCensus(
                    "workflow-run census continued after total_count records"
                )
            if fetched > expected_total:
                raise IncompleteCensus(
                    "workflow-run census fetched more than total_count records"
                )
            next_url = continuation
        if expected_total is None or len(seen_run_ids) != expected_total:
            raise IncompleteCensus("workflow-run census count is incomplete")
        if not sentinel_seen:
            raise IncompleteCensus("exact workflow-run sentinel is missing")
        self._last_census = CensusObservation(
            total_count=expected_total,
            fetched_count=len(seen_run_ids),
            page_count=len(seen_urls),
        )
        return sorted(runs, key=lambda run: run.run_id)

    def _validate_census_url(
        self,
        url: str,
        context: ReconciliationContext,
    ) -> int:
        parsed = urllib.parse.urlsplit(url)
        if (
            parsed.scheme != "https"
            or parsed.hostname != "api.github.com"
            or parsed.port not in (None, 443)
            or parsed.username is not None
            or parsed.password is not None
            or parsed.fragment
        ):
            raise IncompleteCensus("workflow-run pagination has a foreign origin")
        decoded_path = urllib.parse.unquote(parsed.path)
        configured_path = (
            f"/repos/{self.repository}/actions/workflows/"
            f"{self.config.workflow}/runs"
        )
        numeric_path = (
            f"/repositories/{context.repository_id}/actions/workflows/"
            f"{self.config.workflow}/runs"
        )
        if decoded_path not in {configured_path, numeric_path}:
            raise IncompleteCensus(
                "workflow-run pagination does not match the governed repository and workflow"
            )
        query: dict[str, list[str]] = {}
        for key, value in urllib.parse.parse_qsl(
            parsed.query, keep_blank_values=True
        ):
            query.setdefault(key, []).append(value)
        required = {
            "branch": self.config.branch,
            "event": self.config.event,
            "created": context.created_filter,
            "per_page": str(self.config.runs_per_page),
        }
        if set(query) - (set(required) | {"page"}):
            raise IncompleteCensus("workflow-run pagination has unknown query keys")
        for key, expected in required.items():
            if query.get(key) != [expected]:
                raise IncompleteCensus(
                    f"workflow-run pagination has invalid {key} query authority"
                )
        page_values = query.get("page")
        if page_values is None:
            return 1
        if len(page_values) != 1:
            raise IncompleteCensus("workflow-run pagination has duplicate page keys")
        try:
            page = int(page_values[0])
        except ValueError as error:
            raise IncompleteCensus(
                "workflow-run pagination page must be positive"
            ) from error
        if page <= 0 or str(page) != page_values[0]:
            raise IncompleteCensus("workflow-run pagination page must be positive")
        return page

    def active_push_runs(
        self,
        context: ReconciliationContext,
        freshness_guard: Callable[[], None],
    ) -> list[WorkflowRun]:
        previous_signature: tuple[tuple[int, int, str, str], ...] | None = None
        stable_sweeps = 0
        last_incomplete: IncompleteCensus | None = None
        for sweep_index in range(self.config.discovery_max_sweeps):
            freshness_guard()
            try:
                runs = self._workflow_run_sweep(context)
            except IncompleteCensus as error:
                last_incomplete = error
                previous_signature = None
                stable_sweeps = 0
                if sweep_index + 1 < self.config.discovery_max_sweeps:
                    self._sleep_between_sweeps(context)
                continue
            freshness_guard()
            signature = tuple(
                (run.run_id, run.run_attempt, run.head_sha, run.created_at)
                for run in runs
            )
            if signature == previous_signature:
                stable_sweeps += 1
            else:
                previous_signature = signature
                stable_sweeps = 1
            if stable_sweeps >= self.config.discovery_stable_sweeps:
                return runs
            if sweep_index + 1 < self.config.discovery_max_sweeps:
                self._sleep_between_sweeps(context)
        message = "active workflow-run discovery did not stabilize"
        if last_incomplete is not None:
            message = f"{message}: {last_incomplete}"
        raise RuntimeError(message)

    def _sleep_between_sweeps(self, context: ReconciliationContext) -> None:
        ledger = getattr(self, "_ledger", None)
        if ledger is not None:
            if ledger.monotonic_now() + self.config.sweep_interval_seconds > context.deadline:
                raise RuntimeError("reconciliation deadline cannot cover sweep interval")
        self._sleep(self.config.sweep_interval_seconds)

    def begin_reconciliation_round(self) -> None:
        ledger = self._ledger
        if ledger is None:
            raise RuntimeError("reconciliation round requires a ledger")
        if ledger.rounds + 1 > self.config.max_reconciliation_rounds:
            raise RuntimeError("reconciliation-round budget exhausted")
        ledger.rounds += 1

    def reconciliation_evidence(self) -> ReconciliationEvidence:
        ledger = self._ledger
        if ledger is None:
            raise RuntimeError("reconciliation evidence requires a ledger")
        return ReconciliationEvidence(
            census=self._last_census,
            computed_request_budget=reconciliation_topology(self.config).requests,
            remaining_primary_rate_limit=ledger.latest_primary_remaining,
            requests_used=ledger.requests,
            secondary_points_used=ledger.secondary_points,
            cancellation_episodes_used=len(ledger.episodes),
            reservations_released=ledger.reservations_released,
            reservations_consumed=ledger.reservations_consumed,
            reconciliation_rounds_used=ledger.rounds,
        )

    def _exact_target(
        self,
        target: WorkflowRun,
        context: ReconciliationContext,
    ) -> WorkflowRun:
        _, document, _ = self._request_json(
            "GET",
            f"repos/{self.repository}/actions/runs/{target.run_id}",
        )
        root = _require_mapping(document, "workflow run response")
        run_id = root.get("id")
        if (
            isinstance(run_id, bool)
            or not isinstance(run_id, int)
            or run_id != target.run_id
        ):
            raise ValueError("exact target run id changed")
        run_attempt = root.get("run_attempt")
        if (
            isinstance(run_attempt, bool)
            or not isinstance(run_attempt, int)
            or run_attempt <= 0
        ):
            raise ValueError("exact target run attempt must be positive")
        repository = _require_mapping(
            root.get("repository"), "exact target repository"
        )
        repository_id = repository.get("id")
        if (
            isinstance(repository_id, bool)
            or not isinstance(repository_id, int)
            or repository_id != context.repository_id
            or _require_string(repository, "full_name")
            != context.repository_full_name
        ):
            raise ValueError("exact target repository identity changed")
        workflow_path = _require_string(root, "path").split("@", maxsplit=1)[0]
        if workflow_path != f".github/workflows/{self.config.workflow}":
            raise ValueError("exact target workflow changed")
        event = _require_string(root, "event")
        if event != self.config.event:
            raise ValueError("exact target event changed")
        branch = _require_string(root, "head_branch")
        if branch != self.config.branch:
            raise ValueError("exact target branch changed")
        head_sha = _require_string(root, "head_sha")
        if head_sha != target.head_sha:
            raise ValueError("exact target SHA changed")
        created_at = _canonical_timestamp(
            _require_timestamp(root, "created_at")
        )
        _require_timestamp(root, "run_started_at")
        return WorkflowRun(
            run_id=run_id,
            head_sha=head_sha,
            event=event,
            status=_require_string(root, "status"),
            run_attempt=run_attempt,
            created_at=created_at,
            head_branch=branch,
        )

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

    def cancel_invoking_run(self, run_id: int) -> None:
        if isinstance(run_id, bool) or not isinstance(run_id, int) or run_id <= 0:
            raise ValueError("invoking run id must be positive")
        if getattr(self, "_ledger", None) is None:
            raise RuntimeError("self-cancellation requires a reconciliation ledger")
        status = self._request_cancel(run_id, force=False)
        if status != 202:
            raise RuntimeError(
                f"GitHub did not accept cancellation of invoking run {run_id}"
            )

    def cancel_and_confirm(
        self,
        target: WorkflowRun,
        context: ReconciliationContext,
        freshness_guard: Callable[[], None],
    ) -> None:
        ledger = getattr(self, "_ledger", None)
        if ledger is None:
            raise RuntimeError("cancellation requires a reconciliation ledger")
        mutations_before = ledger.mutations
        try:
            self._cancel_and_confirm(target, context, freshness_guard)
        except Exception:
            if ledger.mutations > mutations_before:
                ledger.consume_ambiguous_reservation()
            raise

    def _cancel_and_confirm(
        self,
        target: WorkflowRun,
        context: ReconciliationContext,
        freshness_guard: Callable[[], None],
    ) -> None:
        ledger = self._ledger
        if ledger is None:
            raise RuntimeError("cancellation requires a reconciliation ledger")
        current = target
        while True:
            episode = ledger.begin_episode(current.run_id, current.run_attempt)
            freshness_guard()
            observed = self._exact_target(current, context)
            if observed.status == self.config.terminal_status:
                return
            if observed.run_attempt != current.run_attempt:
                current = self._advance_before_mutation(ledger, current, observed)
                continue
            current, terminal = self._mutate_and_poll(
                current,
                context,
                episode,
                force=False,
            )
            if terminal:
                return
            freshness_guard()
            observed = self._exact_target(current, context)
            if observed.status == self.config.terminal_status:
                return
            if observed.run_attempt != current.run_attempt:
                current = self._advance_before_mutation(ledger, current, observed)
                continue
            current, terminal = self._mutate_and_poll(
                current,
                context,
                (current.run_id, current.run_attempt),
                force=True,
            )
            if terminal:
                return
            raise RuntimeError(
                f"run {current.run_id} attempt {current.run_attempt} "
                "remained active after force-cancellation"
            )

    def _advance_before_mutation(
        self,
        ledger: ReconciliationLedger,
        previous: WorkflowRun,
        observed: WorkflowRun,
    ) -> WorkflowRun:
        if observed.run_attempt <= previous.run_attempt:
            ledger.consume_ambiguous_reservation()
            raise RuntimeError("exact target attempt decreased before mutation")
        ledger.begin_episode(observed.run_id, observed.run_attempt)
        return observed

    def _mutate_and_poll(
        self,
        target: WorkflowRun,
        context: ReconciliationContext,
        episode: tuple[int, int],
        *,
        force: bool,
    ) -> tuple[WorkflowRun, bool]:
        ledger = self._ledger
        if ledger is None:
            raise RuntimeError("cancellation requires a reconciliation ledger")
        ledger.reserve_successor(
            episode,
            self._remaining_stage_capacity(force=force),
        )
        try:
            self._request_cancel(target.run_id, force=force)
        except Exception:
            ledger.consume_ambiguous_reservation()
            raise
        try:
            observed = self._exact_target(target, context)
            bound_episode = ledger.bind_first_observation(observed.run_attempt)
        except Exception:
            ledger.consume_ambiguous_reservation()
            raise
        current = observed
        if (current.run_id, current.run_attempt) != bound_episode:
            raise AssertionError("bound cancellation episode does not match target")
        if current.status == self.config.terminal_status:
            return current, True
        for _ in range(1, self.config.cancel_poll_attempts):
            self._sleep_for_poll(context)
            observed = self._exact_target(current, context)
            if observed.run_attempt != current.run_attempt:
                ledger.consume_ambiguous_reservation()
                raise RuntimeError(
                    "attempt changed after the one-read mutation binding"
                )
            current = observed
            if current.status == self.config.terminal_status:
                return current, True
        return current, False

    def _remaining_stage_capacity(self, *, force: bool) -> EpisodeCapacity:
        polls = self.config.cancel_poll_attempts
        if force:
            reads = polls
            mutations = 1
            pacing = (polls - 1) * self.config.cancel_poll_interval_seconds
        else:
            reads = polls + 2 + polls
            mutations = 2
            pacing = (
                2
                * (polls - 1)
                * self.config.cancel_poll_interval_seconds
            )
        return EpisodeCapacity(
            requests=reads + mutations,
            secondary_points=(
                reads * self.config.secondary_read_points
                + mutations * self.config.secondary_mutation_points
            ),
            pacing_seconds=pacing,
        )

    def _sleep_for_poll(self, context: ReconciliationContext) -> None:
        ledger = self._ledger
        if ledger is None:
            raise RuntimeError("polling requires a reconciliation ledger")
        if (
            ledger.monotonic_now() + self.config.cancel_poll_interval_seconds
            > context.deadline
        ):
            raise RuntimeError("reconciliation deadline cannot cover poll interval")
        self._sleep(self.config.cancel_poll_interval_seconds)


def _next_link(header: str | None) -> str | None:
    if header is None:
        return None
    next_urls: list[str] = []
    for entry in header.split(","):
        parts = [part.strip() for part in entry.split(";")]
        if 'rel="next"' not in parts[1:]:
            continue
        if not parts[0].startswith("<") or not parts[0].endswith(">"):
            raise IncompleteCensus("workflow-run next link is malformed")
        next_urls.append(parts[0][1:-1])
    if len(next_urls) > 1:
        raise IncompleteCensus("workflow-run response has duplicate next links")
    return next_urls[0] if next_urls else None


def reconcile(
    client: ActionsClient,
    *,
    run_id: int,
    run_sha: str,
) -> ReconcileResult:
    context = client.capture_context(run_id=run_id, run_sha=run_sha)

    def require_current_main() -> None:
        if client.current_branch_sha() != run_sha:
            raise SupersededRun(f"run {run_id} ceased to be exact-current main")

    active_runs = client.active_push_runs(context, require_current_main)
    cancelled: list[int] = []
    while True:
        stale_runs = [
            run
            for run in active_runs
            if run.run_id != run_id and run.head_sha != run_sha
        ]
        if not stale_runs:
            require_current_main()
            return ReconcileResult(
                cancelled_run_ids=tuple(cancelled),
                evidence=client.reconciliation_evidence(),
            )
        client.begin_reconciliation_round()
        for run in stale_runs:
            client.cancel_and_confirm(run, context, require_current_main)
            cancelled.append(run.run_id)
        active_runs = client.active_push_runs(context, require_current_main)


def cancel_superseded_target(
    client: ActionsClient,
    *,
    run_id: int,
    run_sha: str,
) -> ReconcileResult:
    context = client.capture_context(run_id=run_id, run_sha=run_sha)
    current_sha = client.current_branch_sha()
    if run_sha == current_sha:
        return ReconcileResult(
            cancelled_run_ids=(),
            evidence=client.reconciliation_evidence(),
        )

    def require_stable_main() -> None:
        if client.current_branch_sha() != current_sha:
            raise RuntimeError("main moved while the watchdog was cancelling a rerun")

    client.cancel_and_confirm(
        context.invoking_run,
        context,
        require_stable_main,
    )
    return ReconcileResult(
        cancelled_run_ids=(run_id,),
        evidence=client.reconciliation_evidence(),
    )


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
        try:
            client.cancel_invoking_run(args.run_id)
        except (OSError, RuntimeError, ValueError) as cancel_error:
            print(f"::error::{cancel_error}", file=sys.stderr)
            return 1
        return 0
    except (OSError, RuntimeError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"::error::{error}", file=sys.stderr)
        return 1
    print(json.dumps(dataclasses.asdict(result), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
