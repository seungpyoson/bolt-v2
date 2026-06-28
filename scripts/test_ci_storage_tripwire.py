from __future__ import annotations

import contextlib
import io
import importlib.util
import json
import pathlib
import sys
import tempfile
import textwrap
import unittest
from typing import Any, Mapping


SCRIPT = pathlib.Path(__file__).with_name("ci_storage_tripwire.py")
if not SCRIPT.exists():
    raise AssertionError("scripts/ci_storage_tripwire.py is missing")
spec = importlib.util.spec_from_file_location("ci_storage_tripwire", SCRIPT)
assert spec is not None
ci_storage_tripwire = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = ci_storage_tripwire
assert spec.loader is not None
spec.loader.exec_module(ci_storage_tripwire)

VERIFIER_SCRIPT = pathlib.Path(__file__).with_name("verify_ci_workflow_hygiene.py")
verifier_spec = importlib.util.spec_from_file_location("verify_ci_workflow_hygiene", VERIFIER_SCRIPT)
assert verifier_spec is not None
verify_ci_workflow_hygiene = importlib.util.module_from_spec(verifier_spec)
sys.modules[verifier_spec.name] = verify_ci_workflow_hygiene
assert verifier_spec.loader is not None
verifier_spec.loader.exec_module(verify_ci_workflow_hygiene)


