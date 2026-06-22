#!/usr/bin/env python3
"""Self-tests for the GitHub Actions runner-minute meter."""

from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
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
gate = "github_hosted"

[workflows.backtester_ci]
test = "managed_heavy"
gate = "github_hosted"

[workflows.ci_runner_debug]
debug-heavy = "managed_heavy"
debug-light = "managed_light"

[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"
debug_workflow = "ci_runner_debug"
included_workflows = ["ci", "backtester_ci", "ci_runner_debug"]

[meter.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
branch_pull_requests_per_page = 20
draft_timeline_items = 100
"""


def load_test_config(module, config_text: str | None = None):
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(config_text or runner_config_text(), encoding="utf-8")
        return module.load_runner_config(config_path)


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
            {
                "artifacts": [
                    artifact_payload(f"{prefix}inputs-b"),
                    artifact_payload(f"{prefix}inputs-a"),
                ]
            },
            prefix,
        )
        is None
    )
    assert (
        module.extract_fingerprint(
            {"artifacts": [artifact_payload(prefix), artifact_payload(f"{prefix}inputs-a")]},
            prefix,
        )
        == "inputs-a"
    )


def assert_meter_api_limits_come_from_config() -> None:
    module = load_script()
    config_text = (
        runner_config_text()
        .replace("workflow_runs_per_page = 100", "workflow_runs_per_page = 37")
        .replace("run_jobs_per_page = 100", "run_jobs_per_page = 38")
        .replace("run_artifacts_per_page = 100", "run_artifacts_per_page = 39")
        .replace("branch_pull_requests_per_page = 20", "branch_pull_requests_per_page = 7")
        .replace("draft_timeline_items = 100", "draft_timeline_items = 11")
    )
    config = load_test_config(module, config_text)

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []
            self.graphql_queries = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            if path == "actions/runs":
                return {"workflow_runs": [run_payload(81)]}
            if path == "actions/workflows":
                return {"workflows": [{"path": ".github/workflows/ci.yml"}]}
            if path == "actions/runs/81/jobs":
                return {"jobs": []}
            if path == "actions/runs/81/artifacts":
                return {"artifacts": []}
            if path == "pulls":
                return [
                    {
                        "number": 81,
                        "draft": False,
                        "state": "open",
                        "created_at": "2026-06-11T00:00:00Z",
                        "closed_at": None,
                        "updated_at": "2026-06-12T00:00:00Z",
                    }
                ]
            raise AssertionError(f"unexpected API path: {path}")

        def graphql(self, query: str, fields: dict[str, str | int]):
            self.graphql_queries.append(query)
            self.graphql_fields.append(fields)
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

    client = FakeClient()
    runs_payload = module.fetch_runs(client, config, [], 1, None)
    module.fetch_jobs_and_artifacts(client, runs_payload, config)
    module.resolve_pr_states(
        client,
        "example/repo",
        {
            "workflow_runs": [
                run_payload(
                    82,
                    pull_requests=[],
                    head_repository={"owner": {"login": "example"}, "full_name": "example/repo"},
                )
            ]
        },
        config,
    )
    assert ("actions/runs", {"per_page": "37", "created": client.api_calls[0][1]["created"]}, True) in client.api_calls
    assert ("actions/runs/81/jobs", {"per_page": "38"}, True) in client.api_calls
    assert ("actions/runs/81/artifacts", {"per_page": "39"}, True) in client.api_calls
    assert ("pulls", {"head": "example:feature/cost", "state": "all", "per_page": "7"}, True) in client.api_calls
    assert "timelineItems(first:$timelineLimit" in client.graphql_queries[0], client.graphql_queries
    assert client.graphql_fields[0]["timelineLimit"] == 11, client.graphql_fields


def assert_configured_workflow_paths_paginates_workflow_list() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def __init__(self) -> None:
            self.calls = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.calls.append((path, params, paginate))
            assert path == "actions/workflows", path
            return {
                "workflows": [
                    {"path": ".github/workflows/ci.yml"},
                    {"path": ".github/workflows/backtester-ci.yml"},
                    {"path": ".github/workflows/ci-runner-debug.yml"},
                    {"path": ".github/workflows/not-metered.yml"},
                ]
            }

    client = FakeClient()
    paths = module.configured_workflow_paths(client, config)
    assert client.calls == [("actions/workflows", None, True)], client.calls
    assert paths == {
        ".github/workflows/ci.yml",
        ".github/workflows/backtester-ci.yml",
        ".github/workflows/ci-runner-debug.yml",
    }, paths


def assert_gh_client_flattens_paginated_list_pages() -> None:
    module = load_script()
    original_run = subprocess.run

    def fake_run(cmd, text, capture_output, check):
        assert "--paginate" in cmd, cmd
        return subprocess.CompletedProcess(
            cmd,
            0,
            stdout=json.dumps([[{"number": 1}], [{"number": 2}]]),
            stderr="",
        )

    subprocess.run = fake_run
    try:
        payload = module.GhClient("example/repo").api("pulls", paginate=True)
    finally:
        subprocess.run = original_run
    assert payload == [{"number": 1}, {"number": 2}], payload

    def fake_empty_run(cmd, text, capture_output, check):
        assert "--paginate" in cmd, cmd
        return subprocess.CompletedProcess(cmd, 0, stdout=json.dumps([[], []]), stderr="")

    subprocess.run = fake_empty_run
    try:
        empty_payload = module.GhClient("example/repo").api("pulls", paginate=True)
    finally:
        subprocess.run = original_run
    assert empty_payload == [], empty_payload


def assert_gh_client_rejects_graphql_errors() -> None:
    module = load_script()
    original_run = subprocess.run

    def fake_error_run(cmd, text, capture_output, check):
        assert cmd[:3] == ["gh", "api", "graphql"], cmd
        return subprocess.CompletedProcess(
            cmd,
            0,
            stdout=json.dumps({"errors": [{"message": "Resource not accessible"}]}),
            stderr="",
        )

    subprocess.run = fake_error_run
    try:
        try:
            module.GhClient("example/repo").graphql("query{}", {})
        except module.MeterError as exc:
            assert "gh graphql returned errors" in str(exc), exc
        else:
            raise AssertionError("GraphQL errors must raise MeterError")
    finally:
        subprocess.run = original_run


def assert_fetch_runs_sorts_before_limit() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            if path == "actions/runs":
                return {
                    "workflow_runs": [
                        run_payload(90, created_at="2026-06-12T00:00:00Z"),
                        run_payload(91, created_at="2026-06-12T01:00:00Z"),
                    ]
                }
            if path == "actions/workflows":
                return {"workflows": [{"path": ".github/workflows/ci.yml"}]}
            raise AssertionError(f"unexpected API path: {path}")

    runs_payload = module.fetch_runs(FakeClient(), config, [], 1, 1)
    assert [run["id"] for run in runs_payload["workflow_runs"]] == [91], runs_payload


def assert_resolve_pr_states_uses_workflow_run_pr_number() -> None:
    module = load_script()
    config = load_test_config(module)

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
    states = module.resolve_pr_states(client, "example/repo", runs_payload, config)
    assert client.api_paths == ["pulls/321"], client.api_paths
    assert client.graphql_fields == [{"owner": "example", "repo": "repo", "number": 321, "timelineLimit": 100}], client.graphql_fields
    assert states["30"]["number"] == 321, states
    assert states["30"]["draft_at_run"] is True, states


def assert_resolve_pr_states_direct_pr_lookup_errors_are_fatal() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            assert path == "pulls/321", path
            raise module.MeterError("rate limit while fetching PR")

        def graphql(self, query: str, fields: dict[str, str | int]):
            raise AssertionError("timeline must not be queried after PR lookup failure")

    try:
        module.resolve_pr_states(
            FakeClient(),
            "example/repo",
            {"workflow_runs": [run_payload(39, pull_requests=[{"number": 321}])]},
            config,
        )
    except module.MeterError as exc:
        assert "rate limit while fetching PR" in str(exc), exc
    else:
        raise AssertionError("direct PR lookup failures must fail the meter")


def assert_resolve_pr_states_direct_pr_lookup_rejects_malformed_payloads() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            assert path == "pulls/321", path
            return []

        def graphql(self, query: str, fields: dict[str, str | int]):
            raise AssertionError("malformed direct PR payload should fail before timeline lookup")

    try:
        module.resolve_pr_states(
            FakeClient(),
            "example/repo",
            {"workflow_runs": [run_payload(40, pull_requests=[{"number": 321}])]},
            config,
        )
    except module.MeterError as exc:
        assert "pulls/321 payload is malformed" in str(exc), exc
    else:
        raise AssertionError("malformed direct PR payloads must fail the meter")


def assert_resolve_pr_states_falls_back_when_run_has_no_pr_refs() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "example:feature/cost", "state": "all", "per_page": "20"}, params
            assert paginate, paginate
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
        {
            "workflow_runs": [
                run_payload(
                    31,
                    created_at="2026-06-12T00:00:00Z",
                    pull_requests=[],
                    head_repository={"owner": {"login": "example"}, "full_name": "example/repo"},
                )
            ]
        },
        config,
    )
    assert client.api_calls == [
        ("pulls", {"head": "example:feature/cost", "state": "all", "per_page": "20"}, True)
    ], client.api_calls
    assert client.graphql_fields == [{"owner": "example", "repo": "repo", "number": 654, "timelineLimit": 100}], client.graphql_fields
    assert states["31"]["number"] == 654, states
    assert states["31"]["draft_at_run"] is True, states
    assert states["31"]["ready_at"] == "2026-06-12T00:05:00Z", states


def assert_resolve_pr_states_abstains_without_pr_refs_or_head_owner() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            raise AssertionError("fallback should not query pulls without a head owner")

        def graphql(self, query: str, fields: dict[str, str | int]):
            raise AssertionError("fallback should not query timeline without a PR")

    states = module.resolve_pr_states(
        FakeClient(),
        "example/repo",
        {"workflow_runs": [run_payload(35, pull_requests=[])]},
        config,
    )
    assert states == {}, states


def assert_resolve_pr_states_fallback_uses_head_repository_owner() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "forker:feature/cost", "state": "all", "per_page": "20"}, params
            assert paginate, paginate
            return [
                {
                    "number": 655,
                    "draft": False,
                    "state": "open",
                    "created_at": "2026-06-11T00:00:00Z",
                    "closed_at": None,
                    "updated_at": "2026-06-12T00:10:00Z",
                }
            ]

        def graphql(self, query: str, fields: dict[str, str | int]):
            assert fields["number"] == 655, fields
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

    client = FakeClient()
    states = module.resolve_pr_states(
        client,
        "example/repo",
        {
            "workflow_runs": [
                run_payload(
                    34,
                    pull_requests=[],
                    head_repository={"owner": {"login": "forker"}, "full_name": "forker/repo"},
                )
            ]
        },
        config,
    )
    assert states["34"]["number"] == 655, states


def assert_resolve_pr_states_fallback_selects_by_run_time() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []
            self.graphql_fields = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "example:feature/cost", "state": "all", "per_page": "20"}, params
            assert paginate, paginate
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
                run_payload(
                    32,
                    created_at="2026-06-10T12:00:00Z",
                    pull_requests=[],
                    head_repository={"owner": {"login": "example"}, "full_name": "example/repo"},
                ),
                run_payload(
                    33,
                    created_at="2026-06-12T12:00:00Z",
                    pull_requests=[],
                    head_repository={"owner": {"login": "example"}, "full_name": "example/repo"},
                ),
            ]
        },
        config,
    )
    assert client.api_calls == [
        ("pulls", {"head": "example:feature/cost", "state": "all", "per_page": "20"}, True)
    ], client.api_calls
    assert states["32"]["number"] == 700, states
    assert states["33"]["number"] == 701, states


def assert_resolve_pr_states_fallback_abstains_when_no_pr_lifetime_matches() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def __init__(self) -> None:
            self.api_calls = []

        def api(self, path: str, *, params=None, paginate: bool = False):
            self.api_calls.append((path, params, paginate))
            assert path == "pulls", path
            assert params == {"head": "example:feature/cost", "state": "all", "per_page": "20"}, params
            assert paginate, paginate
            return [
                {
                    "number": 710,
                    "draft": False,
                    "state": "closed",
                    "created_at": "2026-06-10T00:00:00Z",
                    "closed_at": "2026-06-11T00:00:00Z",
                    "updated_at": "2026-06-12T00:00:00Z",
                },
                {
                    "number": 711,
                    "draft": False,
                    "state": "open",
                    "created_at": "2026-06-13T00:00:00Z",
                    "closed_at": None,
                    "updated_at": "2026-06-13T01:00:00Z",
                },
            ]

        def graphql(self, query: str, fields: dict[str, str | int]):
            raise AssertionError("fallback should not query timeline when no PR lifetime matches")

    states = module.resolve_pr_states(
        FakeClient(),
        "example/repo",
        {
            "workflow_runs": [
                run_payload(
                    36,
                    created_at="2026-06-12T12:00:00Z",
                    pull_requests=[],
                    head_repository={"owner": {"login": "example"}, "full_name": "example/repo"},
                )
            ]
        },
        config,
    )
    assert states == {}, states


def assert_timeline_truncation_is_visible_in_report() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            assert path == "pulls/321", path
            return {"number": 321, "draft": True, "state": "open"}

        def graphql(self, query: str, fields: dict[str, str | int]):
            return {
                "data": {
                    "repository": {
                        "pullRequest": {
                            "timelineItems": {
                                "nodes": [],
                                "pageInfo": {"hasNextPage": True},
                            }
                        }
                    }
                }
            }

    runs_payload = {"workflow_runs": [run_payload(37, pull_requests=[{"number": 321}])]}
    states = module.resolve_pr_states(FakeClient(), "example/repo", runs_payload, config)
    assert states["37"]["draft_timeline_truncated"] is True, states
    assert states["37"]["draft_at_run"] is None, states

    report = module.build_report(
        repo="example/repo",
        runs_payload=runs_payload,
        jobs_payload_by_run_id={
            37: {
                "jobs": [
                    job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T00:00:00Z", "2026-06-12T00:01:00Z")
                ]
            }
        },
        artifacts_payload_by_run_id={37: {"artifacts": []}},
        pr_state_by_run_id=states,
        runner_config=config,
        generated_at="2026-06-12T02:00:00Z",
    )
    assert "draft-timeline-truncated" in report["runs"][0]["classifications"], report["runs"][0]
    assert "draft-stage" not in report["runs"][0]["classifications"], report["runs"][0]
    assert report["lever_b_bounds"]["draft_stage"] == {}, report["lever_b_bounds"]


def assert_resolve_pr_states_handles_null_graphql_payloads() -> None:
    module = load_script()
    config = load_test_config(module)

    class FakeClient:
        def api(self, path: str, *, params=None, paginate: bool = False):
            assert path == "pulls/321", path
            return {"number": 321, "draft": True, "state": "open"}

        def graphql(self, query: str, fields: dict[str, str | int]):
            return {"data": None}

    states = module.resolve_pr_states(
        FakeClient(),
        "example/repo",
        {"workflow_runs": [run_payload(38, pull_requests=[{"number": 321}])]},
        config,
    )
    assert states["38"]["draft_at_run"] is None, states
    assert states["38"]["draft_timeline_unavailable"] is True, states

    runs_payload = {"workflow_runs": [run_payload(38, pull_requests=[{"number": 321}])]}
    report = module.build_report(
        repo="example/repo",
        runs_payload=runs_payload,
        jobs_payload_by_run_id={
            38: {
                "jobs": [
                    job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T00:00:00Z", "2026-06-12T00:01:00Z")
                ]
            }
        },
        artifacts_payload_by_run_id={38: {"artifacts": []}},
        pr_state_by_run_id=states,
        runner_config=config,
        generated_at="2026-06-12T02:00:00Z",
    )
    assert "draft-timeline-unavailable" in report["runs"][0]["classifications"], report["runs"][0]
    assert "draft-stage" not in report["runs"][0]["classifications"], report["runs"][0]
    assert report["lever_b_bounds"]["draft_stage"] == {}, report["lever_b_bounds"]


def assert_cancelled_superseded_requires_pull_request_pr_match_and_overlap() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(runner_config_text(), encoding="utf-8")
        config = module.load_runner_config(config_path)

    def report_for(runs, pr_states):
        jobs_by_run_id = {
            run["id"]: {
                "jobs": [
                    job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T00:00:00Z", "2026-06-12T00:01:00Z")
                ]
            }
            for run in runs
        }
        artifacts_by_run_id = {run["id"]: {"artifacts": []} for run in runs}
        return module.build_report(
            repo="example/repo",
            runs_payload={"workflow_runs": runs},
            jobs_payload_by_run_id=jobs_by_run_id,
            artifacts_payload_by_run_id=artifacts_by_run_id,
            pr_state_by_run_id=pr_states,
            runner_config=config,
            generated_at="2026-06-12T02:00:00Z",
        )

    push_report = report_for(
        [
            run_payload(
                40,
                event="push",
                head_branch="main",
                conclusion="cancelled",
                created_at="2026-06-12T00:00:00Z",
                updated_at="2026-06-12T00:05:00Z",
            ),
            run_payload(
                41,
                event="push",
                head_branch="main",
                created_at="2026-06-12T00:02:00Z",
                updated_at="2026-06-12T00:06:00Z",
            ),
        ],
        {},
    )
    push_run = {run["id"]: run for run in push_report["runs"]}[40]
    assert "cancelled" in push_run["classifications"], push_run
    assert "cancelled-superseded" not in push_run["classifications"], push_run

    different_pr_report = report_for(
        [
            run_payload(
                50,
                conclusion="cancelled",
                created_at="2026-06-12T00:00:00Z",
                updated_at="2026-06-12T00:05:00Z",
            ),
            run_payload(51, created_at="2026-06-12T00:02:00Z", updated_at="2026-06-12T00:06:00Z"),
        ],
        {
            50: {"number": 100, "draft_at_run": False, "ready_at": None},
            51: {"number": 101, "draft_at_run": False, "ready_at": None},
        },
    )
    old_run = {run["id"]: run for run in different_pr_report["runs"]}[50]
    assert "cancelled" in old_run["classifications"], old_run
    assert "cancelled-superseded" not in old_run["classifications"], old_run

    stale_successor_report = report_for(
        [
            run_payload(
                60,
                conclusion="cancelled",
                created_at="2026-06-12T00:00:00Z",
                updated_at="2026-06-12T00:05:00Z",
            ),
            run_payload(61, created_at="2026-06-12T00:06:00Z", updated_at="2026-06-12T00:10:00Z"),
        ],
        {
            60: {"number": 200, "draft_at_run": False, "ready_at": None},
            61: {"number": 200, "draft_at_run": False, "ready_at": None},
        },
    )
    stale_run = {run["id"]: run for run in stale_successor_report["runs"]}[60]
    assert "cancelled" in stale_run["classifications"], stale_run
    assert "cancelled-superseded" not in stale_run["classifications"], stale_run


def assert_build_report_classifies_runs_and_totals_minutes() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        config_path = pathlib.Path(tmpdir) / "github-actions-runners.toml"
        config_path.write_text(runner_config_text(), encoding="utf-8")
        config = module.load_runner_config(config_path)
        assert config.workflow_keys == {"ci", "backtester_ci", "ci_runner_debug"}, config.workflow_keys

    runs = [
        run_payload(
            10,
            conclusion="cancelled",
            created_at="2026-06-12T00:00:00Z",
            updated_at="2026-06-12T00:06:00Z",
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
        run_payload(
            14,
            name="Backtester CI",
            path=".github/workflows/backtester-ci.yml",
            created_at="2026-06-12T01:30:00Z",
            updated_at="2026-06-12T01:35:00Z",
        ),
        run_payload(
            15,
            conclusion="failure",
            created_at="2026-06-12T01:40:00Z",
            updated_at="2026-06-12T01:41:00Z",
        ),
        run_payload(
            16,
            created_at="2026-06-12T01:45:00Z",
            updated_at="2026-06-12T01:46:00Z",
        ),
        run_payload(
            17,
            created_at="2026-06-12T01:50:00Z",
            updated_at="2026-06-12T01:54:00Z",
        ),
    ]
    jobs_by_run_id = {
        10: {
            "jobs": [
                job_payload(
                    "nextest archive",
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
                job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T00:07:00Z", "2026-06-12T00:17:00Z"),
            ]
        },
        12: {
            "jobs": [
                job_payload("fmt-check", "ubicloud-standard-2", "2026-06-12T00:20:00Z", "2026-06-12T00:21:00Z"),
                job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T00:21:00Z", "2026-06-12T00:26:00Z"),
            ]
        },
        13: {
            "jobs": [
                job_payload("debug-heavy", "ubicloud-standard-4", "2026-06-12T01:00:00Z", "2026-06-12T01:30:00Z"),
            ]
        },
        14: {
            "jobs": [
                job_payload("test", "ubicloud-standard-4", "2026-06-12T01:30:00Z", "2026-06-12T01:34:00Z"),
            ]
        },
        15: {"jobs": []},
        16: {"jobs": []},
        17: {
            "jobs": [
                job_payload("nextest archive", "ubicloud-standard-4", "2026-06-12T01:50:00Z", "2026-06-12T01:53:00Z"),
            ]
        },
    }
    artifacts_by_run_id = {
        10: {"artifacts": []},
        11: {"artifacts": [artifact_payload("nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-inputs-a")]},
        12: {"artifacts": [artifact_payload("nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-inputs-a")]},
        13: {"artifacts": []},
        14: {"artifacts": []},
        15: {"artifacts": []},
        16: {
            "artifacts": [
                artifact_payload("nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-inputs-b"),
                artifact_payload("nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-inputs-c"),
            ]
        },
        17: {"artifacts": []},
    }
    pr_state_by_run_id = {
        10: {"number": 648, "draft_at_run": True, "ready_at": "2026-06-12T00:04:00Z"},
        11: {"number": 648, "draft_at_run": False, "ready_at": "2026-06-12T00:04:00Z"},
        12: {"number": 648, "draft_at_run": False, "ready_at": "2026-06-12T00:04:00Z"},
        17: {"number": 649, "draft_at_run": True, "ready_at": None},
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
    assert "fingerprint-unknown" in runs_by_id[10]["classifications"], runs_by_id[10]
    assert "completed-green" in runs_by_id[11]["classifications"], runs_by_id[11]
    assert "fingerprint-identical" in runs_by_id[12]["classifications"], runs_by_id[12]
    assert "failed" in runs_by_id[15]["classifications"], runs_by_id[15]
    assert "fingerprint-ambiguous" in runs_by_id[16]["classifications"], runs_by_id[16]
    assert "fingerprint-unknown" not in runs_by_id[16]["classifications"], runs_by_id[16]
    assert "draft-stage" in runs_by_id[17]["classifications"], runs_by_id[17]
    assert "cancelled-superseded" not in runs_by_id[17]["classifications"], runs_by_id[17]
    assert runs_by_id[11]["fingerprint"] == "v2-Linux-X64-test-profile-shards-4-inputs-a", runs_by_id[11]
    assert runs_by_id[13]["workflow_key"] == "ci_runner_debug", runs_by_id[13]
    assert runs_by_id[14]["workflow_key"] == "backtester_ci", runs_by_id[14]

    totals = report["totals_by_tier"]
    assert totals["managed_heavy"]["minutes"] == 54.0, totals
    assert totals["managed_light"]["minutes"] == 3.0, totals
    assert report["lever_b_bounds"]["draft_stage"]["managed_heavy"]["minutes"] == 5.0, report["lever_b_bounds"]
    assert report["lever_b_bounds"]["draft_stage_cancelled_superseded"]["managed_heavy"]["minutes"] == 2.0, report["lever_b_bounds"]
    assert report["debug_sessions"][0]["id"] == 13, report["debug_sessions"]


def assert_fingerprint_identity_is_scoped_to_fingerprint_workflow() -> None:
    module = load_script()
    config = load_test_config(module)
    runs = [
        run_payload(
            70,
            name="Backtester CI",
            path=".github/workflows/backtester-ci.yml",
            created_at="2026-06-12T00:00:00Z",
        ),
        run_payload(
            71,
            name="Backtester CI",
            path=".github/workflows/backtester-ci.yml",
            created_at="2026-06-12T00:01:00Z",
        ),
        run_payload(72, created_at="2026-06-12T00:02:00Z"),
        run_payload(73, created_at="2026-06-12T00:03:00Z"),
    ]
    artifacts = {
        run["id"]: {"artifacts": [artifact_payload("nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-inputs-z")]}
        for run in runs
    }
    report = module.build_report(
        repo="example/repo",
        runs_payload={"workflow_runs": runs},
        jobs_payload_by_run_id={run["id"]: {"jobs": []} for run in runs},
        artifacts_payload_by_run_id=artifacts,
        pr_state_by_run_id={},
        runner_config=config,
        generated_at="2026-06-12T02:00:00Z",
    )
    runs_by_id = {run["id"]: run for run in report["runs"]}
    assert "fingerprint-identical" not in runs_by_id[71]["classifications"], runs_by_id[71]
    assert "fingerprint-identical" not in runs_by_id[72]["classifications"], runs_by_id[72]
    assert "fingerprint-identical" in runs_by_id[73]["classifications"], runs_by_id[73]
    assert runs_by_id[70]["fingerprint"] is None, runs_by_id[70]
    assert runs_by_id[71]["fingerprint"] is None, runs_by_id[71]
    assert runs_by_id[72]["fingerprint"] == "v2-Linux-X64-test-profile-shards-4-inputs-z", runs_by_id[72]


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
    assert "Lever B bounds:" not in module.render_text(report), module.render_text(report)


def main() -> int:
    assert_extract_fingerprint_ignores_empty_suffix()
    assert_meter_api_limits_come_from_config()
    assert_configured_workflow_paths_paginates_workflow_list()
    assert_gh_client_flattens_paginated_list_pages()
    assert_gh_client_rejects_graphql_errors()
    assert_fetch_runs_sorts_before_limit()
    assert_resolve_pr_states_uses_workflow_run_pr_number()
    assert_resolve_pr_states_direct_pr_lookup_errors_are_fatal()
    assert_resolve_pr_states_direct_pr_lookup_rejects_malformed_payloads()
    assert_resolve_pr_states_falls_back_when_run_has_no_pr_refs()
    assert_resolve_pr_states_abstains_without_pr_refs_or_head_owner()
    assert_resolve_pr_states_fallback_uses_head_repository_owner()
    assert_resolve_pr_states_fallback_selects_by_run_time()
    assert_resolve_pr_states_fallback_abstains_when_no_pr_lifetime_matches()
    assert_timeline_truncation_is_visible_in_report()
    assert_resolve_pr_states_handles_null_graphql_payloads()
    assert_cancelled_superseded_requires_pull_request_pr_match_and_overlap()
    assert_build_report_classifies_runs_and_totals_minutes()
    assert_fingerprint_identity_is_scoped_to_fingerprint_workflow()
    assert_unknown_labels_are_reported_without_crashing()
    print("OK: Ubicloud runner-minute meter self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
