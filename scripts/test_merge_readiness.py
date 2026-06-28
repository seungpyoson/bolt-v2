#!/usr/bin/env python3
"""Tests for merge_readiness.py."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "merge_readiness.py"
SHA = "a1a6be0d94e887538ebcd9afced6c94046a557d6"
OTHER_SHA = "b" * 40
RUN_ID = 24623219988
NEWER_RUN_ID = RUN_ID + 1
PR_NUMBER = 942
REPO = "seungpyoson/bolt-v2"
ACTIONS_BOT_USER = {"login": "github-actions[bot]", "type": "Bot"}
HUMAN_USER = {"login": "reviewer", "type": "User"}

CONFIG_TOML = """
[ci_provenance]
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000

[ci_provenance.merge_readiness]
comment_marker = "bolt-v2-merge-readiness"
poll_seconds = 1
max_watch_seconds = 1

[ci_provenance.gate_names]
gate_required = "gate"
gate_iteration = "gate-iteration"
gate_dispatch_full = "gate-dispatch"
backtester_required = "backtester-gate"
backtester_iteration = "backtester-gate-iteration"
backtester_dispatch_full = "backtester-gate-dispatch"

[ci_provenance.required_checks.gate]
context = "gate"
required = true

[ci_provenance.required_checks.backtester-gate]
context = "backtester-gate"
required = true

[ci_provenance.required_checks.host-health]
context = "host-health"
required = true

[ci_provenance.required_checks.actionlint]
context = "actionlint"
required = true

