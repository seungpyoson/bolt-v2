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

    def test_fetch_cache_key_probes_keeps_later_probes_after_api_error(self) -> None:
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

        probes = ci_storage_audit.fetch_cache_key_probes(
            client,
            [
                ci_storage_audit.CacheKeyProbeRequest("unavailable", "unavailable-key"),
                ci_storage_audit.CacheKeyProbeRequest("present", "present-key"),
            ],
        )

        self.assertFalse(probes[0]["available"])
        self.assertFalse(probes[0]["present"])
        self.assertIn("rate limited", probes[0]["reason"])
        self.assertTrue(probes[1]["available"])
        self.assertTrue(probes[1]["present"])
        self.assertEqual(
            [call[0] for call in client.calls],
            ["actions/caches", "actions/caches"],
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

    def test_normalize_cache_refs_drops_empty_values_and_duplicates(self) -> None:
        refs = ci_storage_audit.normalize_cache_ref_inputs(
            cache_refs=[" refs/pull/986/merge ", "", "refs/pull/986/merge"],
            cache_branches=["main", "", "main", "release/train"],
        )

        self.assertEqual(
            refs,
            ["refs/pull/986/merge", "refs/heads/main", "refs/heads/release/train"],
        )

    def test_fetch_cache_usage_unavailable_keeps_reason(self) -> None:
        client = FakeClient(
            {
                "actions/cache/usage": ci_storage_audit.GhApiError(
                    "actions/cache/usage",
                    "secondary rate limit",
                )
            }
        )

        usage = ci_storage_audit.fetch_cache_usage(client)

        self.assertEqual(
            usage,
            {
                "available": False,
                "active_caches_count": 0,
                "active_caches_size_in_bytes": 0,
                "source": "unavailable",
                "reason": "actions/cache/usage: secondary rate limit",
            },
        )

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

        self.assertIn("Cache usage: 11 active caches, 10.3 GiB (source: rest)", rendered)
        self.assertIn("present; exact_count=1", rendered)
        self.assertIn("id=501 ref=refs/heads/main size=1.0 KiB", rendered)
        # The workflow warning grep is coupled to this exact missing-key marker.
        self.assertIn(": missing;", rendered)
        self.assertIn("missing; exact_count=0", rendered)

    def test_render_cache_key_probe_text_reports_unavailable_probe(self) -> None:
        snapshot = {
            "snapshot_utc": "2026-06-23T00:00:00+00:00",
            "repo": "owner/repo",
            "cache_usage": {
                "available": False,
                "active_caches_count": 0,
                "active_caches_size_in_bytes": 0,
                "source": "unavailable",
                "reason": "actions/cache/usage: secondary rate limit",
            },
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
                    "prefix_only_count": 0,
                    "entries": [],
                    "reason": "actions/caches: rate limited",
                }
            ],
        }

        rendered = ci_storage_audit.render_cache_key_probe_text(snapshot)

        self.assertIn(
            "Cache usage: unavailable (source: unavailable; reason=actions/cache/usage: secondary rate limit)",
            rendered,
        )
        self.assertIn(": unavailable;", rendered)
        self.assertIn("reason=actions/caches: rate limited", rendered)

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
