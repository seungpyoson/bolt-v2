#!/usr/bin/env python3

from __future__ import annotations

import pathlib
import sys
import tempfile
import unittest
import urllib.parse
import dataclasses

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import advisory_supersession  # noqa: E402


GOVERNED_CONFIG = advisory_supersession.load_config(
    REPO_ROOT / "ci" / "advisory-supersession.toml"
)


class FakeClient:
    def __init__(
        self,
        *,
        current_shas: list[str],
        runs: list[advisory_supersession.WorkflowRun] | None = None,
        cancellation_error: RuntimeError | None = None,
    ) -> None:
        self.current_shas = current_shas
        self.current_sha_index = 0
        self.runs = runs or []
        self.cancellation_error = cancellation_error
        self.cancelled: list[int] = []
        self.listed = False

    def capture_context(
        self,
        *,
        run_id: int,
        run_sha: str,
        monotonic_now: object = None,
    ) -> advisory_supersession.ReconciliationContext:
        return advisory_supersession.ReconciliationContext(
            invoking_run=advisory_supersession.WorkflowRun(
                run_id,
                run_sha,
                "push",
                "in_progress",
            ),
            repository_id=456,
            repository_full_name="owner/repository",
            github_now=advisory_supersession.datetime.datetime(
                2026, 7, 22, tzinfo=advisory_supersession.datetime.UTC
            ),
            created_filter=">=2026-05-17T00:00:00Z",
            deadline=600.0,
        )

    def current_branch_sha(self) -> str:
        index = min(self.current_sha_index, len(self.current_shas) - 1)
        self.current_sha_index += 1
        return self.current_shas[index]

    def active_push_runs(
        self,
        context: advisory_supersession.ReconciliationContext,
        freshness_guard: object,
    ) -> list[advisory_supersession.WorkflowRun]:
        self.listed = True
        return self.runs

    def cancel_self(self, run_id: int) -> None:
        self.cancelled.append(run_id)

    def cancel_and_confirm(self, run_id: int, freshness_guard: object) -> None:
        if self.cancellation_error is not None:
            raise self.cancellation_error
        self.cancelled.append(run_id)


class CancellationClient(advisory_supersession.GitHubActionsClient):
    def __init__(self, statuses: list[str]) -> None:
        self.config = dataclasses.replace(
            GOVERNED_CONFIG,
            cancel_poll_attempts=1,
            cancel_poll_interval_seconds=1,
        )
        self.statuses = iter(statuses)
        self.requests: list[bool] = []

    def _request_cancel(self, run_id: int, *, force: bool) -> int:
        self.requests.append(force)
        return 202

    def _run_status(self, run_id: int) -> str:
        return next(self.statuses)


class DiscoveryClient(advisory_supersession.GitHubActionsClient):
    def __init__(
        self,
        sweeps: list[list[advisory_supersession.WorkflowRun]],
    ) -> None:
        self.config = dataclasses.replace(
            GOVERNED_CONFIG,
            cancel_poll_attempts=1,
            cancel_poll_interval_seconds=1,
        )
        self.sweeps = iter(sweeps)
        self.sleeps: list[float] = []
        self._sleep = self.sleeps.append

    def _workflow_run_sweep(
        self, context: advisory_supersession.ReconciliationContext
    ) -> list[advisory_supersession.WorkflowRun]:
        return next(self.sweeps)


class ExactRunClient(advisory_supersession.GitHubActionsClient):
    def __init__(
        self,
        *,
        document: dict[str, object],
        date: str | None = "Wed, 22 Jul 2026 12:00:00 GMT",
    ) -> None:
        self.config = GOVERNED_CONFIG
        self.repository = "owner/repository"
        self.document = document
        self.date = date
        self.requests: list[tuple[str, str]] = []

    def _request_json(
        self,
        method: str,
        path_or_url: str,
        **_kwargs: object,
    ) -> tuple[int, object, dict[str, str]]:
        self.requests.append((method, path_or_url))
        headers = {} if self.date is None else {"date": self.date}
        return 200, self.document, headers


def exact_run_document(**overrides: object) -> dict[str, object]:
    document: dict[str, object] = {
        "id": 123,
        "run_attempt": 2,
        "head_sha": "current",
        "head_branch": "main",
        "event": "push",
        "status": "in_progress",
        "path": ".github/workflows/advisory.yml@refs/heads/main",
        "created_at": "2026-05-20T12:00:00Z",
        "run_started_at": "2026-07-22T11:59:00Z",
        "repository": {"id": 456, "full_name": "owner/repository"},
    }
    document.update(overrides)
    return document