class FakeIssueClient:
    def __init__(self, matches_by_marker: dict[str, list[dict[str, Any]]] | None = None) -> None:
        self.matches_by_marker = matches_by_marker or {}
        self.calls: list[tuple[str, Any]] = []
        self.created: list[dict[str, Any]] = []
        self.edited: list[dict[str, Any]] = []

    def find_open_issues_by_marker(
        self, *, marker: str, result_limit: int, page_size: int
    ) -> list[dict[str, Any]]:
        self.calls.append(
            ("find_open_issues_by_marker", (marker, result_limit, page_size))
        )
        return self.matches_by_marker.get(marker, [])[:result_limit]

    def create_issue(self, *, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        record = {"number": 100 + len(self.created), "title": title, "body": body, "labels": labels}
        self.calls.append(("create_issue", record))
        self.created.append(record)
        return record

    def edit_issue(self, *, number: int, title: str, body: str, labels: list[str]) -> dict[str, Any]:
        record = {"number": number, "title": title, "body": body, "labels": labels}
        self.calls.append(("edit_issue", record))
        self.edited.append(record)
        return record


class RecordingGhIssueClient(ci_storage_tripwire.GhIssueClient):
    def __init__(self, response: Any) -> None:
        super().__init__("owner/repo")
        self.response = response
        self.api_calls: list[
            tuple[str, str, Mapping[str, Any] | None, Mapping[str, Any] | None, bool]
        ] = []

    def api(
        self,
        method: str,
        path: str,
        payload: Mapping[str, Any] | None = None,
        fields: Mapping[str, Any] | None = None,
        paginate: bool = False,
    ) -> Any:
        self.api_calls.append((method, path, payload, fields, paginate))
        return self.response


class CiStorageTripwireTests(unittest.TestCase):
    def write_policy(self, directory: pathlib.Path, body: str | None = None) -> pathlib.Path:
        policy = directory / "policy.toml"
        policy.write_text(
            body
            or textwrap.dedent(
                """
                [storage_tripwire]
                schema_version = 1
                policy_id = "ci-storage-tripwire"
                storage_cap_bytes = 10737418240
                cap_source = "operator policy"
                owner = "@ci-owner"
                escalation = "Inspect the P0 ci-storage-audit JSON and open the relevant control issue."
                update_cadence = "weekly scheduled run plus workflow_dispatch"
                issue_labels = ["infra", "ops", "github_actions"]

                [storage_tripwire.issue_match]
                result_limit = 2
                max_open_matches_per_marker = 1
                page_size = 50

                [storage_tripwire.marker]
                prefix = "<!-- ci-storage-tripwire:"
                suffix = " -->"

                [storage_tripwire.workflow]
                path = ".github/workflows/ci-storage-tripwire.yml"
                job_id = "storage-tripwire"
                runner_var = "CI_RUNNER_GITHUB_HOSTED"
                schedule_cron = "17 9 * * 1"
                triggers = ["schedule", "workflow_dispatch"]
                permissions = { contents = "read", actions = "read", issues = "write" }
                required_fragments = ["python3 scripts/ci_storage_tripwire.py", "apply-live"]
                forbidden_fragments = ["actions/artifacts", "statuses/"]

                [storage_tripwire.metrics.cache]
                label = "Actions cache listed bytes"
                json_paths = ["cache.total_bytes"]

                [storage_tripwire.metrics.artifacts]
                label = "Actions artifact listed bytes"
                json_paths = ["artifacts.total_bytes"]

                [storage_tripwire.metrics.total]
                label = "Actions cache plus artifact listed bytes"
                json_paths = ["cache.total_bytes", "artifacts.total_bytes"]

                [[storage_tripwire.thresholds]]
                id = "cache-over-cap"
                metric = "cache"
                limit_bytes = 1000
                severity = "warning"
                title = "CI storage tripwire: cache threshold crossed"

                [[storage_tripwire.thresholds]]
                id = "artifact-over-cap"
                metric = "artifacts"
                limit_bytes = 2000
                severity = "critical"
                title = "CI storage tripwire: artifact threshold crossed"

                [[storage_tripwire.thresholds]]
                id = "total-over-cap"
                metric = "total"
                limit_bytes = 10000
                severity = "critical"
                title = "CI storage tripwire: total threshold crossed"
                """
            ).lstrip(),
            encoding="utf-8",
        )
        return policy

    def write_audit(self, directory: pathlib.Path, cache_bytes: int, artifact_bytes: int) -> pathlib.Path:
        audit = directory / "audit.json"
        audit.write_text(
            json.dumps(
                {
                    "snapshot_utc": "2026-06-28T11:21:08+00:00",
                    "repo": "owner/repo",
                    "cache": {"total_bytes": cache_bytes, "count": 3},
                    "artifacts": {"total_bytes": artifact_bytes, "count": 7},
                }
            ),
            encoding="utf-8",
        )
        return audit

    def workflow_text(self) -> str:
        return textwrap.dedent(
            """
            name: CI Storage Tripwire

            on:
              schedule:
                - cron: "17 9 * * 1"
              workflow_dispatch:

            permissions:
              contents: read
              actions: read
              issues: write

            concurrency:
              group: ci-storage-tripwire
              cancel-in-progress: false

            jobs:
              storage-tripwire:
                name: storage-tripwire
                runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
                steps:
                  - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
                    with:
                      persist-credentials: false
                  - name: Run storage tripwire
                    env:
                      GH_TOKEN: ${{ github.token }}
                      GITHUB_REPOSITORY: ${{ github.repository }}
                      GITHUB_REF_NAME: ${{ github.ref_name }}
                    run: |
                      python3 scripts/ci_storage_tripwire.py apply-live --repo "$GITHUB_REPOSITORY" --branch "$GITHUB_REF_NAME"
            """
        ).lstrip()

    def test_evaluate_reports_only_fixed_threshold_breaches_from_policy_metrics(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy = ci_storage_tripwire.load_policy(self.write_policy(root))
            audit = ci_storage_tripwire.load_audit_json(self.write_audit(root, 1200, 1500))

            evaluation = ci_storage_tripwire.evaluate_tripwire(policy, audit)

        self.assertTrue(evaluation["breached"])
        self.assertEqual(evaluation["snapshot_utc"], "2026-06-28T11:21:08+00:00")
        self.assertEqual(evaluation["repo"], "owner/repo")
        self.assertEqual(evaluation["metrics"]["cache"]["actual_bytes"], 1200)
        self.assertEqual(evaluation["metrics"]["artifacts"]["actual_bytes"], 1500)
        self.assertEqual(evaluation["metrics"]["total"]["actual_bytes"], 2700)
        threshold_status = {
            threshold["id"]: threshold["breached"] for threshold in evaluation["thresholds"]
        }
        self.assertEqual(
            threshold_status,
            {
                "cache-over-cap": True,
                "artifact-over-cap": False,
                "total-over-cap": False,
            },
        )

    def test_evaluate_cli_does_not_require_issue_alert_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(root)
            audit_path = self.write_audit(root, 500, 600)
            stdout = io.StringIO()

            with contextlib.redirect_stdout(stdout):
                exit_code = ci_storage_tripwire.main(
                    [
                        "--policy",
                        str(policy_path),
                        "evaluate",
                        "--audit-json",
                        str(audit_path),
                        "--json",
                    ]
                )

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertFalse(payload["evaluation"]["breached"])

    def test_policy_discovery_fails_closed_when_git_inventory_is_unavailable(self) -> None:
        original_run = ci_storage_tripwire.subprocess.run

        class FailedGitInventory:
            returncode = 1
            stdout = ""
            stderr = "fatal: not a git repository"

        def fail_git_inventory(*_args: Any, **_kwargs: Any) -> FailedGitInventory:
            return FailedGitInventory()

        ci_storage_tripwire.subprocess.run = fail_git_inventory  # type: ignore[assignment]
        try:
            with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "git ls-files"):
                ci_storage_tripwire.repository_toml_paths(pathlib.Path("."))
        finally:
            ci_storage_tripwire.subprocess.run = original_run  # type: ignore[assignment]

    def test_apply_cli_exits_zero_after_successful_breach_alerting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(root)
            audit_path = self.write_audit(root, 1200, 600)
            fake = FakeIssueClient()
            stdout = io.StringIO()

            original_client = ci_storage_tripwire.GhIssueClient

            def fake_client(repo: str) -> FakeIssueClient:
                self.assertEqual(repo, "owner/repo")
                return fake

            ci_storage_tripwire.GhIssueClient = fake_client  # type: ignore[assignment]
            try:
                with contextlib.redirect_stdout(stdout):
                    exit_code = ci_storage_tripwire.main(
                        [
                            "--policy",
                            str(policy_path),
                            "apply",
                            "--audit-json",
                            str(audit_path),
                            "--json",
                        ]
                    )
            finally:
                ci_storage_tripwire.GhIssueClient = original_client  # type: ignore[assignment]

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertTrue(payload["evaluation"]["breached"])
        self.assertEqual(payload["alerts"], {"created": [100], "updated": [], "unchanged": []})
        self.assertEqual(len(fake.created), 1)

    def test_apply_live_cli_exits_zero_after_successful_breach_alerting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(root)
            fake = FakeIssueClient()
            stdout = io.StringIO()

            original_client = ci_storage_tripwire.GhIssueClient
            original_build_live_audit = ci_storage_tripwire.build_live_audit

            def fake_client(repo: str) -> FakeIssueClient:
                self.assertEqual(repo, "owner/repo")
                return fake

            def fake_build_live_audit(repo: str, branch: str) -> dict[str, Any]:
                self.assertEqual(repo, "owner/repo")
                self.assertEqual(branch, "main")
                return {
                    "snapshot_utc": "2026-06-28T11:21:08+00:00",
                    "repo": repo,
                    "cache": {"total_bytes": 1200, "count": 3},
                    "artifacts": {"total_bytes": 600, "count": 7},
                }

            ci_storage_tripwire.GhIssueClient = fake_client  # type: ignore[assignment]
            ci_storage_tripwire.build_live_audit = fake_build_live_audit  # type: ignore[assignment]
            try:
                with contextlib.redirect_stdout(stdout):
                    exit_code = ci_storage_tripwire.main(
                        [
                            "--policy",
                            str(policy_path),
                            "apply-live",
                            "--repo",
                            "owner/repo",
                            "--branch",
                            "main",
                            "--json",
                        ]
                    )
            finally:
                ci_storage_tripwire.GhIssueClient = original_client  # type: ignore[assignment]
                ci_storage_tripwire.build_live_audit = original_build_live_audit  # type: ignore[assignment]

        self.assertEqual(exit_code, 0)
        payload = json.loads(stdout.getvalue())
        self.assertTrue(payload["evaluation"]["breached"])
        self.assertEqual(payload["alerts"], {"created": [100], "updated": [], "unchanged": []})

    def test_apply_updates_existing_marker_issue_and_creates_missing_breach_issue(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy = ci_storage_tripwire.load_policy(self.write_policy(root))
            audit = ci_storage_tripwire.load_audit_json(self.write_audit(root, 1200, 2500))
            evaluation = ci_storage_tripwire.evaluate_tripwire(policy, audit)

        fake = FakeIssueClient(
            {
                "<!-- ci-storage-tripwire:cache-over-cap -->": [
                    {
                        "number": 42,
                        "title": "old cache title",
                        "body": "<!-- ci-storage-tripwire:cache-over-cap -->\nold body",
                    }
                ]
            }
        )

        result = ci_storage_tripwire.apply_alerts(policy, evaluation, fake)

        self.assertEqual(result["created"], [100])
        self.assertEqual(result["updated"], [42])
        find_calls = [call for call in fake.calls if call[0] == "find_open_issues_by_marker"]
        self.assertEqual(
            find_calls,
            [
                ("find_open_issues_by_marker", ("<!-- ci-storage-tripwire:cache-over-cap -->", 2, 50)),
                ("find_open_issues_by_marker", ("<!-- ci-storage-tripwire:artifact-over-cap -->", 2, 50)),
            ],
        )
        self.assertEqual(len(fake.edited), 1)
        self.assertEqual(fake.edited[0]["number"], 42)
        self.assertIn("<!-- ci-storage-tripwire:cache-over-cap -->", fake.edited[0]["body"])
        self.assertIn("No storage mutation was performed.", fake.edited[0]["body"])
        self.assertEqual(len(fake.created), 1)
        self.assertEqual(fake.created[0]["title"], "CI storage tripwire: artifact threshold crossed")
        self.assertIn("<!-- ci-storage-tripwire:artifact-over-cap -->", fake.created[0]["body"])
        self.assertEqual(fake.created[0]["labels"], ["infra", "ops", "github_actions"])

    def test_apply_updates_existing_marker_issue_when_labels_drift(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy = ci_storage_tripwire.load_policy(self.write_policy(root))
            audit = ci_storage_tripwire.load_audit_json(self.write_audit(root, 1200, 600))
            evaluation = ci_storage_tripwire.evaluate_tripwire(policy, audit)
        threshold = next(item for item in evaluation["thresholds"] if item["id"] == "cache-over-cap")
        marker = "<!-- ci-storage-tripwire:cache-over-cap -->"
        body = ci_storage_tripwire.render_issue_body(policy, evaluation, threshold)
        fake = FakeIssueClient(
            {
                marker: [
                    {
                        "number": 42,
                        "title": threshold["title"],
                        "body": body,
                        "labels": [{"name": "infra"}],
                    }
                ]
            }
        )

        result = ci_storage_tripwire.apply_alerts(policy, evaluation, fake)

        self.assertEqual(result, {"created": [], "updated": [42], "unchanged": []})
        self.assertEqual(fake.edited[0]["labels"], ["infra", "ops", "github_actions"])

    def test_apply_rejects_duplicate_marker_matches(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy = ci_storage_tripwire.load_policy(self.write_policy(root))
            audit = ci_storage_tripwire.load_audit_json(self.write_audit(root, 1200, 600))
            evaluation = ci_storage_tripwire.evaluate_tripwire(policy, audit)

        fake = FakeIssueClient(
            {
                "<!-- ci-storage-tripwire:cache-over-cap -->": [
                    {
                        "number": 41,
                        "title": "cache title",
                        "body": "<!-- ci-storage-tripwire:cache-over-cap -->\nold body",
                    },
                    {
                        "number": 42,
                        "title": "cache duplicate",
                        "body": "<!-- ci-storage-tripwire:cache-over-cap -->\nold body",
                    },
                ]
            }
        )

        with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "multiple open issues"):
            ci_storage_tripwire.apply_alerts(policy, evaluation, fake)

    def test_github_issue_client_lists_open_issues_by_marker(self) -> None:
        marker = "<!-- ci-storage-tripwire:cache-over-cap -->"
        client = RecordingGhIssueClient(
            [
                [
                    {"number": 6, "body": "different marker"},
                    {"number": 7, "body": marker},
                    {
                        "number": 8,
                        "body": marker,
                        "pull_request": {"url": "https://example.invalid/pr"},
                    },
                ],
                [{"number": 9, "body": f"{marker}\nsecond match"}],
            ]
        )

        matches = client.find_open_issues_by_marker(
            marker=marker,
            result_limit=2,
            page_size=50,
        )

        self.assertEqual(
            matches,
            [
                {"number": 7, "body": marker},
                {"number": 9, "body": f"{marker}\nsecond match"},
            ],
        )
        self.assertEqual(len(client.api_calls), 1)
        method, path, payload, fields, paginate = client.api_calls[0]
        self.assertEqual(method, "GET")
        self.assertEqual(path, "repos/owner/repo/issues")
        self.assertIsNone(payload)
        self.assertIsNotNone(fields)
        assert fields is not None
        self.assertEqual(fields["state"], "open")
        self.assertEqual(fields["per_page"], "50")
        self.assertTrue(paginate)

    def test_apply_does_not_touch_issues_when_no_threshold_crosses(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy = ci_storage_tripwire.load_policy(self.write_policy(root))
            audit = ci_storage_tripwire.load_audit_json(self.write_audit(root, 500, 600))
            evaluation = ci_storage_tripwire.evaluate_tripwire(policy, audit)

        fake = FakeIssueClient()

        result = ci_storage_tripwire.apply_alerts(policy, evaluation, fake)

        self.assertEqual(result, {"created": [], "updated": [], "unchanged": []})
        self.assertEqual(fake.calls, [])
    def test_policy_rejects_bool_positive_int(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(
                root,
                """
                [storage_tripwire]
                schema_version = 1
                policy_id = "ci-storage-tripwire"
                storage_cap_bytes = true
                cap_source = "operator policy"
                owner = "@ci-owner"
                escalation = "x"
                update_cadence = "weekly"
                issue_labels = ["infra"]

                [storage_tripwire.issue_match]
                result_limit = 2
                max_open_matches_per_marker = 1
                page_size = 50

                [storage_tripwire.marker]
                prefix = "<!-- ci-storage-tripwire:"
                suffix = " -->"

                [storage_tripwire.workflow]
                path = ".github/workflows/ci-storage-tripwire.yml"
                job_id = "storage-tripwire"
                runner_var = "CI_RUNNER_GITHUB_HOSTED"
                schedule_cron = "17 9 * * 1"
                triggers = ["schedule", "workflow_dispatch"]
                permissions = { contents = "read", actions = "read", issues = "write" }
                required_fragments = ["python3 scripts/ci_storage_tripwire.py", "apply-live"]
                forbidden_fragments = ["actions/artifacts", "statuses/"]

                [storage_tripwire.metrics.cache]
                label = "Actions cache listed bytes"
                json_paths = ["cache.total_bytes"]

                [[storage_tripwire.thresholds]]
                id = "cache-over-cap"
                metric = "cache"
                limit_bytes = 1000
                severity = "warning"
                title = "CI storage tripwire: cache threshold crossed"

                [[storage_tripwire.thresholds]]
                id = "cache-over-cap"
                metric = "cache"
                limit_bytes = 2000
                severity = "warning"
                title = "CI storage tripwire: duplicate"
                """,
            )

            with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "storage_cap_bytes"):
                ci_storage_tripwire.load_policy(policy_path)

    def test_policy_rejects_duplicate_threshold_ids(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(
                root,
                """
                [storage_tripwire]
                schema_version = 1
                policy_id = "ci-storage-tripwire"
                storage_cap_bytes = 10737418240
                cap_source = "operator policy"
                owner = "@ci-owner"
                escalation = "x"
                update_cadence = "weekly"
                issue_labels = ["infra"]

                [storage_tripwire.issue_match]
                result_limit = 2
                max_open_matches_per_marker = 1
                page_size = 50

                [storage_tripwire.marker]
                prefix = "<!-- ci-storage-tripwire:"
                suffix = " -->"

                [storage_tripwire.workflow]
                path = ".github/workflows/ci-storage-tripwire.yml"
                job_id = "storage-tripwire"
                runner_var = "CI_RUNNER_GITHUB_HOSTED"
                schedule_cron = "17 9 * * 1"
                triggers = ["schedule", "workflow_dispatch"]
                permissions = { contents = "read", actions = "read", issues = "write" }
                required_fragments = ["python3 scripts/ci_storage_tripwire.py", "apply-live"]
                forbidden_fragments = ["actions/artifacts", "statuses/"]

                [storage_tripwire.metrics.cache]
                label = "Actions cache listed bytes"
                json_paths = ["cache.total_bytes"]

                [[storage_tripwire.thresholds]]
                id = "cache-over-cap"
                metric = "cache"
                limit_bytes = 1000
                severity = "warning"
                title = "CI storage tripwire: cache threshold crossed"

                [[storage_tripwire.thresholds]]
                id = "cache-over-cap"
                metric = "cache"
                limit_bytes = 2000
                severity = "warning"
                title = "CI storage tripwire: duplicate"
                """,
            )

            with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "duplicates threshold id"):
                ci_storage_tripwire.load_policy(policy_path)

    def test_policy_rejects_issue_match_limit_that_cannot_detect_duplicates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(
                root,
                self.write_policy(root)
                .read_text(encoding="utf-8")
                .replace("result_limit = 2", "result_limit = 1"),
            )

            with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "result_limit"):
                ci_storage_tripwire.load_policy(policy_path)

    def test_policy_rejects_issue_match_page_size_outside_github_api_bounds(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(
                root,
                self.write_policy(root)
                .read_text(encoding="utf-8")
                .replace("page_size = 50", "page_size = 101"),
            )

            with self.assertRaisesRegex(ci_storage_tripwire.TripwireError, "page_size"):
                ci_storage_tripwire.load_policy(policy_path)

    def test_workflow_contract_is_driven_by_tripwire_policy(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            policy_path = self.write_policy(root)
            policy_text = policy_path.read_text(encoding="utf-8")

        workflow_name = ".github/workflows/ci-storage-tripwire.yml"
        workflow = self.workflow_text()
        clean_errors = verify_ci_workflow_hygiene.verify_storage_tripwire_workflow(
            {workflow_name: workflow},
            policy_text,
        )
        self.assertEqual(clean_errors, [])
        quoted_permission_workflow = (
            workflow.replace("  contents: read\n", '  contents: "read"\n')
            .replace("  actions: read\n", "  actions: 'read'\n")
            .replace("  issues: write\n", '  issues: "write"\n')
        )
        quoted_permission_errors = verify_ci_workflow_hygiene.verify_storage_tripwire_workflow(
            {workflow_name: quoted_permission_workflow},
            policy_text,
        )
        self.assertEqual(quoted_permission_errors, [])

        cases = [
            (
                workflow.replace("  workflow_dispatch:\n", "  pull_request:\n"),
                "triggers must match",
            ),
            (
                workflow.replace("  actions: read\n", ""),
                "permissions must match",
            ),
            (
                workflow.replace("CI_RUNNER_GITHUB_HOSTED", "CI_RUNNER_MANAGED_LIGHT"),
                "runner_var",
            ),
            (
                workflow.replace(" apply-live ", " evaluate "),
                "required_fragments",
            ),
            (
                workflow + "\n          gh api repos/$GITHUB_REPOSITORY/actions/artifacts\n",
                "forbidden workflow fragment",
            ),
        ]
        for mutated_workflow, expected in cases:
            with self.subTest(expected=expected):
                errors = verify_ci_workflow_hygiene.verify_storage_tripwire_workflow(
                    {workflow_name: mutated_workflow},
                    policy_text,
                )
                self.assertTrue(any(expected in error for error in errors), errors)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
