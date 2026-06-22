#!/usr/bin/env python3
"""Self-tests for the GLM AI-review fallback."""

from __future__ import annotations

import importlib.util
import os
import pathlib
import sys
import tempfile
from dataclasses import dataclass


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "ai_review_deliverables.py"


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("ai_review_deliverables", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load ai_review_deliverables.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@dataclass
class FakeGitHub:
    files: list[dict[str, object]]
    issue_comments: list[dict[str, object]] | None = None
    reviews: list[dict[str, object]] | None = None

    def __post_init__(self) -> None:
        self.issue_comments = list(self.issue_comments or [])
        self.reviews = list(self.reviews or [])
        self.posted: list[str] = []

    def list_pr_files(self) -> list[dict[str, object]]:
        return list(self.files)

    def list_issue_comments(self) -> list[dict[str, object]]:
        return list(self.issue_comments or [])

    def list_reviews(self) -> list[dict[str, object]]:
        return list(self.reviews or [])

    def post_issue_comment(self, body: str) -> None:
        self.posted.append(body)


class FakeGLM:
    def __init__(self, *, fail: bool = False, response: str | None = None, failure_message: str | None = None) -> None:
        self.fail = fail
        self.failure_message = failure_message or "provider rejected request"
        self.response = response or "Findings\n\nNo hard-evidence findings in this chunk."
        self.prompts: list[str] = []

    def review_chunk(self, *, system_prompt: str, user_prompt: str) -> str:
        del system_prompt
        self.prompts.append(user_prompt)
        if self.fail:
            raise RuntimeError(self.failure_message)
        return self.response


class FakeProvider(FakeGLM):
    pass


def file_payload(name: str, patch: str) -> dict[str, object]:
    return {
        "filename": name,
        "status": "modified",
        "additions": 1,
        "deletions": 0,
        "changes": 1,
        "patch": patch,
    }


def test_packs_more_than_two_review_chunks_when_budget_requires_it() -> None:
    module = load_script()
    files = [
        module.ReviewFile.from_api_payload(file_payload(f"src/file_{idx}.rs", "+" + ("x" * 80)))
        for idx in range(7)
    ]

    chunks = module.pack_review_chunks(files, max_chars=260)

    assert len(chunks) >= 4, [chunk.title for chunk in chunks]
    assert all(len(chunk.body) <= 260 for chunk in chunks), [len(chunk.body) for chunk in chunks]
    flattened = "\n".join(chunk.body for chunk in chunks)
    for idx in range(7):
        assert f"src/file_{idx}.rs" in flattened


def test_splits_one_oversized_file_patch_into_multiple_review_chunks() -> None:
    module = load_script()
    patch = "\n".join(f"+line {idx} {'x' * 40}" for idx in range(12))
    files = [module.ReviewFile.from_api_payload(file_payload("src/huge.rs", patch))]

    chunks = module.pack_review_chunks(files, max_chars=260)

    assert len(chunks) > 1, [chunk.title for chunk in chunks]
    assert all("src/huge.rs" in chunk.body for chunk in chunks)
    assert all(len(chunk.body) <= 260 for chunk in chunks), [len(chunk.body) for chunk in chunks]
    assert any("part 2" in chunk.title for chunk in chunks), [chunk.title for chunk in chunks]
    flattened = "\n".join(chunk.body for chunk in chunks)
    assert "+line 0 " in flattened
    assert "+line 11 " in flattened


def test_truncated_file_fragment_keeps_markdown_fence_closed() -> None:
    module = load_script()
    review_file = module.ReviewFile.from_api_payload(
        file_payload("src/huge.rs", "+" + ("x" * 200))
    )

    chunk = module.render_file_fragment(
        review_file,
        ["+" + ("x" * 200)],
        title="src/huge.rs",
        max_chars=120,
    )

    assert len(chunk.body) <= 120
    assert "\n```\n\n[fragment truncated to fit review budget]\n" in chunk.body
    assert chunk.body.count("```") % 2 == 0, chunk.body


def test_skips_fallback_when_pr_agent_deliverable_exists_after_start() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "## PR Reviewer Guide\n\nexisting PR-Agent result",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM()

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "existing-review-deliverable"
    assert glm.prompts == []
    assert github.posted == []


def test_plain_pr_agent_phrase_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "Someone mentioned PR Reviewer Guide in conversation.",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
            }
        ],
    )
    glm = FakeGLM()

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert glm.prompts
    assert len(github.posted) == 1, github.posted


def test_human_pr_agent_marker_comment_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "## PR Reviewer Guide\n\ncopied into a human discussion comment",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "User", "login": "reviewer"},
            }
        ],
    )
    glm = FakeGLM()

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert glm.prompts
    assert len(github.posted) == 1, github.posted


