#!/usr/bin/env python3
"""Self-tests for direct, non-fallback AI review clients."""

from __future__ import annotations

import json
import os
import pathlib
import tempfile
import tomllib

import direct_ai_review as owner
from direct_ai_review import EvidenceError, chunk_text, output_contract_text, read_review_diff, render_comment, review_transaction


def assert_chunks_preserve_all_input() -> None:
    text = "alpha\nbeta\ngamma\ndelta\n"
    chunks = chunk_text(text, 11)
    if "".join(chunks) != text:
        raise AssertionError(chunks)
    if not chunks or any(len(chunk) > 11 for chunk in chunks):
        raise AssertionError(chunks)


def assert_comment_binds_provider_model_and_head() -> None:
    body = render_comment(
        marker="<!-- marker -->",
        source="GLM Direct (`glm-5.2`)",
        head_sha="a" * 40,
        review="No hard-evidence findings",
    )
    required = (
        "<!-- marker -->",
        "**Source:** GLM Direct (`glm-5.2`)",
        f"**Head:** {'a' * 40}",
        "No hard-evidence findings",
    )
    if any(value not in body for value in required):
        raise AssertionError(body)


def assert_invalid_limits_fail_closed() -> None:
    for limit in (0, -1):
        try:
            chunk_text("content", limit)
        except EvidenceError:
            continue
        raise AssertionError(f"invalid limit accepted: {limit}")


def assert_github_comment_timeout_reaches_http_boundary() -> None:
    observed: list[int] = []
    original_urlopen = owner.urllib.request.urlopen

    class Response:
        def __enter__(self) -> "Response":
            return self

        def __exit__(self, *args: object) -> None:
            return None

        def read(self) -> bytes:
            return b""

    def fake_urlopen(request: object, *, timeout: int) -> Response:
        observed.append(timeout)
        return Response()

    try:
        owner.urllib.request.urlopen = fake_urlopen
        owner.post_comment("owner/repo", "1457", "token", "body", "https://api.github.invalid", 17)
    finally:
        owner.urllib.request.urlopen = original_urlopen
    if observed != [17]:
        raise AssertionError(observed)


def assert_diff_is_bound_to_exact_base_and_head() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "review.diff"
        path.write_text("diff --git a/a b/a\n", encoding="utf-8")
        diff = read_review_diff(path, "1" * 40, "2" * 40)
        if diff != "diff --git a/a b/a\n":
            raise AssertionError(diff)


def assert_diff_rejects_invalid_identity() -> None:
    for base_sha, head_sha in (("", "2" * 40), ("1" * 40, "short")):
        try:
            read_review_diff(pathlib.Path("unused.diff"), base_sha, head_sha)
        except EvidenceError:
            continue
        raise AssertionError((base_sha, head_sha))


def assert_output_contract_is_consumed() -> None:
    rendered = output_contract_text(
        {
            "finding_required_labels": ["Severity:", "Evidence:"],
            "no_findings_indicator": "No hard-evidence findings",
        }
    )
    for required in ("Severity:", "Evidence:", "No hard-evidence findings"):
        if required not in rendered:
            raise AssertionError(rendered)


def assert_every_provider_uses_one_deliverable_marker_schema() -> None:
    config = tomllib.loads(pathlib.Path(__file__).with_name("review-config").read_text(encoding="utf-8"))
    for provider in ("claude", "kimi", "glm"):
        if not config[provider].get("deliverable_marker"):
            raise AssertionError(f"{provider} does not use the shared deliverable_marker schema")
        if "comment_marker" in config[provider]:
            raise AssertionError(f"{provider} retains alternate comment_marker schema")


def assert_large_diff_produces_one_synthesized_review() -> None:
    calls: list[str] = []

    def reviewer(prompt: str) -> str:
        calls.append(prompt)
        if "# Final synthesis" in prompt:
            if "chunk-one-finding" not in prompt or "chunk-two-finding" not in prompt:
                raise AssertionError(prompt)
            return "one final review"
        return "chunk-one-finding" if len(calls) == 1 else "chunk-two-finding"

    review = review_transaction(
        ("first diff chunk", "second diff chunk"),
        instructions="review instructions",
        reviewer=reviewer,
    )
    if review != "one final review" or len(calls) != 3:
        raise AssertionError((review, calls))


