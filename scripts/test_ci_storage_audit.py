from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import pathlib
import sys
import tempfile
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
        branch_ref_events = { push = ["*"] }
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


def cleanup_alert_policy(label: str) -> ci_storage_audit.CleanupAlertPolicy:
    return ci_storage_audit.load_cleanup_alert_policy_text(
        """
        [storage_audit.cleanup_feasibility_alert]
        schema_version = 1
        title = "Artifact cleanup feasibility alert"
        clear_title = "Artifact cleanup feasibility clear"
        candidate_count_error_threshold = 1
        candidate_count_error_reason = "delete candidates require operator review"
        expected_reclaim_proxy_bytes_error_threshold = 1
        expected_reclaim_proxy_bytes_error_reason = "proxy reclaim requires operator review"
        unverified_candidate_count_warning_threshold = 1
        unverified_candidate_count_warning_reason = "unverified rows require metadata review"
        metadata_unavailable_count_warning_threshold = 1
        metadata_unavailable_count_warning_reason = "metadata-unavailable rows require review"
        """,
        label=label,
    )


def cleanup_candidate_alert_policy_text() -> str:
    return """
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
    branch_ref_events = { push = ["*"] }
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

    [storage_audit.cleanup_feasibility_alert]
    schema_version = 1
    title = "Artifact cleanup feasibility alert"
    clear_title = "Artifact cleanup feasibility clear"
    candidate_count_error_threshold = 1
    candidate_count_error_reason = "delete candidates require operator review"
    expected_reclaim_proxy_bytes_error_threshold = 1
    expected_reclaim_proxy_bytes_error_reason = "proxy reclaim requires operator review"
    unverified_candidate_count_warning_threshold = 1
    unverified_candidate_count_warning_reason = "unverified rows require metadata review"
    metadata_unavailable_count_warning_threshold = 1
    metadata_unavailable_count_warning_reason = "metadata-unavailable rows require review"
    """


def cleanup_artifacts_with_entry(entry: dict[str, Any]) -> dict[str, Any]:
    return {
        "total_bytes": entry["size_bytes"],
        "expired_bytes": entry["size_bytes"],
        "non_expired_bytes": 0,
        "unknown_expiration_bytes": 0,
        "entries": [entry],
    }


def cleanup_alert_candidate_responses() -> dict[str, Any]:
    return {
        "actions/caches": {"total_count": 0, "actions_caches": []},
        "actions/artifacts": {
            "total_count": 1,
            "artifacts": [
                {
                    "id": 1,
                    "name": "nextest-archive",
                    "size_in_bytes": 100,
                    "created_at": "2026-06-01T00:00:00Z",
                    "expires_at": "2026-06-15T00:00:00Z",
                    "expired": True,
                    "workflow_run": {
                        "id": 501,
                        "head_branch": "feature/done",
                        "head_sha": "a" * 40,
                    },
                },
            ],
        },
        "actions/permissions/artifact-and-log-retention": {"days": 30},
        "rules/branches/main": [],
        "branches/main/protection/required_status_checks": {
            "contexts": [],
            "checks": [],
        },
        "actions/runs/501": {
            "id": 501,
            "status": "completed",
            "conclusion": "success",
            "event": "push",
            "head_branch": "feature/done",
            "head_sha": "a" * 40,
        },
    }