def census_run_document(
    run_id: int,
    *,
    attempt: int = 1,
    sha: str = "old",
    status: str = "in_progress",
) -> dict[str, object]:
    return {
        "id": run_id,
        "run_attempt": attempt,
        "head_sha": sha,
        "head_branch": "main",
        "event": "push",
        "status": status,
        "created_at": "2026-07-22T11:00:00Z",
    }


class CensusClient(advisory_supersession.GitHubActionsClient):
    def __init__(
        self,
        responses: list[tuple[dict[str, object], str | None]],
    ) -> None:
        self.config = GOVERNED_CONFIG
        self.repository = "owner/repository"
        self.responses = iter(responses)
        self.requests: list[str] = []
        self._sleep = lambda _seconds: None

    def _request_json(
        self,
        method: str,
        path_or_url: str,
        **_kwargs: object,
    ) -> tuple[int, object, dict[str, str]]:
        self.requests.append(path_or_url)
        document, link = next(self.responses)
        headers = {} if link is None else {"link": link}
        return 200, document, headers


def census_context() -> advisory_supersession.ReconciliationContext:
    return FakeClient(current_shas=["current"]).capture_context(
        run_id=123,
        run_sha="current",
    )


def census_url(
    *,
    page: int,
    numeric_repository_id: int | None = None,
    query_items: list[tuple[str, str]] | None = None,
) -> str:
    if numeric_repository_id is None:
        path = "/repos/owner/repository/actions/workflows/advisory.yml/runs"
    else:
        path = (
            f"/repositories/{numeric_repository_id}/actions/workflows/"
            "advisory.yml/runs"
        )
    items = query_items or [
        ("event", "push"),
        ("created", ">=2026-05-17T00:00:00Z"),
        ("per_page", "100"),
        ("branch", "main"),
        ("page", str(page)),
    ]
    return f"https://api.github.com{path}?{urllib.parse.urlencode(items)}"


def next_link(url: str) -> str:
    return f'<{url}>; rel="next"'


class GuardedCancellationClient(CancellationClient):
    def __init__(
        self,
        *,
        current_shas: list[str],
        runs: list[advisory_supersession.WorkflowRun],
    ) -> None:
        super().__init__(["in_progress"])
        self.current_shas = iter(current_shas)
        self.runs = runs
        self.self_cancellations: list[int] = []

    def capture_context(
        self,
        *,
        run_id: int,
        run_sha: str,
        monotonic_now: object = None,
    ) -> advisory_supersession.ReconciliationContext:
        return FakeClient(current_shas=[run_sha]).capture_context(
            run_id=run_id,
            run_sha=run_sha,
        )

    def current_branch_sha(self) -> str:
        return next(self.current_shas)

    def active_push_runs(
        self,
        context: advisory_supersession.ReconciliationContext,
        freshness_guard: object,
    ) -> list[advisory_supersession.WorkflowRun]:
        assert callable(freshness_guard)
        freshness_guard()
        freshness_guard()
        return self.runs

    def cancel_self(self, run_id: int) -> None:
        self.self_cancellations.append(run_id)


