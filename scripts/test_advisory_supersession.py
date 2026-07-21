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
        ancestors: set[tuple[str, str]] | None = None,
    ) -> None:
        self.current_shas = iter(current_shas)
        self.runs = runs or []
        self.ancestors = ancestors or set()
        self.cancelled: list[int] = []
        self.listed = False

    def current_branch_sha(self) -> str:
        return next(self.current_shas)

    def active_push_runs(self) -> list[advisory_supersession.WorkflowRun]:
        self.listed = True
        return self.runs

    def is_ancestor(self, older_sha: str, newer_sha: str) -> bool:
        return (older_sha, newer_sha) in self.ancestors

    def cancel_run(self, run_id: int) -> bool:
        self.cancelled.append(run_id)
        return True


class ReconcileTests(unittest.TestCase):
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

    def test_current_run_does_not_cancel_a_known_descendant(self) -> None:
        runs = [
            advisory_supersession.WorkflowRun(18, "descendant", "push", "queued"),
        ]
        client = FakeClient(
            current_shas=["current", "current"],
            runs=runs,
            ancestors={("current", "descendant")},
        )

        result = advisory_supersession.reconcile(
            client,
            run_id=19,
            run_sha="current",
        )

        self.assertEqual(result.cancelled_run_ids, ())
        self.assertEqual(client.cancelled, [])

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
active_statuses = ["in_progress"]
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
active_statuses = ["in_progress"]
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
