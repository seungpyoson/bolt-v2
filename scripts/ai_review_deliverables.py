#!/usr/bin/env python3
"""Post AI review deliverables and failure notices for optional PR reviewers."""

from __future__ import annotations

import argparse
import json
import os
import sys
import textwrap
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


DEFAULT_GITHUB_API = "https://api.github.com"
DEFAULT_GLM_API_BASE = "https://api.z.ai/api/coding/paas/v4"
DEFAULT_GLM_MODEL = "glm-5.2"
DEFAULT_KIMI_API_BASE = "https://api.kimi.com/coding/v1"
DEFAULT_KIMI_MODEL = "kimi-for-coding"
DEFAULT_MAX_CHUNK_CHARS = 60000
DEFAULT_MAX_COMMENT_CHARS = 60000
DEFAULT_RESPONSE_CHARS_PER_CHUNK = 8000
PR_AGENT_MARKERS = (
    "## PR Reviewer Guide",
    "## Incremental PR Reviewer Guide",
)
EXPLICIT_SECRET_ENV_NAMES = frozenset(("GLM_API_KEY", "KIMI_API_KEY", "GITHUB_TOKEN", "OPENAI__KEY", "OPENAI_KEY"))
SECRET_ENV_SUFFIXES = ("_API_KEY", "_KEY", "_TOKEN")
SECRET_ENV_PREFIXES = ("OPENAI__",)


class ReviewFailed(RuntimeError):
    """Raised after a visible failure notice has been posted."""


@dataclass(frozen=True)
class FallbackConfig:
    repo: str
    pr_number: int
    started_at: str
    instructions: str
    max_chunk_chars: int
    run_url: str
    max_comment_chars: int = DEFAULT_MAX_COMMENT_CHARS
    response_chars_per_chunk: int = DEFAULT_RESPONSE_CHARS_PER_CHUNK
    provider: str = "GLM"
    deliverable_markers: tuple[str, ...] = PR_AGENT_MARKERS
    expected_bot_login: str = "github-actions[bot]"


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
        api_url: str = DEFAULT_GITHUB_API,
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

    def post_issue_comment_for(self, pr_number: int, body: str) -> None:
        self._request_json("POST", f"issues/{pr_number}/comments", payload={"body": body})

    def post_issue_comment(self, body: str) -> None:
        self.post_issue_comment_for(self.pr_number, body)


class OpenAIChatClient:
    def __init__(
        self,
        *,
        api_key: str,
        api_base: str,
        model: str,
        provider: str,
        timeout_seconds: int = 180,
    ) -> None:
        self.api_key = api_key
        self.api_base = api_base.rstrip("/")
        self.model = model
        self.provider = provider
        self.timeout_seconds = timeout_seconds

    def review_chunk(self, *, system_prompt: str, user_prompt: str) -> str:
        payload = {
            "model": self.model,
            "temperature": 0.2,
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
) -> bool:
    threshold = parse_iso_timestamp(started_at)
    for comment in comments:
        if not actor_is_expected_bot(comment, expected_bot_login):
            continue
        if not body_has_deliverable_marker(comment.get("body"), markers):
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
        if text_time_is_after_or_equal(review.get("submitted_at"), threshold):
            return True
    return False


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


