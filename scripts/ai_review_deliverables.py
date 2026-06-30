#!/usr/bin/env python3
"""Post AI review deliverables and failure notices for optional PR reviewers."""

from __future__ import annotations

import argparse
import shlex
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_CONFIG_PATH = Path(__file__).resolve().parents[1] / "ci" / "ai-review.toml"
EXPLICIT_SECRET_ENV_NAMES = frozenset(("GLM_API_KEY", "KIMI_API_KEY", "GITHUB_TOKEN", "OPENAI__KEY", "OPENAI_KEY"))
SECRET_ENV_SUFFIXES = ("_API_KEY", "_KEY", "_TOKEN")
SECRET_ENV_PREFIXES = ("OPENAI__",)


class ReviewFailed(RuntimeError):
    """Raised after a visible failure notice has been posted."""


@dataclass(frozen=True)
class ReviewOutputContract:
    finding_required_labels: tuple[str, ...]
    finding_guidance: tuple[str, ...]
    no_findings_indicator: str
    no_findings_intro: str
    no_findings_required_labels: tuple[str, ...]
    no_findings_guidance: tuple[str, ...]
    non_deliverable_indicators: tuple[str, ...]
    pr_agent_deliverable_headings: tuple[str, ...]
    pr_agent_disabled_noise: tuple[str, ...]


@dataclass(frozen=True)
class FallbackConfig:
    repo: str
    pr_number: int
    started_at: str
    instructions: str
    max_chunk_chars: int
    run_url: str
    max_comment_chars: int
    response_chars_per_chunk: int
    output_contract: ReviewOutputContract
    provider: str = "GLM"
    deliverable_markers: tuple[str, ...] = ()
    deliverable_bot_logins: tuple[str, ...] = ()
    expected_bot_login: str = ""
    comment_marker: str = ""
    notice_marker: str = ""
    review_intro: str = ""
    source_label: str = ""


@dataclass(frozen=True)
class ReviewFile:
    filename: str
    status: str
    additions: int
    deletions: int
    changes: int
    patch: str

    @classmethod
    def from_api_payload(cls, payload: dict[str, object]) -> "ReviewFile":
        return cls(
            filename=str(payload.get("filename", "")),
            status=str(payload.get("status", "")),
            additions=int(payload.get("additions") or 0),
            deletions=int(payload.get("deletions") or 0),
            changes=int(payload.get("changes") or 0),
            patch=str(payload.get("patch") or ""),
        )


@dataclass(frozen=True)
class ReviewChunk:
    title: str
    body: str


class GitHubClient:
    def __init__(
        self,
        *,
        repo: str,
        pr_number: int,
        token: str,
        api_url: str,
    ) -> None:
        self.repo = repo
        self.pr_number = pr_number
        self.token = token
        self.api_url = api_url.rstrip("/")

    def _request_json(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, str] | None = None,
        payload: dict[str, object] | None = None,
    ) -> Any:
        query = f"?{urllib.parse.urlencode(params)}" if params else ""
        url = f"{self.api_url}/repos/{self.repo}/{path}{query}"
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            url,
            data=data,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/json",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                body = response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"GitHub API {method} {path} failed with HTTP {exc.code}: {detail}") from exc
        if not body.strip():
            return {}
        return json.loads(body)

    def _list_paginated(self, path: str) -> list[dict[str, object]]:
        items: list[dict[str, object]] = []
        page = 1
        while True:
            payload = self._request_json("GET", path, params={"per_page": "100", "page": str(page)})
            if not isinstance(payload, list):
                raise RuntimeError(f"GitHub API {path} returned non-list payload")
            items.extend(item for item in payload if isinstance(item, dict))
            if len(payload) < 100:
                return items
            page += 1

    def list_pr_files(self) -> list[dict[str, object]]:
        return self._list_paginated(f"pulls/{self.pr_number}/files")

    def list_issue_comments_for(self, pr_number: int) -> list[dict[str, object]]:
        return self._list_paginated(f"issues/{pr_number}/comments")

    def list_issue_comments(self) -> list[dict[str, object]]:
        return self.list_issue_comments_for(self.pr_number)

    def list_reviews_for(self, pr_number: int) -> list[dict[str, object]]:
        return self._list_paginated(f"pulls/{pr_number}/reviews")

    def list_reviews(self) -> list[dict[str, object]]:
        return self.list_reviews_for(self.pr_number)

    def list_pull_review_comments_for(self, pr_number: int) -> list[dict[str, object]]:
        return self._list_paginated(f"pulls/{pr_number}/comments")

    def list_pull_review_comments(self) -> list[dict[str, object]]:
        return self.list_pull_review_comments_for(self.pr_number)

    def post_issue_comment_for(self, pr_number: int, body: str) -> None:
        self._request_json("POST", f"issues/{pr_number}/comments", payload={"body": body})

    def post_issue_comment(self, body: str) -> None:
        self.post_issue_comment_for(self.pr_number, body)

    def update_issue_comment(self, comment_id: int, body: str) -> None:
        self._request_json("PATCH", f"issues/comments/{comment_id}", payload={"body": body})

    def update_pull_review_comment(self, comment_id: int, body: str) -> None:
        self._request_json("PATCH", f"pulls/comments/{comment_id}", payload={"body": body})

class OpenAIChatClient:
    def __init__(
        self,
        *,
        api_key: str,
        api_base: str,
        model: str,
        provider: str,
        temperature: float,
        timeout_seconds: int = 180,
    ) -> None:
        self.api_key = api_key
        self.api_base = api_base.rstrip("/")
        self.model = model
        self.provider = provider
        self.temperature = temperature
        self.timeout_seconds = timeout_seconds

    def review_chunk(self, *, system_prompt: str, user_prompt: str) -> str:
        payload = {
            "model": self.model,
            "temperature": self.temperature,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_prompt},
            ],
        }
        request = urllib.request.Request(
            f"{self.api_base}/chat/completions",
            data=json.dumps(payload).encode("utf-8"),
            method="POST",
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                raw = response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"{self.provider} API request failed with HTTP {exc.code}: {detail}") from exc
        payload_json = json.loads(raw)
        choices = payload_json.get("choices")
        if not isinstance(choices, list) or not choices:
            raise RuntimeError(f"{self.provider} API response did not include choices")
        message = choices[0].get("message")
        if not isinstance(message, dict):
            raise RuntimeError(f"{self.provider} API response did not include a message")
        content = message.get("content")
        if not isinstance(content, str) or not content.strip():
            raise RuntimeError(f"{self.provider} API response content was empty")
        return content.strip()


class KimiCliClient:
    def __init__(
        self,
        *,
        api_key: str,
        api_base: str,
        model: str,
        provider: str,
        provider_type: str,
        model_max_context_size: int,
        default_thinking: bool,
        telemetry_disabled: bool,
        timeout_seconds: int = 180,
        binary: str = "kimi",
    ) -> None:
        self.api_key = api_key
        self.api_base = api_base.rstrip("/")
        self.model = model
        self.provider = provider
        self.provider_type = provider_type
        self.model_max_context_size = model_max_context_size
        self.default_thinking = default_thinking
        self.telemetry_disabled = telemetry_disabled
        self.timeout_seconds = timeout_seconds
        self.binary = binary
        self.kimi_home = os.environ.get("KIMI_CODE_HOME") or tempfile.mkdtemp(prefix="kimi-code-")

    def review_chunk(self, *, system_prompt: str, user_prompt: str) -> str:
        kimi_home = Path(self.kimi_home)
        kimi_home.mkdir(parents=True, exist_ok=True)
        config_path = kimi_home / "config.toml"
        if not config_path.exists():
            telemetry_value = "false" if self.telemetry_disabled else "true"
            config_path.write_text(f"telemetry = {telemetry_value}\n", encoding="utf-8")

        env = os.environ.copy()
        env.update(
            {
                "KIMI_CODE_HOME": str(kimi_home),
                "KIMI_DISABLE_TELEMETRY": "1" if self.telemetry_disabled else "0",
                "KIMI_MODEL_NAME": self.model,
                "KIMI_MODEL_API_KEY": self.api_key,
                "KIMI_MODEL_BASE_URL": self.api_base,
                "KIMI_MODEL_PROVIDER_TYPE": self.provider_type,
                "KIMI_MODEL_MAX_CONTEXT_SIZE": str(self.model_max_context_size),
                "KIMI_MODEL_DEFAULT_THINKING": "true" if self.default_thinking else "false",
            }
        )
        prompt = f"{system_prompt}\n\n{user_prompt}"
        try:
            completed = subprocess.run(
                [self.binary, "-p", prompt],
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                env=env,
            )
        except subprocess.TimeoutExpired as exc:
            raise RuntimeError(f"{self.provider} CLI review timed out after {self.timeout_seconds} seconds") from exc
        except FileNotFoundError as exc:
            raise RuntimeError(f"{self.provider} CLI binary {self.binary!r} was not found") from exc
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout or f"exit {completed.returncode}").strip()
            raise RuntimeError(f"{self.provider} CLI review failed: {detail}")
        content = completed.stdout.strip()
        if not content:
            raise RuntimeError(f"{self.provider} CLI review produced empty output")
        return content


