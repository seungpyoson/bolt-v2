from __future__ import annotations

import contextlib
import importlib.util
import io
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
        self.assertEqual(
            decoded["artifacts"]["by_name"],
            [
                {"name": "binary", "total_bytes": 4096, "count": 1},
                {"name": "logs", "total_bytes": 2048, "count": 2},
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

    def test_merge_paginated_payload_rejects_scalar_drift(self) -> None:
        payload = [
            {
                "total_count": 1,
                "artifacts": [{"name": "first", "size_in_bytes": 1}],
            },
            {
                "total_count": 2,
                "artifacts": [{"name": "second", "size_in_bytes": 2}],
            },
        ]

        with self.assertRaises(ci_storage_audit.AuditError) as raised:
            ci_storage_audit.merge_paginated_payload(payload)

        self.assertEqual(raised.exception.kind, ci_storage_audit.FailureKind.AMBIGUOUS)
        self.assertEqual(raised.exception.field, "actions/artifacts.total_count")

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
