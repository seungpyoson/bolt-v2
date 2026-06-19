#!/usr/bin/env python3
"""Fail a PR check unless the required reviewer approved the PR head."""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from typing import Any

import github_commit_status


DEFAULT_REVIEWER = "sp-reviewer"
DECISIVE_REVIEW_STATES = {"APPROVED", "CHANGES_REQUESTED", "DISMISSED"}


class ReviewerGateError(RuntimeError):
    """Raised when the reviewer gate cannot inspect the PR."""


@dataclass(frozen=True)
class GateResult:
    passed: bool
    reviewer: str
    requested: bool
    latest_decisive_state: str | None
    message: str


@dataclass(frozen=True)
class ReviewState:
    state: str | None
    commit_id: str | None


def _login(value: object) -> str:
    return str(value or "").casefold()


def _github_user_id(value: object) -> int | None:
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.isdigit():
        return int(value)
    return None


def _github_node_id(value: object) -> str | None:
    if isinstance(value, str) and value:
        return value
    return None


def _matches_required_user(
    user: object,
    reviewer: str,
    reviewer_id: int | None,
    reviewer_node_id: str | None,
) -> bool:
    if not isinstance(user, dict):
        return False
    if reviewer_node_id is not None:
        return _github_node_id(user.get("node_id")) == reviewer_node_id
    if reviewer_id is not None:
        return _github_user_id(user.get("id")) == reviewer_id
    return _login(user.get("login")) == _login(reviewer)


def _reviewer_display(
    reviewer: str,
    reviewer_id: int | None,
    reviewer_node_id: str | None,
) -> str:
    if reviewer_node_id is not None:
        return f"GitHub user node_id {reviewer_node_id}"
    if reviewer_id is not None:
        return f"GitHub user id {reviewer_id}"
    return f"@{reviewer}"


def _is_reviewer_requested(
    requested_reviewers: dict[str, Any],
    reviewer: str,
    reviewer_id: int | None,
    reviewer_node_id: str | None,
) -> bool:
    users = requested_reviewers.get("users", [])
    if not isinstance(users, list):
        return False
    return any(_matches_required_user(user, reviewer, reviewer_id, reviewer_node_id) for user in users)


def latest_decisive_review(
    reviews: list[dict[str, Any]],
    reviewer: str,
    reviewer_id: int | None = None,
    reviewer_node_id: str | None = None,
) -> ReviewState:
    latest_state: str | None = None
    latest_commit_id: str | None = None
    for review in reviews:
        if not isinstance(review, dict):
            continue
        user = review.get("user")
        if not _matches_required_user(user, reviewer, reviewer_id, reviewer_node_id):
            continue
        state = str(review.get("state") or "").upper()
        if state in DECISIVE_REVIEW_STATES:
            latest_state = state
            commit_id = review.get("commit_id")
            latest_commit_id = commit_id if isinstance(commit_id, str) else None
    return ReviewState(state=latest_state, commit_id=latest_commit_id)


def latest_decisive_review_state(
    reviews: list[dict[str, Any]],
    reviewer: str,
    reviewer_id: int | None = None,
    reviewer_node_id: str | None = None,
) -> str | None:
    return latest_decisive_review(reviews, reviewer, reviewer_id, reviewer_node_id).state


def evaluate_reviewer_gate(
    *,
    requested_reviewers: dict[str, Any],
    reviews: list[dict[str, Any]],
    reviewer: str = DEFAULT_REVIEWER,
    reviewer_id: int | None = None,
    reviewer_node_id: str | None = None,
    head_sha: str | None = None,
) -> GateResult:
    display = _reviewer_display(reviewer, reviewer_id, reviewer_node_id)
    requested = _is_reviewer_requested(requested_reviewers, reviewer, reviewer_id, reviewer_node_id)
    latest_review = latest_decisive_review(reviews, reviewer, reviewer_id, reviewer_node_id)
    latest_state = latest_review.state
    approved = latest_state == "APPROVED" and (
        head_sha is None or latest_review.commit_id == head_sha
    )
    passed = approved

    if approved:
        message = f"{display} has approved this PR."
    elif requested:
        message = f"{display} is currently requested for review, but approval on the current PR head is required."
    elif latest_state == "APPROVED" and head_sha is not None:
        message = (
            f"{display} approved {latest_review.commit_id or 'unknown commit'}, "
            f"but current PR head is {head_sha}; request review again."
        )
    elif latest_state:
        message = (
            f"{display} has not approved the current PR head; "
            f"latest decisive review state is {latest_state}."
        )
    else:
        message = f"PR must have approval from {display}."

    return GateResult(
        passed=passed,
        reviewer=reviewer,
        requested=requested,
        latest_decisive_state=latest_state,
        message=message,
    )


def _env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ReviewerGateError(f"{name} is required")
    return value


def _optional_int_env(name: str) -> int | None:
    value = os.environ.get(name)
    if value is None or value == "":
        return None
    parsed = _github_user_id(value)
    if parsed is None:
        raise ReviewerGateError(f"{name} must be an integer")
    return parsed


def _optional_str_env(name: str) -> str | None:
    value = os.environ.get(name)
    if value is None or value == "":
        return None
    return value


