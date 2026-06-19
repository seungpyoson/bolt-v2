#!/usr/bin/env python3
"""Self-tests for the required resolved review-thread gate."""

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
SCRIPT_PATH = REPO_ROOT / "scripts" / "require_resolved_review_threads.py"
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "require-resolved-review-threads.yml"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("require_resolved_review_threads", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load {SCRIPT_PATH.name}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def thread(
    thread_id: str,
    resolved: bool,
    outdated: bool = False,
) -> dict[str, object]:
    return {
        "id": thread_id,
        "isResolved": resolved,
        "isOutdated": outdated,
    }


def decision(review_threads: list[dict[str, object]]):
    module = load_script()
    return module.evaluate_review_thread_gate(review_threads=review_threads)


def assert_no_threads_passes() -> None:
    result = decision([])
    assert result.passed is True
    assert result.unresolved_count == 0
    assert "No unresolved review threads" in result.message


def assert_resolved_threads_pass() -> None:
    result = decision([thread("A", True), thread("B", True, outdated=True)])
    assert result.passed is True
    assert result.unresolved_count == 0


def assert_unresolved_thread_fails_with_id_fallback() -> None:
    result = decision([thread("A", True), thread("B", False)])
    assert result.passed is False
    assert result.unresolved_count == 1
    assert "B" in result.message


def assert_unresolved_thread_uses_first_comment_url() -> None:
    result = decision(
        [
            {
                "id": "thread-with-comment",
                "isResolved": False,
                "comments": {"nodes": [{"url": "https://github.test/comment/1"}]},
            }
        ]
    )
    assert result.passed is False
    assert "https://github.test/comment/1" in result.message


def assert_unresolved_outdated_thread_still_fails() -> None:
    result = decision([thread("old", False, outdated=True)])
    assert result.passed is False
    assert result.unresolved_count == 1
    assert "old" in result.message


def assert_non_thread_payload_is_rejected() -> None:
    module = load_script()
    try:
        module.evaluate_review_thread_gate(review_threads=[{"id": "missing-state"}])
    except module.ReviewThreadGateError as exc:
        assert "isResolved" in str(exc)
    else:
        raise AssertionError("missing isResolved should fail")


def assert_graphql_non_object_nodes_fail_closed() -> None:
    module = load_script()
    payload = {
        "data": {
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "nodes": [None],
                        "pageInfo": {"hasNextPage": False, "endCursor": None},
                    }
                }
            }
        }
    }
    try:
        module._extract_review_threads(payload)
    except module.ReviewThreadGateError as exc:
        assert "reviewThreads.nodes entries" in str(exc)
    else:
        raise AssertionError("malformed reviewThreads.nodes entry should fail")


def assert_graphql_next_page_without_cursor_fails_closed() -> None:
    module = load_script()
    payload = {
        "data": {
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "nodes": [],
                        "pageInfo": {"hasNextPage": True, "endCursor": None},
                    }
                }
            }
        }
    }
    try:
        module._extract_review_threads(payload)
    except module.ReviewThreadGateError as exc:
        assert "endCursor" in str(exc)
    else:
        raise AssertionError("hasNextPage without endCursor should fail closed")


def assert_graphql_errors_fail_closed_at_extract_boundary() -> None:
    module = load_script()
    payload = {
        "errors": [{"message": "rate limited"}],
        "data": {
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "nodes": [],
                        "pageInfo": {"hasNextPage": False, "endCursor": None},
                    }
                }
            }
        },
    }
    try:
        module._extract_review_threads(payload)
    except module.ReviewThreadGateError as exc:
        assert "GraphQL returned errors" in str(exc)
    else:
        raise AssertionError("GraphQL errors should fail closed at extract boundary")


def assert_graphql_invalid_json_fails_closed_at_request_boundary() -> None:
    module = load_script()

    class FakeResponse(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, traceback):
            return False

    def fake_urlopen(_request, timeout: int):
        assert timeout == 30
        return FakeResponse(b"not-json")

    original_urlopen = module.urllib.request.urlopen
    try:
        module.urllib.request.urlopen = fake_urlopen
        module._request_graphql(token="token", query="query", variables={})
    except module.ReviewThreadGateError as exc:
        assert "valid JSON" in str(exc)
    else:
        raise AssertionError("invalid GraphQL JSON should fail closed at request boundary")
    finally:
        module.urllib.request.urlopen = original_urlopen


