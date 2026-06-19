#!/usr/bin/env python3
"""Fail a PR check unless all GitHub review threads are resolved."""

from __future__ import annotations

import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any

import github_commit_status


REVIEW_THREADS_QUERY = """
query($owner: String!, $name: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 1) {
            nodes {
              url
            }
          }
        }
      }
    }
  }
}
"""


class ReviewThreadGateError(RuntimeError):
    """Raised when the review-thread gate cannot inspect the PR."""


@dataclass(frozen=True)
class ReviewThreadGateResult:
    passed: bool
    unresolved_count: int
    message: str


def _env(name: str) -> str:
    value = os.environ.get(name)
    if not value:
        raise ReviewThreadGateError(f"{name} is required")
    return value


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
        raise ReviewThreadGateError("GitHub event payload must be a JSON object")
    return payload


def _pull_request(payload: dict[str, Any]) -> dict[str, Any]:
    pull_request = payload.get("pull_request")
    if not isinstance(pull_request, dict):
        raise ReviewThreadGateError("GitHub event payload does not contain pull_request")
    return pull_request


def _pull_number(payload: dict[str, Any]) -> int:
    number = _pull_request(payload).get("number")
    if not isinstance(number, int):
        raise ReviewThreadGateError("GitHub event pull_request.number is missing")
    return number


def _pull_head_sha(payload: dict[str, Any]) -> str:
    head = _pull_request(payload).get("head")
    if not isinstance(head, dict):
        raise ReviewThreadGateError("GitHub event pull_request.head is missing")
    sha = head.get("sha")
    if not isinstance(sha, str) or not sha:
        raise ReviewThreadGateError("GitHub event pull_request.head.sha is missing")
    return sha


def _repo_parts(repository: str) -> tuple[str, str]:
    owner, separator, name = repository.partition("/")
    if not owner or separator != "/" or not name:
        raise ReviewThreadGateError("GITHUB_REPOSITORY must be formatted as owner/repo")
    return owner, name


def _graphql_url() -> str:
    return os.environ.get("GITHUB_GRAPHQL_URL", "https://api.github.com/graphql").rstrip("/")


def _api_base() -> str:
    return os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")