def _read_event_payload() -> dict[str, Any]:
    path = _env("GITHUB_EVENT_PATH")
    with open(path, "r", encoding="utf-8") as handle:
        payload = json.load(handle)
    if not isinstance(payload, dict):
        raise ReviewerGateError("GitHub event payload must be a JSON object")
    return payload


def _pull_request(payload: dict[str, Any]) -> dict[str, Any]:
    pull_request = payload.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ReviewerGateError("GitHub event payload does not contain pull_request")
    return pull_request


def _pull_number(payload: dict[str, Any]) -> int:
    pull_request = _pull_request(payload)
    number = pull_request.get("number")
    if not isinstance(number, int):
        raise ReviewerGateError("GitHub event pull_request.number is missing")
    return number


def _pull_head_sha(payload: dict[str, Any]) -> str:
    pull_request = _pull_request(payload)
    head = pull_request.get("head")
    if not isinstance(head, dict):
        raise ReviewerGateError("GitHub event pull_request.head is missing")
    sha = head.get("sha")
    if not isinstance(sha, str) or not sha:
        raise ReviewerGateError("GitHub event pull_request.head.sha is missing")
    return sha


def _api_base() -> str:
    return os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")


def _request_json(url: str, token: str) -> tuple[Any, str | None]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "bolt-v2-reviewer-node-gate",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response), response.headers.get("Link")
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise ReviewerGateError(f"GitHub API request failed with HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise ReviewerGateError(f"GitHub API request failed: {exc}") from exc


def _next_link(link_header: str | None) -> str | None:
    if not link_header:
        return None
    for part in link_header.split(","):
        url_part, _, rel_part = part.strip().partition(";")
        if 'rel="next"' not in rel_part:
            continue
        if url_part.startswith("<") and url_part.endswith(">"):
            return url_part[1:-1]
    return None


def _get_json(url: str, token: str) -> Any:
    payload, _link = _request_json(url, token)
    return payload


def _post_json(url: str, token: str, payload: dict[str, Any]) -> None:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "bolt-v2-reviewer-node-gate",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30):
            return
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise ReviewerGateError(f"GitHub status update failed with HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise ReviewerGateError(f"GitHub status update failed: {exc}") from exc


def _paginate_json_list(url: str, token: str) -> list[dict[str, Any]]:
    items: list[dict[str, Any]] = []
    next_url: str | None = url
    while next_url:
        payload, link = _request_json(next_url, token)
        if not isinstance(payload, list):
            raise ReviewerGateError("GitHub API pagination payload must be a JSON array")
        items.extend(item for item in payload if isinstance(item, dict))
        next_url = _next_link(link)
    return items


def _pulls_api_url(repository: str, pull_number: int, suffix: str) -> str:
    owner_repo = "/".join(urllib.parse.quote(part, safe="") for part in repository.split("/", 1))
    return f"{_api_base()}/repos/{owner_repo}/pulls/{pull_number}/{suffix}"


def commit_status_payload(
    *,
    result: GateResult,
    context: str,
    target_url: str | None,
) -> dict[str, str]:
    return github_commit_status.commit_status_payload(
        passed=result.passed,
        context=context,
        description=result.message,
        target_url=target_url,
    )


def post_commit_status(
    *,
    repository: str,
    sha: str,
    token: str,
    result: GateResult,
    context: str,
) -> None:
    github_commit_status.publish_commit_status(
        repository=repository,
        sha=sha,
        token=token,
        passed=result.passed,
        description=result.message,
        context=context,
        post_json=_post_json,
        api_base=_api_base(),
        target_url=github_commit_status.run_target_url(),
    )


def run() -> int:
    reviewer_node_id = _optional_str_env("REQUIRED_REVIEWER_NODE_ID")
    reviewer_id = _optional_int_env("REQUIRED_REVIEWER_ID")
    status_context = _optional_str_env("REVIEWER_GATE_STATUS_CONTEXT")
    reviewer = os.environ.get("REQUIRED_REVIEWER", "")
    if reviewer_node_id is None and reviewer_id is None and not reviewer:
        reviewer = DEFAULT_REVIEWER
    repository = _env("GITHUB_REPOSITORY")
    token = _env("GITHUB_TOKEN")
    payload = _read_event_payload()
    pull_number = _pull_number(payload)
    head_sha = _pull_head_sha(payload)

    requested = _get_json(_pulls_api_url(repository, pull_number, "requested_reviewers"), token)
    if not isinstance(requested, dict):
        raise ReviewerGateError("requested reviewers payload must be a JSON object")
    reviews = _paginate_json_list(_pulls_api_url(repository, pull_number, "reviews?per_page=100"), token)
    result = evaluate_reviewer_gate(
        requested_reviewers=requested,
        reviews=reviews,
        reviewer=reviewer,
        reviewer_id=reviewer_id,
        reviewer_node_id=reviewer_node_id,
        head_sha=head_sha,
    )
    print(result.message)
    if status_context is not None:
        post_commit_status(
            repository=repository,
            sha=head_sha,
            token=token,
            result=result,
            context=status_context,
        )
        return 0
    return 0 if result.passed else 1


def main() -> int:
    try:
        return run()
    except ReviewerGateError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