def test_human_kimi_marker_comment_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "<!-- ai-pr-reviewer-kimi -->\n\ncopied into a human discussion comment",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "User", "login": "reviewer"},
            }
        ],
    )
    kimi = FakeProvider()

    result = module.run_fallback_review(
        github=github,
        reviewer=kimi,
        config=module.FallbackConfig(
            provider="Kimi",
            deliverable_markers=("<!-- ai-pr-reviewer-kimi -->",),
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert kimi.prompts
    assert len(github.posted) == 1, github.posted


def test_unexpected_bot_marker_comment_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "## PR Reviewer Guide\n\nmarker from a different automation bot",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "dependabot[bot]"},
            }
        ],
    )
    glm = FakeGLM()

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert glm.prompts
    assert len(github.posted) == 1, github.posted


def test_incremental_pr_agent_deliverable_suppresses_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "## Incremental PR Reviewer Guide\n\nexisting incremental PR-Agent result",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM()

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "existing-review-deliverable"
    assert glm.prompts == []
    assert github.posted == []


def test_posts_failure_notice_when_glm_fallback_fails() -> None:
    module = load_script()
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(fail=True)

    try:
        module.run_fallback_review(
            github=github,
            reviewer=glm,
            config=module.FallbackConfig(
                repo="seungpyoson/bolt-v2",
                pr_number=895,
                started_at="2026-06-22T12:21:00Z",
                instructions="review hard evidence only",
                max_chunk_chars=260,
                run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
            ),
        )
    except module.ReviewFailed:
        pass
    else:
        raise AssertionError("expected ReviewFailed")

    assert len(github.posted) == 1, github.posted
    assert "GLM review did not produce a deliverable" in github.posted[0]
    assert "provider rejected request" in github.posted[0]
    assert "GLM_API_KEY" not in github.posted[0]


def test_sets_failure_notice_output_after_posting_failure_notice() -> None:
    module = load_script()
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(fail=True)

    with tempfile.NamedTemporaryFile() as output_file:
        previous_output = os.environ.get("GITHUB_OUTPUT")
        os.environ["GITHUB_OUTPUT"] = output_file.name
        try:
            try:
                module.run_fallback_review(
                    github=github,
                    reviewer=glm,
                    config=module.FallbackConfig(
                        repo="seungpyoson/bolt-v2",
                        pr_number=895,
                        started_at="2026-06-22T12:21:00Z",
                        instructions="review hard evidence only",
                        max_chunk_chars=260,
                        run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
                    ),
                )
            except module.ReviewFailed:
                pass
            else:
                raise AssertionError("expected ReviewFailed")
        finally:
            if previous_output is None:
                os.environ.pop("GITHUB_OUTPUT", None)
            else:
                os.environ["GITHUB_OUTPUT"] = previous_output
        output_file.seek(0)
        output = output_file.read().decode("utf-8")

    assert "failure_notice_posted=true" in output