def parse_iso_timestamp(value: str) -> datetime:
    normalized = value.strip().replace("Z", "+00:00")
    return datetime.fromisoformat(normalized).astimezone(timezone.utc)


def text_time_is_after_or_equal(value: object, threshold: datetime) -> bool:
    if not isinstance(value, str) or not value.strip():
        return False
    try:
        return parse_iso_timestamp(value) >= threshold
    except ValueError:
        return False


def body_has_deliverable_marker(body: object, markers: tuple[str, ...]) -> bool:
    return isinstance(body, str) and any(body.lstrip().startswith(marker) for marker in markers)


def payload_time(payload: dict[str, object], *fields: str) -> datetime | None:
    for field in fields:
        value = payload.get(field)
        if not isinstance(value, str) or not value.strip():
            continue
        try:
            return parse_iso_timestamp(value)
        except ValueError:
            continue
    return None


def comment_id(payload: dict[str, object]) -> int | None:
    value = payload.get("id")
    return value if isinstance(value, int) else None


def actor_is_expected_bot(payload: dict[str, object], expected_login: str) -> bool:
    user = payload.get("user")
    if not isinstance(user, dict):
        return False
    return str(user.get("type", "")).lower() == "bot" and user.get("login") == expected_login


def has_review_deliverable(
    *,
    comments: list[dict[str, object]],
    reviews: list[dict[str, object]],
    started_at: str,
    markers: tuple[str, ...],
    expected_bot_login: str,
    output_contract: ReviewOutputContract,
    require_source_line: bool,
) -> bool:
    threshold = parse_iso_timestamp(started_at)
    for comment in comments:
        if not actor_is_expected_bot(comment, expected_bot_login):
            continue
        if not body_has_deliverable_marker(comment.get("body"), markers):
            continue
        body = str(comment.get("body") or "")
        if require_source_line and not review_body_has_source_line(body):
            continue
        if not review_body_is_quality_deliverable(body, output_contract):
            continue
        if text_time_is_after_or_equal(comment.get("updated_at"), threshold) or text_time_is_after_or_equal(
            comment.get("created_at"), threshold
        ):
            return True
    for review in reviews:
        if not actor_is_expected_bot(review, expected_bot_login):
            continue
        if not body_has_deliverable_marker(review.get("body"), markers):
            continue
        body = str(review.get("body") or "")
        if require_source_line and not review_body_has_source_line(body):
            continue
        if not review_body_is_quality_deliverable(body, output_contract):
            continue
        if text_time_is_after_or_equal(review.get("submitted_at"), threshold):
            return True
    return False


def latest_failure_notice_time(*, github: Any, expected_bot_login: str, notice_marker: str) -> datetime | None:
    if not notice_marker:
        return None
    times = [
        timestamp
        for comment in github.list_issue_comments()
        if actor_is_expected_bot(comment, expected_bot_login)
        and body_has_deliverable_marker(comment.get("body"), (notice_marker,))
        for timestamp in [payload_time(comment, "updated_at", "created_at")]
        if timestamp is not None
    ]
    return max(times, default=None)


def latest_quality_review_deliverable_time(
    *,
    github: Any,
    expected_bot_login: str,
    deliverable_markers: tuple[str, ...],
    output_contract: ReviewOutputContract,
    require_source_line: bool = False,
) -> datetime | None:
    if not deliverable_markers:
        return None
    times: list[datetime] = []
    for comment in github.list_issue_comments():
        body = str(comment.get("body") or "")
        if (
            actor_is_expected_bot(comment, expected_bot_login)
            and body_has_deliverable_marker(body, deliverable_markers)
            and (not require_source_line or review_body_has_source_line(body))
            and review_body_is_quality_deliverable(body, output_contract)
        ):
            timestamp = payload_time(comment, "updated_at", "created_at")
            if timestamp is not None:
                times.append(timestamp)
    for review in github.list_reviews():
        body = str(review.get("body") or "")
        if (
            actor_is_expected_bot(review, expected_bot_login)
            and body_has_deliverable_marker(body, deliverable_markers)
            and (not require_source_line or review_body_has_source_line(body))
            and review_body_is_quality_deliverable(body, output_contract)
        ):
            timestamp = payload_time(review, "submitted_at")
            if timestamp is not None:
                times.append(timestamp)
    return max(times, default=None)


def claude_body_is_review_deliverable(
    body: object,
    *,
    output_contract: ReviewOutputContract,
    deliverable_marker: str,
) -> bool:
    if not isinstance(body, str) or not deliverable_marker:
        return False
    text = body.strip()
    if not body_has_deliverable_marker(text, (deliverable_marker,)):
        return False
    return review_body_has_source_line(text) and review_body_is_quality_deliverable(text, output_contract)


def actor_is_any_expected_bot(payload: dict[str, object], expected_logins: tuple[str, ...]) -> bool:
    return any(actor_is_expected_bot(payload, login) for login in expected_logins)


def latest_claude_visible_deliverable_time(
    *,
    github: Any,
    deliverable_bot_logins: tuple[str, ...],
    output_contract: ReviewOutputContract,
    deliverable_marker: str,
) -> datetime | None:
    expected_logins = tuple(login for login in deliverable_bot_logins if login)
    if not expected_logins or not deliverable_marker:
        return None

    times: list[datetime] = []
    for comment in github.list_issue_comments():
        if (
            actor_is_any_expected_bot(comment, expected_logins)
            and claude_body_is_review_deliverable(
                comment.get("body"),
                output_contract=output_contract,
                deliverable_marker=deliverable_marker,
            )
        ):
            timestamp = payload_time(comment, "updated_at", "created_at")
            if timestamp is not None:
                times.append(timestamp)
    for review in github.list_reviews():
        if (
            actor_is_any_expected_bot(review, expected_logins)
            and claude_body_is_review_deliverable(
                review.get("body"),
                output_contract=output_contract,
                deliverable_marker=deliverable_marker,
            )
        ):
            timestamp = payload_time(review, "submitted_at")
            if timestamp is not None:
                times.append(timestamp)
    for comment in github.list_pull_review_comments():
        if (
            actor_is_any_expected_bot(comment, expected_logins)
            and claude_body_is_review_deliverable(
                comment.get("body"),
                output_contract=output_contract,
                deliverable_marker=deliverable_marker,
            )
        ):
            timestamp = payload_time(comment, "updated_at", "created_at")
            if timestamp is not None:
                times.append(timestamp)
    return max(times, default=None)


def has_claude_visible_deliverable(
    *,
    github: Any,
    started_at: str,
    deliverable_bot_logins: tuple[str, ...],
    output_contract: ReviewOutputContract,
    deliverable_marker: str,
) -> bool:
    threshold = parse_iso_timestamp(started_at)
    timestamp = latest_claude_visible_deliverable_time(
        github=github,
        deliverable_bot_logins=deliverable_bot_logins,
        output_contract=output_contract,
        deliverable_marker=deliverable_marker,
    )
    return timestamp is not None and timestamp >= threshold


def provider_retry_needed(
    *,
    github: Any,
    expected_bot_login: str,
    notice_marker: str,
    output_contract: ReviewOutputContract,
    deliverable_markers: tuple[str, ...] = (),
    deliverable_bot_logins: tuple[str, ...] = (),
    claude_deliverable_marker: str = "",
    require_source_line: bool = False,
) -> bool:
    retry_needed, _reason = provider_retry_decision(
        github=github,
        expected_bot_login=expected_bot_login,
        notice_marker=notice_marker,
        output_contract=output_contract,
        deliverable_markers=deliverable_markers,
        deliverable_bot_logins=deliverable_bot_logins,
        claude_deliverable_marker=claude_deliverable_marker,
        require_source_line=require_source_line,
    )
    return retry_needed