class ReconcileTests(unittest.TestCase):
    def discovery_context(self) -> advisory_supersession.ReconciliationContext:
        return FakeClient(current_shas=["current"]).capture_context(
            run_id=1,
            run_sha="current",
        )

    def test_discovery_repeats_until_transitioning_run_set_is_stable(self) -> None:
        pending = advisory_supersession.WorkflowRun(7, "old", "push", "pending")
        queued = advisory_supersession.WorkflowRun(7, "old", "push", "queued")
        client = DiscoveryClient([[pending], [queued]])
        guard_calls = 0

        def freshness_guard() -> None:
            nonlocal guard_calls
            guard_calls += 1

        runs = client.active_push_runs(self.discovery_context(), freshness_guard)

        self.assertEqual(runs, [queued])
        self.assertEqual(guard_calls, 4)

    def test_discovery_repeats_when_a_run_arrives_between_sweeps(self) -> None:
        first = advisory_supersession.WorkflowRun(8, "old", "push", "queued")
        second = advisory_supersession.WorkflowRun(9, "older", "push", "queued")
        client = DiscoveryClient([[first], [first, second], [first, second]])

        runs = client.active_push_runs(self.discovery_context(), lambda: None)

        self.assertEqual(runs, [first, second])

    def test_stale_rerun_cancels_only_itself(self) -> None:
        client = FakeClient(current_shas=["current"])

        with self.assertRaises(advisory_supersession.SupersededRun):
            advisory_supersession.reconcile(
                client,
                run_id=22,
                run_sha="old",
            )

        self.assertEqual(client.cancelled, [22])
        self.assertFalse(client.listed)

    def test_current_run_cancels_only_active_different_sha_pushes(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(11, "old", "push", "in_progress"),
            advisory_supersession.WorkflowRun(12, "unrelated-stale", "push", "queued"),
            advisory_supersession.WorkflowRun(13, "current", "push", "in_progress"),
            advisory_supersession.WorkflowRun(14, "old", "schedule", "in_progress"),
            advisory_supersession.WorkflowRun(15, "old", "push", "completed"),
        ]
        client = FakeClient(
            current_shas=["current", "current"],
            runs=runs,
        )

        result = advisory_supersession.reconcile(
            client,
            run_id=13,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, (11, 12))
        self.assertEqual(client.cancelled, [11, 12])

    def test_current_run_cancels_stale_nonancestor_after_stable_main_reads(
        self,
    ) -> None:
        runs = [
            advisory_supersession.WorkflowRun(
                16, "force-pushed-away", "push", "in_progress"
            ),
        ]
        client = FakeClient(current_shas=["current", "current"], runs=runs)

        result = advisory_supersession.reconcile(
            client,
            run_id=17,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, (16,))
        self.assertEqual(client.cancelled, [16])

    def test_current_run_cancels_a_rolled_back_descendant(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(18, "descendant", "push", "queued"),
        ]
        client = FakeClient(
            current_shas=["current", "current"],
            runs=runs,
        )

        result = advisory_supersession.reconcile(
            client,
            run_id=19,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, (18,))
        self.assertEqual(client.cancelled, [18])

    def test_run_that_becomes_stale_cannot_admit_heavy_jobs(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(34, "newer", "push", "queued"),
        ]
        client = FakeClient(current_shas=["current", "newer"], runs=runs)

        with self.assertRaises(advisory_supersession.SupersededRun):
            advisory_supersession.reconcile(
                client,
                run_id=33,
                run_sha="current",
            )

        self.assertEqual(client.cancelled, [33])

    def test_main_movement_before_cancellation_cancels_only_current_run(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(35, "newer", "push", "queued"),
        ]
        client = FakeClient(
            current_shas=["current", "current", "newer"],
            runs=runs,
        )

        with self.assertRaises(advisory_supersession.SupersededRun):
            advisory_supersession.reconcile(
                client,
                run_id=34,
                run_sha="current",
            )

        self.assertEqual(client.cancelled, [34])

    def test_final_main_movement_prevents_admission(self) -> None:
        client = FakeClient(current_shas=["current", "current", "newer"])

        with self.assertRaises(advisory_supersession.SupersededRun):
            advisory_supersession.reconcile(
                client,
                run_id=36,
                run_sha="current",
            )

        self.assertEqual(client.cancelled, [36])

    def test_unconfirmed_cancellation_fails_admission(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(37, "old", "push", "in_progress"),
        ]
        client = FakeClient(
            current_shas=["current"],
            runs=runs,
            cancellation_error=RuntimeError("run remained active"),
        )

        with self.assertRaisesRegex(RuntimeError, "remained active"):
            advisory_supersession.reconcile(
                client,
                run_id=38,
                run_sha="current",
            )

    def test_watchdog_cancels_a_legacy_stale_rerun(self) -> None:
        client = FakeClient(current_shas=["current", "current"])

        result = advisory_supersession.cancel_superseded_target(
            client,
            run_id=39,
            run_sha="old",
        )

        self.assertEqual(result.cancelled_run_ids, (39,))
        self.assertEqual(client.cancelled, [39])

    def test_watchdog_preserves_current_main_run(self) -> None:
        client = FakeClient(current_shas=["current"])

        result = advisory_supersession.cancel_superseded_target(
            client,
            run_id=40,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, ())
        self.assertEqual(client.cancelled, [])

    def test_client_force_cancels_when_normal_cancellation_stays_active(self) -> None:
        client = CancellationClient(["in_progress", "completed"])

        client.cancel_and_confirm(42, lambda: None)

        self.assertEqual(client.requests, [False, True])

    def test_client_fails_when_force_cancellation_stays_active(self) -> None:
        client = CancellationClient(["in_progress", "in_progress"])

        with self.assertRaisesRegex(RuntimeError, "remained active"):
            client.cancel_and_confirm(43, lambda: None)

        self.assertEqual(client.requests, [False, True])

    def test_client_refuses_force_cancel_when_freshness_changes_during_poll(
        self,
    ) -> None:
        client = CancellationClient(["in_progress"])
        guard_calls = 0

        def freshness_guard() -> None:
            nonlocal guard_calls
            guard_calls += 1
            if guard_calls == 2:
                raise advisory_supersession.SupersededRun("main moved")

        with self.assertRaises(advisory_supersession.SupersededRun):
            client.cancel_and_confirm(44, freshness_guard)

        self.assertEqual(client.requests, [False])

    def test_client_treats_unknown_status_as_unconfirmed(self) -> None:
        client = CancellationClient(["new-active-state", "new-active-state"])

        with self.assertRaisesRegex(RuntimeError, "remained active"):
            client.cancel_and_confirm(45, lambda: None)

        self.assertEqual(client.requests, [False, True])

    def test_controller_does_not_force_cancel_after_main_moves_during_poll(
        self,
    ) -> None:
        stale = advisory_supersession.WorkflowRun(50, "old", "push", "in_progress")
        client = GuardedCancellationClient(
            current_shas=[
                "current",
                "current",
                "current",
                "current",
                "current",
                "current",
                "newer",
            ],
            runs=[stale],
        )

        with self.assertRaises(advisory_supersession.SupersededRun):
            advisory_supersession.reconcile(client, run_id=51, run_sha="current")

        self.assertEqual(client.requests, [False])
        self.assertEqual(client.self_cancellations, [51])

    def test_watchdog_does_not_force_cancel_after_main_moves_during_poll(
        self,
    ) -> None:
        client = GuardedCancellationClient(
            current_shas=["current", "current", "current", "newer"],
            runs=[],
        )

        with self.assertRaisesRegex(RuntimeError, "main moved"):
            advisory_supersession.cancel_superseded_target(
                client,
                run_id=52,
                run_sha="old",
            )

        self.assertEqual(client.requests, [False])
        self.assertEqual(client.self_cancellations, [])

    def test_watchdog_refuses_cancellation_when_main_moves(self) -> None:
        client = FakeClient(current_shas=["current", "newer"])

        with self.assertRaisesRegex(RuntimeError, "main moved"):
            advisory_supersession.cancel_superseded_target(
                client,
                run_id=41,
                run_sha="old",
            )

        self.assertEqual(client.cancelled, [])

    def test_same_sha_rerun_is_not_cancelled_by_current_run(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(41, "current", "push", "queued"),
        ]
        client = FakeClient(
            current_shas=["current", "current"],
            runs=runs,
        )

        result = advisory_supersession.reconcile(
            client,
            run_id=42,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, ())
        self.assertEqual(client.cancelled, [])