def test_kimi_fallback_uses_same_chunked_deliverable_contract() -> None:
    module = load_script()
    files = [file_payload(f"src/kimi_{idx}.rs", "+" + ("k" * 80)) for idx in range(5)]
    github = FakeGitHub(files=files)
    kimi = FakeProvider()

    result = module.run_fallback_review(
        github=github,
        reviewer=kimi,
        config=module.FallbackConfig(
            provider="Kimi",
            deliverable_markers=("## Kimi PR Review", "Kimi Misospace"),
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert len(kimi.prompts) >= 3, len(kimi.prompts)
    assert len(github.posted) == 1, github.posted
    assert "## Kimi PR Review" in github.posted[0]
    assert "Review chunks:" in github.posted[0]


def test_posts_fallback_review_across_multiple_comments_when_comment_budget_requires_it() -> None:
    module = load_script()
    files = [file_payload(f"src/file_{idx}.rs", "+" + ("x" * 80)) for idx in range(7)]
    github = FakeGitHub(files=files)
    glm = FakeGLM(response="Findings\n\n" + ("hard evidence finding. " * 12))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            max_comment_chars=520,
            response_chars_per_chunk=220,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert len(github.posted) > 1, github.posted
    assert all(len(body) <= 520 for body in github.posted), [len(body) for body in github.posted]
    assert all("## GLM PR Review (part " in body for body in github.posted), github.posted
    joined = "\n".join(github.posted)
    assert "Chunk 1/" in joined
    assert f"Chunk {len(glm.prompts)}/" in joined


def test_large_model_response_is_split_without_truncating_content() -> None:
    module = load_script()
    response = "Findings\n\n" + ("X" * 20000)
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(response=response)

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            max_comment_chars=60000,
            response_chars_per_chunk=8000,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    joined = "\n".join(github.posted)
    assert joined.count("X") == 20000
    assert "[truncated to fit GitHub comment limit]" not in joined
    assert "response part 1/3" in joined
    assert "response part 3/3" in joined


def test_split_model_response_keeps_markdown_fences_balanced() -> None:
    module = load_script()
    response = "```python\n" + "\n".join(f"print({idx})" for idx in range(60)) + "\n```"
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(response=response)

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=module.FallbackConfig(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            started_at="2026-06-22T12:21:00Z",
            instructions="review hard evidence only",
            max_chunk_chars=260,
            max_comment_chars=60000,
            response_chars_per_chunk=160,
            run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        ),
    )

    assert result == "fallback-posted"
    assert len(github.posted) == 1, github.posted
    assert github.posted[0].count("```") % 2 == 0, github.posted[0]
    assert "[truncated to fit GitHub comment limit]" not in github.posted[0]


def test_posts_failure_notice_when_kimi_fallback_fails() -> None:
    module = load_script()
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    kimi = FakeProvider(fail=True)

    try:
        module.run_fallback_review(
            github=github,
            reviewer=kimi,
            config=module.FallbackConfig(
                provider="Kimi",
                deliverable_markers=("## Kimi PR Review", "Kimi Misospace"),
                repo="seungpyoson/bolt-v2",
                pr_number=895,
                started_at="2026-06-22T12:21:00Z",
                instructions="review hard evidence only",
                max_chunk_chars=260,
                run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
            ),
        )
    except module.ReviewFailed:
        pass
    else:
        raise AssertionError("expected ReviewFailed")

    assert len(github.posted) == 1, github.posted
    assert "Kimi review did not produce a deliverable" in github.posted[0]
    assert "provider rejected request" in github.posted[0]


def test_redacts_kimi_api_key_from_failure_notice() -> None:
    module = load_script()
    secret = "fake-kimi-secret-for-redaction"
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    kimi = FakeProvider(fail=True, failure_message=f"provider echoed {secret}")
    previous_secret = os.environ.get("KIMI_API_KEY")
    os.environ["KIMI_API_KEY"] = secret

    try:
        try:
            module.run_fallback_review(
                github=github,
                reviewer=kimi,
                config=module.FallbackConfig(
                    provider="Kimi",
                    deliverable_markers=("## Kimi PR Review", "Kimi Misospace"),
                    repo="seungpyoson/bolt-v2",
                    pr_number=895,
                    started_at="2026-06-22T12:21:00Z",
                    instructions="review hard evidence only",
                    max_chunk_chars=260,
                    run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
                ),
            )
        except module.ReviewFailed:
            pass
        else:
            raise AssertionError("expected ReviewFailed")
    finally:
        if previous_secret is None:
            os.environ.pop("KIMI_API_KEY", None)
        else:
            os.environ["KIMI_API_KEY"] = previous_secret

    assert len(github.posted) == 1, github.posted
    assert secret not in github.posted[0]
    assert "provider echoed ***" in github.posted[0]


def test_render_notice_redacts_secret_values() -> None:
    module = load_script()
    secret = "fake-kimi-secret-for-notice-redaction"
    previous_secret = os.environ.get("KIMI_API_KEY")
    os.environ["KIMI_API_KEY"] = secret
    try:
        notice = module.render_notice(
            "Kimi",
            895,
            "https://github.com/seungpyoson/bolt-v2/actions/runs/1",
            f"provider returned {secret}",
        )
    finally:
        if previous_secret is None:
            os.environ.pop("KIMI_API_KEY", None)
        else:
            os.environ["KIMI_API_KEY"] = previous_secret

    assert secret not in notice
    assert "provider returned ***" in notice


def test_render_notice_redacts_new_provider_secret_env_names() -> None:
    module = load_script()
    secret = "fake-new-provider-secret-for-redaction"
    previous_secret = os.environ.get("EXPERIMENTAL_PROVIDER_API_KEY")
    os.environ["EXPERIMENTAL_PROVIDER_API_KEY"] = secret
    try:
        notice = module.render_notice(
            "Provider",
            895,
            "https://github.com/seungpyoson/bolt-v2/actions/runs/1",
            f"provider returned {secret}",
        )
    finally:
        if previous_secret is None:
            os.environ.pop("EXPERIMENTAL_PROVIDER_API_KEY", None)
        else:
            os.environ["EXPERIMENTAL_PROVIDER_API_KEY"] = previous_secret

    assert secret not in notice
    assert "provider returned ***" in notice


def main() -> int:
    test_packs_more_than_two_review_chunks_when_budget_requires_it()
    test_splits_one_oversized_file_patch_into_multiple_review_chunks()
    test_truncated_file_fragment_keeps_markdown_fence_closed()
    test_skips_fallback_when_pr_agent_deliverable_exists_after_start()
    test_plain_pr_agent_phrase_does_not_suppress_fallback()
    test_human_pr_agent_marker_comment_does_not_suppress_fallback()
    test_human_kimi_marker_comment_does_not_suppress_fallback()
    test_unexpected_bot_marker_comment_does_not_suppress_fallback()
    test_incremental_pr_agent_deliverable_suppresses_fallback()
    test_posts_failure_notice_when_glm_fallback_fails()
    test_sets_failure_notice_output_after_posting_failure_notice()
    test_kimi_fallback_uses_same_chunked_deliverable_contract()
    test_posts_fallback_review_across_multiple_comments_when_comment_budget_requires_it()
    test_large_model_response_is_split_without_truncating_content()
    test_split_model_response_keeps_markdown_fences_balanced()
    test_posts_failure_notice_when_kimi_fallback_fails()
    test_redacts_kimi_api_key_from_failure_notice()
    test_render_notice_redacts_secret_values()
    test_render_notice_redacts_new_provider_secret_env_names()
    print("GLM fallback self-tests OK")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