def _post_json(url: str, token: str, payload: dict[str, Any]) -> None:
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "bolt-v2-review-thread-gate",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30):
            return
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise ReviewThreadGateError(f"GitHub status update failed with HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise ReviewThreadGateError(f"GitHub status update failed: {exc}") from exc


def _request_graphql(
    *,
    token: str,
    query: str,
    variables: dict[str, object],
) -> dict[str, Any]:
    body = json.dumps({"query": query, "variables": variables}).encode("utf-8")
    request = urllib.request.Request(
        _graphql_url(),
        data=body,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": "bolt-v2-review-thread-gate",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", errors="replace")
        raise ReviewThreadGateError(f"GitHub GraphQL request failed with HTTP {exc.code}: {detail}") from exc
    except urllib.error.URLError as exc:
        raise ReviewThreadGateError(f"GitHub GraphQL request failed: {exc}") from exc
    if not isinstance(payload, dict):
        raise ReviewThreadGateError("GitHub GraphQL response must be a JSON object")
    errors = payload.get("errors")
    if errors:
        raise ReviewThreadGateError(f"GitHub GraphQL returned errors: {errors}")
    return payload


def _thread_url(thread: dict[str, Any]) -> str:
    comments = thread.get("comments")
    if isinstance(comments, dict):
        nodes = comments.get("nodes")
        if isinstance(nodes, list) and nodes:
            first = nodes[0]
            if isinstance(first, dict):
                comment_url = first.get("url")
                if isinstance(comment_url, str) and comment_url:
                    return comment_url
    thread_id = thread.get("id")
    if isinstance(thread_id, str) and thread_id:
        return thread_id
    return "unknown review thread"


def evaluate_review_thread_gate(
    *,
    review_threads: list[dict[str, Any]],
) -> ReviewThreadGateResult:
    unresolved: list[dict[str, Any]] = []
    for thread in review_threads:
        if not isinstance(thread, dict):
            raise ReviewThreadGateError("review thread payload entries must be JSON objects")
        resolved = thread.get("isResolved")
        if not isinstance(resolved, bool):
            raise ReviewThreadGateError("review thread payload is missing boolean isResolved")
        if not resolved:
            unresolved.append(thread)

    if not unresolved:
        return ReviewThreadGateResult(
            passed=True,
            unresolved_count=0,
            message="No unresolved review threads.",
        )

    details = "\n".join(f"  - {_thread_url(thread)}" for thread in unresolved)
    return ReviewThreadGateResult(
        passed=False,
        unresolved_count=len(unresolved),
        message=(
            f"{len(unresolved)} unresolved review thread(s) remain. "
            "Reply and resolve them, or reply with a technical reason they do not apply:\n"
            f"{details}"
        ),
    )


def _extract_review_threads(payload: dict[str, Any]) -> tuple[list[dict[str, Any]], bool, str | None]:
    errors = payload.get("errors")
    if errors:
        raise ReviewThreadGateError(f"GitHub GraphQL returned errors: {errors}")
    data = payload.get("data")
    if not isinstance(data, dict):
        raise ReviewThreadGateError("GitHub GraphQL response is missing data")
    repository = data.get("repository")
    if not isinstance(repository, dict):
        raise ReviewThreadGateError("GitHub GraphQL response is missing repository")
    pull_request = repository.get("pullRequest")
    if not isinstance(pull_request, dict):
        raise ReviewThreadGateError("GitHub GraphQL response is missing pullRequest")
    review_threads = pull_request.get("reviewThreads")
    if not isinstance(review_threads, dict):
        raise ReviewThreadGateError("GitHub GraphQL response is missing reviewThreads")
    nodes = review_threads.get("nodes")
    if not isinstance(nodes, list):
        raise ReviewThreadGateError("GitHub GraphQL reviewThreads.nodes must be a list")
    for node in nodes:
        if not isinstance(node, dict):
            raise ReviewThreadGateError("GitHub GraphQL reviewThreads.nodes entries must be objects")
    page_info = review_threads.get("pageInfo")
    if not isinstance(page_info, dict):
        raise ReviewThreadGateError("GitHub GraphQL reviewThreads.pageInfo must be an object")
    has_next_page = page_info.get("hasNextPage")
    if not isinstance(has_next_page, bool):
        raise ReviewThreadGateError("GitHub GraphQL pageInfo.hasNextPage must be boolean")
    end_cursor = page_info.get("endCursor")
    if end_cursor is not None and not isinstance(end_cursor, str):
        raise ReviewThreadGateError("GitHub GraphQL pageInfo.endCursor must be string or null")
    if has_next_page and end_cursor is None:
        raise ReviewThreadGateError("GitHub GraphQL pageInfo.endCursor is required when hasNextPage is true")
    return nodes, has_next_page, end_cursor


def fetch_review_threads(
    *,
    owner: str,
    name: str,
    pull_number: int,
    token: str,
) -> list[dict[str, Any]]:
    threads: list[dict[str, Any]] = []
    cursor: str | None = None
    while True:
        payload = _request_graphql(
            token=token,
            query=REVIEW_THREADS_QUERY,
            variables={
                "owner": owner,
                "name": name,
                "number": pull_number,
                "cursor": cursor,
            },
        )
        page_threads, has_next_page, cursor = _extract_review_threads(payload)
        threads.extend(page_threads)
        if not has_next_page:
            return threads


def run() -> int:
    repository = _env("GITHUB_REPOSITORY")
    token = _env("GITHUB_TOKEN")
    status_context = _optional_str_env("REVIEW_THREAD_GATE_STATUS_CONTEXT")
    payload = _read_event_payload()
    pull_number = _pull_number(payload)
    owner, name = _repo_parts(repository)
    threads = fetch_review_threads(
        owner=owner,
        name=name,
        pull_number=pull_number,
        token=token,
    )
    result = evaluate_review_thread_gate(review_threads=threads)
    print(result.message)
    if status_context is not None:
        head_sha = _pull_head_sha(payload)
        # Publish the verdict as a durable commit status keyed to the PR head.
        # The required merge context is this status, not this Actions job, so a
        # cancelled or superseded run cannot leave a terminal "cancelled" check
        # that blocks the merge. The exit code still mirrors the verdict so the
        # job check-run stays meaningful for humans.
        github_commit_status.publish_commit_status(
            repository=repository,
            sha=head_sha,
            token=token,
            passed=result.passed,
            description=result.message,
            context=status_context,
            post_json=_post_json,
            api_base=_api_base(),
            target_url=github_commit_status.run_target_url(),
        )
    return 0 if result.passed else 1


def main() -> int:
    try:
        return run()
    except ReviewThreadGateError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