def assert_claude_execution_publishes_once_or_not_at_all() -> None:
    calls: list[tuple[str, str, str, str, str, int]] = []

    def publisher(repo: str, pr_number: str, token: str, body: str, api_url: str, timeout_seconds: int) -> None:
        calls.append((repo, pr_number, token, body, api_url, timeout_seconds))

    with tempfile.TemporaryDirectory() as tmp:
        execution = pathlib.Path(tmp) / "claude-execution.json"
        execution.write_text(
            json.dumps([{"type": "result", "subtype": "success", "is_error": False, "result": "final Claude review"}]),
            encoding="utf-8",
        )
        owner.publish_claude_execution(
            execution,
            provider_config={
                "model": "configured-model",
                "deliverable_marker": "<!-- claude-marker -->",
                "source_label_template": "Claude (`{model}`)",
            },
            review_config={"max_comment_chars": 60000},
            repo="owner/repo",
            pr_number="1457",
            head_sha="a" * 40,
            token="publisher-token",
            api_url="https://api.github.com",
            api_timeout_seconds=17,
            publisher=publisher,
        )
        if len(calls) != 1 or "final Claude review" not in calls[0][3] or "<!-- claude-marker -->" not in calls[0][3]:
            raise AssertionError(calls)
        for payload in ([], [{"type": "result", "subtype": "error", "is_error": True, "result": "partial"}]):
            calls.clear()
            execution.write_text(json.dumps(payload), encoding="utf-8")
            try:
                owner.publish_claude_execution(
                    execution,
                    provider_config={
                        "model": "configured-model",
                        "deliverable_marker": "<!-- claude-marker -->",
                        "source_label_template": "Claude (`{model}`)",
                    },
                    review_config={"max_comment_chars": 60000},
                    repo="owner/repo",
                    pr_number="1457",
                    head_sha="a" * 40,
                    token="publisher-token",
                    api_url="https://api.github.com",
                    api_timeout_seconds=17,
                    publisher=publisher,
                )
            except EvidenceError:
                pass
            else:
                raise AssertionError(f"invalid Claude execution was published: {payload!r}")
            if calls:
                raise AssertionError(calls)


def assert_configured_adapter_transaction_publishes_once_after_success_and_never_after_failure() -> None:
    calls: list[tuple[str, str, str, str, str, int]] = []
    original_review = owner.openai_chat_review
    required_environment = {
        "GITHUB_REPOSITORY": "owner/repo",
        "PR_NUMBER": "1457",
        "PR_BASE_SHA": "1" * 40,
        "PR_HEAD_SHA": "2" * 40,
        "PR_DIFF_PATH": "",
        "GITHUB_TOKEN": "publisher-token",
        "REVIEW_PROVIDER_TOKEN": "provider-token",
    }
    saved_environment = {key: os.environ.get(key) for key in required_environment}

    def publisher(repo: str, pr_number: str, token: str, body: str, api_url: str, timeout_seconds: int) -> None:
        calls.append((repo, pr_number, token, body, api_url, timeout_seconds))

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        diff = root / "review.diff"
        diff.write_text("diff --git a/a b/a\n", encoding="utf-8")
        instructions = root / "instructions.md"
        instructions.write_text("review instructions", encoding="utf-8")
        config = root / "ai-review.toml"
        config.write_text(
            """
[github]
api_url = "https://api.github.com"
comment_timeout_seconds = 17
[review]
max_comment_chars = 60000
[review.output_contract]
finding_required_labels = ["Evidence:"]
[reviewer]
model = "configured-model"
deliverable_marker = "<!-- kimi-marker -->"
source_label_template = "Kimi (`{model}`)"
review_max_chunk_chars = 60000
adapter = "openai_chat"
secret_env = "REVIEW_PROVIDER_TOKEN"
api_base = "https://example.invalid"
api_timeout_seconds = 30
""".strip(),
            encoding="utf-8",
        )
        required_environment["PR_DIFF_PATH"] = str(diff)
        try:
            os.environ.update(required_environment)
            owner.openai_chat_review = lambda prompt, provider_config, api_key: "final review"
            owner.run("reviewer", instructions, config, publisher=publisher)
            if len(calls) != 1 or "final review" not in calls[0][3]:
                raise AssertionError(calls)
            calls.clear()

            def fail_review(prompt: str, provider_config: object, api_key: str) -> str:
                raise EvidenceError("analysis failed")

            owner.openai_chat_review = fail_review
            try:
                owner.run("reviewer", instructions, config, publisher=publisher)
            except EvidenceError:
                pass
            else:
                raise AssertionError("failed Kimi analysis returned success")
            if calls:
                raise AssertionError(calls)
        finally:
            owner.openai_chat_review = original_review
            for key, value in saved_environment.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value


def main() -> int:
    assert_chunks_preserve_all_input()
    assert_comment_binds_provider_model_and_head()
    assert_invalid_limits_fail_closed()
    assert_github_comment_timeout_reaches_http_boundary()
    assert_diff_is_bound_to_exact_base_and_head()
    assert_diff_rejects_invalid_identity()
    assert_output_contract_is_consumed()
    assert_every_provider_uses_one_deliverable_marker_schema()
    assert_large_diff_produces_one_synthesized_review()
    assert_claude_execution_publishes_once_or_not_at_all()
    assert_configured_adapter_transaction_publishes_once_after_success_and_never_after_failure()
    print("OK: direct AI review tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
