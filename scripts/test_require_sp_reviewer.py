#!/usr/bin/env python3
"""Self-tests for the required PR reviewer gate."""

from __future__ import annotations

import importlib.util
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "require_sp_reviewer.py"
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "require-reviewer-node.yml"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("require_sp_reviewer", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load {SCRIPT_PATH.name}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def requested_user(
    login: str,
    user_id: int | None = None,
    node_id: str | None = None,
) -> dict[str, object]:
    payload: dict[str, object] = {"login": login}
    if user_id is not None:
        payload["id"] = user_id
    if node_id is not None:
        payload["node_id"] = node_id
    return payload


def review(
    login: str,
    state: str,
    review_id: int,
    user_id: int | None = None,
    node_id: str | None = None,
    commit_id: str | None = None,
) -> dict[str, object]:
    user: dict[str, object] = {"login": login}
    if user_id is not None:
        user["id"] = user_id
    if node_id is not None:
        user["node_id"] = node_id
    payload: dict[str, object] = {"id": review_id, "state": state, "user": user}
    if commit_id is not None:
        payload["commit_id"] = commit_id
    return payload


def decision(
    requested: list[dict[str, object]],
    reviews: list[dict[str, object]],
    reviewer: str = "sp-reviewer",
    reviewer_id: int | None = None,
    reviewer_node_id: str | None = None,
    head_sha: str | None = None,
):
    module = load_script()
    return module.evaluate_reviewer_gate(
        requested_reviewers={"users": requested, "teams": []},
        reviews=reviews,
        reviewer=reviewer,
        reviewer_id=reviewer_id,
        reviewer_node_id=reviewer_node_id,
        head_sha=head_sha,
    )


def assert_requested_reviewer_passes() -> None:
    result = decision([requested_user("sp-reviewer")], [])
    assert result.passed is True
    assert result.requested is True
    assert result.latest_decisive_state is None


def assert_approved_reviewer_passes_after_request_is_consumed() -> None:
    result = decision([], [review("sp-reviewer", "APPROVED", 10)])
    assert result.passed is True
    assert result.requested is False
    assert result.latest_decisive_state == "APPROVED"


def assert_missing_reviewer_fails() -> None:
    result = decision([], [review("other-reviewer", "APPROVED", 10)])
    assert result.passed is False
    assert "@sp-reviewer" in result.message


def assert_later_changes_requested_overrides_approval() -> None:
    result = decision(
        [],
        [
            review("sp-reviewer", "APPROVED", 10),
            review("sp-reviewer", "CHANGES_REQUESTED", 11),
        ],
    )
    assert result.passed is False
    assert result.latest_decisive_state == "CHANGES_REQUESTED"


def assert_later_comment_does_not_override_approval() -> None:
    result = decision(
        [],
        [
            review("sp-reviewer", "APPROVED", 10),
            review("sp-reviewer", "COMMENTED", 11),
        ],
    )
    assert result.passed is True
    assert result.latest_decisive_state == "APPROVED"


def assert_logins_are_case_insensitive() -> None:
    result = decision([requested_user("SP-Reviewer")], [], reviewer="sp-reviewer")
    assert result.passed is True
    assert result.requested is True


def assert_numeric_user_id_survives_login_rename() -> None:
    result = decision([requested_user("renamed-reviewer", 294847876)], [], reviewer_id=294847876)
    assert result.passed is True
    assert result.requested is True


def assert_node_id_survives_login_rename() -> None:
    result = decision(
        [requested_user("renamed-reviewer", node_id="U_kgDOEZMFhA")],
        [],
        reviewer_node_id="U_kgDOEZMFhA",
    )
    assert result.passed is True
    assert result.requested is True


def assert_configured_node_id_ignores_login_collision() -> None:
    result = decision(
        [requested_user("sp-reviewer", node_id="different-node")],
        [],
        reviewer_node_id="U_kgDOEZMFhA",
    )
    assert result.passed is False
    assert result.requested is False


def assert_stale_approval_does_not_pass_for_new_head_sha() -> None:
    result = decision(
        [],
        [review("sp-reviewer", "APPROVED", 10, commit_id="old-head")],
        head_sha="new-head",
    )
    assert result.passed is False
    assert result.latest_decisive_state == "APPROVED"
    assert "new-head" in result.message


def assert_current_head_approval_passes() -> None:
    result = decision(
        [],
        [review("sp-reviewer", "APPROVED", 10, commit_id="current-head")],
        head_sha="current-head",
    )
    assert result.passed is True
    assert result.latest_decisive_state == "APPROVED"


def assert_workflow_bootstraps_script_only_when_base_lacks_it() -> None:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "Run the reviewer gate from the protected base branch" in workflow
    assert "Bootstrap reviewer gate script" in workflow
    assert "test -f scripts/require_sp_reviewer.py" in workflow
    assert "github.event.pull_request.head.sha" in workflow


def main() -> int:
    assert_requested_reviewer_passes()
    assert_approved_reviewer_passes_after_request_is_consumed()
    assert_missing_reviewer_fails()
    assert_later_changes_requested_overrides_approval()
    assert_later_comment_does_not_override_approval()
    assert_logins_are_case_insensitive()
    assert_numeric_user_id_survives_login_rename()
    assert_node_id_survives_login_rename()
    assert_configured_node_id_ignores_login_collision()
    assert_stale_approval_does_not_pass_for_new_head_sha()
    assert_current_head_approval_passes()
    assert_workflow_bootstraps_script_only_when_base_lacks_it()
    print("OK: required reviewer gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