class CiStorageAuditTests(unittest.TestCase):
    def test_parse_cache_key_probe_parses_label_and_key(self) -> None:
        self.assertEqual(
            ci_storage_audit.parse_cache_key_probe("nextest=exact-key"),
            ci_storage_audit.CacheKeyProbeRequest("nextest", "exact-key"),
        )
    def test_parse_cache_key_probe_rejects_invalid_inputs(self) -> None:
        for raw in ("nokey", "=key", "label=", " ", " cargo=v0-rust-cache", "cargo =v0-rust-cache"):
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
                    "actions_caches": [
                        {
                            "id": 1,
                            "ref": "refs/heads/main",
                            "key": "cache-key",
                            "size_in_bytes": 100,
                        }
                    ],
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

    def test_counts_reject_invalid_total_count_contracts(self) -> None:
        for total_count in (None, "20", -1, True):
            with self.subTest(total_count=total_count):
                cache_client = FakeClient(
                    {
                        "actions/caches": {
                            "total_count": total_count,
                            "actions_caches": [
                                {
                                    "id": 1,
                                    "ref": "refs/heads/main",
                                    "key": "cache-key",
                                    "size_in_bytes": 100,
                                }
                            ],
                        },
                    }
                )
                artifact_client = FakeClient(
                    {
                        "actions/artifacts": {
                            "total_count": total_count,
                            "artifacts": [{"name": "logs", "size_in_bytes": 200}],
                        },
                    }
                )
                probe_client = FakeClient(
                    {
                        (
                            "actions/caches",
                            (("key", "cache-key"), ("per_page", "100")),
                        ): {
                            "total_count": total_count,
                            "actions_caches": [
                                {
                                    "id": 1,
                                    "ref": "refs/heads/main",
                                    "key": "cache-key",
                                    "size_in_bytes": 100,
                                }
                            ],
                        },
                    }
                )

                with self.assertRaises(ci_storage_audit.AuditError) as cache_raised:
                    ci_storage_audit.fetch_cache(cache_client)
                with self.assertRaises(ci_storage_audit.AuditError) as artifact_raised:
                    ci_storage_audit.fetch_artifacts(artifact_client)
                with self.assertRaises(ci_storage_audit.AuditError) as probe_raised:
                    ci_storage_audit.fetch_cache_key_probes(
                        probe_client,
                        [ci_storage_audit.CacheKeyProbeRequest("probe", "cache-key")],
                        cache_refs=["refs/heads/main"],
                    )

                self.assertEqual(cache_raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
                self.assertEqual(cache_raised.exception.field, "actions/caches.total_count")
                self.assertEqual(artifact_raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
                self.assertEqual(artifact_raised.exception.field, "actions/artifacts.total_count")
                self.assertEqual(probe_raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
                self.assertEqual(probe_raised.exception.field, "actions/caches.total_count")

    def test_fetch_cache_rejects_malformed_rows(self) -> None:
        malformed_payloads = (
            {
                "total_count": 1,
                "actions_caches": ["not-an-object"],
            },
            {
                "total_count": 1,
                "actions_caches": [
                    {
                        "id": 1,
                        "ref": "refs/heads/main",
                        "key": "cache-key",
                        "size_in_bytes": "100",
                    }
                ],
            },
        )
        for payload in malformed_payloads:
            with self.subTest(payload=payload):
                client = FakeClient({"actions/caches": payload})

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.fetch_cache(client)

                self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)

    def test_fetch_artifacts_rejects_malformed_rows(self) -> None:
        malformed_payloads = (
            {
                "total_count": 1,
                "artifacts": ["not-an-object"],
            },
            {
                "total_count": 1,
                "artifacts": [{"name": "", "size_in_bytes": 100}],
            },
            {
                "total_count": 1,
                "artifacts": [{"name": "logs", "size_in_bytes": "100"}],
            },
        )
        for payload in malformed_payloads:
            with self.subTest(payload=payload):
                client = FakeClient({"actions/artifacts": payload})

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.fetch_artifacts(client)

                self.assertIn(
                    raised.exception.kind,
                    (ci_storage_audit.FailureKind.EMPTY, ci_storage_audit.FailureKind.INVALID),
                )

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
            branch_ref_events = { push = ["*"] }
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
                            "workflow_run": {
                                "id": 501,
                                "head_branch": "feature/done",
                                "head_sha": "a" * 40,
                            },
                        },
                        {
                            "id": 2,
                            "name": "nextest-archive",
                            "size_in_bytes": 200,
                            "created_at": "2026-06-20T00:00:00Z",
                            "expires_at": "2026-07-20T00:00:00Z",
                            "expired": False,
                            "workflow_run": {
                                "id": 502,
                                "ref": "refs/heads/feature/future",
                                "head_branch": "feature/future",
                                "head_sha": "b" * 40,
                            },
                        },
                        {
                            "id": 3,
                            "name": "ci-provenance-attempt-1",
                            "size_in_bytes": 50,
                            "created_at": "2026-06-02T00:00:00Z",
                            "expires_at": "2026-06-16T00:00:00Z",
                            "expired": True,
                            "workflow_run": {
                                "id": 503,
                                "ref": "refs/heads/feature/proof",
                                "head_branch": "feature/proof",
                                "head_sha": "c" * 40,
                            },
                        },
                        {
                            "id": 4,
                            "name": "unknown-report",
                            "size_in_bytes": 70,
                            "created_at": "2026-06-03T00:00:00Z",
                            "expires_at": "2026-06-17T00:00:00Z",
                            "expired": True,
                            "workflow_run": {
                                "id": 504,
                                "ref": "refs/heads/feature/unknown",
                                "head_branch": "feature/unknown",
                                "head_sha": "d" * 40,
                            },
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
                            "workflow_run": {
                                "id": 506,
                                "head_branch": "feature/live",
                                "head_sha": "f" * 40,
                            },
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
                    "event": "push",
                    "head_branch": "feature/done",
                    "head_sha": "a" * 40,
                },
                "actions/runs/506": {
                    "id": 506,
                    "status": "in_progress",
                    "conclusion": None,
                    "event": "push",
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
        summary = ci_storage_audit.render_cleanup_alert_summary(snapshot, cleanup_alert_policy("alert-policy"))
        self.assertIn("Candidate classes:", summary)
        self.assertIn("- `nextest_archive`: `1` rows, `100 B`", summary)
        self.assertEqual(rows_by_id[2]["decision"], "KEEP")
        self.assertEqual(rows_by_id[2]["reason_code"], "not_expired")
        self.assertEqual(rows_by_id[2]["reason"], "artifact has not expired")
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

    def test_cleanup_feasibility_passes_report_rows_to_self_clear_horizon(self) -> None:
        policy = cleanup_candidate_policy("row-horizon-policy")
        captured: dict[str, list[dict[str, Any]]] = {}
        original_horizon = ci_storage_audit.cleanup_self_clear_horizon

        def fake_horizon(rows: list[dict[str, Any]]) -> dict[str, Any]:
            captured["rows"] = rows
            return {"expires_at": "2026-07-20T00:00:00Z", "source": "row_argument"}

        ci_storage_audit.cleanup_self_clear_horizon = fake_horizon
        try:
            cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                FakeClient({}),
                repo="owner/repo",
                artifacts={
                    "total_bytes": 200,
                    "expired_bytes": 0,
                    "non_expired_bytes": 200,
                    "unknown_expiration_bytes": 0,
                    "entries": [
                        {
                            "artifact_id": 2,
                            "name": "nextest-archive",
                            "size_bytes": 200,
                            "created_at": "2026-06-20T00:00:00Z",
                            "expires_at": "2026-07-20T00:00:00Z",
                            "expired": False,
                            "expiration_failure": None,
                            "workflow_run": {
                                "id": 502,
                                "status": "completed",
                                "conclusion": "success",
                                "ref": "feature/future",
                                "head_sha": "b" * 40,
                            },
                        }
                    ],
                },
                policy=policy,
            )
        finally:
            ci_storage_audit.cleanup_self_clear_horizon = original_horizon

        self.assertEqual(cleanup["self_clear_horizon"]["source"], "row_argument")
        self.assertEqual(captured["rows"][0]["decision"], ci_storage_audit.KEEP_DECISION)
        self.assertEqual(captured["rows"][0]["reason_code"], ci_storage_audit.REASON_NOT_EXPIRED)

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
        self.assertEqual(
            rules["backtester_payload"].name_equals,
            ("bvs-test-payload", "ra001a-durable-tracer-receipt"),
        )
        self.assertEqual(
            rules["backtester_payload"].name_prefixes,
            ("issue-789-first-pl-",),
        )
        self.assertEqual(
            rules["backtester_payload"].candidate_reason,
            "expired backtester test payload outside protected refs",
        )
        self.assertEqual(rules["nextest_fingerprint"].name_prefixes, ("nextest-archive-fingerprint-",))
        self.assertIsNone(rules["nextest_fingerprint"].candidate_reason)
        self.assertEqual(rules["sarif_code_scanning"].name_prefixes, ("sarif-artifact-",))
        self.assertIsNone(rules["sarif_code_scanning"].candidate_reason)
        self.assertIn("cargo-timings-", rules["debug_evidence"].name_prefixes)
        self.assertIsNone(rules["debug_evidence"].candidate_reason)
        self.assertEqual(rules["personal_non_ci"].name_prefixes, ("DynaMOS",))
        self.assertIsNone(rules["personal_non_ci"].candidate_reason)
        self.assertIn("users/{owner}/settings/billing/actions", policy.billing_probe_paths)

    def test_cleanup_policy_discovery_finds_single_tracked_policy(self) -> None:
        policy_path = ci_storage_audit.discover_cleanup_policy_path()
        self.assertEqual(
            policy_path.relative_to(SCRIPT.parent.parent).as_posix(),
            "ci/github-actions-runners.toml",
        )
        self.assertEqual(ci_storage_audit.load_cleanup_policy_path(policy_path).default_class, "ambiguous")

    def test_cleanup_policy_discovery_finds_policy_from_subdirectory(self) -> None:
        original_cwd = pathlib.Path.cwd()
        try:
            import os

            os.chdir(SCRIPT.parent)
            policy_path = ci_storage_audit.discover_cleanup_policy_path()
            self.assertEqual(
                policy_path.relative_to(SCRIPT.parent.parent).as_posix(),
                "ci/github-actions-runners.toml",
            )
            self.assertEqual(ci_storage_audit.load_cleanup_policy_path(policy_path).default_class, "ambiguous")
        finally:
            os.chdir(original_cwd)

    def test_cleanup_policy_discovery_ignores_marker_outside_policy_table(self) -> None:
        original_paths = ci_storage_audit.repository_toml_paths
        with tempfile.TemporaryDirectory() as raw_tmp:
            tmp = pathlib.Path(raw_tmp)
            decoy = tmp / "decoy.toml"
            malformed = tmp / "malformed.toml"
            policy_path = tmp / "policy.toml"
            decoy.write_text("# [storage_audit.cleanup_feasibility]\n", encoding="utf-8")
            malformed.write_text("[not valid", encoding="utf-8")
            policy_path.write_text(
                """
                [storage_audit.cleanup_feasibility]
                schema_version = 1
                default_class = "ambiguous"
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
                branch_ref_events = { push = ["*"] }
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
                encoding="utf-8",
            )

            ci_storage_audit.repository_toml_paths = lambda: [decoy, malformed, policy_path]
            try:
                self.assertEqual(ci_storage_audit.discover_cleanup_policy_path(), policy_path)
            finally:
                ci_storage_audit.repository_toml_paths = original_paths

    def test_ref_protection_normalizes_default_branch_and_tag_shapes(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)

        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "main"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "refs/heads/main"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "refs/tags/v0.1.0"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "tags/v0.1.0"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "tags/feature-branch"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "v0.1.0"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "deploy/eu-west-2/2026-06-18-0ddd9f73"))
        self.assertTrue(ci_storage_audit.ref_is_protected(policy, "refs/heads/deploy/eu-west-2/2026-06-18-0ddd9f73"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "feature/artifact-observe"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "issue-955"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "incident-2026-06-28"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "v2-cleanup"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "v2.0-cleanup"))
        self.assertFalse(ci_storage_audit.ref_is_protected(policy, "v2.0/feature"))
        self.assertIn("pull_request", policy.branch_ref_events)
        self.assertIn("workflow_dispatch", policy.branch_ref_events)
        self.assertNotIn("push", policy.branch_ref_events)
        self.assertEqual(policy.branch_ref_events["workflow_dispatch"], ("codex/*",))
        self.assertEqual(policy.workflow_run_fetch_limit, 900)
        self.assertEqual(
            ci_storage_audit.classify_workflow_ref(
                {"head_branch": "feature/pr-artifact", "event": "pull_request"},
                branch_ref_events=policy.branch_ref_events,
            ).value,
            "refs/heads/feature/pr-artifact",
        )
        self.assertEqual(
            ci_storage_audit.classify_workflow_ref(
                {"head_branch": "codex/manual-artifact", "event": "workflow_dispatch"},
                branch_ref_events=policy.branch_ref_events,
            ).value,
            "refs/heads/codex/manual-artifact",
        )
        manual_tag_ref = ci_storage_audit.classify_workflow_ref(
            {"head_branch": "v0.1.3", "event": "workflow_dispatch"},
            branch_ref_events=policy.branch_ref_events,
        )
        self.assertIsNone(manual_tag_ref.value)
        self.assertIsNotNone(manual_tag_ref.failure)
        self.assertEqual(
            ci_storage_audit.classify_workflow_ref(
                {"head_branch": "v0.1.3", "event": "push"},
                branch_ref_events=policy.branch_ref_events,
            ).value,
            "v0.1.3",
        )

    def test_branch_ref_events_require_event_only_for_constrained_globs(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)

        self.assertTrue(ci_storage_audit.branch_ref_events_require_event(policy.branch_ref_events))
        self.assertFalse(ci_storage_audit.branch_ref_events_require_event({"push": ("*",)}))

    def test_real_cleanup_policy_classifies_workflow_dispatch_branch_archive(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
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
                    "head_branch": "codex/cleanup-feasibility",
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
                    "event": "workflow_dispatch",
                    "ref": "refs/heads/codex/cleanup-feasibility",
                    "head_branch": "codex/cleanup-feasibility",
                    "head_sha": "a" * 40,
                },
                ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "users/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
                ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "orgs/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 1)
        self.assertEqual(cleanup["candidate_bytes"], 100)
        self.assertEqual(cleanup["metadata_unavailable_count"], 0)
        self.assertEqual(cleanup["workflow_run_metadata"]["fetches"], 1)
        self.assertFalse(cleanup["workflow_run_metadata"]["fetch_limit_reached"])
        row = cleanup["rows"][0]
        self.assertEqual(row["decision"], "DELETE-CANDIDATE")
        self.assertEqual(row["class"], "nextest_archive")
        self.assertEqual(row["workflow_run"]["ref"], "refs/heads/codex/cleanup-feasibility")
        self.assertEqual(row["workflow_run"]["status"], "completed")
        self.assertEqual(row["workflow_run"]["status_source"], "run_api")

    def test_real_cleanup_policy_keeps_workflow_dispatch_tag_shaped_ref_ambiguous(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
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
                    "head_branch": "v0.1.3",
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
                    "event": "workflow_dispatch",
                    "head_branch": "v0.1.3",
                    "head_sha": "a" * 40,
                },
                ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "users/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
                ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "orgs/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        row = cleanup["rows"][0]
        self.assertEqual(row["decision"], "KEEP")
        self.assertEqual(row["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(row["workflow_run"]["ref"], None)
        self.assertEqual(
            row["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "invalid",
                "code": "artifact_ref_invalid",
            },
        )

    def test_real_cleanup_policy_keeps_workflow_dispatch_canonical_ref_outside_allowed_glob(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
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
                    "head_branch": "feature/manual",
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
                    "event": "workflow_dispatch",
                    "ref": "refs/heads/feature/manual",
                    "head_branch": "feature/manual",
                    "head_sha": "a" * 40,
                },
                ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "users/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
                ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "orgs/owner/settings/billing/actions",
                    "billing endpoint denied",
                ),
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        row = cleanup["rows"][0]
        self.assertEqual(row["decision"], "KEEP")
        self.assertEqual(row["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(row["workflow_run"]["ref"], None)
        self.assertEqual(
            row["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "invalid",
                "code": "artifact_ref_invalid",
            },
        )

    def test_real_cleanup_policy_keeps_canonical_branch_ref_when_event_metadata_is_missing(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
        event_cases = (
            ("missing", {}),
            ("null", {"event": None}),
            ("empty", {"event": ""}),
        )
        for label, event_payload in event_cases:
            with self.subTest(label=label):
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
                            "head_branch": "feature/manual",
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
                            "ref": "refs/heads/feature/manual",
                            "head_branch": "feature/manual",
                            "head_sha": "a" * 40,
                            **event_payload,
                        },
                        ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                            "users/owner/settings/billing/actions",
                            "billing endpoint denied",
                        ),
                        ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                            "orgs/owner/settings/billing/actions",
                            "billing endpoint denied",
                        ),
                    }
                )

                cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                    client,
                    repo="owner/repo",
                    artifacts=cleanup_artifacts_with_entry(entry),
                    policy=policy,
                )

                self.assertEqual(cleanup["candidate_count"], 0)
                self.assertEqual(cleanup["metadata_unavailable_count"], 1)
                row = cleanup["rows"][0]
                self.assertEqual(row["decision"], "KEEP")
                self.assertEqual(row["reason_code"], "artifact_metadata_unavailable")
                self.assertEqual(row["workflow_run"]["ref"], None)
                self.assertEqual(row["workflow_run"]["event"], None)
                self.assertEqual(
                    row["metadata_failure"],
                    {
                        "field": "workflow_run.ref",
                        "state": "invalid",
                        "code": "artifact_ref_invalid",
                    },
                )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_has_unsupported_shape(self) -> None:
        policy = cleanup_candidate_policy("unsupported-ref-policy")
        unsupported_refs = ("tags/v0.1.0", "heads/feature/audit", "refs/pull/1/merge")
        for raw_ref in unsupported_refs:
            with self.subTest(raw_ref=raw_ref):
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
                            "status": "completed",
                            "head_branch": raw_ref,
                            "head_sha": "a" * 40,
                        },
                    }
                )

                cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                    FakeClient({}),
                    repo="owner/repo",
                    artifacts=cleanup_artifacts_with_entry(entry),
                    policy=policy,
                )

                self.assertEqual(cleanup["candidate_count"], 0)
                self.assertEqual(cleanup["metadata_unavailable_count"], 1)
                self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
                self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
                self.assertEqual(
                    cleanup["rows"][0]["metadata_failure"],
                    {
                        "field": "workflow_run.ref",
                        "state": "invalid",
                        "code": "artifact_ref_invalid",
                    },
                )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_has_empty_canonical_ref(self) -> None:
        policy = cleanup_candidate_policy("empty-canonical-ref-policy")
        for raw_ref in ("refs/heads/", "refs/tags/"):
            with self.subTest(raw_ref=raw_ref):
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
                            "status": "completed",
                            "ref": raw_ref,
                            "head_sha": "a" * 40,
                        },
                    }
                )

                cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                    FakeClient({}),
                    repo="owner/repo",
                    artifacts=cleanup_artifacts_with_entry(entry),
                    policy=policy,
                )

                self.assertEqual(cleanup["candidate_count"], 0)
                self.assertEqual(cleanup["metadata_unavailable_count"], 1)
                self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
                self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
                self.assertEqual(
                    cleanup["rows"][0]["metadata_failure"],
                    {
                        "field": "workflow_run.ref",
                        "state": "invalid",
                        "code": "artifact_ref_invalid",
                    },
                )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_is_ambiguous_bare_ref(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
        for raw_ref in (
            "v0.1.0",
            "release-1.0",
            "v2.0-cleanup",
            "v2.0/feature",
            "feature/artifact",
            "deploy/eu-west-2/2026-06-18-0ddd9f73",
            "archive/2026-06-18",
            "tag-archive-v0.1.0",
        ):
            with self.subTest(raw_ref=raw_ref):
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
                            "status": "completed",
                            "head_branch": raw_ref,
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
                            "event": "release",
                            "head_branch": raw_ref,
                            "head_sha": "a" * 40,
                        },
                        ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                            "users/owner/settings/billing/actions",
                            "billing unavailable",
                        ),
                        ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                            "orgs/owner/settings/billing/actions",
                            "billing unavailable",
                        ),
                    }
                )
                cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
                    client,
                    repo="owner/repo",
                    artifacts=cleanup_artifacts_with_entry(entry),
                    policy=policy,
                )

                self.assertEqual(cleanup["candidate_count"], 0)
                self.assertEqual(cleanup["metadata_unavailable_count"], 1)
                self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
                self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
                self.assertEqual(
                    cleanup["rows"][0]["metadata_failure"],
                    {
                        "field": "workflow_run.ref",
                        "state": "invalid",
                        "code": "artifact_ref_invalid",
                    },
                )
                self.assertEqual(client.calls, [("actions/runs/501", None, False)])

    def test_cleanup_feasibility_keeps_tag_push_bare_ref_with_committed_policy(self) -> None:
        policy_path = SCRIPT.parent.parent / "ci" / "github-actions-runners.toml"
        policy = ci_storage_audit.load_cleanup_policy_path(policy_path)
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
                    "status": "completed",
                    "head_branch": "v0.1.3",
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
                    "event": "push",
                    "head_branch": "v0.1.3",
                    "head_sha": "a" * 40,
                },
                ("GLOBAL", "users/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "users/owner/settings/billing/actions",
                    "billing unavailable",
                ),
                ("GLOBAL", "orgs/owner/settings/billing/actions"): ci_storage_audit.GhApiError(
                    "orgs/owner/settings/billing/actions",
                    "billing unavailable",
                ),
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "invalid",
                "code": "artifact_ref_invalid",
            },
        )
        self.assertEqual(client.calls, [("actions/runs/501", None, False)])

    def test_cleanup_feasibility_keeps_candidate_when_run_api_returns_wrong_identity(self) -> None:
        policy = cleanup_candidate_policy("run-api-identity-policy")
        cases = (None, 999)
        for fetched_run_id in cases:
            with self.subTest(fetched_run_id=fetched_run_id):
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
                            "ref": "refs/heads/feature/unverified-run-api",
                            "head_branch": "feature/unverified-run-api",
                            "head_sha": "a" * 40,
                        },
                    }
                )
                fetched_payload = {
                    "status": "completed",
                    "conclusion": "success",
                    "ref": "refs/heads/feature/unverified-run-api",
                    "head_branch": "feature/unverified-run-api",
                    "head_sha": "a" * 40,
                }
                if fetched_run_id is not None:
                    fetched_payload["id"] = fetched_run_id
                client = FakeClient({"actions/runs/501": fetched_payload})

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
                        "field": "workflow_run.api",
                        "state": "invalid",
                        "code": "workflow_run_api_invalid",
                    },
                )

    def test_cleanup_feasibility_keeps_candidate_when_ref_metadata_has_surrounding_whitespace(self) -> None:
        policy = cleanup_candidate_policy("whitespace-ref-policy")
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
                    "status": "completed",
                    "head_branch": "main ",
                    "head_sha": "a" * 40,
                },
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            FakeClient({}),
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 0)
        self.assertEqual(cleanup["metadata_unavailable_count"], 1)
        self.assertEqual(cleanup["rows"][0]["decision"], "KEEP")
        self.assertEqual(cleanup["rows"][0]["reason_code"], "artifact_metadata_unavailable")
        self.assertEqual(
            cleanup["rows"][0]["metadata_failure"],
            {
                "field": "workflow_run.ref",
                "state": "invalid",
                "code": "artifact_ref_invalid",
            },
        )

    def test_fetch_artifacts_rejects_malformed_artifact_rows(self) -> None:
        client = FakeClient(
            {
                "actions/artifacts": {
                    "total_count": 1,
                    "artifacts": ["not-an-object"],
                },
            }
        )

        with self.assertRaisesRegex(ci_storage_audit.AuditError, r"actions/artifacts\[0\]"):
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

    def test_workflow_run_from_raw_treats_empty_optional_fields_as_absent(self) -> None:
        workflow_run = ci_storage_audit.workflow_run_from_raw(
            {
                "id": 501,
                "status": "completed",
                "conclusion": "",
                "head_branch": "feature/optional-fields",
                "head_sha": "",
                "event": "",
            }
        )

        self.assertEqual(workflow_run["conclusion"], None)
        self.assertEqual(workflow_run["head_branch"], "feature/optional-fields")
        self.assertEqual(workflow_run["head_sha"], None)
        self.assertEqual(workflow_run["event"], None)

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
        policy = cleanup_candidate_policy("metadata-failure-policy")
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
                failure = ci_storage_audit.artifact_metadata_failure(policy, entry)

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
            branch_ref_events = { push = ["*"] }
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
                            "workflow_run": {
                                "id": 501,
                                "ref": "refs/heads/feature/done",
                                "head_branch": "feature/done",
                            },
                        },
                        {
                            "id": 2,
                            "name": "nextest-archive",
                            "size_in_bytes": 200,
                            "created_at": "2026-06-02T00:00:00Z",
                            "expires_at": "2026-06-16T00:00:00Z",
                            "expired": True,
                            "workflow_run": {
                                "id": 502,
                                "ref": "refs/heads/feature/unfetched",
                                "head_branch": "feature/unfetched",
                            },
                        },
                    ],
                },
                "actions/runs/501": {
                    "id": 501,
                    "status": "completed",
                    "conclusion": "success",
                    "ref": "refs/heads/feature/done",
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

    def test_cleanup_feasibility_recovers_absent_ref_from_workflow_run_api(self) -> None:
        policy = cleanup_candidate_policy("recover-ref-policy")
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
                    "head_branch": "feature/recovered-ref",
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
                    "event": "push",
                    "head_branch": "feature/recovered-ref",
                    "head_sha": "a" * 40,
                },
            }
        )

        cleanup = ci_storage_audit.build_artifact_cleanup_feasibility(
            client,
            repo="owner/repo",
            artifacts=cleanup_artifacts_with_entry(entry),
            policy=policy,
        )

        self.assertEqual(cleanup["candidate_count"], 1)
        self.assertEqual(cleanup["candidate_bytes"], 100)
        self.assertEqual(cleanup["metadata_unavailable_count"], 0)
        self.assertEqual(cleanup["rows"][0]["decision"], "DELETE-CANDIDATE")
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["ref"], "refs/heads/feature/recovered-ref")
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["ref_failure"], None)
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["status"], "completed")
        self.assertEqual(cleanup["rows"][0]["workflow_run"]["status_source"], "run_api")
        self.assertEqual(client.calls, [("actions/runs/501", None, False)])

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
                    "ref": "refs/heads/feature/invalid-status",
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
                    "ref": "refs/heads/feature/invalid-status",
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
                    "ref": "refs/heads/feature/status-absent",
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
                            "ref": f"refs/heads/feature/{expected_state}",
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
            branch_ref_events = { push = ["*"] }
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
                        "ref": "refs/heads/feature/bool-id",
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
            branch_ref_events = { push = ["*"] }
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
                        "ref": "refs/heads/feature/absent-id",
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
            branch_ref_events = { push = ["*"] }
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
            branch_ref_events = { push = ["*"] }
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
            branch_ref_events = { push = ["*"] }
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
            branch_ref_events = { push = ["*"] }
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

    def test_cleanup_alert_policy_rejects_missing_thresholds(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.load_cleanup_alert_policy_text(
                """
                [storage_audit.cleanup_feasibility_alert]
                schema_version = 1
                title = "Artifact cleanup feasibility alert"
                clear_title = "Artifact cleanup feasibility clear"
                """,
                label="alert-policy",
            )

        self.assertIn(
            "storage_audit.cleanup_feasibility_alert.candidate_count_error_threshold",
            str(raised.exception),
        )

    def test_cleanup_alert_findings_fail_on_delete_candidates_and_warn_on_metadata_gaps(self) -> None:
        policy = cleanup_alert_policy("alert-policy")
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "artifact_cleanup_feasibility": {
                "candidate_count": 2,
                "expected_reclaim_proxy_bytes": 4096,
                "unverified_candidate_count": 1,
                "metadata_unavailable_count": 3,
                "reclaim_basis": "listed_artifact_bytes_proxy",
                "measured_billed_reclaim_bytes": None,
                "billing": {
                    "status": "unavailable",
                    "message": "billing impact unverifiable from API",
                },
            },
        }

        findings = ci_storage_audit.cleanup_alert_findings(snapshot, policy)

        self.assertEqual(
            findings,
            [
                ci_storage_audit.CleanupAlertFinding(
                    level="error",
                    metric="candidate_count",
                    value=2,
                    threshold=1,
                    reason="delete candidates require operator review",
                ),
                ci_storage_audit.CleanupAlertFinding(
                    level="error",
                    metric="expected_reclaim_proxy_bytes",
                    value=4096,
                    threshold=1,
                    reason="proxy reclaim requires operator review",
                ),
                ci_storage_audit.CleanupAlertFinding(
                    level="warning",
                    metric="unverified_candidate_count",
                    value=1,
                    threshold=1,
                    reason="unverified rows require metadata review",
                ),
                ci_storage_audit.CleanupAlertFinding(
                    level="warning",
                    metric="metadata_unavailable_count",
                    value=3,
                    threshold=1,
                    reason="metadata-unavailable rows require review",
                ),
            ],
        )
        self.assertTrue(ci_storage_audit.cleanup_alert_has_errors(findings))
        self.assertEqual(
            ci_storage_audit.cleanup_alert_annotations(findings),
            [
                "::error::cleanup feasibility candidate_count=2 crossed threshold=1: delete candidates require operator review",
                "::error::cleanup feasibility expected_reclaim_proxy_bytes=4096 crossed threshold=1: proxy reclaim requires operator review",
                "::warning::cleanup feasibility unverified_candidate_count=1 crossed threshold=1: unverified rows require metadata review",
                "::warning::cleanup feasibility metadata_unavailable_count=3 crossed threshold=1: metadata-unavailable rows require review",
            ],
        )

    def test_cleanup_alert_summary_reports_counts_and_aggregates_without_raw_rows(self) -> None:
        policy = cleanup_alert_policy("alert-policy")
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "artifact_cleanup_feasibility": {
                "candidate_count": 2,
                "expected_reclaim_proxy_bytes": 3072,
                "unverified_candidate_count": 0,
                "metadata_unavailable_count": 1,
                "reclaim_basis": "listed_artifact_bytes_proxy",
                "measured_billed_reclaim_bytes": None,
                "billing": {
                    "status": "unavailable",
                    "message": "billing impact unverifiable from API",
                },
                "rows": [
                    {
                        "name": "nextest-archive",
                        "artifact_id": 1,
                        "class": "nextest_archive",
                        "decision": "DELETE-CANDIDATE",
                        "reason_code": "delete_candidate",
                        "size_bytes": 1024,
                    },
                    {
                        "name": "nextest-archive-hidden",
                        "artifact_id": 2,
                        "class": "nextest_archive",
                        "decision": "DELETE-CANDIDATE",
                        "reason_code": "delete_candidate",
                        "size_bytes": 2048,
                    },
                    {
                        "name": "metadata-gap-hidden",
                        "artifact_id": 3,
                        "class": "nextest_archive",
                        "decision": "KEEP",
                        "reason_code": "artifact_metadata_unavailable",
                        "size_bytes": 4096,
                    },
                ],
            },
        }

        summary = ci_storage_audit.render_cleanup_alert_summary(snapshot, policy)

        self.assertIn("### Artifact cleanup feasibility alert", summary)
        self.assertIn("- delete candidates: `2`", summary)
        self.assertIn("- proxy reclaim: `3.0 KiB`", summary)
        self.assertIn("- measured billed reclaim: `unavailable`", summary)
        self.assertIn("- reclaim basis: `listed_artifact_bytes_proxy`", summary)
        self.assertIn("Candidate classes:", summary)
        self.assertIn("- `nextest_archive`: `2` rows, `3.0 KiB`", summary)
        self.assertIn("Keep reason codes:", summary)
        self.assertIn("- `artifact_metadata_unavailable`: `1` rows, `4.0 KiB`", summary)
        self.assertIn("delete candidates require operator review", summary)
        self.assertIn("Artifacts section to download `ci-storage-cleanup-feasibility`", summary)
        self.assertIn("Exact billed reclaim is unavailable from the GitHub API", summary)
        self.assertIn("This workflow is read-only and does not delete artifacts.", summary)
        self.assertNotIn("nextest-archive", summary)
        self.assertNotIn("metadata-gap-hidden", summary)
        self.assertNotIn("artifact_id", summary)

    def test_cleanup_alert_summary_reports_clear_state(self) -> None:
        policy = cleanup_alert_policy("alert-policy")
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "artifact_cleanup_feasibility": {
                "candidate_count": 0,
                "expected_reclaim_proxy_bytes": 0,
                "unverified_candidate_count": 0,
                "metadata_unavailable_count": 0,
                "reclaim_basis": "listed_artifact_bytes_proxy",
                "measured_billed_reclaim_bytes": None,
                "billing": {
                    "status": "unavailable",
                    "message": "billing impact unverifiable from API",
                },
                "rows": [],
            },
        }

        summary = ci_storage_audit.render_cleanup_alert_summary(snapshot, policy)

        self.assertIn("### Artifact cleanup feasibility clear", summary)
        self.assertIn("No configured cleanup alert thresholds were crossed.", summary)

    def test_validate_args_rejects_cleanup_alert_without_cleanup_feasibility(self) -> None:
        args = ci_storage_audit.parse_args(
            [
                "--repo",
                "owner/repo",
                "--cleanup-alert",
            ]
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.validate_args(args)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.ABSENT)
        self.assertEqual(raised.exception.field, "--cleanup-feasibility")

    def test_validate_args_rejects_cleanup_json_output_without_cleanup_feasibility(self) -> None:
        args = ci_storage_audit.parse_args(
            [
                "--repo",
                "owner/repo",
                "--cleanup-json-output",
                "cleanup.json",
            ]
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.validate_args(args)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.ABSENT)
        self.assertEqual(raised.exception.field, "--cleanup-feasibility")

    def test_validate_args_rejects_cleanup_policy_without_cleanup_feasibility(self) -> None:
        args = ci_storage_audit.parse_args(
            [
                "--repo",
                "owner/repo",
                "--cleanup-policy",
                "cleanup-policy.toml",
            ]
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.validate_args(args)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.ABSENT)
        self.assertEqual(raised.exception.field, "--cleanup-feasibility")

    def test_write_json_snapshot_writes_full_snapshot_contract(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "artifact_cleanup_feasibility": {
                "candidate_count": 1,
                "rows": [{"artifact_id": 1, "decision": "DELETE-CANDIDATE"}],
            },
        }

        with tempfile.TemporaryDirectory() as tmp:
            output_path = pathlib.Path(tmp) / "cleanup-feasibility.json"
            ci_storage_audit.write_json_snapshot(output_path, snapshot)

            decoded = json.loads(output_path.read_text(encoding="utf-8"))

        self.assertEqual(decoded, snapshot)

    def test_main_cleanup_alert_threshold_writes_outputs_and_returns_one(self) -> None:
        client = FakeClient(cleanup_alert_candidate_responses())
        original_client = ci_storage_audit.GhClient
        ci_storage_audit.GhClient = lambda repo: client
        try:
            with tempfile.TemporaryDirectory() as tmp:
                tmp_path = pathlib.Path(tmp)
                policy_path = tmp_path / "policy.toml"
                summary_path = tmp_path / "summary.md"
                json_path = tmp_path / "cleanup.json"
                policy_path.write_text(cleanup_candidate_alert_policy_text(), encoding="utf-8")
                stdout = io.StringIO()
                stderr = io.StringIO()

                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    result = ci_storage_audit.main(
                        [
                            "--repo",
                            "owner/repo",
                            "--branch",
                            "main",
                            "--cleanup-feasibility",
                            "--cleanup-alert",
                            "--cleanup-policy",
                            str(policy_path),
                            "--cleanup-json-output",
                            str(json_path),
                            "--github-step-summary",
                            str(summary_path),
                            "--github-annotations",
                        ]
                    )

                summary = summary_path.read_text(encoding="utf-8")
                decoded = json.loads(json_path.read_text(encoding="utf-8"))
        finally:
            ci_storage_audit.GhClient = original_client

        self.assertEqual(result, 1)
        self.assertIn("Candidate classes:", summary)
        self.assertIn("- `nextest_archive`: `1` rows, `100 B`", summary)
        self.assertIn("Operator next steps:", summary)
        self.assertIn("Artifacts section to download `ci-storage-cleanup-feasibility`", summary)
        self.assertIn("Exact billed reclaim is unavailable from the GitHub API", summary)
        self.assertIn("::error::cleanup feasibility candidate_count=1 crossed threshold=1", stdout.getvalue())
        self.assertEqual(stderr.getvalue(), "")
        self.assertEqual(decoded["artifact_cleanup_feasibility"]["candidate_count"], 1)

    def test_main_cleanup_alert_json_keeps_annotations_on_stderr(self) -> None:
        client = FakeClient(cleanup_alert_candidate_responses())
        original_client = ci_storage_audit.GhClient
        ci_storage_audit.GhClient = lambda repo: client
        try:
            with tempfile.TemporaryDirectory() as tmp:
                policy_path = pathlib.Path(tmp) / "policy.toml"
                policy_path.write_text(cleanup_candidate_alert_policy_text(), encoding="utf-8")
                stdout = io.StringIO()
                stderr = io.StringIO()

                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    result = ci_storage_audit.main(
                        [
                            "--repo",
                            "owner/repo",
                            "--branch",
                            "main",
                            "--cleanup-feasibility",
                            "--cleanup-alert",
                            "--cleanup-policy",
                            str(policy_path),
                            "--json",
                            "--github-annotations",
                        ]
                    )

                decoded = json.loads(stdout.getvalue())
        finally:
            ci_storage_audit.GhClient = original_client

        self.assertEqual(result, 1)
        self.assertEqual(decoded["artifact_cleanup_feasibility"]["candidate_count"], 1)
        self.assertIn("::error::cleanup feasibility candidate_count=1 crossed threshold=1", stderr.getvalue())
        self.assertNotIn("::error::", stdout.getvalue())

    def test_main_cache_json_keeps_annotations_on_stderr(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 0,
                    "actions_caches": [],
                },
                "actions/cache/usage": {
                    "full_name": "owner/repo",
                    "active_caches_size_in_bytes": 0,
                    "active_caches_count": 0,
                },
            }
        )
        original_client = ci_storage_audit.GhClient
        ci_storage_audit.GhClient = lambda repo: client
        try:
            stdout = io.StringIO()
            stderr = io.StringIO()

            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = ci_storage_audit.main(
                    [
                        "--repo",
                        "owner/repo",
                        "--cache-key",
                        "probe=exact-key",
                        "--cache-ref",
                        "refs/heads/main",
                        "--json",
                        "--github-annotations",
                    ]
                )

            decoded = json.loads(stdout.getvalue())
        finally:
            ci_storage_audit.GhClient = original_client

        self.assertEqual(result, 0)
        self.assertEqual(decoded["cache_key_probes"][0]["present"], False)
        self.assertIn("::warning::one or more root nextest cache keys are missing", stderr.getvalue())
        self.assertNotIn("::warning::", stdout.getvalue())

    def test_cleanup_feasibility_failure_text_reports_contract_failure(self) -> None:
        error = ci_storage_audit.AuditError(
            "artifact metadata drifted",
            kind=ci_storage_audit.FailureKind.INVALID,
            field="artifact_cleanup_feasibility.rows[0].class",
        )

        rendered = ci_storage_audit.render_cleanup_feasibility_failure_text(error)

        self.assertIn("### Cleanup feasibility audit", rendered)
        self.assertIn("- contract failure kind: `invalid`", rendered)
        self.assertIn("- contract failure field: `artifact_cleanup_feasibility.rows[0].class`", rendered)
        self.assertIn(
            "ERROR: invalid artifact_cleanup_feasibility.rows[0].class: artifact metadata drifted",
            rendered,
        )

    def test_main_labels_cleanup_alert_validation_failures_as_cleanup_feasibility(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = ci_storage_audit.main(
                [
                    "--repo",
                    "owner/repo",
                    "--cleanup-alert",
                    "--github-annotations",
                ]
            )

        self.assertEqual(result, 2)
        self.assertIn("::error::cleanup feasibility audit contract failed:", stdout.getvalue())
        self.assertNotIn("cache persistence", stdout.getvalue())
        self.assertIn("ERROR: absent --cleanup-feasibility", stderr.getvalue())

    def test_main_json_routes_validation_failure_annotations_to_stderr(self) -> None:
        stdout = io.StringIO()
        stderr = io.StringIO()

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = ci_storage_audit.main(
                [
                    "--repo",
                    "owner/repo",
                    "--cleanup-alert",
                    "--json",
                    "--github-annotations",
                ]
            )

        self.assertEqual(result, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("::error::cleanup feasibility audit contract failed:", stderr.getvalue())
        self.assertIn("ERROR: absent --cleanup-feasibility", stderr.getvalue())

    def test_main_labels_cache_run_failures_as_cache_persistence(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 0,
                    "actions_caches": [],
                },
                "actions/cache/usage": {
                    "full_name": "owner/repo",
                    "active_caches_size_in_bytes": 0,
                    "active_caches_count": 0,
                },
            }
        )
        original_client = ci_storage_audit.GhClient
        ci_storage_audit.GhClient = lambda repo: client
        try:
            with tempfile.TemporaryDirectory() as tmp:
                summary_path = pathlib.Path(tmp) / "summary.md"
                stdout = io.StringIO()
                stderr = io.StringIO()

                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    result = ci_storage_audit.main(
                        [
                            "--repo",
                            "owner/repo",
                            "--cache-key",
                            "probe=exact-key",
                            "--cache-ref",
                            "refs/heads/main",
                            "--github-step-summary",
                            str(summary_path),
                            "--github-annotations",
                        ]
                    )

                summary = summary_path.read_text(encoding="utf-8")
        finally:
            ci_storage_audit.GhClient = original_client

        self.assertEqual(result, 2)
        self.assertIn("### Cache persistence audit", summary)
        self.assertIn("::error::cache persistence audit contract failed:", stdout.getvalue())
        self.assertNotIn("cleanup feasibility", stdout.getvalue())
        self.assertIn("ERROR: absent --restore-hit", stderr.getvalue())

    def test_main_json_routes_run_failure_annotations_to_stderr(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 0,
                    "actions_caches": [],
                },
                "actions/cache/usage": {
                    "full_name": "owner/repo",
                    "active_caches_size_in_bytes": 0,
                    "active_caches_count": 0,
                },
            }
        )
        original_client = ci_storage_audit.GhClient
        ci_storage_audit.GhClient = lambda repo: client
        try:
            with tempfile.TemporaryDirectory() as tmp:
                summary_path = pathlib.Path(tmp) / "summary.md"
                stdout = io.StringIO()
                stderr = io.StringIO()

                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    result = ci_storage_audit.main(
                        [
                            "--repo",
                            "owner/repo",
                            "--cache-key",
                            "probe=exact-key",
                            "--cache-ref",
                            "refs/heads/main",
                            "--json",
                            "--github-step-summary",
                            str(summary_path),
                            "--github-annotations",
                        ]
                    )
        finally:
            ci_storage_audit.GhClient = original_client

        self.assertEqual(result, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("::error::cache persistence audit contract failed:", stderr.getvalue())
        self.assertIn("ERROR: absent --restore-hit", stderr.getvalue())

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
            branch_ref_events = { push = ["*"] }
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

    def test_billing_probe_sanitizes_unavailable_errors(self) -> None:
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
            branch_ref_events = { push = ["*"] }
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
            label="billing-error-policy",
        )
        raw_error = '{"message":"Not Found","documentation_url":"https://docs.github.com/rest"}'
        client = FakeClient(
            {
                ("GLOBAL", "repos/owner/repo/actions/cache/usage"): ci_storage_audit.GhApiError(
                    "repos/owner/repo/actions/cache/usage",
                    raw_error,
                ),
            }
        )

        billing = ci_storage_audit.probe_billing_endpoint(client, repo="owner/repo", policy=policy)

        self.assertEqual(billing["status"], "unavailable")
        self.assertEqual(
            billing["probes"],
            [
                {
                    "path": "repos/owner/repo/actions/cache/usage",
                    "status": "unavailable",
                    "error": "unavailable",
                }
            ],
        )
        self.assertNotIn("Not Found", json.dumps(billing))
        self.assertNotIn("documentation_url", json.dumps(billing))

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
                branch_ref_events = { push = ["*"] }
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

    def test_cleanup_policy_rejects_surrounding_whitespace_in_string_lists(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, r"protected_refs\[0\]"):
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
                protected_refs = ["main "]
                protected_ref_prefixes = []
                protected_ref_globs = []
                branch_ref_events = { push = ["*"] }
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
                label="whitespace-policy",
            )

    def test_cleanup_policy_rejects_surrounding_whitespace_in_referenced_name_matchers(self) -> None:
        cases = (
            ("name_equals_from", "artifact_names.exact", 'exact = " nextest-archive"'),
            ("name_prefixes_from", "artifact_names.prefix", 'prefix = " nextest-"'),
        )
        for matcher_key, reference, referenced_value in cases:
            with self.subTest(matcher_key=matcher_key):
                with self.assertRaisesRegex(ci_storage_audit.AuditError, reference):
                    ci_storage_audit.load_cleanup_policy_text(
                        f"""
                        [artifact_names]
                        {referenced_value}

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
                        branch_ref_events = {{ push = ["*"] }}
                        active_run_statuses = ["queued"]
                        terminal_run_statuses = ["completed"]
                        workflow_run_fetch_limit = 1
                        billing_probe_paths = []

                        [[storage_audit.cleanup_feasibility.classes]]
                        id = "nextest_archive"
                        name_equals = []
                        name_prefixes = []
                        {matcher_key} = ["{reference}"]
                        expired_decision = "DELETE-CANDIDATE"
                        candidate_reason = "candidate"
                        keep_reason = "keep"
                        """,
                        label=f"{matcher_key}-whitespace-policy",
                    )

    def test_cleanup_policy_rejects_surrounding_whitespace_in_referenced_templates(self) -> None:
        templates = (
            " issue-789-first-pl-{run_id}-{run_attempt}",
            "issue-789-first-pl- {run_id}-{run_attempt}",
        )
        for template in templates:
            with self.subTest(template=template):
                with self.assertRaisesRegex(ci_storage_audit.AuditError, "artifact_name_template"):
                    ci_storage_audit.load_cleanup_policy_text(
                        f"""
                        [backtester.issue_789]
                        artifact_name_template = "{template}"

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
                        branch_ref_events = {{ push = ["*"] }}
                        active_run_statuses = ["queued"]
                        terminal_run_statuses = ["completed"]
                        workflow_run_fetch_limit = 1
                        billing_probe_paths = []

                        [[storage_audit.cleanup_feasibility.classes]]
                        id = "backtester_payload"
                        name_equals = []
                        name_prefixes_from_templates = ["backtester.issue_789.artifact_name_template"]
                        expired_decision = "DELETE-CANDIDATE"
                        candidate_reason = "candidate"
                        keep_reason = "keep"
                        """,
                        label="whitespace-template-policy",
                    )

    def test_cleanup_policy_rejects_surrounding_whitespace_in_scalar_strings(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "default_class"):
            ci_storage_audit.load_cleanup_policy_text(
                """
                [storage_audit.cleanup_feasibility]
                schema_version = 1
                default_class = " ambiguous"
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
                branch_ref_events = { push = ["*"] }
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
                label="whitespace-scalar-policy",
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
                branch_ref_events = { push = ["*"] }
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
                branch_ref_events = { push = ["*"] }
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

    def test_merge_paginated_payload_rejects_malformed_page_shape(self) -> None:
        with self.assertRaisesRegex(ci_storage_audit.AuditError, "not an object or list"):
            ci_storage_audit.merge_paginated_payload(["not-an-object-or-list"])

    def test_merge_paginated_payload_rejects_empty_payload(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.merge_paginated_payload([])

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "paginated")

    def test_merge_paginated_payload_tolerates_live_total_count_churn(self) -> None:
        payload = [
            {
                "total_count": 2,
                "artifacts": [{"name": "first", "size_in_bytes": 1}],
            },
            {
                "total_count": 3,
                "artifacts": [{"name": "second", "size_in_bytes": 2}],
            },
        ]

        self.assertEqual(
            ci_storage_audit.merge_paginated_payload(payload),
            {
                "total_count": 3,
                "artifacts": [
                    {"name": "first", "size_in_bytes": 1},
                    {"name": "second", "size_in_bytes": 2},
                ],
            },
        )

    def test_fetch_cache_rejects_invalid_total_count_on_any_paginated_page(self) -> None:
        client = FakeClient(
            {
                "actions/caches": [
                    {
                        "total_count": "bad",
                        "actions_caches": [],
                    },
                    {
                        "total_count": 1,
                        "actions_caches": [
                            {
                                "id": 1,
                                "ref": "refs/heads/main",
                                "key": "cache-key",
                                "size_in_bytes": 100,
                            }
                        ],
                    },
                ],
            }
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.fetch_cache(client)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "actions/caches.total_count")

    def test_fetch_artifacts_rejects_missing_total_count_on_any_paginated_page(self) -> None:
        client = FakeClient(
            {
                "actions/artifacts": [
                    {
                        "artifacts": [],
                    },
                    {
                        "total_count": 1,
                        "artifacts": [{"name": "logs", "size_in_bytes": 200}],
                    },
                ],
            }
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.fetch_artifacts(client)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.ABSENT)
        self.assertEqual(raised.exception.field, "actions/artifacts.total_count")

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
            cache_refs=["refs/heads/main", "refs/pull/2/merge"],
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
            cache_refs=["refs/heads/main"],
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
            cache_refs=["refs/heads/main"],
        )

        self.assertTrue(probes[0]["present"])
        self.assertTrue(probes[1]["present"])
        self.assertEqual(probes[0]["exact_count"], 1)
        self.assertEqual(probes[1]["exact_count"], 1)
        self.assertEqual(
            [call[0] for call in client.calls],
            ["actions/caches", "actions/caches"],
        )

    def test_fetch_cache_key_probes_fails_closed_on_api_error(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "unavailable-key"), ("per_page", "100")),
                ): ci_storage_audit.GhApiError("actions/caches", "rate limited"),
                (
                    "actions/caches",
                    (("key", "present-key"), ("per_page", "100")),
                ): {
                    "total_count": 1,
                    "actions_caches": [
                        {
                            "id": 402,
                            "ref": "refs/pull/986/merge",
                            "key": "present-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 1024,
                        }
                    ],
                },
            }
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.fetch_cache_key_probes(
                client,
                [
                    ci_storage_audit.CacheKeyProbeRequest("unavailable", "unavailable-key"),
                    ci_storage_audit.CacheKeyProbeRequest("present", "present-key"),
                ],
                cache_refs=["refs/pull/986/merge"],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.UNAVAILABLE)
        self.assertEqual(raised.exception.field, "actions/caches")
        self.assertIn("rate limited", str(raised.exception))
        self.assertEqual(
            [call[0] for call in client.calls],
            ["actions/caches"],
        )

    def test_fetch_cache_key_probes_ignores_exact_keys_on_unusable_refs(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 1,
                    "actions_caches": [
                        {
                            "id": 403,
                            "ref": "refs/pull/1/merge",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 2048,
                        }
                    ],
                },
            }
        )

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [ci_storage_audit.CacheKeyProbeRequest("current", "exact-key")],
            cache_refs=["refs/pull/986/merge", "refs/heads/main"],
        )

        self.assertTrue(probes[0]["available"])
        self.assertFalse(probes[0]["present"])
        self.assertEqual(probes[0]["exact_count"], 0)
        self.assertEqual(probes[0]["api_prefix_count"], 1)
        self.assertEqual(probes[0]["api_prefix_enumerated_count"], 1)
        self.assertEqual(probes[0]["ref_filtered_prefix_enumerated_count"], 0)
        self.assertEqual(probes[0]["ref_filter"], ["refs/pull/986/merge", "refs/heads/main"])

    def test_fetch_cache_key_probes_accepts_cache_branch_filters(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "exact-key"), ("per_page", "100")),
                ): {
                    "total_count": 1,
                    "actions_caches": [
                        {
                            "id": 404,
                            "ref": "refs/heads/release/train",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_in_bytes": 2048,
                        }
                    ],
                },
            }
        )

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [ci_storage_audit.CacheKeyProbeRequest("release", "exact-key")],
            cache_refs=["refs/pull/986/merge"],
            cache_branches=["release/train"],
        )

        self.assertTrue(probes[0]["available"])
        self.assertTrue(probes[0]["present"])
        self.assertEqual(probes[0]["exact_count"], 1)
        self.assertEqual(probes[0]["ref_filter"], ["refs/pull/986/merge", "refs/heads/release/train"])

    def test_normalize_cache_refs_rejects_absent_filter(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs()

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.ABSENT)
        self.assertEqual(raised.exception.field, "cache_ref_filter")

    def test_normalize_cache_refs_rejects_empty_ref_list(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(cache_refs=[])

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--cache-ref")

    def test_normalize_cache_refs_rejects_empty_branch_list(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(
                cache_refs=["refs/pull/986/merge"],
                cache_branches=[],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--cache-branch")

    def test_normalize_cache_refs_rejects_empty_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(cache_refs=["refs/pull/986/merge", ""])

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--cache-ref")

    def test_normalize_cache_refs_rejects_invalid_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(cache_refs=["main"])

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "--cache-ref")

    def test_normalize_cache_refs_rejects_whitespace_padded_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(cache_refs=[" refs/pull/986/merge "])

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "--cache-ref")

    def test_normalize_cache_refs_rejects_duplicate_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(
                cache_refs=["refs/pull/986/merge", "refs/pull/986/merge"],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.DUPLICATE)
        self.assertEqual(raised.exception.field, "cache_ref_filter")

    def test_normalize_cache_refs_rejects_empty_branch(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(
                cache_refs=["refs/pull/986/merge"],
                cache_branches=[""],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--cache-branch")

    def test_normalize_cache_refs_rejects_full_ref_branch(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(
                cache_refs=["refs/pull/986/merge"],
                cache_branches=["refs/heads/main"],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "--cache-branch")

    def test_normalize_cache_refs_rejects_duplicate_branch_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.normalize_cache_ref_inputs(
                cache_refs=["refs/heads/main"],
                cache_branches=["main"],
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.DUPLICATE)
        self.assertEqual(raised.exception.field, "cache_ref_filter")

    def test_normalize_cache_refs_accepts_explicit_refs_and_branches(self) -> None:
        refs = ci_storage_audit.normalize_cache_ref_inputs(
            cache_refs=["refs/pull/986/merge"],
            cache_branches=["main", "release/train"],
        )

        self.assertEqual(
            refs,
            ["refs/pull/986/merge", "refs/heads/main", "refs/heads/release/train"],
        )

    def test_resolve_cache_refs_accepts_pull_request_github_context(self) -> None:
        refs = ci_storage_audit.resolve_cache_ref_inputs(
            github_event_name="pull_request",
            github_ref="refs/pull/986/merge",
            github_base_ref="release/train",
            github_default_branch="main",
        )

        self.assertEqual(
            refs,
            ["refs/pull/986/merge", "refs/heads/release/train", "refs/heads/main"],
        )

    def test_resolve_cache_refs_deduplicates_matching_base_and_default_branch(self) -> None:
        refs = ci_storage_audit.resolve_cache_ref_inputs(
            github_event_name="pull_request",
            github_ref="refs/pull/986/merge",
            github_base_ref="main",
            github_default_branch="main",
        )

        self.assertEqual(refs, ["refs/pull/986/merge", "refs/heads/main"])

    def test_resolve_cache_refs_rejects_pull_request_without_base_ref(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.resolve_cache_ref_inputs(
                github_event_name="pull_request",
                github_ref="refs/pull/986/merge",
                github_base_ref="",
                github_default_branch="main",
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--github-base-ref")

    def test_resolve_cache_refs_rejects_ambiguous_explicit_and_github_inputs(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.resolve_cache_ref_inputs(
                cache_refs=["refs/pull/986/merge"],
                github_event_name="pull_request",
                github_ref="refs/pull/986/merge",
                github_base_ref="main",
                github_default_branch="main",
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.AMBIGUOUS)
        self.assertEqual(raised.exception.field, "cache_ref_filter")

    def test_resolve_cache_refs_rejects_unsupported_github_event(self) -> None:
        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.resolve_cache_ref_inputs(
                github_event_name="schedule",
                github_ref="refs/heads/main",
                github_base_ref="",
                github_default_branch="main",
            )

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "--github-event-name")

    def test_fetch_cache_usage_fails_closed_when_unavailable(self) -> None:
        client = FakeClient(
            {
                "actions/cache/usage": ci_storage_audit.GhApiError(
                    "actions/cache/usage",
                    "secondary rate limit",
                )
            }
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.fetch_cache_usage(client)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.UNAVAILABLE)
        self.assertEqual(raised.exception.field, "actions/cache/usage")
        self.assertIn("secondary rate limit", str(raised.exception))

    def test_fetch_cache_usage_rejects_malformed_numeric_fields(self) -> None:
        malformed_payloads = (
            {"active_caches_count": "many", "active_caches_size_in_bytes": 1},
            {"active_caches_count": 1, "active_caches_size_in_bytes": -1},
            {"active_caches_count": True, "active_caches_size_in_bytes": 1},
        )
        for payload in malformed_payloads:
            with self.subTest(payload=payload):
                client = FakeClient({"actions/cache/usage": payload})

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.fetch_cache_usage(client)

                self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
                self.assertIn("actions/cache/usage", raised.exception.field)

    def test_fetch_cache_key_probes_reports_malformed_payload_as_invalid(self) -> None:
        malformed_payloads = (
            ["not-an-object"],
            {"total_count": 1, "actions_caches": "not-a-list"},
        )
        for payload in malformed_payloads:
            with self.subTest(payload=payload):
                client = FakeClient(
                    {
                        (
                            "actions/caches",
                            (("key", "exact-key"), ("per_page", "100")),
                        ): payload,
                    }
                )

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.fetch_cache_key_probes(
                        client,
                        [ci_storage_audit.CacheKeyProbeRequest("probe", "exact-key")],
                        cache_refs=["refs/heads/main"],
                    )

                self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)

    def test_fetch_cache_key_probes_rejects_malformed_cache_entries(self) -> None:
        malformed_entries = (
            (
                {"id": 1, "ref": "refs/heads/main", "key": "exact-key", "size_in_bytes": "bad"},
                ci_storage_audit.FailureKind.INVALID,
                "actions/caches.size_in_bytes",
            ),
            (
                {"id": 1, "ref": "", "key": "exact-key", "size_in_bytes": 1},
                ci_storage_audit.FailureKind.EMPTY,
                "actions/caches.ref",
            ),
            (
                {"id": 1, "ref": "refs/heads/main", "key": "", "size_in_bytes": 1},
                ci_storage_audit.FailureKind.EMPTY,
                "actions/caches.key",
            ),
        )
        for entry, expected_kind, expected_field in malformed_entries:
            with self.subTest(entry=entry):
                client = FakeClient(
                    {
                        (
                            "actions/caches",
                            (("key", "exact-key"), ("per_page", "100")),
                        ): {
                            "total_count": 1,
                            "actions_caches": [entry],
                        },
                    }
                )

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.fetch_cache_key_probes(
                        client,
                        [ci_storage_audit.CacheKeyProbeRequest("probe", "exact-key")],
                        cache_refs=["refs/heads/main"],
                    )

                self.assertEqual(raised.exception.kind, expected_kind)
                self.assertEqual(raised.exception.field, expected_field)

    def test_main_reports_empty_step_summary_without_traceback(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            rc = ci_storage_audit.main(
                [
                    "--repo",
                    "owner/repo",
                    "--cache-key",
                    "probe=exact-key",
                    "--github-event-name",
                    "pull_request",
                    "--github-ref",
                    "refs/pull/986/merge",
                    "--github-base-ref",
                    "main",
                    "--github-default-branch",
                    "main",
                    "--github-step-summary",
                    "",
                ]
            )

        self.assertEqual(rc, 2)
        self.assertIn("empty --github-step-summary", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_validate_args_rejects_empty_step_summary(self) -> None:
        args = ci_storage_audit.parse_args(
            [
                "--repo",
                "owner/repo",
                "--cache-key",
                "probe=exact-key",
                "--github-event-name",
                "pull_request",
                "--github-ref",
                "refs/pull/986/merge",
                "--github-base-ref",
                "main",
                "--github-default-branch",
                "main",
                "--github-step-summary",
                "",
            ]
        )

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.validate_args(args)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.EMPTY)
        self.assertEqual(raised.exception.field, "--github-step-summary")

    def test_build_cache_key_probe_snapshot_includes_cache_usage(self) -> None:
        client = FakeClient(
            {
                (
                    "actions/caches",
                    (("key", "missing-key"), ("per_page", "100")),
                ): {
                    "total_count": 0,
                    "actions_caches": [],
                },
                "actions/cache/usage": {
                    "active_caches_count": 11,
                    "active_caches_size_in_bytes": 11_044_557_069,
                },
            }
        )

        snapshot = ci_storage_audit.build_cache_key_probe_snapshot(
            client,
            repo="owner/repo",
            snapshot_utc="2026-06-28T10:37:43+00:00",
            requests=[ci_storage_audit.CacheKeyProbeRequest("missing", "missing-key")],
            cache_refs=["refs/heads/main"],
        )

        self.assertEqual(
            snapshot["cache_usage"],
            {
                "available": True,
                "active_caches_count": 11,
                "active_caches_size_in_bytes": 11_044_557_069,
                "source": "rest",
            },
        )
        self.assertEqual(
            [call[0] for call in client.calls],
            ["actions/caches", "actions/cache/usage"],
        )

    def test_build_cache_key_probe_snapshot_validates_json_snapshot_contract(self) -> None:
        original_fetch_cache_key_probes = ci_storage_audit.fetch_cache_key_probes
        original_fetch_cache_usage = ci_storage_audit.fetch_cache_usage

        def fake_fetch_cache_key_probes(*args: Any, **kwargs: Any) -> list[dict[str, Any]]:
            return [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": True,
                    "present": True,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 0,
                    "ref_filtered_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": ["refs/heads/main"],
                }
            ]

        def fake_fetch_cache_usage(client: Any) -> dict[str, Any]:
            return {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            }

        try:
            ci_storage_audit.fetch_cache_key_probes = fake_fetch_cache_key_probes
            ci_storage_audit.fetch_cache_usage = fake_fetch_cache_usage

            with self.assertRaises(ci_storage_audit.AuditError) as raised:
                ci_storage_audit.build_cache_key_probe_snapshot(
                    FakeClient({}),
                    repo="owner/repo",
                    snapshot_utc="2026-06-28T10:37:43+00:00",
                    requests=[ci_storage_audit.CacheKeyProbeRequest("probe", "exact-key")],
                    cache_refs=["refs/heads/main"],
                )

        finally:
            ci_storage_audit.fetch_cache_key_probes = original_fetch_cache_key_probes
            ci_storage_audit.fetch_cache_usage = original_fetch_cache_usage

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertEqual(raised.exception.field, "cache_key_probe_snapshot.cache_key_probes.present")

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
            "cache_usage": {
                "available": True,
                "active_caches_count": 11,
                "active_caches_size_in_bytes": 11_044_557_069,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "present",
                    "key": "exact-key",
                    "available": True,
                    "present": True,
                    "exact_count": 1,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "ref_filtered_prefix_enumerated_count": 1,
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
                    "ref_filter": [],
                },
                {
                    "label": "missing",
                    "key": "missing-key",
                    "available": True,
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 0,
                    "ref_filtered_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": [],
                },
            ],
        }

        rendered = ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertIn("Cache usage: 11 active caches, 10.3 GiB (source: rest)", rendered)
        self.assertIn("present; exact_count=1", rendered)
        self.assertIn("id=501 ref=refs/heads/main size=1.0 KiB", rendered)
        # The missing-key marker is part of the human-readable audit summary.
        self.assertIn(": missing;", rendered)
        self.assertIn("missing; exact_count=0", rendered)

    def test_render_cache_key_probe_text_rejects_unavailable_probe_snapshot(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 0,
                "active_caches_size_in_bytes": 0,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": False,
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "unavailable",
                    "api_prefix_enumerated_count": 0,
                    "ref_filtered_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": [],
                    "reason": "actions/caches: rate limited",
                }
            ],
        }

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.UNAVAILABLE)
        self.assertEqual(raised.exception.field, "cache_key_probes")

    def test_render_cache_key_probe_text_rejects_incomplete_probe_snapshot(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": True,
                    "present": True,
                    "exact_count": 1,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "ref_filtered_prefix_enumerated_count": 1,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": [],
                }
            ],
        }
        incomplete_snapshots = (
            ("cache_usage", lambda value: value.pop("cache_usage")),
            ("cache_key_probes.label", lambda value: value["cache_key_probes"][0].pop("label")),
            ("cache_key_probes.entries", lambda value: value["cache_key_probes"][0].pop("entries")),
        )
        for expected_field, mutate in incomplete_snapshots:
            with self.subTest(expected_field=expected_field):
                candidate = json.loads(json.dumps(snapshot))
                mutate(candidate)

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.render_cache_key_probe_text(candidate)

                self.assertIn(expected_field, raised.exception.field)

    def test_render_cache_key_probe_text_rejects_malformed_probe_entries(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": True,
                    "present": True,
                    "exact_count": 1,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "ref_filtered_prefix_enumerated_count": 1,
                    "prefix_only_count": 0,
                    "entries": [
                        {
                            "cache_id": 501,
                            "ref": "refs/heads/main",
                            "key": "exact-key",
                            "last_accessed_at": "2026-06-25T10:00:00Z",
                            "size_bytes": "1024",
                        }
                    ],
                    "ref_filter": [],
                }
            ],
        }

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
        self.assertIn("size_bytes", raised.exception.field)

    def test_render_cache_key_probe_text_rejects_contradictory_probe_state(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            },
            "cache_refs": ["refs/heads/main"],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": True,
                    "present": True,
                    "exact_count": 1,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "ref_filtered_prefix_enumerated_count": 1,
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
                    "ref_filter": ["refs/heads/main"],
                }
            ],
        }
        contradictions = (
            ("cache_key_probe_snapshot.cache_key_probes.present", lambda value: value["cache_key_probes"][0].update({"exact_count": 0})),
            ("cache_key_probe_snapshot.cache_key_probes.entries", lambda value: value["cache_key_probes"][0].update({"entries": []})),
            (
                "cache_key_probe_snapshot.cache_key_probes.api_prefix_count_source",
                lambda value: value["cache_key_probes"][0].update({"api_prefix_count_source": "unavailable"}),
            ),
            (
                "cache_key_probe_snapshot.cache_key_probes.prefix_only_count",
                lambda value: value["cache_key_probes"][0].update({"prefix_only_count": 99}),
            ),
            (
                "cache_key_probe_snapshot.cache_key_probes.ref_filtered_prefix_enumerated_count",
                lambda value: value["cache_key_probes"][0].update({"ref_filtered_prefix_enumerated_count": 0}),
            ),
        )
        for expected_field, mutate in contradictions:
            with self.subTest(expected_field=expected_field):
                candidate = json.loads(json.dumps(snapshot))
                mutate(candidate)

                with self.assertRaises(ci_storage_audit.AuditError) as raised:
                    ci_storage_audit.render_cache_key_probe_text(candidate)

                self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.INVALID)
                self.assertEqual(raised.exception.field, expected_field)

    def test_render_cache_persistence_failure_text_reports_contract_failure(self) -> None:
        error = ci_storage_audit.AuditError(
            "actions/caches: rate limited",
            kind=ci_storage_audit.FailureKind.UNAVAILABLE,
            field="actions/caches",
        )

        rendered = ci_storage_audit.render_cache_persistence_failure_text(error)

        self.assertIn("- contract failure kind: `unavailable`", rendered)
        self.assertIn("- contract failure field: `actions/caches`", rendered)
        self.assertIn("ERROR: unavailable actions/caches: actions/caches: rate limited", rendered)

    def test_render_cache_persistence_audit_text_includes_evidence_and_probe_text(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 11,
                "active_caches_size_in_bytes": 11_044_557_069,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "nextest-archive",
                    "key": "exact-key",
                    "available": True,
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 0,
                    "ref_filtered_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": [],
                },
            ],
        }

        rendered = ci_storage_audit.render_cache_persistence_audit_text(
            snapshot,
            restore_hits=[
                ci_storage_audit.LabeledValue("nextest archive", "false"),
            ],
            save_outcomes=[
                ci_storage_audit.LabeledValue("nextest archive", "success"),
            ],
        )

        self.assertIn("### Cache persistence audit", rendered)
        self.assertIn("- nextest archive restore hit: `false`", rendered)
        self.assertIn("- nextest archive save outcome: `success`", rendered)
        self.assertIn("```text", rendered)
        self.assertIn(": missing;", rendered)

    def test_cache_persistence_annotations_report_missing_keys(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "exact-key",
                    "available": True,
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 0,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 0,
                    "ref_filtered_prefix_enumerated_count": 0,
                    "prefix_only_count": 0,
                    "entries": [],
                    "ref_filter": [],
                },
            ],
        }

        self.assertEqual(
            ci_storage_audit.cache_persistence_annotations(snapshot),
            [
                "::warning::one or more root nextest cache keys are missing from the Actions cache inventory after save/restore; inspect cache save outcomes and repository cache usage above for quota/eviction context",
            ],
        )

    def test_render_cache_key_probe_text_warns_on_prefix_only_match(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": True,
                "active_caches_count": 1,
                "active_caches_size_in_bytes": 1024,
                "source": "rest",
            },
            "cache_refs": [],
            "cache_key_probes": [
                {
                    "label": "probe",
                    "key": "foo",
                    "available": True,
                    "present": False,
                    "exact_count": 0,
                    "api_prefix_count": 1,
                    "api_prefix_count_source": "github_total_count",
                    "api_prefix_enumerated_count": 1,
                    "ref_filtered_prefix_enumerated_count": 1,
                    "prefix_only_count": 1,
                    "entries": [],
                    "ref_filter": [],
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
