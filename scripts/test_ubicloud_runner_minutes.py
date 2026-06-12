#!/usr/bin/env python3
"""Self-tests for the GitHub Actions runner-minute meter."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "ubicloud_runner_minutes.py"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("ubicloud_runner_minutes", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load ubicloud_runner_minutes.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def runner_config_text() -> str:
    return """
schema_version = 1

[runners.github_hosted]
variable = "CI_RUNNER_GITHUB_HOSTED"
label = "ubuntu-latest"

[runners.managed_heavy]
variable = "CI_RUNNER_MANAGED_HEAVY"
label = "ubicloud-standard-4"

[runners.managed_light]
variable = "CI_RUNNER_MANAGED_LIGHT"
label = "ubicloud-standard-2"

[workflows.ci]
detector = "github_hosted"
test-archive = "managed_heavy"
test-shards = "managed_heavy"
gate = "github_hosted"

[workflows.ci_runner_debug]
debug-heavy = "managed_heavy"
debug-light = "managed_light"

[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"
debug_workflow = "ci_runner_debug"
included_workflows = ["ci", "ci_runner_debug"]
"""


def run_payload(run_id: int, **overrides):
    payload = {
        "id": run_id,
        "name": "CI",
        "path": ".github/workflows/ci.yml",
        "event": "pull_request",
        "head_branch": "feature/cost",
        "head_sha": f"{run_id:040x}"[-40:],
        "status": "completed",
        "conclusion": "success",
        "created_at": "2026-06-12T00:00:00Z",
        "updated_at": "2026-06-12T00:20:00Z",
        "html_url": f"https://github.com/example/repo/actions/runs/{run_id}",
    }
    payload.update(overrides)
    return payload


def job_payload(name: str, label: str, started_at: str, completed_at: str, conclusion: str = "success"):
    return {
        "name": name,
        "labels": [label],
        "status": "completed",
        "conclusion": conclusion,
        "started_at": started_at,
        "completed_at": completed_at,
    }


def artifact_payload(name: str):
    return {"name": name, "expired": False}


def assert_extract_fingerprint_ignores_empty_suffix() -> None:
    module = load_script()
    prefix = "nextest-archive-fingerprint-"
    assert module.extract_fingerprint({"artifacts": [artifact_payload(prefix)]}, prefix) is None
    assert (
        module.extract_fingerprint(
            {"artifacts": [artifact_payload(prefix), artifact_payload(f"{prefix}inputs-a")]},
            prefix,
        )
        == "inputs-a"
    )


def assert_configured_workflow_paths_paginates_workflow_list() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(runner_config_text(), encoding="utf-8")
        config = module.load_runner_config(config_path)

    class FakeClient:
        def __init__(self) -> None:
            self.calls = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.calls.append((path, params, paginate))
            assert path == "actions/workflows", path
            return {
                "workflows": [
                    {"path": ".github/workflows/ci.yml"},
                    {"path": ".github/workflows/ci-runner-debug.yml"},
                    {"path": ".github/workflows/not-metered.yml"},
                ]
            }

    client = FakeClient()
    paths = module.configured_workflow_paths(client, config)
    assert client.calls == [("actions/workflows", None, True)], client.calls
    assert paths == {".github/workflows/ci.yml", ".github/workflows/ci-runner-debug.yml"}, paths


def assert_resolve_pr_states_uses_workflow_run_pr_number() -> None:
    module = load_script()

    class FakeClient:
        def __init__(self) -> None:
            self.api_paths = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_paths.append(path)
            assert params is None, params
            assert not paginate, paginate
            assert path == "pulls/321", path
            return {"number": 321, "draft": True, "state": "open"}

        def graphql(self, query: str, fields: dict[str, str | int]):
            self.graphql_fields.append(fields)
            assert fields["number"] == 321, fields
            return {
                "data": {
                    "repository": {
                        "pullRequest": {
                            "timelineItems": {
                                "nodes": [],
                            }
                        }
                    }
                }
            }

    runs_payload = {
        "workflow_runs": [
            run_payload(
                30,
                head_branch="same-name-in-fork",
                pull_requests=[{"number": 321}],
            )
        ]
    }
    client = FakeClient()
    states = module.resolve_pr_states(client, "example/repo", runs_payload)
    assert client.api_paths == ["pulls/321"], client.api_paths
    assert client.graphql_fields == [{"owner": "example", "repo": "repo", "number": 321}], client.graphql_fields
    assert states["30"]["number"] == 321, states
    assert states["30"]["draft_at_run"] is True, states


def assert_resolve_pr_states_falls_back_when_run_has_no_pr_refs() -> None:
    module = load_script()

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "example:feature/cost", "state": "all", "per_page": "20"}, params
            assert not paginate, paginate
            return [
                {
                    "number": 654,
                    "draft": False,
                    "state": "open",
                    "created_at": "2026-06-11T00:00:00Z",
                    "closed_at": None,
                    "updated_at": "2026-06-12T00:10:00Z",
                }
            ]

        def graphql(self, query: str, fields: dict[str, str | int]):
            self.graphql_fields.append(fields)
            assert fields["number"] == 654, fields
            return {
                "data": {
                    "repository": {
                        "pullRequest": {
                            "timelineItems": {
                                "nodes": [{"__typename": "ReadyForReviewEvent", "createdAt": "2026-06-12T00:05:00Z"}],
                            }
                        }
                    }
                }
            }

    client = FakeClient()
    states = module.resolve_pr_states(
        client,
        "example/repo",
        {"workflow_runs": [run_payload(31, created_at="2026-06-12T00:00:00Z", pull_requests=[])]},
    )
    assert client.api_calls == [
        ("pulls", {"head": "example:feature/cost", "state": "all", "per_page": "20"}, False)
    ], client.api_calls
    assert client.graphql_fields == [{"owner": "example", "repo": "repo", "number": 654}], client.graphql_fields
    assert states["31"]["number"] == 654, states
    assert states["31"]["draft_at_run"] is True, states
    assert states["31"]["ready_at"] == "2026-06-12T00:05:00Z", states


def assert_resolve_pr_states_fallback_selects_by_run_time() -> None:
    module = load_script()

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "example:feature/cost", "state": "all", "per_page": "20"}, params
            assert not paginate, paginate
            return [
                {
                    "number": 700,
                    "draft": False,
                    "state": "closed",
                    "created_at": "2026-06-10T00:00:00Z",
                    "closed_at": "2026-06-11T00:00:00Z",
                    "updated_at": "2026-06-11T00:00:00Z",
                },
                {
                    "number": 701,
                    "draft": False,
                    "state": "open",
                    "created_at": "2026-06-12T00:00:00Z",
                    "closed_at": None,
                    "updated_at": "2026-06-12T01:00:00Z",
                },
            ]

        def graphql(self, query: str, fields: dict[str, str | int]):
            self.graphql_fields.append(fields)
            return {
                "data": {
                    "repository": {
                        "pullRequest": {
                            "timelineItems": {"nodes": []},
                        }
                    }
                }
            }

    client = FakeClient()
    states = module.resolve_pr_states(
        client,
        "example/repo",
        {
            "workflow_runs": [
                run_payload(32, created_at="2026-06-10T12:00:00Z", pull_requests=[]),
                run_payload(33, created_at="2026-06-12T12:00:00Z", pull_requests=[]),
            ]
        },
    )
    assert client.api_calls == [
        ("pulls", {"head": "example:feature/cost", "state": "all", "per_page": "20"}, False)
    ], client.api_calls
    assert states["32"]["number"] == 700, states
    assert states["33"]["number"] == 701, states


def assert_build_report_classifies_runs_and_totals_minutes() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(runner_config_text(), encoding="utf-8")
        config = module.load_runner_config(config_path)
        assert config.workflow_keys == {"ci", "ci_runner_debug"}, config.workflow_keys

    runs = [
        run_payload(
            10,
            conclusion="cancelled",
            created_at="2026-06-12T00:00:00Z",
            updated_at="2026-06-12T00:03:00Z",
        ),
        run_payload(11, created_at="2026-06-12T00:05:00Z", updated_at="2026-06-12T00:17:00Z"),
        run_payload(12, created_at="2026-06-12T00:20:00Z", updated_at="2026-06-12T00:27:00Z"),
        run_payload(
            13,
            name="CI Runner Debug",
            path=".github/workflows/ci-runner-debug.yml",
            event="workflow_dispatch",
            head_branch="main",
            created_at="2026-06-12T01:00:00Z",
            updated_at="2026-06-12T01:30:00Z",
        ),
    ]
    jobs_by_run_id = {
        10: {
            "jobs": [
                job_payload(
                    "nextest shard 1 of 4",
                    "ubicloud-standard-4",
                    "2026-06-12T00:01:00Z",
                    "2026-06-12T00:03:00Z",
                    "cancelled",
                )
            ]
        },
        11: {
            "jobs": [
                job_payload("fmt-check", "ubicloud-standard-2", "2026-06-12T00:05:00Z", "2026-06-12T00:07:00Z"),
                job_payload("nextest shard 1 of 4", "ubicloud-standard-4", "2026-06-12T00:07:00Z", "2026-06-12T00:17:00Z"),
            ]
        },
        12: {
            "jobs": [
                job_payload("fmt-check", "ubicloud-standard-2", "2026-06-12T00:20:00Z", "2026-06-12T00:21:00Z"),
                job_payload("nextest shard 1 of 4", "ubicloud-standard-4", "2026-06-12T00:21:00Z", "2026-06-12T00:26:00Z"),
            ]
        },
        13: {
            "jobs": [
                job_payload("debug-heavy", "ubicloud-standard-4", "2026-06-12T01:00:00Z", "2026-06-12T01:30:00Z"),
            ]
        },
    }
    artifacts_by_run_id = {
        10: {"artifacts": []},
        11: {"artifacts": [artifact_payload("nextest-archive-fingerprint-v1-Linux-X64-test-profile-shards-4-inputs-a")]},
        12: {"artifacts": [artifact_payload("nextest-archive-fingerprint-v1-Linux-X64-test-profile-shards-4-inputs-a")]},
        13: {"artifacts": []},
    }
    pr_state_by_run_id = {
        10: {"number": 648, "draft_at_run": True, "ready_at": "2026-06-12T00:04:00Z"},
        11: {"number": 648, "draft_at_run": False, "ready_at": "2026-06-12T00:04:00Z"},
        12: {"number": 648, "draft_at_run": False, "ready_at": "2026-06-12T00:04:00Z"},
    }

    report = module.build_report(
        repo="example/repo",
        runs_payload={"workflow_runs": runs},
        jobs_payload_by_run_id=jobs_by_run_id,
        artifacts_payload_by_run_id=artifacts_by_run_id,
        pr_state_by_run_id=pr_state_by_run_id,
        runner_config=config,
        generated_at="2026-06-12T02:00:00Z",
    )

    runs_by_id = {run["id"]: run for run in report["runs"]}
    assert "cancelled-superseded" in runs_by_id[10]["classifications"], runs_by_id[10]
    assert "draft-stage" in runs_by_id[10]["classifications"], runs_by_id[10]
    assert "completed-green" in runs_by_id[11]["classifications"], runs_by_id[11]
    assert "fingerprint-identical" in runs_by_id[12]["classifications"], runs_by_id[12]
    assert runs_by_id[11]["fingerprint"] == "v1-Linux-X64-test-profile-shards-4-inputs-a", runs_by_id[11]
    assert runs_by_id[13]["workflow_key"] == "ci_runner_debug", runs_by_id[13]

    totals = report["totals_by_tier"]
    assert totals["managed_heavy"]["minutes"] == 47.0, totals
    assert totals["managed_light"]["minutes"] == 3.0, totals
    assert report["debug_sessions"][0]["id"] == 13, report["debug_sessions"]


def assert_unknown_labels_are_reported_without_crashing() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(runner_config_text(), encoding="utf-8")
        config = module.load_runner_config(config_path)
    report = module.build_report(
        repo="example/repo",
        runs_payload={"workflow_runs": [run_payload(20)]},
        jobs_payload_by_run_id={
            20: {
                "jobs": [
                    job_payload("mystery", "custom-runner", "2026-06-12T00:00:00Z", "2026-06-12T00:01:00Z")
                ]
            }
        },
        artifacts_payload_by_run_id={20: {"artifacts": []}},
        pr_state_by_run_id={},
        runner_config=config,
        generated_at="2026-06-12T02:00:00Z",
    )
    run = report["runs"][0]
    assert run["jobs"][0]["tier"] == "unknown", run
    assert run["jobs"][0]["runner_label"] == "custom-runner", run


def main() -> int:
    assert_extract_fingerprint_ignores_empty_suffix()
    assert_configured_workflow_paths_paginates_workflow_list()
    assert_resolve_pr_states_uses_workflow_run_pr_number()
    assert_resolve_pr_states_falls_back_when_run_has_no_pr_refs()
    assert_resolve_pr_states_fallback_selects_by_run_time()
    assert_build_report_classifies_runs_and_totals_minutes()
    assert_unknown_labels_are_reported_without_crashing()
    print("OK: Ubicloud runner-minute meter self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
