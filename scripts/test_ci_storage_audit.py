from __future__ import annotations

import importlib.util
import json
import pathlib
import sys
import unittest
from typing import Any


SCRIPT = pathlib.Path(__file__).with_name("ci_storage_audit.py")
spec = importlib.util.spec_from_file_location("ci_storage_audit", SCRIPT)
assert spec is not None
ci_storage_audit = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = ci_storage_audit
assert spec.loader is not None
spec.loader.exec_module(ci_storage_audit)


class FakeClient:
    def __init__(self, responses: dict[str, Any]) -> None:
        self.responses = responses
        self.calls: list[tuple[str, dict[str, str] | None, bool]] = []
        self.global_calls: list[tuple[str, dict[str, str] | None, bool]] = []

    def api(self, path: str, *, params: dict[str, str] | None = None, paginate: bool = False) -> Any:
        self.calls.append((path, params, paginate))
        response_key = (path, tuple(sorted((params or {}).items())))
        value = self.responses.get(response_key, self.responses.get(path))
        if value is None:
            raise KeyError(response_key)
        if isinstance(value, Exception):
            raise value
        if paginate:
            return ci_storage_audit.merge_paginated_payload(value)
        return value

    def api_global(self, path: str, *, params: dict[str, str] | None = None, paginate: bool = False) -> Any:
        self.global_calls.append((path, params, paginate))
        response_key = ("GLOBAL", path, tuple(sorted((params or {}).items())))
        value = self.responses.get(response_key, self.responses.get(("GLOBAL", path), self.responses.get(path)))
        if value is None:
            raise KeyError(response_key)
        if isinstance(value, Exception):
            raise value
        if paginate:
            return ci_storage_audit.merge_paginated_payload(value)
        return value


def cleanup_candidate_policy(label: str) -> ci_storage_audit.ArtifactCleanupPolicy:
    return ci_storage_audit.load_cleanup_policy_text(
        """
        [storage_audit.cleanup_feasibility]
        schema_version = 1
        default_class = "ambiguous"
        default_decision = "KEEP"
        default_keep_reason = "ambiguous artifact is not a cleanup candidate"
        protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
        artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
        active_run_keep_reason = "workflow run is still active"
        status_unavailable_keep_reason = "workflow run status is unavailable"
        expiration_unknown_keep_reason = "artifact expiration status is unavailable"
        not_expired_keep_reason = "artifact has not expired"
        billing_impact_unverifiable = "billing impact unverifiable from API"
        wait_and_remeasure = "wait and remeasure natural expiry before deletion"
        protected_refs = ["main"]
        protected_ref_prefixes = []
        protected_ref_globs = []
        active_run_statuses = ["queued"]
        terminal_run_statuses = ["completed"]
        workflow_run_fetch_limit = 1
        billing_probe_paths = []

        [[storage_audit.cleanup_feasibility.classes]]
        id = "nextest_archive"
        name_equals = ["nextest-archive"]
        name_prefixes = []
        expired_decision = "DELETE-CANDIDATE"
        candidate_reason = "expired test archive outside protected refs"
        keep_reason = "test archive is retained until it expires"
        """,
        label=label,
    )


def cleanup_artifacts_with_entry(entry: dict[str, Any]) -> dict[str, Any]:
    return {
        "total_bytes": entry["size_bytes"],
        "expired_bytes": entry["size_bytes"],
        "non_expired_bytes": 0,
        "unknown_expiration_bytes": 0,
        "entries": [entry],
    }