def provider_retry_decision(
    *,
    github: Any,
    expected_bot_login: str,
    notice_marker: str,
    output_contract: ReviewOutputContract,
    deliverable_markers: tuple[str, ...] = (),
    deliverable_bot_logins: tuple[str, ...] = (),
    claude_deliverable_marker: str = "",
    require_source_line: bool = False,
) -> tuple[bool, str]:
    notice_time = latest_failure_notice_time(
        github=github,
        expected_bot_login=expected_bot_login,
        notice_marker=notice_marker,
    )
    if notice_time is None:
        return False, "no-failure-notice"
    quality_times = [
        latest_quality_review_deliverable_time(
            github=github,
            expected_bot_login=expected_bot_login,
            deliverable_markers=deliverable_markers,
            output_contract=output_contract,
            require_source_line=require_source_line,
        ),
        latest_claude_visible_deliverable_time(
            github=github,
            deliverable_bot_logins=deliverable_bot_logins,
            output_contract=output_contract,
            deliverable_marker=claude_deliverable_marker,
        ),
    ]
    latest_deliverable = max((timestamp for timestamp in quality_times if timestamp is not None), default=None)
    if latest_deliverable is None or notice_time >= latest_deliverable:
        return True, "previous-failure-notice"
    return False, "deliverable-after-notice"


def review_body_has_source_line(body: str) -> bool:
    return any(line.strip().startswith(("**Source:**", "- Source:")) for line in body.splitlines())


def review_body_is_quality_deliverable(body: str, output_contract: ReviewOutputContract) -> bool:
    text = body.strip()
    if not text:
        return False
    lowered = text.lower()
    if any(indicator.lower() in lowered for indicator in output_contract.non_deliverable_indicators):
        return False
    finding_labels = tuple(label.lower() for label in output_contract.finding_required_labels)
    if finding_labels and all(review_body_has_line_starting_with(lowered, label) for label in finding_labels):
        return True
    if review_body_has_no_findings_contract(lowered, output_contract):
        return True
    pr_agent_headings = tuple(heading.lower() for heading in output_contract.pr_agent_deliverable_headings)
    if any(heading in lowered for heading in pr_agent_headings):
        if any(noise.lower() in lowered for noise in output_contract.pr_agent_disabled_noise):
            return False
        return pr_agent_body_has_substantive_review(lowered, output_contract)
    return False


def review_body_has_line_starting_with(lowered: str, label: str) -> bool:
    return any(line.strip().startswith(label) for line in lowered.splitlines())


def review_body_has_no_findings_contract(lowered: str, output_contract: ReviewOutputContract) -> bool:
    if output_contract.no_findings_indicator.lower() not in lowered:
        return False
    return all(
        review_body_has_line_starting_with(lowered, label.lower())
        for label in output_contract.no_findings_required_labels
    )


def pr_agent_body_has_substantive_review(lowered: str, output_contract: ReviewOutputContract) -> bool:
    finding_labels = tuple(label.lower() for label in output_contract.finding_required_labels)
    has_finding_contract = bool(finding_labels) and all(
        review_body_has_line_starting_with(lowered, label)
        for label in finding_labels
    )
    return has_finding_contract or review_body_has_no_findings_contract(lowered, output_contract)


def comment_part_key(body: object) -> str:
    if not isinstance(body, str):
        return ""
    for line in body.splitlines():
        if line.startswith("- Comment part: "):
            return line.strip()
    return ""


def latest_marker_comment(
    *,
    comments: list[dict[str, object]],
    marker: str,
    expected_bot_login: str,
    run_url: str,
    part_key: str,
) -> dict[str, object] | None:
    marker_comments = [
        comment
        for comment in comments
        if actor_is_expected_bot(comment, expected_bot_login)
        and isinstance(comment.get("body"), str)
        and body_has_deliverable_marker(comment.get("body"), (marker,))
        and run_url in comment.get("body", "")
        and comment_part_key(comment.get("body")) == part_key
        and isinstance(comment.get("id"), int)
        and not isinstance(comment.get("id"), bool)
    ]
    if not marker_comments:
        return None
    return max(marker_comments, key=lambda comment: str(comment.get("updated_at") or comment.get("created_at") or ""))


def post_or_update_marker_comment(*, github: Any, config: FallbackConfig, body: str) -> None:
    if not config.comment_marker:
        github.post_issue_comment(body)
        return
    existing = latest_marker_comment(
        comments=github.list_issue_comments(),
        marker=config.comment_marker,
        expected_bot_login=config.expected_bot_login,
        run_url=config.run_url,
        part_key=comment_part_key(body),
    )
    if existing is None:
        github.post_issue_comment(body)
        return
    github.update_issue_comment(int(existing["id"]), body)


def post_or_update_notice_comment(*, github: Any, config: FallbackConfig, body: str) -> None:
    marker = config.notice_marker or config.comment_marker
    if not marker:
        github.post_issue_comment(body)
        return
    existing = latest_marker_comment(
        comments=github.list_issue_comments(),
        marker=marker,
        expected_bot_login=config.expected_bot_login,
        run_url=config.run_url,
        part_key=comment_part_key(body),
    )
    if existing is None:
        github.post_issue_comment(body)
        return
    github.update_issue_comment(int(existing["id"]), body)


def add_source_line(body: str, *, marker: str, source_label: str, run_url: str = "") -> str:
    if not source_label:
        return body
    source_line = f"**Source:** {source_label}"
    run_line = f"**Action run:** {run_url}" if run_url else ""
    text = body.lstrip()
    prefix = ""
    if marker and text.startswith(marker):
        prefix = f"{marker}\n\n"
        text = text[len(marker):].lstrip()
    elif marker:
        prefix = f"{marker}\n\n"
    if source_line in text and (not run_line or run_line in text):
        return prefix + text
    lines = text.splitlines()
    insert_at = 1 if lines and lines[0].startswith("## ") else 0
    metadata = ["", source_line]
    if run_line:
        metadata.append(run_line)
    metadata.append("")
    lines[insert_at:insert_at] = metadata
    return prefix + "\n".join(lines).strip() + "\n"


def stamp_existing_review_comment(
    *,
    github: Any,
    started_at: str,
    markers: tuple[str, ...],
    expected_bot_login: str,
    marker: str,
    source_label: str,
    run_url: str,
) -> str:
    threshold = parse_iso_timestamp(started_at)
    issue_candidates = [
        comment
        for comment in github.list_issue_comments()
        if actor_is_expected_bot(comment, expected_bot_login)
        and body_has_deliverable_marker(comment.get("body"), markers)
        and isinstance(comment.get("id"), int)
        and not isinstance(comment.get("id"), bool)
        and (
            text_time_is_after_or_equal(comment.get("updated_at"), threshold)
            or text_time_is_after_or_equal(comment.get("created_at"), threshold)
        )
    ]
    review_comment_candidates = [
        comment
        for comment in github.list_pull_review_comments()
        if actor_is_expected_bot(comment, expected_bot_login)
        and body_has_deliverable_marker(comment.get("body"), markers)
        and isinstance(comment.get("body"), str)
        and isinstance(comment.get("id"), int)
        and not isinstance(comment.get("id"), bool)
        and (
            text_time_is_after_or_equal(comment.get("updated_at"), threshold)
            or text_time_is_after_or_equal(comment.get("created_at"), threshold)
        )
    ]
    if not issue_candidates and not review_comment_candidates:
        return "no-existing-review"
    updated = 0
    already_stamped = 0
    for comment in issue_candidates:
        body = str(comment.get("body") or "")
        stamped = add_source_line(body, marker=marker, source_label=source_label, run_url=run_url)
        if stamped == body:
            already_stamped += 1
            continue
        github.update_issue_comment(int(comment["id"]), stamped)
        updated += 1
    for comment in review_comment_candidates:
        body = str(comment.get("body") or "")
        stamped = add_source_line(body, marker=marker, source_label=source_label, run_url=run_url)
        if stamped == body:
            already_stamped += 1
            continue
        github.update_pull_review_comment(int(comment["id"]), stamped)
        updated += 1
    if updated:
        return "existing-reviews-stamped"
    if already_stamped:
        return "existing-reviews-already-stamped"
    return "no-existing-review"