[ci_provenance.required_checks.coverage-enforcer]
context = "coverage-enforcer"
required = false
"""


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("merge_readiness", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load merge_readiness.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_config(tmpdir: pathlib.Path, text: str = CONFIG_TOML) -> pathlib.Path:
    path = tmpdir / "github-actions-runners.toml"
    path.write_text(text, encoding="utf-8")
    return path


def check_run(name: str, status: str = "completed", conclusion: str | None = "success", **overrides: object) -> dict[str, object]:
    payload: dict[str, object] = {
        "id": abs(hash((name, status, conclusion))) % 1_000_000 + 1,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "completed_at": "2026-06-27T00:00:00Z",
    }
    payload.update(overrides)
    return payload


def pull_payload(*, head_sha: str = SHA, fork: bool = False) -> dict[str, object]:
    head_repo = "external/fork" if fork else REPO
    return {
        "number": PR_NUMBER,
        "head": {"sha": head_sha, "repo": {"full_name": head_repo}},
        "base": {"repo": {"full_name": REPO}},
    }


def workflow_run_payload(
    *,
    run_id: int = RUN_ID,
    run_attempt: int = 1,
    head_sha: str = SHA,
    path: str = ".github/workflows/ci.yml",
) -> dict[str, object]:
    return {
        "id": run_id,
        "run_attempt": run_attempt,
        "head_sha": head_sha,
        "event": "pull_request",
        "path": path,
        "pull_requests": [{"number": PR_NUMBER}],
    }


def marker_body(module, *, state: str = "running", run_id: int = RUN_ID, run_attempt: int = 1, head_sha: str = SHA) -> str:
    marker = module.comment_marker(
        marker_name="bolt-v2-merge-readiness",
        head_sha=head_sha,
        workflow=".github/workflows/ci.yml",
        run_id=run_id,
        run_attempt=run_attempt,
        state=state,
    )
    return f"{marker}\nold body\n"


def issue_comment(comment_id: int, body: str, *, user: dict[str, object] | None = None) -> dict[str, object]:
    return {"id": comment_id, "body": body, "user": user or ACTIONS_BOT_USER}


class FakeGitHub:
    def __init__(
        self,
        *,
        checks: list[dict[str, object]] | None = None,
        pr: dict[str, object] | None = None,
        comments: list[dict[str, object]] | None = None,
        runs: list[dict[str, object]] | None = None,
        forbid_writes: bool = False,
    ) -> None:
        self.checks = checks or []
        self.pr = pr or pull_payload()
        self.comments = comments or []
        self.runs = runs or []
        self.forbid_writes = forbid_writes
        self.requests: list[tuple[str, str, dict[str, str] | None, object]] = []

    def json(
        self,
        repo: str,
        token: str,
        path: str,
        query: dict[str, str] | None = None,
        *,
        method: str = "GET",
        data: object = None,
    ) -> dict[str, object]:
        self.requests.append((method, path, query, data))
        if method in {"POST", "PATCH"} and self.forbid_writes:
            raise PermissionError("pull-requests: write unavailable")
        if path == f"pulls/{PR_NUMBER}":
            return self.pr
        if path == f"commits/{SHA}/check-runs":
            return {"check_runs": self.checks}
        if path == f"issues/{PR_NUMBER}/comments":
            if method == "GET":
                return {"comments": self.comments}
            comment = issue_comment(1000 + len(self.comments), data["body"])
            self.comments.append(comment)
            return comment
        if path.startswith("issues/comments/") and method == "PATCH":
            comment_id = int(path.rsplit("/", 1)[1])
            for comment in self.comments:
                if comment["id"] == comment_id:
                    comment["body"] = data["body"]
                    return comment
            raise AssertionError(f"unknown comment id {comment_id}")
        if path == "actions/workflows/ci.yml/runs":
            return {"workflow_runs": self.runs}
        raise AssertionError(f"unexpected request: {(repo, token, path, query, method, data)!r}")


@contextlib.contextmanager
def patched_env(values: dict[str, str]):
    old_values = {key: os.environ.get(key) for key in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, value in old_values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def run_cli(args: list[str], env: dict[str, str] | None = None) -> tuple[int, str, str]:
    module = load_script()
    stdout = io.StringIO()
    stderr = io.StringIO()
    with patched_env(env or {}), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        code = module.main(args)
    return code, stdout.getvalue(), stderr.getvalue()


def assert_positive_int_rejects_bool_values() -> None:
    module = load_script()
    for value in (True, False):
        try:
            module.positive_int(value, "x")
        except module.MergeReadinessError:
            pass
        else:
            raise AssertionError(f"positive_int accepted bool value: {value!r}")

    if module.positive_int(5, "x") != 5:
        raise AssertionError("positive_int rejected int value")
    if module.positive_int("5", "x") != 5:
        raise AssertionError("positive_int rejected decimal string value")


def assert_status_mapping() -> None:
    module = load_script()
    contexts = ("gate", "backtester-gate", "host-health", "actionlint")
    all_success = module.evaluate_required_checks(
        contexts,
        [check_run(name) for name in contexts],
    )
    if all_success.state != "passed" or "passed" not in module.status_summary(all_success):
        raise AssertionError(all_success)

    failed = module.evaluate_required_checks(
        contexts,
        [check_run("gate"), check_run("backtester-gate", conclusion="failure"), check_run("host-health"), check_run("actionlint")],
    )
    if failed.state != "failed" or failed.failed != ("backtester-gate",):
        raise AssertionError(failed)
    if module.status_summary(failed) != "failed: backtester-gate":
        raise AssertionError(module.status_summary(failed))

    running = module.evaluate_required_checks(
        contexts,
        [check_run("gate"), check_run("backtester-gate", status="in_progress", conclusion=None)],
    )
    if running.state != "running" or running.completed != 1 or running.total != 4:
        raise AssertionError(running)
    if module.status_summary(running) != "running (1/4 done)":
        raise AssertionError(module.status_summary(running))


def assert_non_blocking_required_check_conclusions_do_not_fail() -> None:
    module = load_script()
    non_blocking = module.evaluate_required_checks(
        ("skipped-context", "neutral-context"),
        [
            check_run("skipped-context", conclusion="skipped"),
            check_run("neutral-context", conclusion="neutral"),
        ],
    )
    if non_blocking.state != "passed" or non_blocking.failed:
        raise AssertionError(non_blocking)

    failed = module.evaluate_required_checks(
        ("failing-context",),
        [check_run("failing-context", conclusion="failure")],
    )
    if failed.state != "failed" or failed.failed != ("failing-context",):
        raise AssertionError(failed)


def assert_registry_context_set_is_source_of_truth() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        if module.required_contexts(config) != ("gate", "backtester-gate", "host-health", "actionlint"):
            raise AssertionError(module.required_contexts(config))
        changed = CONFIG_TOML.replace(
            "[ci_provenance.required_checks.actionlint]\ncontext = \"actionlint\"\nrequired = true",
            "[ci_provenance.required_checks.new-required]\ncontext = \"new-required\"\nrequired = true",
        )
        changed_config = write_config(pathlib.Path(tmp), changed)
        if module.required_contexts(changed_config) != ("gate", "backtester-gate", "host-health", "new-required"):
            raise AssertionError(module.required_contexts(changed_config))


def assert_iteration_gate_contexts_follow_observed_gate_names() -> None:
    module = load_script()
    iteration_checks = [
        check_run("gate-iteration"),
        check_run("backtester-gate-iteration"),
        check_run("host-health"),
        check_run("actionlint"),
    ]
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        contexts = module.required_contexts(config, check_runs=iteration_checks)
    expected = ("gate-iteration", "backtester-gate-iteration", "host-health", "actionlint")
    if contexts != expected:
        raise AssertionError(contexts)
    status = module.evaluate_required_checks(contexts, iteration_checks)
    if status.state != "passed":
        raise AssertionError(status)


def assert_newer_partial_gate_pair_fails_closed_over_stale_complete_pair() -> None:
    module = load_script()
    mixed_checks = [
        check_run(
            "gate",
            id=101,
            started_at="2026-06-27T00:00:00Z",
            completed_at="2026-06-27T00:00:10Z",
        ),
        check_run(
            "backtester-gate",
            id=102,
            started_at="2026-06-27T00:00:00Z",
            completed_at="2026-06-27T00:00:10Z",
        ),
        check_run("host-health"),
        check_run("actionlint"),
        check_run(
            "gate-iteration",
            id=201,
            started_at="2026-06-27T00:01:00Z",
            completed_at="2026-06-27T00:01:10Z",
        ),
    ]
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        contexts = module.required_contexts(config, check_runs=mixed_checks)
    expected = ("gate-iteration", "backtester-gate-iteration", "host-health", "actionlint")
    if contexts != expected:
        raise AssertionError(contexts)
    status = module.evaluate_required_checks(contexts, mixed_checks)
    if status.state != "running" or status.pending != ("backtester-gate-iteration",):
        raise AssertionError(status)


def assert_comment_upsert_replaces_existing_marker() -> None:
    module = load_script()
    existing = issue_comment(777, marker_body(module))
    fake = FakeGitHub(
        checks=[check_run("gate"), check_run("backtester-gate"), check_run("host-health"), check_run("actionlint")],
        comments=[existing],
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.update_progress_comment(
            repo=REPO,
            token="token",
            pr_number=PR_NUMBER,
            config_path=config,
            head_sha=SHA,
            workflow=".github/workflows/ci.yml",
            run_id=RUN_ID,
            run_attempt=1,
            api_json=fake.json,
        )
    if result.posted is not True:
        raise AssertionError(result)
    patch_requests = [request for request in fake.requests if request[0] == "PATCH"]
    post_requests = [request for request in fake.requests if request[0] == "POST"]
    if len(patch_requests) != 1 or post_requests:
        raise AssertionError(fake.requests)
    if fake.comments[0]["body"].count("bolt-v2-merge-readiness") != 1:
        raise AssertionError(fake.comments)
    if "all required checks passed" not in fake.comments[0]["body"]:
        raise AssertionError(fake.comments[0]["body"])


def assert_comment_upsert_finds_sticky_comment_on_second_page() -> None:
    module = load_script()

    class PaginatedGitHub(FakeGitHub):
        def __init__(self) -> None:
            super().__init__(
                checks=[check_run("gate"), check_run("backtester-gate"), check_run("host-health"), check_run("actionlint")],
                comments=[
                    issue_comment(111, "ordinary human comment", user=HUMAN_USER),
                    issue_comment(222, marker_body(module)),
                ],
            )

        def json(
            self,
            repo: str,
            token: str,
            path: str,
            query: dict[str, str] | None = None,
            *,
            method: str = "GET",
            data: object = None,
        ) -> dict[str, object]:
            if path == f"issues/{PR_NUMBER}/comments" and method == "GET":
                self.requests.append((method, path, query, data))
                page = int((query or {}).get("page", "1"))
                if page == 1:
                    return {"comments": [self.comments[0]], "next": {"per_page": "1", "page": "2"}}
                if page == 2:
                    return {"comments": [self.comments[1]]}
                return {"comments": []}
            return super().json(repo, token, path, query, method=method, data=data)

    fake = PaginatedGitHub()
    config_text = CONFIG_TOML.replace("run_jobs_per_page = 100", "run_jobs_per_page = 1")
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp), config_text)
        result = module.update_progress_comment(
            repo=REPO,
            token="token",
            pr_number=PR_NUMBER,
            config_path=config,
            head_sha=SHA,
            workflow=".github/workflows/ci.yml",
            run_id=RUN_ID,
            run_attempt=1,
            api_json=fake.json,
        )
    if result.posted is not True:
        raise AssertionError(result)
    page_queries = [
        request[2]
        for request in fake.requests
        if request[0] == "GET" and request[1] == f"issues/{PR_NUMBER}/comments"
    ]
    if page_queries != [{"per_page": "1"}, {"per_page": "1", "page": "2"}]:
        raise AssertionError(fake.requests)
    patch_requests = [request for request in fake.requests if request[0] == "PATCH"]
    post_requests = [request for request in fake.requests if request[0] == "POST"]
    if len(patch_requests) != 1 or post_requests:
        raise AssertionError(fake.requests)
    if len(fake.comments) != 2 or "all required checks passed" not in fake.comments[1]["body"]:
        raise AssertionError(fake.comments)


def assert_find_sticky_comment_ignores_forged_human_marker() -> None:
    module = load_script()
    forged = issue_comment(111, marker_body(module), user=HUMAN_USER)
    genuine = issue_comment(222, marker_body(module, run_id=NEWER_RUN_ID))
    found = module.find_sticky_comment([forged, genuine], "bolt-v2-merge-readiness")
    if found is None or found[0]["id"] != 222:
        raise AssertionError(found)


def assert_no_pull_requests_write_falls_back_without_failure() -> None:
    module = load_script()
    fake = FakeGitHub(
        checks=[check_run("gate"), check_run("backtester-gate"), check_run("host-health"), check_run("actionlint")],
        forbid_writes=True,
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.update_progress_comment(
            repo=REPO,
            token="token",
            pr_number=PR_NUMBER,
            config_path=config,
            head_sha=SHA,
            workflow=".github/workflows/ci.yml",
            run_id=RUN_ID,
            run_attempt=1,
            api_json=fake.json,
        )
    if result.posted is not False or "fallback" not in result.reason:
        raise AssertionError(result)
    if result.status.state != "passed":
        raise AssertionError(result.status)


def assert_fork_pr_skips_comment_posting() -> None:
    module = load_script()
    fake = FakeGitHub(
        pr=pull_payload(fork=True),
        checks=[check_run("gate"), check_run("backtester-gate"), check_run("host-health"), check_run("actionlint")],
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.update_progress_comment(
            repo=REPO,
            token="token",
            pr_number=PR_NUMBER,
            config_path=config,
            head_sha=SHA,
            workflow=".github/workflows/ci.yml",
            run_id=RUN_ID,
            run_attempt=1,
            api_json=fake.json,
        )
    if result.posted is not False or "fork" not in result.reason:
        raise AssertionError(result)
    if any(request[0] in {"POST", "PATCH"} for request in fake.requests):
        raise AssertionError(fake.requests)


def assert_watch_returns_immediately_for_comment_fallback() -> None:
    module = load_script()
    fake = FakeGitHub(
        pr=pull_payload(fork=True),
        checks=[
            check_run("gate"),
            check_run("backtester-gate"),
            check_run("host-health"),
            check_run("actionlint", status="in_progress", conclusion=None),
        ],
    )
    original_sleep = module.time.sleep

    def fail_sleep(seconds: float) -> None:
        raise AssertionError(f"watch should not sleep after fallback, got {seconds}")

    module.time.sleep = fail_sleep
    try:
        with tempfile.TemporaryDirectory() as tmp:
            config = write_config(pathlib.Path(tmp))
            result = module.watch_progress_comment(
                repo=REPO,
                token="token",
                pr_number=PR_NUMBER,
                config_path=config,
                head_sha=SHA,
                workflow=".github/workflows/ci.yml",
                run_id=RUN_ID,
                run_attempt=1,
                api_json=fake.json,
            )
    finally:
        module.time.sleep = original_sleep
    if result.posted is not False or result.status.state != "running":
        raise AssertionError(result)


def assert_run_dominance_stale_head_noops() -> None:
    module = load_script()
    fake = FakeGitHub(
        pr=pull_payload(head_sha=OTHER_SHA),
        comments=[issue_comment(777, marker_body(module))],
        runs=[workflow_run_payload()],
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.finalize_stalled_comment(
            repo=REPO,
            token="token",
            workflow_run=workflow_run_payload(),
            config_path=config,
            api_json=fake.json,
        )
    if result.posted is not False or "stale head" not in result.reason:
        raise AssertionError(result)
    if any(request[0] == "PATCH" for request in fake.requests):
        raise AssertionError(fake.requests)


def assert_run_dominance_older_run_noops() -> None:
    module = load_script()
    fake = FakeGitHub(
        comments=[issue_comment(777, marker_body(module))],
        runs=[workflow_run_payload(run_id=NEWER_RUN_ID), workflow_run_payload(run_id=RUN_ID)],
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.finalize_stalled_comment(
            repo=REPO,
            token="token",
            workflow_run=workflow_run_payload(run_id=RUN_ID),
            config_path=config,
            api_json=fake.json,
        )
    if result.posted is not False or "newer run" not in result.reason:
        raise AssertionError(result)
    if any(request[0] == "PATCH" for request in fake.requests):
        raise AssertionError(fake.requests)


def assert_latest_run_finalizer_updates_stalled() -> None:
    module = load_script()
    fake = FakeGitHub(
        comments=[issue_comment(777, marker_body(module))],
        runs=[workflow_run_payload(run_id=RUN_ID)],
    )
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        result = module.finalize_stalled_comment(
            repo=REPO,
            token="token",
            workflow_run=workflow_run_payload(run_id=RUN_ID),
            config_path=config,
            api_json=fake.json,
        )
    if result.posted is not True:
        raise AssertionError(result)
    if not any(request[0] == "PATCH" for request in fake.requests):
        raise AssertionError(fake.requests)
    if "CI stalled" not in fake.comments[0]["body"]:
        raise AssertionError(fake.comments[0]["body"])


def assert_cli_pr_status_uses_fallback_engine() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        config = write_config(pathlib.Path(tmp))
        event = pathlib.Path(tmp) / "event.json"
        event.write_text(json.dumps({}), encoding="utf-8")
        code, stdout, stderr = run_cli(
            [str(PR_NUMBER), "--config", str(config), "--repo", REPO],
            {"GITHUB_EVENT_PATH": str(event)},
        )
    if code == 0:
        raise AssertionError("CLI should fail without GITHUB_TOKEN in this unit harness")
    if "ERROR:" not in stderr and "ERROR:" not in stdout:
        raise AssertionError((stdout, stderr))


def main() -> int:
    assert_positive_int_rejects_bool_values()
    assert_status_mapping()
    assert_non_blocking_required_check_conclusions_do_not_fail()
    assert_registry_context_set_is_source_of_truth()
    assert_iteration_gate_contexts_follow_observed_gate_names()
    assert_newer_partial_gate_pair_fails_closed_over_stale_complete_pair()
    assert_comment_upsert_replaces_existing_marker()
    assert_comment_upsert_finds_sticky_comment_on_second_page()
    assert_find_sticky_comment_ignores_forged_human_marker()
    assert_no_pull_requests_write_falls_back_without_failure()
    assert_fork_pr_skips_comment_posting()
    assert_watch_returns_immediately_for_comment_fallback()
    assert_run_dominance_stale_head_noops()
    assert_run_dominance_older_run_noops()
    assert_latest_run_finalizer_updates_stalled()
    assert_cli_pr_status_uses_fallback_engine()
    print("OK: merge_readiness tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