class CiStorageAuditTests(unittest.TestCase):
    def test_parse_cache_key_probe_parses_label_and_key(self) -> None:
        self.assertEqual(
            ci_storage_audit.parse_cache_key_probe("nextest=exact-key"),
            ci_storage_audit.CacheKeyProbeRequest("nextest", "exact-key"),
        )
        self.assertEqual(
            ci_storage_audit.parse_cache_key_probe(" cargo = v0-rust-cache "),
            ci_storage_audit.CacheKeyProbeRequest("cargo", "v0-rust-cache"),
        )

    def test_parse_cache_key_probe_rejects_invalid_inputs(self) -> None:
        for raw in ("nokey", "=key", "label=", " "):
            with self.subTest(raw=raw):
                with self.assertRaises(ci_storage_audit.AuditError):
                    ci_storage_audit.parse_cache_key_probe(raw)

    def test_build_snapshot_serializes_stable_contract_from_fixture_payloads(self) -> None:
        client = FakeClient(
            {
                "actions/caches": [
                    {
                        "total_count": 2,
                        "actions_caches": [
                            {
                                "id": 101,
                                "ref": "refs/heads/main",
                                "key": "linux-a",
                                "last_accessed_at": "2026-06-20T00:00:00Z",
                                "size_in_bytes": 1024,
                            }
                        ],
                    },
                    {
                        "total_count": 2,
                        "actions_caches": [
                            {
                                "id": 102,
                                "ref": "refs/pull/1/merge",
                                "key": "linux-b",
                                "last_accessed_at": "2026-06-21T00:00:00Z",
                                "size_in_bytes": 2048,
                            }
                        ],
                    },
                ],
                "actions/artifacts": [
                    {
                        "total_count": 3,
                        "artifacts": [
                            {"name": "logs", "size_in_bytes": 512},
                            {"name": "binary", "size_in_bytes": 4096},
                        ],
                    },
                    {
                        "total_count": 3,
                        "artifacts": [
                            {"name": "logs", "size_in_bytes": 1536},
                        ],
                    },
                ],
                "actions/permissions/artifact-and-log-retention": {
                    "days": 30,
                    "maximum_allowed_days": 400,
                },
                "rules/branches/main": [
                    {
                        "type": "required_status_checks",
                        "parameters": {
                            "required_status_checks": [
                                {"context": "gate"},
                                {"context": "actionlint", "integration_id": 15368},
                            ]
                        },
                    }
                ],
            }
        )

        snapshot = ci_storage_audit.build_snapshot(
            client,
            repo="owner/repo",
            branch="main",
            snapshot_utc="2026-06-23T00:00:00+00:00",
        )
        encoded = json.dumps(snapshot, sort_keys=True)
        decoded = json.loads(encoded)

        self.assertEqual(decoded["snapshot_utc"], "2026-06-23T00:00:00+00:00")
        self.assertEqual(decoded["repo"], "owner/repo")
        self.assertEqual(decoded["cache"]["total_bytes"], 3072)
        self.assertEqual(decoded["cache"]["count"], 2)
        self.assertEqual(decoded["cache"]["count_source"], "github_total_count")
        self.assertEqual(decoded["cache"]["enumerated_count"], 2)
        self.assertEqual(decoded["cache"]["enumeration_consistency"], "live_churn_possible")
        self.assertEqual(
            decoded["cache"]["entries"][0],
            {
                "cache_id": 101,
                "ref": "refs/heads/main",
                "key": "linux-a",
                "last_accessed_at": "2026-06-20T00:00:00Z",
                "size_bytes": 1024,
            },
        )
        self.assertEqual(decoded["artifacts"]["total_bytes"], 6144)
        self.assertEqual(decoded["artifacts"]["count"], 3)
        self.assertEqual(decoded["artifacts"]["count_source"], "github_total_count")
        self.assertEqual(decoded["artifacts"]["enumerated_count"], 3)
        self.assertEqual(decoded["artifacts"]["enumeration_consistency"], "live_churn_possible")
        self.assertEqual(decoded["artifacts"]["expired_bytes"], 0)
        self.assertEqual(decoded["artifacts"]["expired_count"], 0)
        self.assertEqual(decoded["artifacts"]["non_expired_bytes"], 0)
        self.assertEqual(decoded["artifacts"]["non_expired_count"], 0)
        self.assertEqual(decoded["artifacts"]["unknown_expiration_bytes"], 6144)
        self.assertEqual(decoded["artifacts"]["unknown_expiration_count"], 3)
        self.assertEqual(
            decoded["artifacts"]["by_name"],
            [
                {
                    "name": "binary",
                    "total_bytes": 4096,
                    "count": 1,
                    "expired_bytes": 0,
                    "expired_count": 0,
                    "non_expired_bytes": 0,
                    "non_expired_count": 0,
                    "unknown_expiration_bytes": 4096,
                    "unknown_expiration_count": 1,
                },
                {
                    "name": "logs",
                    "total_bytes": 2048,
                    "count": 2,
                    "expired_bytes": 0,
                    "expired_count": 0,
                    "non_expired_bytes": 0,
                    "non_expired_count": 0,
                    "unknown_expiration_bytes": 2048,
                    "unknown_expiration_count": 2,
                },
            ],
        )
        self.assertEqual(
            decoded["retention_setting"],
            {"artifact_and_log_days": 30, "source": "rest"},
        )
        self.assertEqual(
            decoded["required_checks"],
            {
                "available": True,
                "source": "rulesets",
                "contexts": [{"context": "gate"}, {"context": "actionlint", "integration_id": 15368}],
            },
        )
        self.assertEqual(
            client.calls,
            [
                ("actions/caches", {"per_page": "100"}, True),
                ("actions/artifacts", {"per_page": "100"}, True),
                ("actions/permissions/artifact-and-log-retention", None, False),
                ("rules/branches/main", None, False),
            ],
        )

    def test_counts_distinguish_github_total_from_enumerated_rows(self) -> None:
        client = FakeClient(
            {
                "actions/caches": {
                    "total_count": 10,
                    "actions_caches": [{"id": 1, "size_in_bytes": 100}],
                },
                "actions/artifacts": {
                    "total_count": 20,
                    "artifacts": [{"name": "logs", "size_in_bytes": 200}],
                },
            }
        )

        cache = ci_storage_audit.fetch_cache(client)
        artifacts = ci_storage_audit.fetch_artifacts(client)

        self.assertEqual(cache["count"], 10)
        self.assertEqual(cache["count_source"], "github_total_count")
        self.assertEqual(cache["enumerated_count"], 1)
        self.assertEqual(cache["enumeration_consistency"], "live_churn_possible")
        self.assertEqual(artifacts["count"], 20)
        self.assertEqual(artifacts["count_source"], "github_total_count")
        self.assertEqual(artifacts["enumerated_count"], 1)
        self.assertEqual(artifacts["enumeration_consistency"], "live_churn_possible")

    def test_counts_fall_back_to_enumerated_rows_when_total_count_is_invalid(self) -> None:
        for total_count in (None, "20", -1, True):
            with self.subTest(total_count=total_count):
                client = FakeClient(
                    {
                        "actions/caches": {
                            "total_count": total_count,
                            "actions_caches": [{"id": 1, "size_in_bytes": 100}],
                        },
                        "actions/artifacts": {
                            "total_count": total_count,
                            "artifacts": [{"name": "logs", "size_in_bytes": 200}],
                        },
                    }
                )

                cache = ci_storage_audit.fetch_cache(client)
                artifacts = ci_storage_audit.fetch_artifacts(client)

                self.assertEqual(cache["count"], 1)
                self.assertEqual(cache["count_source"], "enumerated_count_fallback")
                self.assertEqual(cache["enumerated_count"], 1)
                self.assertEqual(artifacts["count"], 1)
                self.assertEqual(artifacts["count_source"], "enumerated_count_fallback")
                self.assertEqual(artifacts["enumerated_count"], 1)

    def test_fetch_artifacts_records_expiration_and_workflow_fields(self) -> None:
        client = FakeClient(
            {
                "actions/artifacts": {
                    "total_count": 3,
                    "artifacts": [
                        {
                            "id": 701,
                            "name": "nextest-archive",
                            "size_in_bytes": 100,
                            "created_at": "2026-06-01T00:00:00Z",
                            "expires_at": "2026-06-15T00:00:00Z",
                            "expired": True,
                            "workflow_run": {
                                "id": 1701,
                                "head_branch": "feature/audit",
                                "head_sha": "a" * 40,
                            },
                        },
                        {
                            "id": 702,
                            "name": "nextest-archive",
                            "size_in_bytes": 200,
                            "created_at": "2026-06-20T00:00:00Z",
                            "expires_at": "2026-07-20T00:00:00Z",
                            "expired": False,
                            "workflow_run": {
                                "id": 1702,
                                "head_branch": "feature/audit",
                                "head_sha": "b" * 40,
                            },
                        },
                        {
                            "id": 703,
                            "name": "ci-provenance-attempt-1",
                            "size_in_bytes": 50,
                            "created_at": "2026-06-02T00:00:00Z",
                            "expires_at": "2026-06-16T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 1703, "head_branch": "main"},
                        },
                    ],
                }
            }
        )

        artifacts = ci_storage_audit.fetch_artifacts(client, include_entries=True)

        self.assertEqual(artifacts["total_bytes"], 350)
        self.assertEqual(artifacts["expired_bytes"], 150)
        self.assertEqual(artifacts["expired_count"], 2)
        self.assertEqual(artifacts["non_expired_bytes"], 200)
        self.assertEqual(artifacts["non_expired_count"], 1)
        self.assertEqual(artifacts["unknown_expiration_bytes"], 0)
        self.assertEqual(artifacts["unknown_expiration_count"], 0)
        self.assertEqual(
            artifacts["by_name"][0],
            {
                "name": "nextest-archive",
                "total_bytes": 300,
                "count": 2,
                "expired_bytes": 100,
                "expired_count": 1,
                "non_expired_bytes": 200,
                "non_expired_count": 1,
                "unknown_expiration_bytes": 0,
                "unknown_expiration_count": 0,
            },
        )
        self.assertEqual(
            artifacts["entries"][0],
            {
                "artifact_id": 701,
                "artifact_id_failure": None,
                "name": "nextest-archive",
                "size_bytes": 100,
                "created_at": "2026-06-01T00:00:00Z",
                "expires_at": "2026-06-15T00:00:00Z",
                "expired": True,
                "expiration_failure": None,
                "workflow_run": {
                    "id": 1701,
                    "id_failure": None,
                    "status": None,
                    "status_failure": None,
                    "conclusion": None,
                    "ref": "feature/audit",
                    "ref_failure": None,
                    "head_branch": "feature/audit",
                    "head_sha": "a" * 40,
                    "event": None,
                    "status_source": "not_fetched",
                },
            },
        )

    def test_cleanup_feasibility_reports_candidates_without_mutation(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "unclassified"
            default_decision = "KEEP"
            default_keep_reason = "artifact is outside configured cleanup candidate classes"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = ["refs/tags/"]
            protected_ref_globs = []
            active_run_statuses = ["queued", "in_progress"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 10
            billing_probe_paths = ["repos/{owner_repo}/actions/cache/usage"]

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"

            [[storage_audit.cleanup_feasibility.classes]]
            id = "provenance"
            name_equals = []
            name_prefixes = ["ci-provenance-attempt-"]
            expired_decision = "KEEP"
            keep_reason = "provenance evidence is not a cleanup candidate"
            """,
            label="test-policy",
        )
        client = FakeClient(
            {
                "actions/caches": {"total_count": 0, "actions_caches": []},
                "actions/artifacts": {
                    "total_count": 6,
                    "artifacts": [
                        {
                            "id": 1,
                            "name": "nextest-archive",
                            "size_in_bytes": 100,
                            "created_at": "2026-06-01T00:00:00Z",
                            "expires_at": "2026-06-15T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 501, "head_branch": "feature/done", "head_sha": "a" * 40},
                        },
                        {
                            "id": 2,
                            "name": "nextest-archive",
                            "size_in_bytes": 200,
                            "created_at": "2026-06-20T00:00:00Z",
                            "expires_at": "2026-07-20T00:00:00Z",
                            "expired": False,
                            "workflow_run": {"id": 502, "head_branch": "feature/future", "head_sha": "b" * 40},
                        },
                        {
                            "id": 3,
                            "name": "ci-provenance-attempt-1",
                            "size_in_bytes": 50,
                            "created_at": "2026-06-02T00:00:00Z",
                            "expires_at": "2026-06-16T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 503, "head_branch": "feature/proof", "head_sha": "c" * 40},
                        },
                        {
                            "id": 4,
                            "name": "unknown-report",
                            "size_in_bytes": 70,
                            "created_at": "2026-06-03T00:00:00Z",
                            "expires_at": "2026-06-17T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 504, "head_branch": "feature/unknown", "head_sha": "d" * 40},
                        },
                        {
                            "id": 5,
                            "name": "nextest-archive",
                            "size_in_bytes": 30,
                            "created_at": "2026-06-04T00:00:00Z",
                            "expires_at": "2026-06-18T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 505, "head_branch": "main", "head_sha": "e" * 40},
                        },
                        {
                            "id": 6,
                            "name": "nextest-archive",
                            "size_in_bytes": 40,
                            "created_at": "2026-06-05T00:00:00Z",
                            "expires_at": "2026-06-19T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 506, "head_branch": "feature/live", "head_sha": "f" * 40},
                        },
                    ],
                },
                "actions/permissions/artifact-and-log-retention": {
                    "days": 30,
                },
                "rules/branches/main": [],
                "branches/main/protection/required_status_checks": {
                    "contexts": [],
                    "checks": [],
                },
                "actions/runs/501": {
                    "id": 501,
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "feature/done",
                    "head_sha": "a" * 40,
                },
                "actions/runs/506": {
                    "id": 506,
                    "status": "in_progress",
                    "conclusion": None,
                    "head_branch": "feature/live",
                    "head_sha": "f" * 40,
                },
                ("GLOBAL", "repos/owner/repo/actions/cache/usage"): ci_storage_audit.GhApiError(
                    "repos/owner/repo/actions/cache/usage",
                    "billing endpoint denied",
                ),
            }
        )

        snapshot = ci_storage_audit.build_snapshot(
            client,
            repo="owner/repo",
            branch="main",
            snapshot_utc="2026-06-23T00:00:00+00:00",
            cleanup_policy=policy,
        )
        cleanup = snapshot["artifact_cleanup_feasibility"]
        rows_by_id = {row["artifact_id"]: row for row in cleanup["rows"]}

        self.assertEqual(cleanup["listed_bytes"], 490)
        self.assertEqual(cleanup["expired_bytes"], 290)
        self.assertEqual(cleanup["non_expired_bytes"], 200)
        self.assertEqual(cleanup["candidate_bytes"], 100)
        self.assertEqual(cleanup["unverified_candidate_bytes"], 0)
        self.assertEqual(cleanup["unverified_candidate_count"], 0)
        self.assertEqual(cleanup["expected_reclaim_proxy_bytes"], 100)
        self.assertEqual(cleanup["measured_billed_reclaim_bytes"], None)
        self.assertEqual(cleanup["billing"]["status"], "unavailable")
        self.assertEqual(cleanup["billing"]["message"], "billing impact unverifiable from API")
        self.assertEqual(cleanup["self_clear_horizon"]["expires_at"], "2026-07-20T00:00:00Z")
        self.assertEqual(cleanup["wait_and_remeasure"], "wait and remeasure natural expiry before deletion")
        self.assertEqual(rows_by_id[1]["class"], "nextest_archive")
        self.assertEqual(rows_by_id[1]["decision"], "DELETE-CANDIDATE")
        self.assertEqual(rows_by_id[1]["reason"], "expired test archive outside protected refs")
        self.assertEqual(rows_by_id[1]["workflow_run"]["status"], "completed")
        self.assertEqual(rows_by_id[2]["decision"], "KEEP")
        self.assertEqual(rows_by_id[2]["reason"], "test archive is retained until it expires")
        self.assertEqual(rows_by_id[3]["class"], "provenance")
        self.assertEqual(rows_by_id[3]["reason"], "provenance evidence is not a cleanup candidate")
        self.assertEqual(rows_by_id[4]["class"], "unclassified")
        self.assertEqual(rows_by_id[4]["reason"], "artifact is outside configured cleanup candidate classes")
        self.assertEqual(rows_by_id[5]["reason"], "protected deploy ref is excluded from cleanup")
        self.assertEqual(rows_by_id[6]["reason"], "workflow run is still active")
        self.assertEqual(rows_by_id[6]["workflow_run"]["status"], "in_progress")
        self.assertEqual(
            client.calls,
            [
                ("actions/caches", {"per_page": "100"}, True),
                ("actions/artifacts", {"per_page": "100"}, True),
                ("actions/permissions/artifact-and-log-retention", None, False),
                ("rules/branches/main", None, False),
                ("branches/main/protection/required_status_checks", None, False),
                ("actions/runs/501", None, False),
                ("actions/runs/506", None, False),
            ],
        )
        self.assertEqual(client.global_calls, [("repos/owner/repo/actions/cache/usage", None, False)])

    def test_committed_cleanup_policy_resolves_existing_artifact_config_references(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"

        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
        rules = {rule.rule_id: rule for rule in policy.classes}

        self.assertEqual(policy.default_class, "ambiguous")
        self.assertEqual(policy.default_decision, "KEEP")
        self.assertEqual(policy.default_keep_reason, "ambiguous artifact is not a cleanup candidate")
        self.assertEqual(rules["deploy_binary"].name_equals, ("bolt-v2-binary",))
        self.assertIsNone(rules["deploy_binary"].candidate_reason)
        self.assertEqual(rules["ci_provenance"].name_prefixes, ("ci-provenance-attempt-",))
        self.assertIsNone(rules["ci_provenance"].candidate_reason)
        self.assertEqual(rules["nextest_fingerprint"].name_prefixes, ("nextest-archive-fingerprint-",))
        self.assertIsNone(rules["nextest_fingerprint"].candidate_reason)
        self.assertEqual(rules["sarif_code_scanning"].name_prefixes, ("sarif-artifact-",))
        self.assertIsNone(rules["sarif_code_scanning"].candidate_reason)
        self.assertIn("cargo-timings-", rules["debug_evidence"].name_prefixes)
        self.assertIsNone(rules["debug_evidence"].candidate_reason)
        self.assertIn("DynaMOS", rules["personal_non_ci"].name_prefixes)
        self.assertIsNone(rules["personal_non_ci"].candidate_reason)
        self.assertIn("users/{owner}/settings/billing/actions", policy.billing_probe_paths)

    def test_cleanup_policy_discovery_finds_single_tracked_policy(self) -> None:
        self.assertEqual(
            ci_storage_audit.discover_cleanup_policy_path().as_posix(),
            "ci/github-actions-runners.toml",
        )

    def test_ref_protection_normalizes_default_branch_and_tag_shapes(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)

        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "main"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "refs/heads/main"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "refs/tags/v0.1.0"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "tags/v0.1.0"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "deploy/eu-west-2/2026-06-18-0ddd9f73"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "feature/artifact-observe"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "issue-955"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "incident-2026-06-28"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "v2-cleanup"))

    def test_fetch_artifacts_rejects_malformed_artifact_rows(self) -> None:
        client = FakeClient(
            {
                "actions/artifacts": {
                    "total_count": 1,
                    "artifacts": ["not-an-object"],
                },
            }
        )

        with self.assertRaisesRegex(ci_storage_audit.AuditError, "actions/artifacts.artifacts"):
            ci_storage_audit.fetch_artifacts(client, include_entries=True)

    def test_fetch_artifacts_rejects_malformed_artifact_names(self) -> None:
        for raw_name in (None, "", True):
            with self.subTest(raw_name=raw_name):
                artifact = {
                    "id": 1,
                    "size_in_bytes": 100,
                    "expired": True,
                }
                if raw_name is not None:
                    artifact["name"] = raw_name
                client = FakeClient(
                    {
                        "actions/artifacts": {
                            "total_count": 1,
                            "artifacts": [artifact],
                        },
                    }
                )

                with self.assertRaisesRegex(ci_storage_audit.AuditError, "actions/artifacts.name"):
                    ci_storage_audit.fetch_artifacts(client)

    def test_fetch_artifacts_requires_workflow_run_for_cleanup_entries(self) -> None:
        for raw_workflow_run in (None, "not-an-object"):
            with self.subTest(raw_workflow_run=raw_workflow_run):
                artifact = {
                    "id": 1,
                    "name": "nextest-archive",
                    "size_in_bytes": 100,
                    "expired": True,
                }
                if raw_workflow_run is not None:
                    artifact["workflow_run"] = raw_workflow_run
                client = FakeClient(
                    {
                        "actions/artifacts": {
                            "total_count": 1,
                            "artifacts": [artifact],
                        },
                    }
                )

                with self.assertRaisesRegex(ci_storage_audit.AuditError, "actions/artifacts.workflow_run"):
                    ci_storage_audit.fetch_artifacts(client, include_entries=True)

    def test_fetch_artifacts_classifies_expiration_status_failures(self) -> None:
        cases = [
            (None, {"field": "expired", "state": "absent", "code": "artifact_expired_absent"}),
            ("", {"field": "expired", "state": "empty", "code": "artifact_expired_empty"}),
            ("true", {"field": "expired", "state": "invalid", "code": "artifact_expired_invalid"}),
        ]

        for raw_expired, expected_failure in cases:
            with self.subTest(raw_expired=raw_expired):
                artifact = {
                    "id": 1,
                    "name": "nextest-archive",
                    "size_in_bytes": 100,
                    "workflow_run": {"id": 501, "head_branch": "feature/expiry"},
                }
                if raw_expired is not None:
                    artifact["expired"] = raw_expired
                client = FakeClient(
                    {
                        "actions/artifacts": {
                            "total_count": 1,
                            "artifacts": [artifact],
                        },
                    }
                )

                artifacts = ci_storage_audit.fetch_artifacts(client, include_entries=True)

                self.assertIsNone(artifacts["entries"][0]["expired"])
                self.assertEqual(artifacts["entries"][0]["expiration_failure"], expected_failure)

    def test_workflow_status_failures_classify_empty_and_invalid_fields(self) -> None:
        cases = [
            ("", {"field": "workflow_run.status", "state": "empty", "code": "workflow_status_empty"}),
            (True, {"field": "workflow_run.status", "state": "invalid", "code": "workflow_status_invalid"}),
        ]

        for raw_status, expected_failure in cases:
            with self.subTest(raw_status=raw_status):
                workflow_run = ci_storage_audit.workflow_run_from_raw(
                    {
                        "id": 501,
                        "status": raw_status,
                        "head_branch": "feature/status",
                    }
                )

                self.assertIsNone(workflow_run["status"])
                self.assertEqual(workflow_run["status_failure"], expected_failure)
                self.assertEqual(workflow_run["status_source"], "not_fetched")

    def test_structured_failures_reject_inconsistent_codes(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "inconsistent failure code"):
            ci_storage_audit.parsed_failure(
                {
                    "field": "artifact_id",
                    "state": "absent",
                    "code": "artifact_id_invalid",
                },
                "test failure",
            )

    def test_artifact_metadata_failures_classify_absent_empty_and_invalid_fields(self) -> None:
        cases = [
            (
                {
                    "artifact_id": None,
                    "workflow_run": {"ref": "feature/valid"},
                },
                {
                    "field": "artifact_id",
                    "state": "absent",
                    "code": "artifact_id_absent",
                },
            ),
            (
                {
                    "artifact_id": 0,
                    "workflow_run": {"ref": "feature/valid"},
                },
                {
                    "field": "artifact_id",
                    "state": "invalid",
                    "code": "artifact_id_invalid",
                },
            ),
            (
                {
                    "artifact_id": 1,
                    "workflow_run": {"ref": None, "head_branch": None},
                },
                {
                    "field": "workflow_run.ref",
                    "state": "absent",
                    "code": "artifact_ref_absent",
                },
            ),
            (
                {
                    "artifact_id": 1,
                    "workflow_run": {"ref": "", "head_branch": ""},
                },
                {
                    "field": "workflow_run.ref",
                    "state": "empty",
                    "code": "artifact_ref_empty",
                },
            ),
            (
                {
                    "artifact_id": 1,
                    "workflow_run": {"ref": True, "head_branch": "feature/valid"},
                },
                {
                    "field": "workflow_run.ref",
                    "state": "invalid",
                    "code": "artifact_ref_invalid",
                },
            ),
        ]

        for entry, expected_failure in cases:
            with self.subTest(expected_failure=expected_failure):
                failure = ci_storage_audit.artifact_metadata_failure(entry)

                self.assertIsNotNone(failure)
                self.assertEqual(failure._asdict(), expected_failure)

    def test_fetch_artifacts_rejects_malformed_artifact_size(self) -> None:
        for raw_size in ("1024", None, True, -1):
            with self.subTest(raw_size=raw_size):
                artifact = {
                    "id": 1,
                    "name": "nextest-archive",
                    "expired": True,
                    "workflow_run": {"id": 501, "head_branch": "feature/done"},
                }
                if raw_size is not None:
                    artifact["size_in_bytes"] = raw_size
                client = FakeClient(
                    {
                        "actions/artifacts": {
                            "total_count": 1,
                            "artifacts": [artifact],
                        },
                    }
                )

                with self.assertRaisesRegex(ci_storage_audit.AuditError, "actions/artifacts.size_in_bytes"):
                    ci_storage_audit.fetch_artifacts(client, include_entries=True)

    def test_cleanup_self_clear_horizon_is_unknown_when_non_expired_expiry_is_missing(self) -> None:
        horizon = ci_storage_audit.cleanup_self_clear_horizon(
            [
                {
                    "artifact_id": 1,
                    "expired": False,
                    "expires_at": "2026-07-20T00:00:00Z",
                },
                {
                    "artifact_id": 2,
                    "expired": False,
                    "expires_at": None,
                },
            ]
        )

        self.assertEqual(
            horizon,
            {
                "expires_at": None,
                "source": "non_expired_artifact_expiry_unknown",
            },
        )

    def test_cleanup_self_clear_horizon_is_unknown_when_expiration_status_is_unknown(self) -> None:
        horizon = ci_storage_audit.cleanup_self_clear_horizon(
            [
                {
                    "artifact_id": 1,
                    "expired": False,
                    "expires_at": "2026-07-20T00:00:00Z",
                },
                {
                    "artifact_id": 2,
                    "expired": None,
                    "expires_at": "2026-07-21T00:00:00Z",
                },
            ]
        )

        self.assertEqual(
            horizon,
            {
                "expires_at": None,
                "source": "artifact_expiration_status_unknown",
            },
        )

    def test_cleanup_feasibility_marks_rows_when_workflow_status_is_not_fetched(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = ["refs/tags/"]
            protected_ref_globs = ["refs/heads/main", "refs/tags/*"]
            active_run_statuses = ["queued", "in_progress"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="status-source-policy",
        )
        client = FakeClient(
            {
                "actions/artifacts": {
                    "total_count": 2,
                    "artifacts": [
                        {
                            "id": 1,
                            "name": "nextest-archive",
                            "size_in_bytes": 100,
                            "created_at": "2026-06-01T00:00:00Z",
                            "expires_at": "2026-06-15T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 501, "head_branch": "feature/done"},
                        },
                        {
                            "id": 2,
                            "name": "nextest-archive",
                            "size_in_bytes": 200,
                            "created_at": "2026-06-02T00:00:00Z",
                            "expires_at": "2026-06-16T00:00:00Z",
                            "expired": True,
                            "workflow_run": {"id": 502, "head_branch": "feature/unfetched"},
                        },
                    ],
                },
                "actions/runs/501": {
                    "id": 501,
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "feature/done",
                },
            }
        )
        artifacts = ci_storage_audit.fetch_artifacts(client, include_entries=True)

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )
        rows_by_id = {row["artifact_id"]: row for row in cleanup["rows"]}

        self.assertEqual(rows_by_id[1]["decision"], "DELETE-CANDIDATE")
        self.assertEqual(rows_by_id[1]["workflow_run"]["status"], "completed")
        self.assertEqual(rows_by_id[1]["workflow_run"]["status_source"], "run_api")
        self.assertEqual(rows_by_id[2]["decision"], "KEEP")
        self.assertEqual(rows_by_id[2]["reason_code"], "workflow_status_unavailable")
        self.assertEqual(rows_by_id[2]["reason"], "workflow run status is unavailable")
        self.assertEqual(rows_by_id[2]["workflow_run"]["status"], None)
        self.assertEqual(rows_by_id[2]["workflow_run"]["status_source"], "fetch_limit")
        self.assertEqual(cleanup["unverified_candidate_bytes"], 200)
        self.assertEqual(cleanup["workflow_run_metadata"]["fetch_limit_reached"], True)

    def test_cleanup_feasibility_does_not_refetch_after_invalid_artifact_status(self) -> None:
        policy = cleanup_candidate_policy("invalid-artifact-status-policy")
        entry = ci_storage_audit.artifact_entry_from_raw(
            {
                "id": 1,
                "name": "nextest-archive",
                "size_in_bytes": 100,
                "created_at": "2026-06-01T00:00:00Z",
                "expires_at": "2026-06-15T00:00:00Z",
                "expired": True,
                "workflow_run": {
                    "id": 501,
                    "status": True,
                    "head_branch": "feature/invalid-status",
                    "head_sha": "a" * 40,
                },
            }
        )
        client = FakeClient(
            {
                "actions/runs/501": {
                    "id": 501,
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "feature/invalid-status",
                },
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["unverified_candidate_count"], 1)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "workflow_status_unavailable")
        self.assertEqual(
            cleanup["rows"][0]["workflow_run"]["status_failure"],
            {
                "field": "workflow_run.status",
                "state": "invalid",
                "code": "workflow_status_invalid",
            },
        )
        self.assertEqual(client.calls, [])

    def test_cleanup_feasibility_classifies_workflow_run_fetch_failures(self) -> None:
        cases = [
            (
                {
                    "id": 501,
                    "head_branch": "feature/status-absent",
                },
                "workflow_run.status",
                "absent",
                "workflow_status_absent",
            ),
            (
                ci_storage_audit.GhApiError("actions/runs/501", "network unavailable"),
                "workflow_run.api",
                "unavailable",
                "workflow_run_api_unavailable",
            ),
            (
                ci_storage_audit.GhApiError("actions/runs/501", "request timed out"),
                "workflow_run.api",
                "timeout",
                "workflow_run_api_timeout",
            ),
            (
                "not-an-object",
                "workflow_run.api",
                "invalid",
                "workflow_run_api_invalid",
            ),
        ]

        for response, expected_field, expected_state, expected_code in cases:
            with self.subTest(expected_state=expected_state):
                policy = cleanup_candidate_policy(f"run-api-{expected_state}-policy")
                entry = ci_storage_audit.artifact_entry_from_raw(
                    {
                        "id": 1,
                        "name": "nextest-archive",
                        "size_in_bytes": 100,
                        "created_at": "2026-06-01T00:00:00Z",
                        "expires_at": "2026-06-15T00:00:00Z",
                        "expired": True,
                        "workflow_run": {
                            "id": 501,
                            "head_branch": f"feature/{expected_state}",
                            "head_sha": "a" * 40,
                        },
                    }
                )
                client = FakeClient({"actions/runs/501": response})

                cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                    client,
                    repo="owner/repo",
                    artifacts=cleanup_artifacts_with_entry(entry),
                    policy=policy,
                )

                self.assertEqual(cleanup["candidate_count"], 0)
                self.assertEqual(cleanup["unverified_candidate_count"], 1)
                self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
                self.assertEqual(cleanup["rows"][0]["reason_code"], "workflow_status_unavailable")
                self.assertEqual(
                    cleanup["rows"][0]["workflow_run"]["status_failure"],
                    {
                        "field": expected_field,
                        "state": expected_state,
                        "code": expected_code,
                    },
                )

    def test_cleanup_feasibility_treats_bool_workflow_run_id_as_missing(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="bool-run-id-policy",
        )
        client = FakeClient({"actions/artifacts": {"total_count": 0, "artifacts": []}})
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": 1,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": True,
                        "status": None,
                        "conclusion": None,
                        "ref": "feature/bool-id",
                        "head_branch": "feature/bool-id",
                        "head_sha": "a" * 40,
                        "event": None,
                        "status_source": "not_fetched",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "workflow_status_unavailable")
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["status_source"], "workflow_run_id_invalid")
        self.assertEqual(client.calls, [])

    def test_cleanup_feasibility_treats_absent_workflow_run_id_as_status_unavailable(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="absent-run-id-policy",
        )
        client = FakeClient({"actions/artifacts": {"total_count": 0, "artifacts": []}})
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": 1,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "status": None,
                        "conclusion": None,
                        "ref": "feature/absent-id",
                        "head_branch": "feature/absent-id",
                        "head_sha": "a" * 40,
                        "event": None,
                        "status_source": "not_fetched",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "workflow_status_unavailable")
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["status_source"], "workflow_run_id_absent")
        self.assertEqual(client.calls, [])

    def test_cleanup_feasibility_keeps_candidate_when_artifact_identity_is_missing(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="missing-artifact-id-policy",
        )
        client = FakeClient({"actions/artifacts": {"total_count": 0, "artifacts": []}})
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": None,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": 501,
                        "status": "completed",
                        "conclusion": "success",
                        "ref": "feature/missing-artifact-id",
                        "head_branch": "feature/missing-artifact-id",
                        "head_sha": "a" * 40,
                        "event": None,
                        "status_source": "artifact_payload",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["candidate_bytes"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["metadata_unavailable_bytes"], 100)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(cleanup["rows"][0]["reason"], "artifact metadata is unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "artifact_id",
                "state": "absent",
                "code": "artifact_id_absent",
            },
        )

    def test_cleanup_feasibility_keeps_candidate_when_artifact_identity_is_zero(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="zero-artifact-id-policy",
        )
        client = FakeClient({"actions/artifacts": {"total_count": 0, "artifacts": []}})
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": 0,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": 501,
                        "status": "completed",
                        "conclusion": "success",
                        "ref": "feature/zero-artifact-id",
                        "head_branch": "feature/zero-artifact-id",
                        "head_sha": "a" * 40,
                        "event": None,
                        "status_source": "artifact_payload",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["candidate_bytes"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["metadata_unavailable_bytes"], 100)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(cleanup["rows"][0]["reason"], "artifact metadata is unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "artifact_id",
                "state": "invalid",
                "code": "artifact_id_invalid",
            },
        )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_is_missing(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="missing-ref-policy",
        )
        client = FakeClient(
            {
                "actions/runs/501": {
                    "id": 501,
                    "status": "completed",
                    "conclusion": "success",
                },
            }
        )
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": 1,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": 501,
                        "status": None,
                        "conclusion": None,
                        "ref": None,
                        "head_branch": None,
                        "head_sha": None,
                        "event": None,
                        "status_source": "not_fetched",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["candidate_bytes"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["metadata_unavailable_bytes"], 100)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(cleanup["rows"][0]["reason"], "artifact metadata is unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "absent",
                "code": "artifact_ref_absent",
            },
        )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_is_empty(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait and remeasure natural expiry before deletion"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = []

            [[storage_audit.cleanup_feasibility.classes]]
            id = "nextest_archive"
            name_equals = ["nextest-archive"]
            name_prefixes = []
            expired_decision = "DELETE-CANDIDATE"
            candidate_reason = "expired test archive outside protected refs"
            keep_reason = "test archive is retained until it expires"
            """,
            label="empty-ref-policy",
        )
        client = FakeClient({"actions/artifacts": {"total_count": 0, "artifacts": []}})
        artifacts = {
            "total_bytes": 100,
            "expired_bytes": 100,
            "non_expired_bytes": 0,
            "unknown_expiration_bytes": 0,
            "entries": [
                {
                    "artifact_id": 1,
                    "name": "nextest-archive",
                    "size_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": 501,
                        "status": "completed",
                        "conclusion": "success",
                        "ref": "",
                        "head_branch": "",
                        "head_sha": "a" * 40,
                        "event": None,
                        "status_source": "artifact_payload",
                    },
                },
            ],
        }

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=artifacts,
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["candidate_bytes"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["metadata_unavailable_bytes"], 100)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(cleanup["rows"][0]["reason"], "artifact metadata is unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "empty",
                "code": "artifact_ref_empty",
            },
        )

    def test_billing_probe_records_reachability_without_raw_payload(self) -> None:
        policy = ci_storage_audit.load_cleanup_policy_text(
            """
            [storage_audit.cleanup_feasibility]
            schema_version = 1
            default_class = "ambiguous"
            default_decision = "KEEP"
            default_keep_reason = "ambiguous artifact is not a cleanup candidate"
            protected_ref_keep_reason = "protected deploy ref is excluded from cleanup"
            artifact_metadata_unavailable_keep_reason = "artifact metadata is unavailable"
            active_run_keep_reason = "workflow run is still active"
            status_unavailable_keep_reason = "workflow run status is unavailable"
            expiration_unknown_keep_reason = "artifact expiration status is unavailable"
            not_expired_keep_reason = "artifact has not expired"
            billing_impact_unverifiable = "billing impact unverifiable from API"
            wait_and_remeasure = "wait"
            protected_refs = ["main"]
            protected_ref_prefixes = []
            protected_ref_globs = []
            active_run_statuses = ["queued"]
            terminal_run_statuses = ["completed"]
            workflow_run_fetch_limit = 1
            billing_probe_paths = ["repos/{owner_repo}/actions/cache/usage"]

            [[storage_audit.cleanup_feasibility.classes]]
            id = "safe_keep"
            name_equals = ["safe"]
            name_prefixes = []
            expired_decision = "KEEP"
            keep_reason = "safe keep"
            """,
            label="billing-policy",
        )
        client = FakeClient(
            {
                ("GLOBAL", "repos/owner/repo/actions/cache/usage"): {
                    "total_active_caches_size_in_bytes": 123,
                    "total_active_caches_count": 4,
                },
            }
        )

        billing = ci_storage_audit.probe_billing_endpoint(client, repo="owner/repo", policy=policy)

        self.assertEqual(billing["status"], "available")
        self.assertEqual(billing["message"], "billing endpoint reachable")
        self.assertEqual(billing["source"], "repos/owner/repo/actions/cache/usage")
        self.assertNotIn("payload", billing)
        self.assertEqual(
            billing["response"],
            {
                "type": "object",
                "keys": ["total_active_caches_count", "total_active_caches_size_in_bytes"],
            },
        )

    def test_billing_probe_rejects_malformed_path_template_as_audit_error(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "billing endpoint path"):
            ci_storage_audit.format_global_api_path("repos/{owner/repo", "owner/repo")

    def test_cleanup_policy_rejects_malformed_billing_path_template(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "billing_probe_paths"):
            ci_storage_audit.load_cleanup_policy_text(
                """
                [storage_audit.cleanup_feasibility]
                schema_version = 1
                default_class = "unclassified"
                default_decision = "KEEP"
                default_keep_reason = "default keep"
                protected_ref_keep_reason = "protected keep"
                artifact_metadata_unavailable_keep_reason = "metadata keep"
                active_run_keep_reason = "active keep"
                status_unavailable_keep_reason = "status keep"
                expiration_unknown_keep_reason = "expiration keep"
                not_expired_keep_reason = "not expired keep"
                billing_impact_unverifiable = "billing unavailable"
                wait_and_remeasure = "wait"
                protected_refs = []
                protected_ref_prefixes = []
                protected_ref_globs = []
                active_run_statuses = ["queued"]
                terminal_run_statuses = ["completed"]
                workflow_run_fetch_limit = 1
                billing_probe_paths = ["repos/{owner/repo"]

                [[storage_audit.cleanup_feasibility.classes]]
                id = "safe_keep"
                name_equals = ["safe"]
                name_prefixes = []
                expired_decision = "KEEP"
                keep_reason = "keep"
                """,
                label="malformed-billing-template-policy",
            )

    def test_cleanup_policy_rejects_bool_schema_version(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "schema_version"):
            ci_storage_audit.load_cleanup_policy_text(
                """
                [storage_audit.cleanup_feasibility]
                schema_version = true
                default_class = "unclassified"
                default_decision = "KEEP"
                default_keep_reason = "default keep"
                protected_ref_keep_reason = "protected keep"
                artifact_metadata_unavailable_keep_reason = "metadata keep"
                active_run_keep_reason = "active keep"
                status_unavailable_keep_reason = "status keep"
                expiration_unknown_keep_reason = "expiration keep"
                not_expired_keep_reason = "not expired keep"
                billing_impact_unverifiable = "billing unavailable"
                wait_and_remeasure = "wait"
                protected_refs = []
                protected_ref_prefixes = []
                protected_ref_globs = []
                active_run_statuses = ["queued"]
                terminal_run_statuses = ["completed"]
                workflow_run_fetch_limit = 1
                billing_probe_paths = []

                [[storage_audit.cleanup_feasibility.classes]]
                id = "safe_keep"
                name_equals = ["safe"]
                name_prefixes = []
                expired_decision = "KEEP"
                keep_reason = "keep"
                """,
                label="bool-schema-policy",
            )

    def test_cleanup_policy_rejects_overlapping_class_matchers(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "overlaps"):
            ci_storage_audit.load_cleanup_policy_text(
                """
                [storage_audit.cleanup_feasibility]
                schema_version = 1
                default_class = "unclassified"
                default_decision = "KEEP"
                default_keep_reason = "default keep"
                protected_ref_keep_reason = "protected keep"
                artifact_metadata_unavailable_keep_reason = "metadata keep"
                active_run_keep_reason = "active keep"
                status_unavailable_keep_reason = "status keep"
                expiration_unknown_keep_reason = "expiration keep"
                not_expired_keep_reason = "not expired keep"
                billing_impact_unverifiable = "billing unavailable"
                wait_and_remeasure = "wait"
                protected_refs = []
                protected_ref_prefixes = []
                protected_ref_globs = []
                active_run_statuses = ["queued"]
                terminal_run_statuses = ["completed"]
                workflow_run_fetch_limit = 1
                billing_probe_paths = []

                [[storage_audit.cleanup_feasibility.classes]]
                id = "exact"
                name_equals = ["artifact-a"]
                name_prefixes = []
                expired_decision = "KEEP"
                keep_reason = "keep"

                [[storage_audit.cleanup_feasibility.classes]]
                id = "prefix"
                name_equals = []
                name_prefixes = ["artifact-"]
                expired_decision = "KEEP"
                keep_reason = "keep"
                """,
                label="overlap-policy",
            )

    def test_merge_paginated_payload_merges_real_slurp_shape(self) -> None:
        payload = [
            {
                "total_count": 2,
                "artifacts": [{"name": "first", "size_in_bytes": 1}],
            },
            {
                "total_count": 2,
                "artifacts": [{"name": "second", "size_in_bytes": 2}],
            },
        ]

        self.assertEqual(
            ci_storage_audit.merge_paginated_payload(payload),
            {
                "total_count": 2,
                "artifacts": [
                    {"name": "first", "size_in_bytes": 1},
                    {"name": "second", "size_in_bytes": 2},
                ],
            },
        )

    def test_merge_paginated_payload_rejects_mixed_page_shapes(self) -> None:
        payload = [
            [{"name": "list-page"}],
            {"artifacts": [{"name": "object-page"}]},
        ]

        with self.assertRaisesRegex(ci_storage_audit.AuditError, "mixed page shapes"):
            ci_storage_audit.merge_paginated_payload(payload)

    def test_human_bytes_uses_binary_units(self) -> None:
        self.assertEqual(ci_storage_audit.human_bytes(0), "0 B")
        self.assertEqual(ci_storage_audit.human_bytes(999), "999 B")
        self.assertEqual(ci_storage_audit.human_bytes(1024), "1.0 KiB")
        self.assertEqual(ci_storage_audit.human_bytes(1536), "1.5 KiB")
        self.assertEqual(ci_storage_audit.human_bytes(1024 * 1024), "1.0 MiB")
        self.assertEqual(ci_storage_audit.human_bytes(1024**3), "1.0 GiB")
        self.assertEqual(ci_storage_audit.human_bytes(1024**4), "1.0 TiB")
        self.assertEqual(ci_storage_audit.human_bytes(1024**5), "1.0 PiB")
        with self.assertRaises(ValueError):
            ci_storage_audit.human_bytes(-1)

    def test_fetch_cache_key_probes_reports_exact_present_and_missing(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 2,
                    "actions_caches": [
                        {
                            "id": 201,
                            "ref": "refs/heads/main",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 1024,
                        },
                        {
                            "id": 202,
                            "ref": "refs/pull/2/merge",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T11:00:00Z",
                            "size_in_bytes": 2048,
                        },
                    ],
                },
                (
                    "actions/caches",
                    (("key", "missing-key"), ("per_page", "100")),
                ): {
                    "total_count": 0,
                    "actions_caches": [],
                },
            }
        )

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [
                ci_storage_audit.CacheKeyProbeRequest("present", "exact-key"),
                ci_storage_audit.CacheKeyProbeRequest("missing", "missing-key"),
            ],
        )

        self.assertTrue(probes[0]["present"])
        self.assertEqual(probes[0]["exact_count"], 2)
        self.assertEqual(probes[0]["api_prefix_count"], 2)
        self.assertEqual(probes[0]["api_prefix_enumerated_count"], 2)
        self.assertEqual(probes[0]["prefix_only_count"], 0)
        self.assertEqual(len(probes[0]["entries"]), 2)
        self.assertFalse(probes[1]["present"])
        self.assertEqual(probes[1]["exact_count"], 0)
        self.assertEqual(probes[1]["api_prefix_count"], 0)
        self.assertEqual(probes[1]["api_prefix_enumerated_count"], 0)
        self.assertEqual(probes[1]["prefix_only_count"], 0)

    def test_fetch_cache_key_probes_rejects_prefix_collision_as_missing(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "foo"), ("per_page", "100")),
                ): {
                    "total_count": 1,
                    "actions_caches": [
                        {
                            "id": 301,
                            "ref": "refs/heads/main",
                            "key": "foo-longer",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 1024,
                        }
                    ],
                },
            }
        )

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [ci_storage_audit.CacheKeyProbeRequest("probe", "foo")],
        )

        self.assertFalse(probes[0]["present"])
        self.assertEqual(probes[0]["exact_count"], 0)
        self.assertEqual(probes[0]["api_prefix_count"], 1)
        self.assertEqual(probes[0]["api_prefix_enumerated_count"], 1)
        self.assertEqual(probes[0]["prefix_only_count"], 1)
        self.assertEqual(probes[0]["entries"], [])

    def test_fetch_cache_key_probes_handles_repeated_requests(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "shared-key"), ("per_page", "100")),
                ): {
                    "total_count": 1,
                    "actions_caches": [
                        {
                            "id": 401,
                            "ref": "refs/heads/main",
                            "key": "shared-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 512,
                        }
                    ],
                },
            }
        )

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [
                ci_storage_audit.CacheKeyProbeRequest("first", "shared-key"),
                ci_storage_audit.CacheKeyProbeRequest("second", "shared-key"),
            ],
        )

        self.assertTrue(probes[0]["present"])
        self.assertTrue(probes[1]["present"])
        self.assertEqual(probes[0]["exact_count"], 1)
        self.assertEqual(probes[1]["exact_count"], 1)
        self.assertEqual(
            [call[0] for call in client.calls],
            ["actions/caches", "actions/caches"],
        )

    def test_render_text_limits_artifact_name_details(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache": {"total_bytes": 0, "count": 0, "entries": []},
            "artifacts": {
                "total_bytes": 10,
                "count": 5,
                "by_name": [
                    {"name": f"artifact-{index}", "total_bytes": index + 1, "count": 1}
                    for index in range(5)
                ],
            },
            "retention_setting": {"artifact_and_log_days": None, "source": "settings-ui-only"},
            "required_checks": {"available": True, "source": "rulesets", "contexts": []},
        }

        rendered = ci_storage_audit.render_text(snapshot, artifact_name_limit=2)

        self.assertIn("artifact-0", rendered)
        self.assertIn("artifact-1", rendered)
        self.assertNotIn("artifact-2", rendered)
        self.assertIn("3 additional artifact names in --json", rendered)

    def test_render_cache_key_probe_text_reports_present_and_missing(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_key_probes": [
                {
                    "label": "present",
                    "key": "exact-key",
                    "present": True,
                    "exact_count": 1,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "prefix_only_count": 0,
                    "entries": [
                        {
                            "cache_id": 501,
                            "ref": "refs/heads/main",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_bytes": 1024,
                        }
                    ],
                },
                {
                    "label": "missing",
                    "key": "missing-key",
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                },
            ],
        }

        rendered = ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertIn("present; exact_count=1", rendered)
        self.assertIn("id=501 ref=refs/heads/main size=1.0 KiB", rendered)
        self.assertIn("missing; exact_count=0", rendered)

    def test_render_cache_key_probe_text_warns_on_prefix_only_match(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "foo",
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "prefix_only_count": 1,
                    "entries": [],
                }
            ],
        }

        rendered = ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertIn("api_prefix_enumerated=1", rendered)
        self.assertIn("API returned prefix matches, but no exact key matched", rendered)
        self.assertNotIn("id=", rendered)

    def test_render_text_includes_retention_days(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache": {"total_bytes": 0, "count": 0, "entries": []},
            "artifacts": {"total_bytes": 0, "count": 0, "by_name": []},
            "retention_setting": {"artifact_and_log_days": 90, "source": "rest"},
            "required_checks": {"available": True, "source": "rulesets", "contexts": []},
        }

        rendered = ci_storage_audit.render_text(snapshot)

        self.assertIn("Retention setting: 90 days (source: rest)", rendered)

    def test_retention_settings_ui_only_when_rest_field_absent(self) -> None:
        client = FakeClient({"actions/permissions/artifact-and-log-retention": {"maximum_allowed_days": 400}})

        self.assertEqual(
            ci_storage_audit.fetch_retention_setting(client),
            {"artifact_and_log_days": None, "source": "settings-ui-only"},
        )

    def test_retention_unavailable_on_api_error(self) -> None:
        client = FakeClient(
            {
                "actions/permissions/artifact-and-log-retention": ci_storage_audit.GhApiError(
                    "actions/permissions/artifact-and-log-retention",
                    "denied",
                )
            }
        )

        self.assertEqual(
            ci_storage_audit.fetch_retention_setting(client),
            {"artifact_and_log_days": None, "source": "unavailable"},
        )

    def test_retention_unavailable_on_malformed_response(self) -> None:
        client = FakeClient(
            {"actions/permissions/artifact-and-log-retention": ["unexpected"]}
        )

        self.assertEqual(
            ci_storage_audit.fetch_retention_setting(client),
            {"artifact_and_log_days": None, "source": "unavailable"},
        )

    def test_required_checks_unavailable_when_all_sources_are_unreadable(self) -> None:
        client = FakeClient(
            {
                "rules/branches/main": ci_storage_audit.GhApiError("rules/branches/main", "denied"),
                "branches/main/protection/required_status_checks": ci_storage_audit.GhApiError(
                    "branches/main/protection/required_status_checks",
                    "not enabled",
                ),
            }
        )

        self.assertEqual(
            ci_storage_audit.fetch_required_checks(client, "main"),
            {"available": False, "source": "unavailable", "contexts": []},
        )

    def test_required_checks_unavailable_when_rulesets_are_unreadable_even_if_branch_protection_works(self) -> None:
        client = FakeClient(
            {
                "rules/branches/main": ci_storage_audit.GhApiError("rules/branches/main", "denied"),
                "branches/main/protection/required_status_checks": {
                    "contexts": ["gate"],
                    "checks": [],
                },
            }
        )

        self.assertEqual(
            ci_storage_audit.fetch_required_checks(client, "main"),
            {"available": False, "source": "unavailable", "contexts": []},
        )

    def test_required_checks_unavailable_when_rulesets_payload_is_malformed(self) -> None:
        client = FakeClient({"rules/branches/main": {"unexpected": []}})

        self.assertEqual(
            ci_storage_audit.fetch_required_checks(client, "main"),
            {"available": False, "source": "unavailable", "contexts": []},
        )

    def test_required_checks_falls_back_to_branch_protection(self) -> None:
        client = FakeClient(
            {
                "rules/branches/main": [],
                "branches/main/protection/required_status_checks": {
                    "contexts": ["gate", "actionlint"],
                    "checks": [],
                },
            }
        )

        self.assertEqual(
            ci_storage_audit.fetch_required_checks(client, "main"),
            {
                "available": True,
                "source": "branch-protection",
                "contexts": ["gate", "actionlint"],
            },
        )

    def test_required_checks_falls_back_to_branch_protection_checks(self) -> None:
        client = FakeClient(
            {
                "rules/branches/main": [],
                "branches/main/protection/required_status_checks": {
                    "contexts": [],
                    "checks": [{"context": "backtester-gate", "app_id": 1234}],
                },
            }
        )

        result = ci_storage_audit.fetch_required_checks(client, "main")

        self.assertEqual(
            result,
            {
                "available": True,
                "source": "branch-protection",
                "contexts": [{"context": "backtester-gate", "app_id": 1234}],
            },
        )
        self.assertEqual(
            ci_storage_audit.check_label(result["contexts"][0]),
            "backtester-gate (app_id=1234)",
        )


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