def file_fragment_body(review_file: ReviewFile, patch_lines: list[str]) -> str:
    patch_body = "\n".join(patch_lines)
    header = file_fragment_header(review_file)
    return f"{header}```diff\n{patch_body}\n```\n"


def file_fragment_header(review_file: ReviewFile) -> str:
    return (
        f"### {review_file.filename}\n"
        f"Status: {review_file.status}; changes: +{review_file.additions}/-{review_file.deletions} "
        f"({review_file.changes} total)\n\n"
    )


def render_file_fragment(review_file: ReviewFile, patch_lines: list[str], *, title: str, max_chars: int) -> ReviewChunk:
    body = file_fragment_body(review_file, patch_lines)
    if len(body) <= max_chars:
        return ReviewChunk(title=title, body=body)
    header = file_fragment_header(review_file)
    code_prefix = f"{header}```diff\n"
    suffix = "\n```\n\n[fragment truncated to fit review budget]\n"
    patch_budget = max_chars - len(code_prefix) - len(suffix)
    if patch_budget <= 0:
        omitted = f"{header}[fragment omitted to fit review budget]\n"
        return ReviewChunk(title=title, body=omitted[:max_chars])
    patch_body = "\n".join(patch_lines)
    return ReviewChunk(title=title, body=code_prefix + patch_body[:patch_budget] + suffix)


def split_review_file(review_file: ReviewFile, max_chars: int) -> list[ReviewChunk]:
    lines = review_file.patch.splitlines() or ["[patch unavailable from GitHub API]"]
    chunks: list[ReviewChunk] = []
    current: list[str] = []
    part = 1

    def title() -> str:
        if part == 1:
            return review_file.filename
        return f"{review_file.filename} part {part}"

    for line in lines:
        candidate = [*current, line]
        if current and len(file_fragment_body(review_file, candidate)) > max_chars:
            chunks.append(render_file_fragment(review_file, current, title=title(), max_chars=max_chars))
            part += 1
            current = [line]
            if len(file_fragment_body(review_file, current)) > max_chars:
                chunks.append(render_file_fragment(review_file, current, title=title(), max_chars=max_chars))
                part += 1
                current = []
            continue
        current = candidate

        if len(file_fragment_body(review_file, current)) == max_chars:
            chunks.append(render_file_fragment(review_file, current, title=title(), max_chars=max_chars))
            part += 1
            current = []

    if current:
        chunks.append(render_file_fragment(review_file, current, title=title(), max_chars=max_chars))

    return chunks


def pack_review_chunks(files: list[ReviewFile], max_chars: int) -> list[ReviewChunk]:
    if max_chars < 200:
        raise ValueError("max_chars must be at least 200")

    fragments: list[ReviewChunk] = []
    for review_file in files:
        fragments.extend(split_review_file(review_file, max_chars))

    chunks: list[ReviewChunk] = []
    current_parts: list[ReviewChunk] = []
    current_body = ""
    separator = "\n\n"

    for fragment in fragments:
        candidate_body = fragment.body if not current_body else current_body + separator + fragment.body
        if current_parts and len(candidate_body) > max_chars:
            chunks.append(
                ReviewChunk(
                    title=", ".join(part.title for part in current_parts),
                    body=current_body,
                )
            )
            current_parts = [fragment]
            current_body = fragment.body
            continue
        current_parts.append(fragment)
        current_body = candidate_body

    if current_parts:
        chunks.append(
            ReviewChunk(
                title=", ".join(part.title for part in current_parts),
                body=current_body,
            )
        )

    return chunks


def build_system_prompt(instructions: str, output_contract: ReviewOutputContract) -> str:
    finding_lines = "\n".join(
        f"        {label} {guidance}"
        for label, guidance in zip(
            output_contract.finding_required_labels,
            output_contract.finding_guidance,
            strict=True,
        )
    )
    no_findings_lines = "\n".join(
        f"        {label} {guidance}"
        for label, guidance in zip(
            output_contract.no_findings_required_labels,
            output_contract.no_findings_guidance,
            strict=True,
        )
    )
    return textwrap.dedent(
        f"""\
        You are conducting an advisory pull request review for bolt-v2.

        Use only hard evidence from the supplied chunk. Report actionable findings only.
        For every finding, include:
{finding_lines}
        Do not write generic summaries, praise, scorecards, or broad review guidance.
        If this chunk contains no hard-evidence findings, use exactly this structure:
        {output_contract.no_findings_intro}
{no_findings_lines}

        Repository review instructions:
        {instructions.strip()}
        """
    ).strip()


def build_user_prompt(chunk: ReviewChunk, index: int, total: int) -> str:
    return textwrap.dedent(
        f"""\
        Review chunk {index} of {total}: {chunk.title}

        This is one chunk of a larger PR diff. Do not claim whole-PR coverage beyond this chunk.
        Do not infer facts from omitted chunks.

        {chunk.body}
        """
    ).strip()


def truncate_text(value: str, limit: int) -> str:
    if len(value) <= limit:
        return value
    suffix = "\n\n[truncated to fit GitHub comment limit]"
    allowed = max(0, limit - len(suffix))
    truncated = value[:allowed]
    if truncated.count("```") % 2 == 1:
        closer = "\n```"
        allowed = max(0, limit - len(suffix) - len(closer))
        truncated = value[:allowed].rstrip()
        if truncated.count("```") % 2 == 1:
            truncated += closer
    return truncated + suffix


def split_text_for_comment_sections(value: str, limit: int) -> list[str]:
    if limit <= 0 or len(value) <= limit:
        return [value]

    parts: list[str] = []
    current = ""
    for line in value.splitlines(keepends=True):
        if len(current) + len(line) <= limit:
            current += line
            continue

        if current:
            remaining = max(0, limit - len(current))
            if remaining:
                current += line[:remaining]
                line = line[remaining:]
            parts.append(current.rstrip("\n"))
            current = ""

        while len(line) > limit:
            parts.append(line[:limit])
            line = line[limit:]
        current = line

    if current or not parts:
        parts.append(current.rstrip("\n"))
    return parts


def balance_markdown_fence_parts(parts: list[str]) -> list[str]:
    balanced: list[str] = []
    inside_fence = False
    for part in parts:
        rendered = part
        if inside_fence:
            rendered = "```\n" + rendered
        if part.count("```") % 2 == 1:
            inside_fence = not inside_fence
        if inside_fence:
            rendered = rendered.rstrip() + "\n```"
        balanced.append(rendered)
    return balanced


def split_response_for_sections(response: str, limit: int) -> list[str]:
    return balance_markdown_fence_parts(split_text_for_comment_sections(response, limit))


def invalid_review_response_detail(index: int, response: str) -> str:
    detail = f"review response {index} did not meet the hard-evidence output contract"
    return f"{detail} (chars={len(response)}; output omitted from PR notice)"


def validate_review_responses(responses: list[str], output_contract: ReviewOutputContract) -> None:
    for index, response in enumerate(responses, start=1):
        if review_body_is_quality_deliverable(response, output_contract):
            continue
        raise RuntimeError(invalid_review_response_detail(index, response))


def write_github_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT", "")
    if not output_path:
        return
    with Path(output_path).open("a", encoding="utf-8") as output:
        output.write(f"{name}={value}\n")


def model_freshness_warning_from_env() -> str:
    return sanitize_detail(os.environ.get("AI_REVIEW_MODEL_FRESHNESS_WARNING", "").strip())


def render_model_freshness_warning_block(warning: str) -> str:
    sanitized = sanitize_detail(warning.strip())
    if not sanitized:
        return ""
    return "\n".join(f"> {line}" if line else ">" for line in ["[!WARNING]", *sanitized.splitlines()])


def model_freshness_notice_marker(provider: str, marker_template: str) -> str:
    if "{provider}" not in marker_template:
        raise RuntimeError("model_freshness.notice_marker_template must contain {provider}")
    provider_key = "".join(char.lower() if char.isalnum() else "-" for char in provider).strip("-")
    return marker_template.replace("{provider}", provider_key)


