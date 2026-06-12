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
        11: {"artifacts": [artifact_payload("nextest-archive-fingerprint-inputs-a")]},
        12: {"artifacts": [artifact_payload("nextest-archive-fingerprint-inputs-a")]},
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
    assert runs_by_id[11]["fingerprint"] == "inputs-a", runs_by_id[11]
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
    assert_build_report_classifies_runs_and_totals_minutes()
    assert_unknown_labels_are_reported_without_crashing()
    print("OK: Ubicloud runner-minute meter self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