def build_system_prompt(instructions: str) -> str:
    return textwrap.dedent(
        f"""\
        You are conducting an advisory pull request review for bolt-v2.

        Use only hard evidence from the supplied chunk. Report actionable findings only.
        Quote the smallest relevant evidence snippet or line reference before explaining the implication.
        If this chunk contains no hard-evidence findings, say exactly:
        No hard-evidence findings in this chunk.

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


def write_github_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT", "")
    if not output_path:
        return
    with Path(output_path).open("a", encoding="utf-8") as output:
        output.write(f"{name}={value}\n")


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
    parts = [
        title,
        "",
        f"The primary {config.provider} review action completed without a visible deliverable after this run started, "
        f"so the fallback reviewer split the PR diff and reviewed each chunk with {config.provider}.",
        "",
        f"- Review chunks: {total_chunks}",
        f"- Per-chunk character budget: {config.max_chunk_chars}",
        f"- Action run: {config.run_url}",
        "",
    ]
    if part_index is not None and part_total is not None:
        parts.insert(6, f"- Comment part: {part_index}/{part_total}")
    parts.extend(sections)
    return "\n".join(parts).strip() + "\n"


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
    single_comment = render_review_comment_body(config=config, total_chunks=len(chunks), sections=sections)
    if len(single_comment) <= config.max_comment_chars:
        return [single_comment]

    packed_sections: list[list[str]] = []
    current_sections: list[str] = []
    max_possible_parts = max(1, len(sections))
    for section in sections:
        candidate_sections = [*current_sections, section]
        candidate = render_review_comment_body(
            config=config,
            total_chunks=len(chunks),
            sections=candidate_sections,
            part_index=len(packed_sections) + 1,
            part_total=max_possible_parts,
        )
        if current_sections and len(candidate) > config.max_comment_chars:
            packed_sections.append(current_sections)
            current_sections = [section]
            continue
        current_sections = candidate_sections

    if current_sections:
        packed_sections.append(current_sections)

    return [
        truncate_text(
            render_review_comment_body(
                config=config,
                total_chunks=len(chunks),
                sections=page_sections,
                part_index=idx,
                part_total=len(packed_sections),
            ),
            config.max_comment_chars,
        )
        for idx, page_sections in enumerate(packed_sections, start=1)
    ]


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
    return textwrap.dedent(
        f"""\
        ## {provider} review did not produce a deliverable

        The optional {provider} AI review workflow failed before posting a usable review comment.

        - PR: #{config.pr_number}
        - Action run: {config.run_url}
        - Failure: `{truncate_text(detail, 1200)}`

        This advisory review is non-blocking, but the missing AI deliverable should not be treated as review evidence.
        """
    ).strip()


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
        ):
            return "existing-review-deliverable"

        review_files = [ReviewFile.from_api_payload(payload) for payload in github.list_pr_files()]
        chunks = pack_review_chunks(review_files, max_chars=config.max_chunk_chars)
        if not chunks:
            github.post_issue_comment(
                f"## {config.provider} PR Review\n\nNo reviewable file diff was available from the GitHub API.\n"
            )
            return "no-reviewable-diff"

        system_prompt = build_system_prompt(config.instructions)
        responses = [
            reviewer.review_chunk(
                system_prompt=system_prompt,
                user_prompt=build_user_prompt(chunk, index, len(chunks)),
            )
            for index, chunk in enumerate(chunks, start=1)
        ]
        for comment in render_review_comments(config=config, chunks=chunks, responses=responses):
            github.post_issue_comment(comment)
        return "fallback-posted"
    except Exception as exc:
        try:
            github.post_issue_comment(render_failure_notice(provider=config.provider, config=config, error=exc))
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


def build_github_client(repo: str, pr_number: int) -> GitHubClient:
    return GitHubClient(
        repo=repo,
        pr_number=pr_number,
        token=env_required("GITHUB_TOKEN"),
        api_url=os.environ.get("GITHUB_API_URL", DEFAULT_GITHUB_API),
    )


def run_glm_fallback_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    github = build_github_client(repo, pr_number)

    api_key = os.environ.get("GLM_API_KEY", "")
    config = FallbackConfig(
        repo=repo,
        pr_number=pr_number,
        started_at=args.started_at,
        instructions=pr_agent_instructions(Path(args.instructions_file)),
        max_chunk_chars=int_env("GLM_REVIEW_MAX_CHUNK_CHARS", DEFAULT_MAX_CHUNK_CHARS),
        max_comment_chars=int_env("AI_REVIEW_MAX_COMMENT_CHARS", DEFAULT_MAX_COMMENT_CHARS),
        run_url=os.environ.get("GITHUB_SERVER_URL", "https://github.com")
        + f"/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}",
        provider="GLM",
        deliverable_markers=PR_AGENT_MARKERS,
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
        api_base=os.environ.get("GLM_API_BASE", DEFAULT_GLM_API_BASE),
        model=os.environ.get("GLM_MODEL", DEFAULT_GLM_MODEL),
        provider="GLM",
        timeout_seconds=int_env("GLM_API_TIMEOUT_SECONDS", 180),
    )
    result = run_fallback_review(github=github, reviewer=reviewer, config=config)
    print(result)
    return 0


def run_kimi_fallback_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    github = build_github_client(repo, pr_number)

    api_key = os.environ.get("KIMI_API_KEY", "")
    config = FallbackConfig(
        repo=repo,
        pr_number=pr_number,
        started_at=args.started_at,
        instructions=Path(args.instructions_file).read_text(encoding="utf-8"),
        max_chunk_chars=int_env("KIMI_REVIEW_MAX_CHUNK_CHARS", DEFAULT_MAX_CHUNK_CHARS),
        max_comment_chars=int_env("AI_REVIEW_MAX_COMMENT_CHARS", DEFAULT_MAX_COMMENT_CHARS),
        run_url=os.environ.get("GITHUB_SERVER_URL", "https://github.com")
        + f"/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}",
        provider="Kimi",
        deliverable_markers=(os.environ.get("KIMI_DELIVERABLE_MARKER", "<!-- ai-pr-reviewer-kimi -->"),),
    )
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

    reviewer = OpenAIChatClient(
        api_key=api_key,
        api_base=os.environ.get("KIMI_API_BASE", DEFAULT_KIMI_API_BASE),
        model=os.environ.get("KIMI_MODEL", DEFAULT_KIMI_MODEL),
        provider="Kimi",
        timeout_seconds=int_env("KIMI_API_TIMEOUT_SECONDS", 180),
    )
    result = run_fallback_review(github=github, reviewer=reviewer, config=config)
    print(result)
    return 0


def post_notice_from_env(args: argparse.Namespace) -> int:
    repo = env_required("GITHUB_REPOSITORY")
    pr_number = int(env_required("PR_NUMBER"))
    github = build_github_client(repo, pr_number)
    run_url = os.environ.get("GITHUB_SERVER_URL", "https://github.com") + f"/{repo}/actions/runs/{os.environ.get('GITHUB_RUN_ID', '')}"
    github.post_issue_comment_for(pr_number, render_notice(args.provider, pr_number, run_url, args.message))
    return 0


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)

    fallback = subparsers.add_parser("glm-fallback")
    fallback.add_argument("--started-at", required=True)
    fallback.add_argument("--instructions-file", required=True)

    kimi_fallback = subparsers.add_parser("kimi-fallback")
    kimi_fallback.add_argument("--started-at", required=True)
    kimi_fallback.add_argument("--instructions-file", required=True)

    notice = subparsers.add_parser("notice")
    notice.add_argument("--provider", required=True)
    notice.add_argument("--message", required=True)

    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.mode == "glm-fallback":
        return run_glm_fallback_from_env(args)
    if args.mode == "kimi-fallback":
        return run_kimi_fallback_from_env(args)
    if args.mode == "notice":
        return post_notice_from_env(args)
    raise RuntimeError(f"unknown mode {args.mode!r}")


if __name__ == "__main__":
    raise SystemExit(main())