def render_model_freshness_notice(
    *,
    provider: str,
    pr_number: int,
    run_url: str,
    warning: str,
    marker_template: str,
) -> str:
    marker = model_freshness_notice_marker(provider, marker_template)
    warning_block = render_model_freshness_warning_block(warning)
    return textwrap.dedent(
        f"""\
        {marker}

        ## {provider} model freshness notice

        {warning_block}

        - PR: #{pr_number}
        - Action run: {run_url}

        The advisory review is non-blocking and continues with the pinned model for auditability.
        """
    ).strip()


def post_model_freshness_notice(
    *,
    github: Any,
    provider: str,
    pr_number: int,
    run_url: str,
    warning: str,
    expected_bot_login: str,
    marker_template: str,
) -> str:
    if not warning.strip():
        return "no-warning"
    marker = model_freshness_notice_marker(provider, marker_template)
    body = render_model_freshness_notice(
        provider=provider,
        pr_number=pr_number,
        run_url=run_url,
        warning=warning,
        marker_template=marker_template,
    )
    for existing in github.list_issue_comments():
        if not actor_is_expected_bot(existing, expected_bot_login):
            continue
        existing_body = existing.get("body")
        if not isinstance(existing_body, str) or not existing_body.lstrip().startswith(marker):
            continue
        existing_id = comment_id(existing)
        if existing_id is None:
            continue
        github.update_issue_comment(existing_id, body)
        return "notice-updated"
    github.post_issue_comment(body)
    return "notice-posted"


def prepend_model_freshness_warning_to_existing_review(
    *,
    github: Any,
    started_at: str,
    markers: tuple[str, ...],
    warning: str,
    expected_bot_login: str,
) -> int:
    warning_block = render_model_freshness_warning_block(warning)
    if not warning_block:
        return 0
    threshold = parse_iso_timestamp(started_at)
    updated = 0
    for comment in github.list_issue_comments():
        if not actor_is_expected_bot(comment, expected_bot_login):
            continue
        existing_id = comment_id(comment)
        if existing_id is None:
            continue
        body = comment.get("body")
        if not body_has_deliverable_marker(body, markers):
            continue
        if warning_block in body:
            continue
        if not (
            text_time_is_after_or_equal(comment.get("updated_at"), threshold)
            or text_time_is_after_or_equal(comment.get("created_at"), threshold)
        ):
            continue
        github.update_issue_comment(existing_id, f"{warning_block}\n\n{str(body).lstrip()}")
        updated += 1
    return updated


def render_chunk_response_section(
    *,
    chunk: ReviewChunk,
    response: str,
    index: int,
    total: int,
    response_part_index: int = 1,
    response_part_total: int = 1,
) -> str:
    title = f"### Chunk {index}/{total}: {chunk.title}"
    if response_part_total > 1:
        title += f" (response part {response_part_index}/{response_part_total})"
    return "\n".join(
        [
            title,
            "",
            response.strip(),
            "",
        ]
    )


def render_review_comment_body(
    *,
    config: FallbackConfig,
    total_chunks: int,
    sections: list[str],
    part_index: int | None = None,
    part_total: int | None = None,
) -> str:
    title = f"## {config.provider} PR Review"
    if part_index is not None and part_total is not None:
        title += f" (part {part_index}/{part_total})"
    intro = config.review_intro or (
        f"The primary {config.provider} review action completed without a visible deliverable after this run started, "
        f"so the fallback reviewer reviewed the PR diff with {config.provider}."
    )
    parts = []
    if config.comment_marker:
        parts.extend([config.comment_marker, ""])
    warning_block = render_model_freshness_warning_block(model_freshness_warning_from_env())
    if warning_block:
        parts.extend([warning_block, ""])
    parts.extend(
        [
        title,
        "",
        intro,
        "",
        f"- Action run: {config.run_url}",
        ]
    )
    if config.source_label:
        parts.append(f"- Source: {config.source_label}")
    if part_index is not None and part_total is not None:
        parts.append(f"- Comment part: {part_index}/{part_total}")
    parts.append("")
    parts.extend(sections)
    return "\n".join(parts).strip() + "\n"


def split_oversized_comment_sections(
    *,
    config: FallbackConfig,
    total_chunks: int,
    sections: list[str],
    part_total_hint: int,
) -> list[str]:
    split_sections: list[str] = []
    section_overhead_len = len(
        render_review_comment_body(
            config=config,
            total_chunks=total_chunks,
            sections=["x"],
            part_index=part_total_hint,
            part_total=part_total_hint,
        )
    ) - len("x")
    section_budget = max(1, config.max_comment_chars - section_overhead_len)
    for section in sections:
        rendered = render_review_comment_body(
            config=config,
            total_chunks=total_chunks,
            sections=[section],
            part_index=part_total_hint,
            part_total=part_total_hint,
        )
        if len(rendered) <= config.max_comment_chars:
            split_sections.append(section)
            continue
        split_sections.extend(balance_markdown_fence_parts(split_text_for_comment_sections(section, section_budget)))
    return split_sections


def pack_review_comment_sections(
    *,
    config: FallbackConfig,
    total_chunks: int,
    sections: list[str],
    part_total_hint: int,
) -> list[list[str]]:
    groups: list[list[str]] = []
    current: list[str] = []
    for section in sections:
        candidate = [*current, section]
        candidate_body = render_review_comment_body(
            config=config,
            total_chunks=total_chunks,
            sections=candidate,
            part_index=len(groups) + 1,
            part_total=part_total_hint,
        )
        if len(candidate_body) <= config.max_comment_chars or not current:
            current = candidate
            continue
        groups.append(current)
        current = [section]
    if current:
        groups.append(current)
    return groups


def render_review_comments(
    *,
    config: FallbackConfig,
    chunks: list[ReviewChunk],
    responses: list[str],
) -> list[str]:
    sections: list[str] = []
    for idx, (chunk, response) in enumerate(zip(chunks, responses, strict=True), start=1):
        response_parts = split_response_for_sections(response.strip(), config.response_chars_per_chunk)
        sections.extend(
            render_chunk_response_section(
                chunk=chunk,
                response=response_part,
                index=idx,
                total=len(chunks),
                response_part_index=response_part_idx,
                response_part_total=len(response_parts),
            )
            for response_part_idx, response_part in enumerate(response_parts, start=1)
        )
    comment = render_review_comment_body(config=config, total_chunks=len(chunks), sections=sections)
    if len(comment) <= config.max_comment_chars:
        return [comment]

    part_total_hint = max(1, len(sections))
    split_sections = split_oversized_comment_sections(
        config=config,
        total_chunks=len(chunks),
        sections=sections,
        part_total_hint=part_total_hint,
    )
    groups = pack_review_comment_sections(
        config=config,
        total_chunks=len(chunks),
        sections=split_sections,
        part_total_hint=max(1, len(split_sections)),
    )
    groups = pack_review_comment_sections(
        config=config,
        total_chunks=len(chunks),
        sections=split_sections,
        part_total_hint=max(1, len(groups)),
    )
    comments = [
        render_review_comment_body(
            config=config,
            total_chunks=len(chunks),
            sections=group,
            part_index=index,
            part_total=len(groups),
        )
        for index, group in enumerate(groups, start=1)
    ]
    oversized = [len(comment) for comment in comments if len(comment) > config.max_comment_chars]
    if oversized:
        raise RuntimeError(
            f"rendered review comment exceeded configured max_comment_chars={config.max_comment_chars}: "
            f"{max(oversized)}"
        )
    return comments


def secret_env_values() -> list[str]:
    names = set(EXPLICIT_SECRET_ENV_NAMES)
    names.update(
        name
        for name in os.environ
        if name.startswith(SECRET_ENV_PREFIXES) or name.endswith(SECRET_ENV_SUFFIXES)
    )
    return sorted(
        {secret for name in names if len(secret := os.environ.get(name, "")) >= 8},
        key=len,
        reverse=True,
    )


def sanitize_detail(value: str) -> str:
    sanitized = value
    for secret in secret_env_values():
        sanitized = sanitized.replace(secret, "***")
    return sanitized


def render_failure_notice(*, provider: str, config: FallbackConfig, error: BaseException) -> str:
    detail = sanitize_detail(str(error))
    marker_value = config.notice_marker or config.comment_marker
    marker = f"{marker_value}\n\n" if marker_value else ""
    source = f"\n        - Source: {config.source_label}" if config.source_label else ""
    return marker + textwrap.dedent(
        f"""\
        ## {provider} review did not produce a deliverable

        The optional {provider} AI review workflow failed before posting a usable review comment.

        - PR: #{config.pr_number}
        - Action run: {config.run_url}
        - Failure: `{truncate_text(detail, 1200)}`
        {source}

        This advisory review is non-blocking, but the missing AI deliverable should not be treated as review evidence.
        """
    ).strip()


