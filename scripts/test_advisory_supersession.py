#!/usr/bin/env python3

from __future__ import annotations

import pathlib
import sys
import tempfile
import unittest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

import advisory_supersession  # noqa: E402


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

    def current_branch_sha(self) -> str:
        index = min(self.current_sha_index, len(self.current_shas) - 1)
        self.current_sha_index += 1
        return self.current_shas[index]

    def active_push_runs(
        self, freshness_guard: object
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
        self.config = advisory_supersession.Config(
            api_version="2026-03-10",
            branch="main",
            workflow="advisory.yml",
            request_timeout_seconds=30,
            runs_per_page=100,
            discovery_stable_sweeps=2,
            discovery_max_sweeps=4,
            cancel_poll_attempts=1,
            cancel_poll_interval_seconds=1,
            terminal_status="completed",
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
        self.config = advisory_supersession.Config(
            api_version="2026-03-10",
            branch="main",
            workflow="advisory.yml",
            request_timeout_seconds=30,
            runs_per_page=100,
            discovery_stable_sweeps=2,
            discovery_max_sweeps=4,
            cancel_poll_attempts=1,
            cancel_poll_interval_seconds=1,
            terminal_status="completed",
        )
        self.sweeps = iter(sweeps)

    def _workflow_run_sweep(self) -> list[advisory_supersession.WorkflowRun]:
        return next(self.sweeps)


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

    def current_branch_sha(self) -> str:
        return next(self.current_shas)

    def active_push_runs(
        self, freshness_guard: object
    ) -> list[advisory_supersession.WorkflowRun]:
        assert callable(freshness_guard)
        freshness_guard()
        freshness_guard()
        return self.runs

    def cancel_self(self, run_id: int) -> None:
        self.self_cancellations.append(run_id)


class ReconcileTests(unittest.TestCase):
    def test_discovery_repeats_until_transitioning_run_set_is_stable(self) -> None:
        pending = advisory_supersession.WorkflowRun(7, "old", "push", "pending")
        queued = advisory_supersession.WorkflowRun(7, "old", "push", "queued")
        client = DiscoveryClient([[pending], [queued]])
        guard_calls = 0

        def freshness_guard() -> None:
            nonlocal guard_calls
            guard_calls += 1

        runs = client.active_push_runs(freshness_guard)

        self.assertEqual(runs, [queued])
        self.assertEqual(guard_calls, 4)

    def test_discovery_repeats_when_a_run_arrives_between_sweeps(self) -> None:
        first = advisory_supersession.WorkflowRun(8, "old", "push", "queued")
        second = advisory_supersession.WorkflowRun(9, "older", "push", "queued")
        client = DiscoveryClient([[first], [first, second], [first, second]])

        runs = client.active_push_runs(lambda: None)

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
        config = advisory_supersession.load_config(
            REPO_ROOT / "ci" / "advisory-supersession.toml"
        )

        self.assertEqual(config.branch, "main")
        self.assertEqual(config.workflow, "advisory.yml")

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


if __name__ == "__main__":
    unittest.main()
