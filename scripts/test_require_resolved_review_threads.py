#!/usr/bin/env python3
"""Self-tests for the required resolved review-thread gate."""

from __future__ import annotations

import importlib.util
import pathlib
import sys


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


def assert_workflow_uses_base_script_and_review_thread_events() -> None:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "Require resolved review threads" in workflow
    assert "review threads resolved" in workflow
    assert "pull_request_review_comment" in workflow
    assert "GitHub Actions does not expose review-thread resolved or reopened events" in workflow
    assert "Native conversation resolution is authoritative at merge" in workflow
    assert "github.event.pull_request.base.sha" in workflow
    assert "test -f scripts/require_resolved_review_threads.py" in workflow
    assert "Remove this block after scripts/require_resolved_review_threads.py exists on main" in workflow
    assert "python3 scripts/require_resolved_review_threads.py" in workflow


def assert_thread_url_uses_real_graphql_shape() -> None:
    source = SCRIPT_PATH.read_text(encoding="utf-8")
    assert 'thread.get("url")' not in source


def main() -> int:
    assert_no_threads_passes()
    assert_resolved_threads_pass()
    assert_unresolved_thread_fails_with_id_fallback()
    assert_unresolved_thread_uses_first_comment_url()
    assert_unresolved_outdated_thread_still_fails()
    assert_non_thread_payload_is_rejected()
    assert_graphql_non_object_nodes_fail_closed()
    assert_graphql_next_page_without_cursor_fails_closed()
    assert_workflow_uses_base_script_and_review_thread_events()
    assert_thread_url_uses_real_graphql_shape()
    print("OK: required resolved review-thread gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
