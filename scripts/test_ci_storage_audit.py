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
        self.calls: list[tuple[str, bool]] = []

    def api(self, path: str, *, params: dict[str, str] | None = None, paginate: bool = False) -> Any:
        del params
        self.calls.append((path, paginate))
        value = self.responses[path]
        if isinstance(value, Exception):
            raise value
        return value


class CiStorageAuditTests(unittest.TestCase):
    def test_build_snapshot_serializes_stable_contract_from_fixture_payloads(self) -> None:
        client = FakeClient(
            {
                "actions/caches": {
                    "total_count": 2,
                    "actions_caches": [
                        {
                            "id": 101,
                            "ref": "refs/heads/main",
                            "key": "linux-a",
                            "last_accessed_at": "2026-06-20T00:00:00Z",
                            "size_in_bytes": 1024,
                        },
                        {
                            "id": 102,
                            "ref": "refs/pull/1/merge",
                            "key": "linux-b",
                            "last_accessed_at": "2026-06-21T00:00:00Z",
                            "size_in_bytes": 2048,
                        },
                    ],
                },
                "actions/artifacts": {
                    "total_count": 3,
                    "artifacts": [
                        {"name": "logs", "size_in_bytes": 512},
                        {"name": "binary", "size_in_bytes": 4096},
                        {"name": "logs", "size_in_bytes": 1536},
                    ],
                },
                "actions/permissions": {"artifact_log_retention_days": 30},
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
                ("actions/caches", True),
                ("actions/artifacts", True),
                ("actions/permissions", False),
                ("rules/branches/main", False),
            ],
        )

    def test_human_bytes_uses_binary_units(self) -> None:
        self.assertEqual(ci_storage_audit.human_bytes(0), "0 B")
        self.assertEqual(ci_storage_audit.human_bytes(999), "999 B")
        self.assertEqual(ci_storage_audit.human_bytes(1024), "1.0 KiB")
        self.assertEqual(ci_storage_audit.human_bytes(1536), "1.5 KiB")
        self.assertEqual(ci_storage_audit.human_bytes(1024 * 1024), "1.0 MiB")

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

    def test_retention_settings_ui_only_when_rest_field_absent(self) -> None:
        client = FakeClient({"actions/permissions": {"enabled": True}})

        self.assertEqual(
            ci_storage_audit.fetch_retention_setting(client),
            {"artifact_and_log_days": None, "source": "settings-ui-only"},
        )

    def test_retention_unavailable_on_api_error(self) -> None:
        client = FakeClient(
            {"actions/permissions": ci_storage_audit.GhApiError("actions/permissions", "denied")}
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


if __name__ == "__main__":
    unittest.main()
