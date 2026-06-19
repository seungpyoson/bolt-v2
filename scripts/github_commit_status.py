#!/usr/bin/env python3
"""Shared helpers for publishing a GitHub commit status from a PR gate.

A required merge check that is the Actions *job* can be left in a terminal
``cancelled`` state by a superseded or duplicate run, which blocks the merge
with no real failure. Publishing the verdict as a *commit status* decouples it
from the run lifecycle: the required context becomes the status (only ever
written ``success``/``failure`` by the gate), so a cancelled run can never
poison it.

This module owns the status payload/URL logic so every gate publishes the same
way. The network POST is injected by the caller (``post_json``) so each gate
keeps its own error typing and stays unit-testable.
"""

from __future__ import annotations

import os
import urllib.parse
from typing import Any, Callable, Mapping


STATUS_DESCRIPTION_LIMIT = 140


def clamp_description(message: str) -> str:
    """GitHub rejects commit-status descriptions longer than 140 chars."""
    normalized = " ".join(message.split())
    if len(normalized) <= STATUS_DESCRIPTION_LIMIT:
        return normalized
    return f"{normalized[: STATUS_DESCRIPTION_LIMIT - 3]}..."


def status_api_url(*, api_base: str, repository: str, sha: str) -> str:
    owner_repo = "/".join(
        urllib.parse.quote(part, safe="") for part in repository.split("/", 1)
    )
    quoted_sha = urllib.parse.quote(sha, safe="")
    return f"{api_base.rstrip('/')}/repos/{owner_repo}/statuses/{quoted_sha}"


def run_target_url(env: Mapping[str, str] | None = None) -> str | None:
    """Link the status back to the Actions run that published it, if known."""
    env = os.environ if env is None else env
    server_url = env.get("GITHUB_SERVER_URL")
    repository = env.get("GITHUB_REPOSITORY")
    run_id = env.get("GITHUB_RUN_ID")
    if not server_url or not repository or not run_id:
        return None
    return f"{server_url.rstrip('/')}/{repository}/actions/runs/{run_id}"


def commit_status_payload(
    *,
    passed: bool,
    context: str,
    description: str,
    target_url: str | None,
) -> dict[str, str]:
    payload = {
        "state": "success" if passed else "failure",
        "context": context,
        "description": clamp_description(description),
    }
    if target_url:
        payload["target_url"] = target_url
    return payload


def publish_commit_status(
    *,
    repository: str,
    sha: str,
    token: str,
    passed: bool,
    description: str,
    context: str,
    post_json: Callable[[str, str, dict[str, Any]], None],
    api_base: str,
    target_url: str | None,
) -> None:
    """Post ``success``/``failure`` for ``context`` onto the head ``sha``."""
    post_json(
        status_api_url(api_base=api_base, repository=repository, sha=sha),
        token,
        commit_status_payload(
            passed=passed,
            context=context,
            description=description,
            target_url=target_url,
        ),
    )