def assert_workflow_uses_base_script_and_review_thread_events() -> None:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "Required reviewer resolution gate" in workflow
    assert "verify review threads are resolved" in workflow
    assert "Require resolved review threads" not in workflow
    assert "pull_request_review_comment" in workflow
    assert "GitHub Actions does not expose review-thread resolved or reopened events" in workflow
    assert "Native conversation resolution is authoritative at merge" in workflow
    assert "github.event.pull_request.base.sha" in workflow
    assert "Bootstrap review-thread gate script" not in workflow
    assert "test -f scripts/require_resolved_review_threads.py" not in workflow
    assert "Remove this block after scripts/require_resolved_review_threads.py exists on main" not in workflow
    assert "python3 scripts/require_resolved_review_threads.py" in workflow
    assert "statuses: write" in workflow
    assert "REVIEW_THREAD_GATE_STATUS_CONTEXT: required review threads resolved" in workflow


def assert_thread_url_uses_real_graphql_shape() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    assert 'thread.get("url")' not in source


def assert_status_mode_publishes_verdict_without_disabling_job() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        return fake_fetch.threads  # type: ignore[attr-defined]

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333, "head": {"sha": "head-sha"}}}, event_file)
        event_file.flush()
        try:
            os.environ.update(
                {
                    "GITHUB_API_URL": "https://api.github.test",
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_RUN_ID": "12345",
                    "GITHUB_SERVER_URL": "https://github.test",
                    "GITHUB_TOKEN": "token",
                    "REVIEW_THREAD_GATE_STATUS_CONTEXT": "required review threads resolved",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            # Unresolved threads -> publish failure, job still exits non-zero so
            # the (currently required) job-name check stays meaningful pre-swap.
            fake_fetch.threads = [{"id": "T1", "isResolved": False}]  # type: ignore[attr-defined]
            posted.clear()
            with contextlib.redirect_stdout(io.StringIO()):
                rc_fail = module.run()
            assert rc_fail == 1
            assert len(posted) == 2
            assert posted[0][1]["state"] == "pending"
            fail_url, fail_payload = posted[1]
            assert fail_url == "https://api.github.test/repos/owner/repo/statuses/head-sha"
            assert fail_payload["state"] == "failure"
            assert fail_payload["context"] == "required review threads resolved"
            assert fail_payload["target_url"] == "https://github.test/owner/repo/actions/runs/12345"

            # Resolved threads -> publish success and exit 0.
            fake_fetch.threads = [{"id": "T1", "isResolved": True}]  # type: ignore[attr-defined]
            posted.clear()
            with contextlib.redirect_stdout(io.StringIO()):
                rc_ok = module.run()
            assert rc_ok == 0
            assert len(posted) == 2
            assert posted[0][1]["state"] == "pending"
            ok_url, ok_payload = posted[1]
            assert ok_url == "https://api.github.test/repos/owner/repo/statuses/head-sha"
            assert ok_payload["state"] == "success"
            assert ok_payload["context"] == "required review threads resolved"
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def assert_status_mode_marks_pending_before_review_thread_fetch() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        assert len(posted) == 1
        assert posted[0][1]["state"] == "pending"
        return [{"id": "T1", "isResolved": True}]

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333, "head": {"sha": "head-sha"}}}, event_file)
        event_file.flush()
        try:
            os.environ.update(
                {
                    "GITHUB_API_URL": "https://api.github.test",
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_TOKEN": "token",
                    "REVIEW_THREAD_GATE_STATUS_CONTEXT": "required review threads resolved",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            with contextlib.redirect_stdout(io.StringIO()):
                rc = module.run()
            assert rc == 0
            assert [payload["state"] for _url, payload in posted] == ["pending", "success"]
            assert posted[0][1]["description"] == "Review-thread gate is inspecting review threads"
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def assert_status_disabled_does_not_require_head_sha() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        return [{"id": "T1", "isResolved": True}]

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333}}, event_file)
        event_file.flush()
        try:
            os.environ.clear()
            os.environ.update(
                {
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_TOKEN": "token",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            with contextlib.redirect_stdout(io.StringIO()):
                rc_ok = module.run()
            assert rc_ok == 0
            assert posted == []
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def assert_status_mode_remains_non_success_when_failure_status_post_errors() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        raise module.ReviewThreadGateError("GraphQL unavailable")

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))
        if len(posted) == 2:
            raise module.ReviewThreadGateError("status write failed")

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333, "head": {"sha": "head-sha"}}}, event_file)
        event_file.flush()
        try:
            os.environ.update(
                {
                    "GITHUB_API_URL": "https://api.github.test",
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_TOKEN": "token",
                    "REVIEW_THREAD_GATE_STATUS_CONTEXT": "required review threads resolved",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                rc = module.main()
            assert rc == 1
            assert [payload["state"] for _url, payload in posted] == ["pending", "failure"]
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def assert_status_mode_marks_pending_before_unexpected_fetch_error() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        raise RuntimeError("unexpected fetch error")

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333, "head": {"sha": "head-sha"}}}, event_file)
        event_file.flush()
        try:
            os.environ.update(
                {
                    "GITHUB_API_URL": "https://api.github.test",
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_TOKEN": "token",
                    "REVIEW_THREAD_GATE_STATUS_CONTEXT": "required review threads resolved",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            try:
                module.run()
            except RuntimeError as exc:
                assert "unexpected fetch error" in str(exc)
            else:
                raise AssertionError("unexpected fetch errors should still fail the job")
            assert [payload["state"] for _url, payload in posted] == ["pending"]
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def assert_status_mode_fails_closed_when_review_thread_fetch_errors() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_fetch(*, owner: str, name: str, pull_number: int, token: str):
        raise module.ReviewThreadGateError("GraphQL unavailable")

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_fetch = module.fetch_review_threads
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 333, "head": {"sha": "head-sha"}}}, event_file)
        event_file.flush()
        try:
            os.environ.update(
                {
                    "GITHUB_API_URL": "https://api.github.test",
                    "GITHUB_EVENT_PATH": event_file.name,
                    "GITHUB_REPOSITORY": "owner/repo",
                    "GITHUB_RUN_ID": "12345",
                    "GITHUB_SERVER_URL": "https://github.test",
                    "GITHUB_TOKEN": "token",
                    "REVIEW_THREAD_GATE_STATUS_CONTEXT": "required review threads resolved",
                }
            )
            module.fetch_review_threads = fake_fetch
            module._post_json = fake_post_json

            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                rc = module.main()
            assert rc == 1
            assert len(posted) == 2
            assert posted[0][1]["state"] == "pending"
            fail_url, fail_payload = posted[1]
            assert fail_url == "https://api.github.test/repos/owner/repo/statuses/head-sha"
            assert fail_payload["state"] == "failure"
            assert fail_payload["context"] == "required review threads resolved"
            assert fail_payload["target_url"] == "https://github.test/owner/repo/actions/runs/12345"
            assert "GraphQL unavailable" in str(fail_payload["description"])
        finally:
            module.fetch_review_threads = original_fetch
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)


def main() -> int:
    assert_no_threads_passes()
    assert_resolved_threads_pass()
    assert_unresolved_thread_fails_with_id_fallback()
    assert_unresolved_thread_uses_first_comment_url()
    assert_unresolved_outdated_thread_still_fails()
    assert_non_thread_payload_is_rejected()
    assert_graphql_non_object_nodes_fail_closed()
    assert_graphql_next_page_without_cursor_fails_closed()
    assert_graphql_errors_fail_closed_at_extract_boundary()
    assert_graphql_invalid_json_fails_closed_at_request_boundary()
    assert_workflow_uses_base_script_and_review_thread_events()
    assert_thread_url_uses_real_graphql_shape()
    assert_status_mode_publishes_verdict_without_disabling_job()
    assert_status_mode_marks_pending_before_review_thread_fetch()
    assert_status_disabled_does_not_require_head_sha()
    assert_status_mode_fails_closed_when_review_thread_fetch_errors()
    assert_status_mode_remains_non_success_when_failure_status_post_errors()
    assert_status_mode_marks_pending_before_unexpected_fetch_error()
    print("OK: required resolved review-thread gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