def read_claude_execution_events(execution_file: Path) -> list[dict[str, object]]:
    if not execution_file:
        return []
    try:
        text = execution_file.read_text(encoding="utf-8").strip()
    except OSError:
        return []
    if not text:
        return []
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError:
        events: list[dict[str, object]] = []
        for line in text.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                parsed_line = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed_line, dict):
                events.append(parsed_line)
        return events
    if isinstance(parsed, dict):
        return [parsed]
    if isinstance(parsed, list):
        return [event for event in parsed if isinstance(event, dict)]
    return []


def claude_execution_failure_detail(*, execution_file: Path, step_outcome: str) -> str:
    if step_outcome and step_outcome != "success":
        return f"Claude action step outcome={step_outcome}"
    events = read_claude_execution_events(execution_file)
    if not events:
        return "Claude action completed without a parseable execution result"
    result = next(
        (event for event in reversed(events) if event.get("type") == "result"),
        None,
    )
    if not result:
        return "Claude action completed without a result event in the execution file"
    if result.get("is_error") is True:
        detail_parts = ["Claude execution result reported is_error=true"]
        for key in ("subtype", "num_turns", "total_cost_usd", "permission_denials_count"):
            if key in result:
                detail_parts.append(f"{key}={result[key]}")
        return "; ".join(detail_parts)
    return "Claude action completed without a visible PR comment or inline comment after this run started"


def ensure_claude_deliverable_or_notice(
    *,
    github: Any,
    execution_file: Path,
    step_outcome: str,
    config: FallbackConfig,
) -> str:
    if has_claude_visible_deliverable(
        github=github,
        started_at=config.started_at,
        deliverable_bot_logins=config.deliverable_bot_logins,
        output_contract=config.output_contract,
        deliverable_marker=config.comment_marker,
    ):
        return "existing-review-deliverable"
    detail = claude_execution_failure_detail(execution_file=execution_file, step_outcome=step_outcome)
    post_or_update_notice_comment(
        github=github,
        config=config,
        body=render_failure_notice(provider=config.provider, config=config, error=RuntimeError(detail)),
    )
    write_github_output("failure_notice_posted", "true")
    return "failure-notice-posted"


def post_split_review(*, github: Any, reviewer: Any, config: FallbackConfig) -> str:
    review_files = [ReviewFile.from_api_payload(payload) for payload in github.list_pr_files()]
    chunks = pack_review_chunks(review_files, max_chars=config.max_chunk_chars)
    if not chunks:
        marker = f"{config.comment_marker}\n\n" if config.comment_marker else ""
        warning_block = render_model_freshness_warning_block(model_freshness_warning_from_env())
        warning = f"{warning_block}\n\n" if warning_block else ""
        details = [f"- Action run: {config.run_url}"]
        if config.source_label:
            details.append(f"- Source: {config.source_label}")
        body = "\n".join(
            [
                f"{marker}{warning}## {config.provider} PR Review",
                "",
                "No reviewable file diff was available from the GitHub API.",
                "",
                *details,
                "",
            ]
        )
        post_or_update_marker_comment(
            github=github,
            config=config,
            body=body,
        )
        return "no-reviewable-diff"

    system_prompt = build_system_prompt(config.instructions, config.output_contract)
    responses = [
        reviewer.review_chunk(
            system_prompt=system_prompt,
            user_prompt=build_user_prompt(chunk, index, len(chunks)),
        )
        for index, chunk in enumerate(chunks, start=1)
    ]
    validate_review_responses(responses, config.output_contract)
    for comment in render_review_comments(config=config, chunks=chunks, responses=responses):
        post_or_update_marker_comment(github=github, config=config, body=comment)
    return "review-posted"


def render_notice(provider: str, pr_number: int, run_url: str, message: str) -> str:
    sanitized_message = sanitize_detail(message.strip())
    return textwrap.dedent(
        f"""\
        ## {provider} review notice

        {sanitized_message}

        - PR: #{pr_number}
        - Action run: {run_url}

        This advisory review is non-blocking, but the missing AI deliverable should not be treated as review evidence.
        """
    ).strip()


def run_fallback_review(*, github: Any, reviewer: Any, config: FallbackConfig) -> str:
    try:
        if has_review_deliverable(
            comments=github.list_issue_comments(),
            reviews=github.list_reviews(),
            started_at=config.started_at,
            markers=config.deliverable_markers,
            expected_bot_login=config.expected_bot_login,
            output_contract=config.output_contract,
            require_source_line=bool(config.source_label),
        ):
            return "existing-review-deliverable"

        result = post_split_review(github=github, reviewer=reviewer, config=config)
        return "fallback-posted" if result == "review-posted" else result
    except Exception as exc:
        try:
            post_or_update_notice_comment(
                github=github,
                config=config,
                body=render_failure_notice(provider=config.provider, config=config, error=exc),
            )
            write_github_output("failure_notice_posted", "true")
        finally:
            raise ReviewFailed(sanitize_detail(str(exc))) from None


def pr_agent_instructions(path: Path) -> str:
    parsed = tomllib.loads(path.read_text(encoding="utf-8"))
    reviewer = parsed.get("pr_reviewer")
    if not isinstance(reviewer, dict):
        raise RuntimeError(f"{path} missing [pr_reviewer]")
    instructions = reviewer.get("extra_instructions")
    if not isinstance(instructions, str) or not instructions.strip():
        raise RuntimeError(f"{path} missing pr_reviewer.extra_instructions")
    return instructions


def config_path_from_args(args: argparse.Namespace) -> Path:
    return Path(args.config_file or os.environ.get("AI_REVIEW_CONFIG", "") or DEFAULT_CONFIG_PATH)


def load_runtime_config(args: argparse.Namespace) -> dict[str, Any]:
    path = config_path_from_args(args)
    try:
        parsed = tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise RuntimeError(f"AI review config file is missing: {path}") from exc
    if not isinstance(parsed, dict):
        raise RuntimeError(f"AI review config file did not parse as a TOML table: {path}")
    return parsed


def config_table(config: dict[str, Any], name: str) -> dict[str, Any]:
    value = config.get(name)
    if not isinstance(value, dict):
        raise RuntimeError(f"AI review config missing [{name}]")
    return value


def nested_config_table(config: dict[str, Any], section: str, name: str) -> dict[str, Any]:
    parent = config_table(config, section)
    value = parent.get(name)
    if not isinstance(value, dict):
        raise RuntimeError(f"AI review config missing [{section}.{name}]")
    return value


def config_str(table: dict[str, Any], key: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"AI review config missing string key {key!r}")
    return value


def config_int(table: dict[str, Any], key: str) -> int:
    value = table.get(key)
    if not isinstance(value, int):
        raise RuntimeError(f"AI review config missing integer key {key!r}")
    return value


def config_float(table: dict[str, Any], key: str) -> float:
    value = table.get(key)
    if not isinstance(value, (int, float)):
        raise RuntimeError(f"AI review config missing numeric key {key!r}")
    return float(value)


def config_bool(table: dict[str, Any], key: str) -> bool:
    value = table.get(key)
    if not isinstance(value, bool):
        raise RuntimeError(f"AI review config missing boolean key {key!r}")
    return value


def config_str_tuple(table: dict[str, Any], key: str, *, allow_empty: bool = False) -> tuple[str, ...]:
    value = table.get(key)
    if value is None and allow_empty:
        return ()
    if (
        not isinstance(value, list)
        or (not value and not allow_empty)
        or not all(isinstance(item, str) and item for item in value)
    ):
        raise RuntimeError(f"AI review config missing string array key {key!r}")
    return tuple(value)