class ConfigTests(unittest.TestCase):
    def write_config(self, text: str) -> pathlib.Path:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = pathlib.Path(directory.name) / "config.toml"
        path.write_text(text, encoding="utf-8")
        return path

    def test_rejects_boolean_schema_version(self) -> None:
        path = self.write_config(
            """
schema_version = true
api_version = "2026-03-10"
branch = "main"
workflow = "advisory.yml"
request_timeout_seconds = 30
terminal_status = "completed"
"""
        )

        with self.assertRaisesRegex(ValueError, "schema_version"):
            advisory_supersession.load_config(path)

    def test_rejects_unknown_config_key(self) -> None:
        path = self.write_config(
            """
schema_version = 1
api_version = "2026-03-10"
branch = "main"
workflow = "advisory.yml"
request_timeout_seconds = 30
terminal_status = "completed"
alternate_api_url = "https://example.com"
"""
        )

        with self.assertRaisesRegex(ValueError, "unknown config keys"):
            advisory_supersession.load_config(path)

    def test_loads_governed_repository_config(self) -> None:
        config = GOVERNED_CONFIG

        self.assertEqual(config.branch, "main")
        self.assertEqual(config.workflow, "advisory.yml")
        self.assertEqual(config.event, "push")

    def test_governed_topology_is_exact(self) -> None:
        topology = advisory_supersession.reconciliation_topology(GOVERNED_CONFIG)

        self.assertEqual(topology.requests, 358)
        self.assertEqual(topology.secondary_points, 438)
        self.assertEqual(topology.minimum_pacing_seconds, 260)

    def test_rejects_runs_per_page_other_than_one_hundred(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(governed.replace("runs_per_page = 100", "runs_per_page = 99"))

        with self.assertRaisesRegex(ValueError, "runs_per_page"):
            advisory_supersession.load_config(path)

    def test_rejects_lookback_without_margin(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(
            governed.replace("created_lookback_days = 66", "created_lookback_days = 65")
        )

        with self.assertRaisesRegex(ValueError, "created_lookback_days"):
            advisory_supersession.load_config(path)

    def test_rejects_search_threshold_at_platform_limit(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(
            governed.replace("max_search_results = 900", "max_search_results = 1000")
        )

        with self.assertRaisesRegex(ValueError, "max_search_results"):
            advisory_supersession.load_config(path)

    def test_rejects_request_ceiling_below_topology(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(
            governed.replace("max_total_requests = 400", "max_total_requests = 357")
        )

        with self.assertRaisesRegex(ValueError, "max_total_requests"):
            advisory_supersession.load_config(path)

    def test_rejects_secondary_ceiling_below_topology(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(
            governed.replace("max_secondary_points = 500", "max_secondary_points = 437")
        )

        with self.assertRaisesRegex(ValueError, "max_secondary_points"):
            advisory_supersession.load_config(path)

    def test_rejects_deadline_below_minimum_pacing(self) -> None:
        governed = (
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        ).read_text(encoding="utf-8")
        path = self.write_config(
            governed.replace(
                "reconciliation_timeout_seconds = 600",
                "reconciliation_timeout_seconds = 259",
            )
        )

        with self.assertRaisesRegex(ValueError, "reconciliation_timeout_seconds"):
            advisory_supersession.load_config(path)

    def test_client_refuses_to_send_token_to_external_pagination_url(self) -> None:
        config = advisory_supersession.load_config(
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        )
        client = advisory_supersession.GitHubActionsClient(
            config=config,
            repository="owner/repository",
            token="test-token",
        )

        with self.assertRaisesRegex(ValueError, "outside the GitHub API"):
            client._request_json("GET", "https://example.com/next")


class ExactRunTests(unittest.TestCase):
    def test_captures_exact_run_identity_and_fixed_github_cutoff(self) -> None:
        client = ExactRunClient(document=exact_run_document())

        context = client.capture_context(
            run_id=123,
            run_sha="current",
            monotonic_now=lambda: 10.0,
        )

        self.assertEqual(context.invoking_run.run_attempt, 2)
        self.assertEqual(context.repository_id, 456)
        self.assertEqual(context.repository_full_name, "owner/repository")
        self.assertEqual(context.created_filter, ">=2026-05-17T12:00:00Z")
        self.assertEqual(context.deadline, 610.0)
        self.assertEqual(
            client.requests,
            [("GET", "repos/owner/repository/actions/runs/123")],
        )

    def test_cutoff_uses_http_date_not_old_rerun_creation(self) -> None:
        client = ExactRunClient(
            document=exact_run_document(created_at="2026-04-01T12:00:00Z")
        )

        context = client.capture_context(run_id=123, run_sha="current")

        self.assertEqual(context.created_filter, ">=2026-05-17T12:00:00Z")

    def test_rejects_non_positive_attempt(self) -> None:
        client = ExactRunClient(document=exact_run_document(run_attempt=0))

        with self.assertRaisesRegex(ValueError, "attempt"):
            client.capture_context(run_id=123, run_sha="current")

    def test_rejects_wrong_repository_identity(self) -> None:
        client = ExactRunClient(
            document=exact_run_document(
                repository={"id": 456, "full_name": "owner/foreign"}
            )
        )

        with self.assertRaisesRegex(ValueError, "repository"):
            client.capture_context(run_id=123, run_sha="current")

    def test_rejects_wrong_workflow(self) -> None:
        client = ExactRunClient(
            document=exact_run_document(path=".github/workflows/other.yml")
        )

        with self.assertRaisesRegex(ValueError, "workflow"):
            client.capture_context(run_id=123, run_sha="current")

    def test_rejects_github_date_before_run_timestamps(self) -> None:
        client = ExactRunClient(
            document=exact_run_document(),
            date="Wed, 22 Jul 2026 11:58:00 GMT",
        )

        with self.assertRaisesRegex(ValueError, "precedes"):
            client.capture_context(run_id=123, run_sha="current")

    def test_rejects_missing_github_date(self) -> None:
        client = ExactRunClient(document=exact_run_document(), date=None)

        with self.assertRaisesRegex(ValueError, "HTTP Date"):
            client.capture_context(run_id=123, run_sha="current")

    def test_rejects_malformed_or_mismatched_run_identity(self) -> None:
        cases = {
            "id": ({"id": 124}, "id"),
            "sha": ({"head_sha": "other"}, "SHA"),
            "branch": ({"head_branch": "other"}, "branch"),
            "event": ({"event": "schedule"}, "event"),
            "created timestamp": ({"created_at": "invalid"}, "created_at"),
            "started timestamp": ({"run_started_at": "invalid"}, "run_started_at"),
        }
        for label, (overrides, message) in cases.items():
            with self.subTest(label=label):
                client = ExactRunClient(document=exact_run_document(**overrides))
                with self.assertRaisesRegex(ValueError, message):
                    client.capture_context(run_id=123, run_sha="current")


class CensusTests(unittest.TestCase):
    def test_accepts_complete_multi_page_configured_repository_census(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(census_url(page=2)),
                ),
                (
                    {
                        "total_count": 2,
                        "workflow_runs": [
                            census_run_document(123, sha="current")
                        ],
                    },
                    None,
                ),
            ]
        )

        runs = client._workflow_run_sweep(census_context())

        self.assertEqual([run.run_id for run in runs], [7, 123])
        self.assertEqual(len(client.requests), 2)

    def test_accepts_canonical_numeric_repository_link(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(census_url(page=2, numeric_repository_id=456)),
                ),
                (
                    {
                        "total_count": 2,
                        "workflow_runs": [
                            census_run_document(123, sha="current")
                        ],
                    },
                    None,
                ),
            ]
        )

        runs = client._workflow_run_sweep(census_context())

        self.assertEqual([run.run_id for run in runs], [7, 123])

    def test_rejects_wrong_numeric_repository_link(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(census_url(page=2, numeric_repository_id=999)),
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "repository"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_duplicate_query_authority(self) -> None:
        duplicate_branch = census_url(
            page=2,
            query_items=[
                ("branch", "main"),
                ("branch", "main"),
                ("event", "push"),
                ("created", ">=2026-05-17T00:00:00Z"),
                ("per_page", "100"),
                ("page", "2"),
            ],
        )
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(duplicate_branch),
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "branch"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_foreign_pagination_origin(self) -> None:
        foreign = census_url(page=2).replace(
            "https://api.github.com", "https://example.com"
        )
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(foreign),
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "foreign origin"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_duplicate_page_boundary(self) -> None:
        client = CensusClient(
            [
                (
                    {
                        "total_count": 2,
                        "workflow_runs": [census_run_document(7)],
                    },
                    next_link(census_url(page=2)),
                ),
                (
                    {
                        "total_count": 2,
                        "workflow_runs": [census_run_document(7)],
                    },
                    None,
                ),
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "duplicate id"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_missing_continuation(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(123, sha="current")]},
                    None,
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "ended before"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_extra_continuation(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 1, "workflow_runs": [census_run_document(123, sha="current")]},
                    next_link(census_url(page=2)),
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "continued after"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_total_count_drift(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(census_url(page=2)),
                ),
                (
                    {"total_count": 3, "workflow_runs": [census_run_document(123, sha="current")]},
                    None,
                ),
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "total_count changed"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_total_count_at_governed_threshold(self) -> None:
        client = CensusClient(
            [({"total_count": 900, "workflow_runs": []}, None)]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "search threshold"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_missing_exact_run_sentinel(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 1, "workflow_runs": [census_run_document(7)]},
                    None,
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "sentinel"
        ):
            client._workflow_run_sweep(census_context())

    def test_accepts_exact_page_multiple_without_continuation(self) -> None:
        records = [census_run_document(run_id) for run_id in range(1, 101)]
        records[99] = census_run_document(123, sha="current")
        client = CensusClient(
            [({"total_count": 100, "workflow_runs": records}, None)]
        )

        runs = client._workflow_run_sweep(census_context())

        self.assertEqual(len(runs), 100)

    def test_accepts_count_immediately_below_threshold_with_nine_pages(self) -> None:
        records = [census_run_document(run_id) for run_id in range(1, 900)]
        records[122] = census_run_document(123, sha="current")
        responses: list[tuple[dict[str, object], str | None]] = []
        for page_index in range(9):
            page_records = records[page_index * 100 : (page_index + 1) * 100]
            link = (
                next_link(census_url(page=page_index + 2))
                if page_index < 8
                else None
            )
            responses.append(
                ({"total_count": 899, "workflow_runs": page_records}, link)
            )
        client = CensusClient(responses)

        runs = client._workflow_run_sweep(census_context())

        self.assertEqual(len(runs), 899)
        self.assertEqual(len(client.requests), 9)

    def test_rejects_missing_changed_and_unknown_query_authority(self) -> None:
        cases = {
            "missing created": [
                ("branch", "main"),
                ("event", "push"),
                ("per_page", "100"),
                ("page", "2"),
            ],
            "changed event": [
                ("branch", "main"),
                ("event", "schedule"),
                ("created", ">=2026-05-17T00:00:00Z"),
                ("per_page", "100"),
                ("page", "2"),
            ],
            "unknown key": [
                ("branch", "main"),
                ("event", "push"),
                ("created", ">=2026-05-17T00:00:00Z"),
                ("per_page", "100"),
                ("page", "2"),
                ("status", "in_progress"),
            ],
        }
        for label, query_items in cases.items():
            with self.subTest(label=label):
                client = CensusClient(
                    [
                        (
                            {
                                "total_count": 2,
                                "workflow_runs": [census_run_document(7)],
                            },
                            next_link(
                                census_url(page=2, query_items=query_items)
                            ),
                        )
                    ]
                )
                with self.assertRaises(advisory_supersession.IncompleteCensus):
                    client._workflow_run_sweep(census_context())

    def test_rejects_semantically_repeated_page_url(self) -> None:
        repeated_first_page = census_url(
            page=1,
            query_items=[
                ("page", "1"),
                ("per_page", "100"),
                ("created", ">=2026-05-17T00:00:00Z"),
                ("event", "push"),
                ("branch", "main"),
            ],
        )
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(repeated_first_page),
                )
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "repeated a page"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_empty_intermediate_page(self) -> None:
        client = CensusClient(
            [
                (
                    {"total_count": 2, "workflow_runs": [census_run_document(7)]},
                    next_link(census_url(page=2)),
                ),
                (
                    {"total_count": 2, "workflow_runs": []},
                    next_link(census_url(page=3)),
                ),
            ]
        )

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "empty"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_page_budget_exhaustion(self) -> None:
        responses = []
        for page_index in range(9):
            responses.append(
                (
                    {
                        "total_count": 899,
                        "workflow_runs": [census_run_document(page_index + 1)],
                    },
                    next_link(census_url(page=page_index + 2)),
                )
            )
        client = CensusClient(responses)

        with self.assertRaisesRegex(
            advisory_supersession.IncompleteCensus, "page budget"
        ):
            client._workflow_run_sweep(census_context())

    def test_rejects_changed_sentinel_attempt_or_sha(self) -> None:
        for label, sentinel in {
            "attempt": census_run_document(123, attempt=2, sha="current"),
            "sha": census_run_document(123, sha="other"),
        }.items():
            with self.subTest(label=label):
                client = CensusClient(
                    [({"total_count": 1, "workflow_runs": [sentinel]}, None)]
                )
                with self.assertRaisesRegex(
                    advisory_supersession.IncompleteCensus, "sentinel identity"
                ):
                    client._workflow_run_sweep(census_context())

    def test_rejects_duplicate_or_malformed_next_links(self) -> None:
        url = census_url(page=2)
        for label, link in {
            "duplicate": f'{next_link(url)}, {next_link(url)}',
            "malformed": f'{url}; rel="next"',
        }.items():
            with self.subTest(label=label):
                client = CensusClient(
                    [
                        (
                            {
                                "total_count": 2,
                                "workflow_runs": [census_run_document(7)],
                            },
                            link,
                        )
                    ]
                )
                with self.assertRaises(advisory_supersession.IncompleteCensus):
                    client._workflow_run_sweep(census_context())


if __name__ == "__main__":
    unittest.main()
