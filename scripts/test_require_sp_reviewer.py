#!/usr/bin/env python3
"""Self-tests for the required PR reviewer approval gate."""

from __future__ import annotations

import importlib.util
import contextlib
import io
import json
import os
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "require_sp_reviewer.py"
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "require-reviewer-node.yml"
CODEOWNERS_PATH = REPO_ROOT / ".github" / "CODEOWNERS"


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


def assert_requested_reviewer_requires_approval() -> None:
    result = decision([requested_user("sp-reviewer")], [])
    assert result.passed is False
    assert result.requested is True
    assert result.latest_decisive_state is None
    assert "approval" in result.message


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
    assert result.passed is False
    assert result.requested is True


def assert_numeric_user_id_survives_login_rename() -> None:
    result = decision([requested_user("renamed-reviewer", 294847876)], [], reviewer_id=294847876)
    assert result.passed is False
    assert result.requested is True


def assert_node_id_survives_login_rename() -> None:
    result = decision(
        [requested_user("renamed-reviewer", node_id="U_kgDOEZMFhA")],
        [],
        reviewer_node_id="U_kgDOEZMFhA",
    )
    assert result.passed is False
    assert result.requested is True


def assert_node_id_approval_survives_login_rename() -> None:
    result = decision(
        [],
        [review("renamed-reviewer", "APPROVED", 10, node_id="U_kgDOEZMFhA")],
        reviewer_node_id="U_kgDOEZMFhA",
    )
    assert result.passed is True
    assert result.requested is False
    assert result.latest_decisive_state == "APPROVED"


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


def assert_commit_status_payload_is_latest_wins_context() -> None:
    module = load_script()
    failed = decision([requested_user("sp-reviewer")], [])
    failed_payload = module.commit_status_payload(
        result=failed,
        context="required reviewer approved",
        target_url="https://github.test/run",
    )
    assert failed_payload["state"] == "failure"
    assert failed_payload["context"] == "required reviewer approved"
    assert failed_payload["target_url"] == "https://github.test/run"
    assert "approval" in failed_payload["description"]
    assert len(failed_payload["description"]) <= 140

    approved = decision([], [review("sp-reviewer", "APPROVED", 10)])
    approved_payload = module.commit_status_payload(
        result=approved,
        context="required reviewer approved",
        target_url=None,
    )
    assert approved_payload["state"] == "success"
    assert approved_payload["context"] == "required reviewer approved"
    assert "target_url" not in approved_payload
    assert "approved" in approved_payload["description"]


def assert_status_mode_posts_failure_without_failing_job() -> None:
    module = load_script()
    posted: list[tuple[str, dict[str, object]]] = []

    def fake_get_json(_url: str, _token: str) -> dict[str, object]:
        return {"users": [requested_user("sp-reviewer", node_id="U_kgDOEZMFhA")], "teams": []}

    def fake_paginate_json_list(_url: str, _token: str) -> list[dict[str, object]]:
        return []

    def fake_post_json(url: str, _token: str, payload: dict[str, object]) -> None:
        posted.append((url, payload))

    old_env = os.environ.copy()
    original_get_json = module._get_json
    original_paginate_json_list = module._paginate_json_list
    original_post_json = module._post_json
    with tempfile.NamedTemporaryFile("w", encoding="utf-8") as event_file:
        json.dump({"pull_request": {"number": 839, "head": {"sha": "current-head"}}}, event_file)
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
                    "REQUIRED_REVIEWER_NODE_ID": "U_kgDOEZMFhA",
                    "REVIEWER_GATE_STATUS_CONTEXT": "required reviewer approved",
                }
            )
            module._get_json = fake_get_json
            module._paginate_json_list = fake_paginate_json_list
            module._post_json = fake_post_json
            with contextlib.redirect_stdout(io.StringIO()):
                assert module.run() == 0
        finally:
            module._get_json = original_get_json
            module._paginate_json_list = original_paginate_json_list
            module._post_json = original_post_json
            os.environ.clear()
            os.environ.update(old_env)

    assert len(posted) == 1
    status_url, status_payload = posted[0]
    assert status_url == "https://api.github.test/repos/owner/repo/statuses/current-head"
    assert status_payload["state"] == "failure"
    assert status_payload["context"] == "required reviewer approved"
    assert status_payload["target_url"] == "https://github.test/owner/repo/actions/runs/12345"


def assert_workflow_uses_base_script_and_requires_node_id() -> None:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    assert "name: publish approval status" in workflow
    assert "name: reviewer node_id status publisher" not in workflow
    assert "statuses: write" in workflow
    assert "required reviewer approved" in workflow
    assert "reviewer node_id requested or approved" not in workflow
    assert "REVIEWER_GATE_STATUS_CONTEXT: required reviewer approved" in workflow
    assert "Run the reviewer gate from the protected base branch" in workflow
    assert "github.event.pull_request.base.sha" in workflow
    assert "Policy identity constant" in workflow
    assert "REQUIRED_REVIEWER_NODE_ID: U_kgDOEZMFhA" in workflow
    assert "REQUIRED_REVIEWER:" not in workflow
    assert "Bootstrap reviewer gate script" not in workflow
    assert "test -f scripts/require_sp_reviewer.py" not in workflow
    assert "Remove this block after scripts/require_sp_reviewer.py exists on main" not in workflow


def assert_codeowners_requires_sp_reviewer_for_all_paths() -> None:
    codeowners = CODEOWNERS_PATH.read_text(encoding="utf-8")
    assert "GitHub CODEOWNERS is login-based" in codeowners
    assert "* @sp-reviewer" in codeowners


def main() -> int:
    assert_requested_reviewer_requires_approval()
    assert_approved_reviewer_passes_after_request_is_consumed()
    assert_missing_reviewer_fails()
    assert_later_changes_requested_overrides_approval()
    assert_later_comment_does_not_override_approval()
    assert_logins_are_case_insensitive()
    assert_numeric_user_id_survives_login_rename()
    assert_node_id_survives_login_rename()
    assert_node_id_approval_survives_login_rename()
    assert_configured_node_id_ignores_login_collision()
    assert_stale_approval_does_not_pass_for_new_head_sha()
    assert_current_head_approval_passes()
    assert_commit_status_payload_is_latest_wins_context()
    assert_status_mode_posts_failure_without_failing_job()
    assert_workflow_uses_base_script_and_requires_node_id()
    assert_codeowners_requires_sp_reviewer_for_all_paths()
    print("OK: required reviewer gate self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lock_handle = lane_governor.acquire()
    try:
        raise SystemExit(main())
    finally:
        lane_governor.release(lock_handle)