def review_output_contract(review_config: dict[str, Any]) -> ReviewOutputContract:
    contract = config_table(review_config, "output_contract")
    pr_agent_output = config_table(review_config, "pr_agent_output")
    return ReviewOutputContract(
        finding_required_labels=config_str_tuple(contract, "finding_required_labels"),
        finding_guidance=config_str_tuple(contract, "finding_guidance"),
        no_findings_indicator=config_str(contract, "no_findings_indicator"),
        no_findings_intro=config_str(contract, "no_findings_intro"),
        no_findings_required_labels=config_str_tuple(contract, "no_findings_required_labels"),
        no_findings_guidance=config_str_tuple(contract, "no_findings_guidance"),
        non_deliverable_indicators=config_str_tuple(contract, "non_deliverable_indicators"),
        pr_agent_deliverable_headings=config_str_tuple(pr_agent_output, "deliverable_headings"),
        pr_agent_disabled_noise=config_str_tuple(pr_agent_output, "disabled_noise", allow_empty=True),
    )


def source_label_from_template(table: dict[str, Any], *, model: str) -> str:
    template = config_str(table, "source_label_template")
    if "{model}" not in template:
        raise RuntimeError("source_label_template must include {model}")
    return template.replace("{model}", model)


def env_required(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise RuntimeError(f"{name} is required")
    return value


def int_env(name: str, default: int) -> int:
    raw = os.environ.get(name, "")
    if not raw:
        return default
    return int(raw)


def build_github_client(repo: str, pr_number: int, config: dict[str, Any]) -> GitHubClient:
    github_config = config_table(config, "github")
    return GitHubClient(
        repo=repo,
        pr_number=pr_number,
        token=env_required("GITHUB_TOKEN"),
        api_url=os.environ.get("GITHUB_API_URL", config_str(github_config, "api_url")),
    )


def run_url_for(repo: str, config: dict[str, Any]) -> str:
    github_config = config_table(config, "github")
    server_url = os.environ.get("GITHUB_SERVER_URL", config_str(github_config, "server_url"))
    return server_url + f"/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}"


def notice_marker_for_provider(runtime_config: dict[str, Any], provider: str) -> str:
    provider_key = provider.lower()
    if provider_key == "glm":
        return config_str(config_table(runtime_config, "glm"), "notice_marker")
    if provider_key == "kimi":
        return config_str(config_table(runtime_config, "kimi"), "notice_marker")
    if provider_key == "claude":
        return config_str(config_table(runtime_config, "claude"), "notice_marker")
    raise RuntimeError(f"unknown AI review provider {provider!r}")


def run_glm_fallback_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    review_config = config_table(runtime_config, "review")
    github_config = config_table(runtime_config, "github")
    glm_config = config_table(runtime_config, "glm")
    github = build_github_client(repo, pr_number, runtime_config)

    api_key = os.environ.get("GLM_API_KEY", "")
    comment_marker = config_str(glm_config, "comment_marker")
    model = os.environ.get("GLM_MODEL", config_str(glm_config, "model"))
    config = FallbackConfig(
        repo=repo,
        pr_number=pr_number,
        started_at=args.started_at,
        instructions=pr_agent_instructions(Path(args.instructions_file)),
        max_chunk_chars=int_env("GLM_REVIEW_MAX_CHUNK_CHARS", config_int(glm_config, "review_max_chunk_chars")),
        max_comment_chars=int_env("AI_REVIEW_MAX_COMMENT_CHARS", config_int(review_config, "max_comment_chars")),
        response_chars_per_chunk=config_int(review_config, "response_chars_per_chunk"),
        output_contract=review_output_contract(review_config),
        run_url=run_url_for(repo, runtime_config),
        provider="GLM",
        deliverable_markers=config_str_tuple(glm_config, "deliverable_markers"),
        expected_bot_login=config_str(github_config, "expected_bot_login"),
        comment_marker=comment_marker,
        notice_marker=config_str(glm_config, "notice_marker"),
        source_label=source_label_from_template(glm_config, model=model),
    )
    if not api_key:
        github.post_issue_comment_for(
            pr_number,
            render_notice(
                "GLM",
                pr_number,
                config.run_url,
                "`GLM_API_KEY` is not configured, so the GLM review was skipped.",
            ),
        )
        return 0

    reviewer = OpenAIChatClient(
        api_key=api_key,
        api_base=os.environ.get("GLM_API_BASE", config_str(glm_config, "api_base")),
        model=model,
        provider="GLM",
        temperature=config_float(glm_config, "temperature"),
        timeout_seconds=int_env("GLM_API_TIMEOUT_SECONDS", config_int(glm_config, "api_timeout_seconds")),
    )
    result = run_fallback_review(github=github, reviewer=reviewer, config=config)
    print(result)
    return 0


def run_glm_stamp_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github_config = config_table(runtime_config, "github")
    glm_config = config_table(runtime_config, "glm")
    pr_agent = nested_config_table(runtime_config, "glm", "pr_agent")
    github = build_github_client(repo, pr_number, runtime_config)
    result = stamp_existing_review_comment(
        github=github,
        started_at=args.started_at,
        markers=config_str_tuple(glm_config, "deliverable_markers"),
        expected_bot_login=config_str(github_config, "expected_bot_login"),
        marker=config_str(glm_config, "comment_marker"),
        source_label=source_label_from_template(pr_agent, model=config_str(pr_agent, "model")),
        run_url=run_url_for(repo, runtime_config),
    )
    print(result)
    return 0


def build_kimi_config_from_env(args: argparse.Namespace, *, repo: str, pr_number: int, review_intro: str = "") -> FallbackConfig:
    runtime_config = load_runtime_config(args)
    review_config = config_table(runtime_config, "review")
    github_config = config_table(runtime_config, "github")
    kimi_config = config_table(runtime_config, "kimi")
    marker = os.environ.get("KIMI_DELIVERABLE_MARKER", config_str(kimi_config, "deliverable_marker"))
    model = os.environ.get("KIMI_MODEL_NAME", config_str(kimi_config, "model"))
    return FallbackConfig(
        repo=repo,
        pr_number=pr_number,
        started_at=args.started_at,
        instructions=Path(args.instructions_file).read_text(encoding="utf-8"),
        max_chunk_chars=int_env("KIMI_REVIEW_MAX_CHUNK_CHARS", config_int(kimi_config, "review_max_chunk_chars")),
        max_comment_chars=int_env("AI_REVIEW_MAX_COMMENT_CHARS", config_int(review_config, "max_comment_chars")),
        response_chars_per_chunk=config_int(review_config, "response_chars_per_chunk"),
        output_contract=review_output_contract(review_config),
        run_url=run_url_for(repo, runtime_config),
        provider="Kimi",
        deliverable_markers=(marker,),
        expected_bot_login=config_str(github_config, "expected_bot_login"),
        comment_marker=marker,
        notice_marker=config_str(kimi_config, "notice_marker"),
        review_intro=review_intro,
        source_label=source_label_from_template(kimi_config, model=model),
    )


def build_kimi_reviewer_from_env(api_key: str, args: argparse.Namespace) -> KimiCliClient:
    kimi_config = config_table(load_runtime_config(args), "kimi")
    return KimiCliClient(
        api_key=api_key,
        api_base=os.environ.get("KIMI_MODEL_BASE_URL", config_str(kimi_config, "api_base")),
        model=os.environ.get("KIMI_MODEL_NAME", config_str(kimi_config, "model")),
        provider="Kimi",
        provider_type=os.environ.get("KIMI_MODEL_PROVIDER_TYPE", config_str(kimi_config, "provider_type")),
        model_max_context_size=int_env("KIMI_MODEL_MAX_CONTEXT_SIZE", config_int(kimi_config, "model_max_context_size")),
        default_thinking=os.environ.get("KIMI_MODEL_DEFAULT_THINKING", str(config_bool(kimi_config, "default_thinking")).lower()).lower()
        == "true",
        telemetry_disabled=os.environ.get("KIMI_DISABLE_TELEMETRY", "1" if config_bool(kimi_config, "telemetry_disabled") else "0")
        == "1",
        timeout_seconds=int_env("KIMI_CLI_TIMEOUT_SECONDS", config_int(kimi_config, "cli_timeout_seconds")),
        binary=os.environ.get("KIMI_CLI_BIN", "kimi"),
    )


def run_kimi_review_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github = build_github_client(repo, pr_number, runtime_config)
    api_key = env_required("KIMI_API_KEY")
    config = build_kimi_config_from_env(
        args,
        repo=repo,
        pr_number=pr_number,
        review_intro="The Kimi CLI reviewer reviewed the PR diff with Kimi.",
    )
    try:
        result = post_split_review(github=github, reviewer=build_kimi_reviewer_from_env(api_key, args), config=config)
    except Exception as exc:
        print(f"Kimi primary review failed: {sanitize_detail(truncate_text(str(exc), 1200))}", file=sys.stderr)
        return 1
    print(result)
    return 0


def run_kimi_fallback_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github = build_github_client(repo, pr_number, runtime_config)

    api_key = os.environ.get("KIMI_API_KEY", "")
    config = build_kimi_config_from_env(args, repo=repo, pr_number=pr_number)
    if not api_key:
        github.post_issue_comment_for(
            pr_number,
            render_notice(
                "Kimi",
                pr_number,
                config.run_url,
                "`KIMI_API_KEY` is not configured, so the Kimi review was skipped.",
            ),
        )
        return 0

    result = run_fallback_review(github=github, reviewer=build_kimi_reviewer_from_env(api_key, args), config=config)
    print(result)
    return 0


def run_claude_deliverable_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github_config = config_table(runtime_config, "github")
    review_config = config_table(runtime_config, "review")
    claude_config = config_table(runtime_config, "claude")
    github = build_github_client(repo, pr_number, runtime_config)
    model = config_str(claude_config, "model")
    config = FallbackConfig(
        repo=repo,
        pr_number=pr_number,
        started_at=args.started_at,
        instructions="",
        max_chunk_chars=0,
        max_comment_chars=config_int(review_config, "max_comment_chars"),
        response_chars_per_chunk=config_int(review_config, "response_chars_per_chunk"),
        output_contract=review_output_contract(review_config),
        run_url=run_url_for(repo, runtime_config),
        provider="Claude",
        deliverable_bot_logins=config_str_tuple(claude_config, "deliverable_bot_logins"),
        expected_bot_login=config_str(github_config, "expected_bot_login"),
        comment_marker=config_str(claude_config, "deliverable_marker"),
        notice_marker=config_str(claude_config, "notice_marker"),
        source_label=source_label_from_template(claude_config, model=model),
    )
    result = ensure_claude_deliverable_or_notice(
        github=github,
        execution_file=Path(args.execution_file) if args.execution_file else Path(),
        step_outcome=args.step_outcome,
        config=config,
    )
    print(result)
    return 0


def post_notice_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github = build_github_client(repo, pr_number, runtime_config)
    run_url = run_url_for(repo, runtime_config)
    github.post_issue_comment_for(pr_number, render_notice(args.provider, pr_number, run_url, args.message))
    return 0


def review_markers_for_provider(runtime_config: dict[str, Any], provider: str) -> tuple[str, ...]:
    provider_key = provider.lower()
    if provider_key == "glm":
        return config_str_tuple(config_table(runtime_config, "glm"), "deliverable_markers")
    if provider_key == "kimi":
        kimi_config = config_table(runtime_config, "kimi")
        return (config_str(kimi_config, "deliverable_marker"),)
    if provider_key == "claude":
        return ()
    raise RuntimeError(f"unknown AI review provider {provider!r}")


def run_retry_needed_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    review_config = config_table(runtime_config, "review")
    github_config = config_table(runtime_config, "github")
    github = build_github_client(repo, pr_number, runtime_config)
    provider_key = args.provider.lower()
    claude_config = config_table(runtime_config, "claude") if provider_key == "claude" else {}
    retry_needed, reason = provider_retry_decision(
        github=github,
        expected_bot_login=config_str(github_config, "expected_bot_login"),
        notice_marker=notice_marker_for_provider(runtime_config, args.provider),
        output_contract=review_output_contract(review_config),
        deliverable_markers=review_markers_for_provider(runtime_config, args.provider),
        deliverable_bot_logins=config_str_tuple(claude_config, "deliverable_bot_logins", allow_empty=True),
        claude_deliverable_marker=config_str(claude_config, "deliverable_marker") if provider_key == "claude" else "",
        require_source_line=provider_key in ("glm", "kimi"),
    )
    print(f"retry_needed={'true' if retry_needed else 'false'}")
    print(f"reason={reason}")
    return 0


def post_model_freshness_notice_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    runtime_config = load_runtime_config(args)
    github_config = config_table(runtime_config, "github")
    freshness_config = config_table(runtime_config, "model_freshness")
    github = build_github_client(repo, pr_number, runtime_config)
    run_url = run_url_for(repo, runtime_config)
    warning = args.warning or model_freshness_warning_from_env()
    expected_bot_login = config_str(github_config, "expected_bot_login")
    marker_template = config_str(freshness_config, "notice_marker_template")
    result = post_model_freshness_notice(
        github=github,
        provider=args.provider,
        pr_number=pr_number,
        run_url=run_url,
        warning=warning,
        expected_bot_login=expected_bot_login,
        marker_template=marker_template,
    )
    updated = 0
    if args.started_at and warning:
        updated = prepend_model_freshness_warning_to_existing_review(
            github=github,
            started_at=args.started_at,
            markers=review_markers_for_provider(runtime_config, args.provider),
            warning=warning,
            expected_bot_login=expected_bot_login,
        )
    print(f"{result}; review-comments-updated={updated}")


def run_notice_env(args: argparse.Namespace) -> int:
    runtime_config = load_runtime_config(args)
    github_config = config_table(runtime_config, "github")
    provider_config = config_table(runtime_config, args.provider)
    print("marker=" + shlex.quote(config_str(provider_config, "notice_marker")))
    print("expected_bot_login=" + shlex.quote(config_str(github_config, "expected_bot_login")))
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)

    fallback = subparsers.add_parser("glm-fallback")
    fallback.add_argument("--started-at", required=True)
    fallback.add_argument("--instructions-file", required=True)
    fallback.add_argument("--config-file", type=Path)

    glm_stamp = subparsers.add_parser("glm-stamp")
    glm_stamp.add_argument("--started-at", required=True)
    glm_stamp.add_argument("--config-file", type=Path)

    kimi_fallback = subparsers.add_parser("kimi-fallback")
    kimi_fallback.add_argument("--started-at", required=True)
    kimi_fallback.add_argument("--instructions-file", required=True)
    kimi_fallback.add_argument("--config-file", type=Path)

    kimi_review = subparsers.add_parser("kimi-review")
    kimi_review.add_argument("--started-at", required=True)
    kimi_review.add_argument("--instructions-file", required=True)
    kimi_review.add_argument("--config-file", type=Path)

    claude_deliverable = subparsers.add_parser("claude-deliverable")
    claude_deliverable.add_argument("--started-at", required=True)
    claude_deliverable.add_argument("--execution-file", default="")
    claude_deliverable.add_argument("--step-outcome", required=True)
    claude_deliverable.add_argument("--config-file", type=Path)

    notice = subparsers.add_parser("notice")
    notice.add_argument("--provider", required=True)
    notice.add_argument("--message", required=True)
    notice.add_argument("--config-file", type=Path)

    model_freshness_notice = subparsers.add_parser("model-freshness-notice")
    model_freshness_notice.add_argument("--provider", required=True)
    model_freshness_notice.add_argument("--started-at", default="")
    model_freshness_notice.add_argument("--warning", default="")
    model_freshness_notice.add_argument("--config-file", type=Path)

    notice_env = subparsers.add_parser("notice-env")
    notice_env.add_argument("--provider", required=True, choices=("glm", "kimi"))
    notice_env.add_argument("--config-file", type=Path)

    retry_needed = subparsers.add_parser("retry-needed")
    retry_needed.add_argument("--provider", required=True, choices=("glm", "kimi", "claude"))
    retry_needed.add_argument("--config-file", type=Path)

    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.mode == "glm-fallback":
        return run_glm_fallback_from_env(args)
    if args.mode == "glm-stamp":
        return run_glm_stamp_from_env(args)
    if args.mode == "kimi-fallback":
        return run_kimi_fallback_from_env(args)
    if args.mode == "kimi-review":
        return run_kimi_review_from_env(args)
    if args.mode == "claude-deliverable":
        return run_claude_deliverable_from_env(args)
    if args.mode == "notice":
        return post_notice_from_env(args)
    if args.mode == "model-freshness-notice":
        return post_model_freshness_notice_from_env(args)
    if args.mode == "notice-env":
        return run_notice_env(args)
    if args.mode == "retry-needed":
        return run_retry_needed_from_env(args)
    raise RuntimeError(f"unknown mode {args.mode!r}")


if __name__ == "__main__":
    raise SystemExit(main())
