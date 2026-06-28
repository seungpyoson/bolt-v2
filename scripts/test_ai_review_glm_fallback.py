#!/usr/bin/env python3
"""Self-tests for the GLM AI-review fallback."""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from dataclasses import dataclass


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "ai_review_deliverables.py"


def valid_no_findings_response() -> str:
    return "\n".join(
        [
            "No hard-evidence findings in this chunk.",
            "Coverage reviewed: changed files in this chunk.",
            "Evidence basis: supplied diff only; no omitted files, logs, or external state were assumed.",
            "Risk areas considered: correctness, security, workflow safety, verification, and repo-governance impact visible in this chunk.",
        ]
    )


def valid_finding_response(issue: str = "example issue") -> str:
    return "\n".join(
        [
            "Severity: medium",
            "Evidence: `+change`",
            f"Issue: {issue}",
            "Fix / verification: add a targeted verification.",
        ]
    )


def default_pr_agent_review_body() -> str:
    return "\n".join(
        [
            "## PR Reviewer Guide",
            "",
            "**Source:** GLM PR-Agent (`configured-pr-agent-model`)",
            "",
            "### Ticket Compliance Analysis",
            "",
            "No linked ticket was found.",
            "",
            "### Estimated effort to review",
            "",
            "2",
            "",
            "### Can be split",
            "",
            "No.",
            "",
            "### Review",
            "",
            "No blocking concern found in the changed workflow.",
        ]
    )


def default_pr_agent_review_body_with_evidence_signal() -> str:
    return "\n".join([default_pr_agent_review_body(), "", "Severity: low"])


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
    review_comments: list[dict[str, object]] | None = None
    reviews: list[dict[str, object]] | None = None

    def __post_init__(self) -> None:
        self.issue_comments = list(self.issue_comments or [])
        self.review_comments = list(self.review_comments or [])
        self.reviews = list(self.reviews or [])
        self.posted: list[str] = []
        self.updated: list[tuple[int, str]] = []
        self.updated_review_comments: list[tuple[int, str]] = []

    def list_pr_files(self) -> list[dict[str, object]]:
        return list(self.files)

    def list_issue_comments(self) -> list[dict[str, object]]:
        return list(self.issue_comments or [])

    def list_reviews(self) -> list[dict[str, object]]:
        return list(self.reviews or [])

    def list_pull_review_comments(self) -> list[dict[str, object]]:
        return list(self.review_comments or [])

    def post_issue_comment(self, body: str) -> None:
        self.posted.append(body)

    def update_issue_comment(self, comment_id: int, body: str) -> None:
        self.updated.append((comment_id, body))

    def update_pull_review_comment(self, comment_id: int, body: str) -> None:
        self.updated_review_comments.append((comment_id, body))


class FakeGLM:
    def __init__(self, *, fail: bool = False, response: str | None = None, failure_message: str | None = None) -> None:
        self.fail = fail
        self.failure_message = failure_message or "provider rejected request"
        self.response = response or valid_no_findings_response()
        self.prompts: list[str] = []

    def review_chunk(self, *, system_prompt: str, user_prompt: str) -> str:
        del system_prompt
        self.prompts.append(user_prompt)
        if self.fail:
            raise RuntimeError(self.failure_message)
        return self.response


class FakeProvider(FakeGLM):
    pass


class CommentApiHandler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        del format, args

    @property
    def comments(self) -> list[dict[str, object]]:
        return self.server.comments  # type: ignore[attr-defined]

    @property
    def requests(self) -> list[tuple[str, str, dict[str, object] | None]]:
        return self.server.requests  # type: ignore[attr-defined]

    def _read_payload(self) -> dict[str, object]:
        length = int(self.headers.get("Content-Length") or "0")
        if length == 0:
            return {}
        payload = json.loads(self.rfile.read(length).decode("utf-8"))
        if not isinstance(payload, dict):
            raise AssertionError("expected JSON object payload")
        return payload

    def _write_json(self, status: int, payload: object) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        self.requests.append(("GET", self.path, None))
        if self.path.startswith("/repos/seungpyoson/bolt-v2/issues/895/comments"):
            self._write_json(200, self.comments)
            return
        self._write_json(404, {"message": "not found"})

    def do_POST(self) -> None:
        payload = self._read_payload()
        self.requests.append(("POST", self.path, payload))
        if self.path == "/repos/seungpyoson/bolt-v2/issues/895/comments":
            body = str(payload.get("body") or "")
            self.comments.append(
                {
                    "id": 100 + len(self.comments),
                    "body": body,
                    "created_at": "2026-06-22T12:23:00Z",
                    "updated_at": "2026-06-22T12:23:00Z",
                    "user": {"type": "Bot", "login": "github-actions[bot]"},
                }
            )
            self._write_json(201, self.comments[-1])
            return
        self._write_json(404, {"message": "not found"})

    def do_PATCH(self) -> None:
        payload = self._read_payload()
        self.requests.append(("PATCH", self.path, payload))
        prefix = "/repos/seungpyoson/bolt-v2/issues/comments/"
        if self.path.startswith(prefix):
            comment_id = int(self.path[len(prefix) :])
            for comment in self.comments:
                if comment.get("id") == comment_id:
                    comment["body"] = str(payload.get("body") or "")
                    comment["updated_at"] = "2026-06-22T12:24:00Z"
                    self._write_json(200, comment)
                    return
        self._write_json(404, {"message": "not found"})


def start_comment_api_server(
    comments: list[dict[str, object]],
) -> tuple[ThreadingHTTPServer, threading.Thread, list[tuple[str, str, dict[str, object] | None]]]:
    requests: list[tuple[str, str, dict[str, object] | None]] = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), CommentApiHandler)
    server.comments = comments  # type: ignore[attr-defined]
    server.requests = requests  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, requests


def file_payload(name: str, patch: str) -> dict[str, object]:
    return {
        "filename": name,
        "status": "modified",
        "additions": 1,
        "deletions": 0,
        "changes": 1,
        "patch": patch,
    }


def fallback_config(module, **overrides):
    values = {
        "repo": "seungpyoson/bolt-v2",
        "pr_number": 895,
        "started_at": "2026-06-22T12:21:00Z",
        "instructions": "review hard evidence only",
        "max_chunk_chars": 260,
        "max_comment_chars": 60000,
        "response_chars_per_chunk": 8000,
        "output_contract": module.ReviewOutputContract(
            finding_required_labels=("Severity:", "Evidence:", "Issue:", "Fix / verification:"),
            finding_guidance=(
                "blocking, high, medium, or low",
                "the smallest relevant snippet or line reference from the supplied chunk",
                "why this is a real behavior, safety, governance, or verification problem",
                "the concrete next step",
            ),
            no_findings_indicator="No hard-evidence findings",
            no_findings_intro="No hard-evidence findings in this chunk.",
            no_findings_required_labels=("Coverage reviewed:", "Evidence basis:", "Risk areas considered:"),
            no_findings_guidance=(
                "<specific changed files or diff areas reviewed in this chunk>.",
                "supplied diff only; no omitted files, logs, or external state were assumed.",
                "correctness, security, workflow safety, verification, and repo-governance impact visible in this chunk.",
            ),
            non_deliverable_indicators=("review did not produce a deliverable", "review notice"),
            pr_agent_deliverable_headings=("## PR Reviewer Guide", "## Incremental PR Reviewer Guide"),
            pr_agent_disabled_noise=(),
        ),
        "run_url": "https://github.com/seungpyoson/bolt-v2/actions/runs/1",
        "provider": "GLM",
        "deliverable_markers": (
            "## PR Reviewer Guide",
            "## Incremental PR Reviewer Guide",
            "<!-- ai-pr-reviewer-glm -->",
        ),
        "expected_bot_login": "github-actions[bot]",
        "comment_marker": "<!-- ai-pr-reviewer-glm -->",
        "source_label": "GLM direct fallback (`configured-glm-model`)",
    }
    values.update(overrides)
    if "source_label" not in overrides and values["provider"] == "Kimi":
        values["source_label"] = "Kimi Code CLI (`configured-kimi-model`)"
    return module.FallbackConfig(**values)


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
                "body": (
                    "## PR Reviewer Guide\n\n"
                    "**Source:** GLM PR-Agent (`configured-pr-agent-model`)\n\n"
                    "Severity: low\n\n"
                    "existing PR-Agent result"
                ),
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
        config=fallback_config(module,
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


def test_pr_agent_severity_review_without_fallback_labels_suppresses_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "\n".join(
                    [
                        "## PR Reviewer Guide",
                        "",
                        "**Source:** GLM PR-Agent (`configured-pr-agent-model`)",
                        "",
                        "### Security Review",
                        "",
                        "Severity: low",
                        "",
                        "No blocking concern found in the changed workflow.",
                    ]
                ),
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
        config=fallback_config(module),
    )

    assert result == "existing-review-deliverable"
    assert glm.prompts == []
    assert github.posted == []


def test_default_pr_agent_sections_without_evidence_do_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": default_pr_agent_review_body(),
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM(response=valid_finding_response("fallback ran after default-only PR-Agent sections."))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module),
    )

    assert result == "fallback-posted"
    assert len(glm.prompts) == 1
    assert len(github.posted) == 1
    assert "fallback ran after default-only PR-Agent sections." in github.posted[0]


def test_default_pr_agent_sections_with_evidence_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": default_pr_agent_review_body_with_evidence_signal(),
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
        config=fallback_config(module),
    )

    assert result == "existing-review-deliverable"
    assert glm.prompts == []
    assert github.posted == []


def test_unstamped_pr_agent_deliverable_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "## PR Reviewer Guide\n\nexisting unstamped PR-Agent result",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM(response=valid_finding_response("fallback ran after unstamped PR-Agent review."))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module),
    )

    assert result == "fallback-posted"
    assert len(glm.prompts) == 1
    assert len(github.posted) == 1
    assert "fallback ran after unstamped PR-Agent review." in github.posted[0]


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
        config=fallback_config(module,
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
        config=fallback_config(module,
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
        config=fallback_config(module,
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
        config=fallback_config(module,
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
                "body": (
                    "## Incremental PR Reviewer Guide\n\n"
                    "**Source:** GLM PR-Agent (`configured-pr-agent-model`)\n\n"
                    "Severity: low\n\n"
                    "existing incremental PR-Agent result"
                ),
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
        config=fallback_config(module,
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


def test_prior_glm_fallback_marker_suppresses_later_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": (
                    "<!-- ai-pr-reviewer-glm -->\n\n"
                    "## GLM PR Review\n\n"
                    "- Source: GLM direct fallback (`configured-glm-model`)\n\n"
                    f"{valid_no_findings_response()}"
                ),
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
        config=fallback_config(module),
    )

    assert result == "existing-review-deliverable"
    assert glm.prompts == []
    assert github.posted == []


def test_low_quality_marker_comment_does_not_suppress_fallback() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "body": "<!-- ai-pr-reviewer-glm -->\n\n## GLM PR Review\n\nNo hard-evidence findings in this chunk.",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM(response=valid_finding_response("fallback ran after weak marker."))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module),
    )

    assert result == "fallback-posted"
    assert len(glm.prompts) == 1
    assert len(github.posted) == 1
    assert "fallback ran after weak marker." in github.posted[0]


def test_same_round_marker_comment_is_updated_instead_of_posting_new_comment() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "id": 42,
                "body": (
                    "<!-- ai-pr-reviewer-glm -->\n\n"
                    "## GLM PR Review\n\n"
                    "older fallback result\n\n"
                    "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n"
                ),
                "created_at": "2026-06-22T11:22:00Z",
                "updated_at": "2026-06-22T11:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM(response=valid_finding_response("updated review body."))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module),
    )

    assert result == "fallback-posted"
    assert github.posted == []
    assert len(github.updated) == 1, github.updated
    assert github.updated[0][0] == 42
    assert github.updated[0][1].startswith("<!-- ai-pr-reviewer-glm -->")
    assert "updated review body." in github.updated[0][1]


def test_previous_round_marker_comment_does_not_get_overwritten() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[file_payload("src/lib.rs", "+change")],
        issue_comments=[
            {
                "id": 42,
                "body": (
                    "<!-- ai-pr-reviewer-glm -->\n\n"
                    "## GLM PR Review\n\n"
                    "older fallback result\n\n"
                    "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/0\n"
                ),
                "created_at": "2026-06-22T11:22:00Z",
                "updated_at": "2026-06-22T11:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
    )
    glm = FakeGLM(response=valid_finding_response("new review round body."))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module),
    )

    assert result == "fallback-posted"
    assert github.updated == []
    assert len(github.posted) == 1
    assert "new review round body." in github.posted[0]
    assert "actions/runs/1" in github.posted[0]


def test_generated_low_quality_review_is_not_posted_as_deliverable() -> None:
    module = load_script()
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(response="No hard-evidence findings in this chunk.")

    try:
        module.run_fallback_review(
            github=github,
            reviewer=glm,
            config=fallback_config(module),
        )
    except module.ReviewFailed as exc:
        assert "did not meet the hard-evidence output contract" in str(exc)
    else:
        raise AssertionError("low-quality review response should fail")

    assert len(github.posted) == 1
    assert "review did not produce a deliverable" in github.posted[0]
    assert "did not meet the hard-evidence output contract" in github.posted[0]


def test_github_client_upserts_marker_comments_through_http_api() -> None:
    module = load_script()
    comments = [
        {
            "id": 42,
            "body": (
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n\n"
                "old body\n"
            ),
            "created_at": "2026-06-22T12:22:00Z",
            "updated_at": "2026-06-22T12:22:00Z",
            "user": {"type": "Bot", "login": "github-actions[bot]"},
        }
    ]
    server, thread, requests = start_comment_api_server(comments)
    try:
        api_url = f"http://127.0.0.1:{server.server_port}"
        github = module.GitHubClient(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            token="test-token",
            api_url=api_url,
        )
        module.post_or_update_marker_comment(
            github=github,
            config=fallback_config(module),
            body=(
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n\n"
                "updated body\n"
            ),
        )
        module.post_or_update_marker_comment(
            github=github,
            config=fallback_config(
                module,
                run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/2",
            ),
            body=(
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/2\n\n"
                "new round body\n"
            ),
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)

    methods = [method for method, _, _ in requests]
    assert methods == ["GET", "PATCH", "GET", "POST"], requests
    assert len(comments) == 2
    assert comments[0]["id"] == 42
    assert "updated body" in str(comments[0]["body"])
    assert "new round body" in str(comments[1]["body"])


def test_github_client_keeps_paginated_marker_comments_distinct() -> None:
    module = load_script()
    comments = [
        {
            "id": 42,
            "body": (
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review (part 1/2)\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n"
                "- Comment part: 1/2\n\n"
                "old part one\n"
            ),
            "created_at": "2026-06-22T12:22:00Z",
            "updated_at": "2026-06-22T12:22:00Z",
            "user": {"type": "Bot", "login": "github-actions[bot]"},
        }
    ]
    server, thread, requests = start_comment_api_server(comments)
    try:
        api_url = f"http://127.0.0.1:{server.server_port}"
        github = module.GitHubClient(
            repo="seungpyoson/bolt-v2",
            pr_number=895,
            token="test-token",
            api_url=api_url,
        )
        config = fallback_config(module)
        module.post_or_update_marker_comment(
            github=github,
            config=config,
            body=(
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review (part 1/2)\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n"
                "- Comment part: 1/2\n\n"
                "updated part one\n"
            ),
        )
        module.post_or_update_marker_comment(
            github=github,
            config=config,
            body=(
                "<!-- ai-pr-reviewer-glm -->\n\n"
                "## GLM PR Review (part 2/2)\n\n"
                "- Action run: https://github.com/seungpyoson/bolt-v2/actions/runs/1\n"
                "- Comment part: 2/2\n\n"
                "new part two\n"
            ),
        )
    finally:
        server.shutdown()
        thread.join(timeout=5)

    methods = [method for method, _, _ in requests]
    assert methods == ["GET", "PATCH", "GET", "POST"], requests
    assert len(comments) == 2
    assert "updated part one" in str(comments[0]["body"])
    assert "new part two" in str(comments[1]["body"])


def test_posts_failure_notice_when_glm_fallback_fails() -> None:
    module = load_script()
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(fail=True)

    try:
        module.run_fallback_review(
            github=github,
            reviewer=glm,
            config=fallback_config(module,
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
                    config=fallback_config(module,
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
    marker = "<!-- ai-pr-reviewer-kimi -->"

    result = module.run_fallback_review(
        github=github,
        reviewer=kimi,
        config=fallback_config(module,
            provider="Kimi",
            deliverable_markers=(marker,),
            comment_marker=marker,
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
    assert github.posted[0].startswith(marker)
    assert "## Kimi PR Review" in github.posted[0]
    assert "### Chunk 1/" in github.posted[0]
    assert "Per-chunk character budget:" not in github.posted[0]
    assert "Source: Kimi Code CLI" in github.posted[0]


def test_chunked_review_comments_respect_configured_comment_limit() -> None:
    module = load_script()
    config = fallback_config(
        module,
        max_comment_chars=900,
        response_chars_per_chunk=180,
    )
    chunks = [
        module.ReviewChunk(title=f"src/file_{idx}.rs", body="+" + ("x" * 80))
        for idx in range(8)
    ]
    responses = [
        "\n".join(
            [
                "### Finding 1",
                "**Severity:** low",
                "**Evidence:** synthetic chunk with enough text to force comment packing",
                "**Issue:** packing must preserve the configured size limit after final part labels render",
                "**Fix / verification:** assert final rendered comments fit",
            ]
        )
        for _ in chunks
    ]

    comments = module.render_review_comments(config=config, chunks=chunks, responses=responses)

    assert len(comments) > 1
    assert all(len(comment) <= config.max_comment_chars for comment in comments)
    assert all("- Comment part: " in comment for comment in comments)


def test_source_label_template_substitutes_configured_model() -> None:
    module = load_script()

    assert (
        module.source_label_from_template(
            {"source_label_template": "Configured reviewer (`{model}`)"},
            model="configured-model",
        )
        == "Configured reviewer (`configured-model`)"
    )
    try:
        module.source_label_from_template({"source_label_template": "Configured reviewer"}, model="configured-model")
    except RuntimeError as exc:
        assert "source_label_template must include {model}" in str(exc)
    else:
        raise AssertionError("source label template without model placeholder was accepted")


def test_stamps_all_pr_agent_review_source_model_comments() -> None:
    module = load_script()
    github = FakeGitHub(
        files=[],
        issue_comments=[
            {
                "id": 77,
                "body": "## PR Reviewer Guide\n\nHere are some observations.",
                "created_at": "2026-06-22T12:22:00Z",
                "updated_at": "2026-06-22T12:22:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            },
            {
                "id": 78,
                "body": "## Incremental PR Reviewer Guide\n\nMore observations.",
                "created_at": "2026-06-22T12:23:00Z",
                "updated_at": "2026-06-22T12:23:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            }
        ],
        review_comments=[
            {
                "id": 79,
                "body": "## PR Reviewer Guide\n\nLine-level PR-Agent observation.",
                "created_at": "2026-06-22T12:24:00Z",
                "updated_at": "2026-06-22T12:24:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            },
            {
                "id": 80,
                "body": "Unrelated bot line-level observation.",
                "created_at": "2026-06-22T12:25:00Z",
                "updated_at": "2026-06-22T12:25:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            },
            {
                "id": 81,
                "body": "Earlier line-level observation.",
                "created_at": "2026-06-22T12:20:00Z",
                "updated_at": "2026-06-22T12:20:00Z",
                "user": {"type": "Bot", "login": "github-actions[bot]"},
            },
        ],
    )

    result = module.stamp_existing_review_comment(
        github=github,
        started_at="2026-06-22T12:21:00Z",
        markers=("## PR Reviewer Guide", "## Incremental PR Reviewer Guide", "<!-- ai-pr-reviewer-glm -->"),
        expected_bot_login="github-actions[bot]",
        marker="<!-- ai-pr-reviewer-glm -->",
        source_label="GLM PR-Agent (`configured-pr-agent-model`)",
        run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
    )

    assert result == "existing-reviews-stamped"
    assert github.posted == []
    assert [comment_id for comment_id, _body in github.updated] == [77, 78]
    assert [comment_id for comment_id, _body in github.updated_review_comments] == [79]
    for _comment_id, body in github.updated:
        assert body.startswith("<!-- ai-pr-reviewer-glm -->")
        assert "**Source:** GLM PR-Agent (`configured-pr-agent-model`)" in body
        assert "**Action run:** https://github.com/seungpyoson/bolt-v2/actions/runs/1" in body
    for _comment_id, body in github.updated_review_comments:
        assert body.startswith("<!-- ai-pr-reviewer-glm -->")
        assert "**Source:** GLM PR-Agent (`configured-pr-agent-model`)" in body
        assert "**Action run:** https://github.com/seungpyoson/bolt-v2/actions/runs/1" in body


def test_stamping_preserves_existing_marker_as_first_line() -> None:
    module = load_script()

    stamped = module.add_source_line(
        "<!-- ai-pr-reviewer-glm -->\n\n## PR Reviewer Guide\n\nBody.",
        marker="<!-- ai-pr-reviewer-glm -->",
        source_label="GLM PR-Agent (`configured-pr-agent-model`)",
        run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
    )

    assert stamped.startswith("<!-- ai-pr-reviewer-glm -->\n\n## PR Reviewer Guide")
    assert stamped.count("<!-- ai-pr-reviewer-glm -->") == 1
    assert "**Source:** GLM PR-Agent (`configured-pr-agent-model`)" in stamped


def test_stamping_detects_existing_source_lines_past_initial_comment_window() -> None:
    module = load_script()
    source_line = "**Source:** GLM PR-Agent (`configured-pr-agent-model`)"
    run_line = "**Action run:** https://github.com/seungpyoson/bolt-v2/actions/runs/1"
    body = "\n".join(
        [
            "<!-- ai-pr-reviewer-glm -->",
            "",
            "## PR Reviewer Guide",
            "",
            "x" * 1200,
            source_line,
            run_line,
            "",
            "Body.",
        ]
    )

    stamped = module.add_source_line(
        body,
        marker="<!-- ai-pr-reviewer-glm -->",
        source_label="GLM PR-Agent (`configured-pr-agent-model`)",
        run_url="https://github.com/seungpyoson/bolt-v2/actions/runs/1",
    )

    assert stamped.count(source_line) == 1
    assert stamped.count(run_line) == 1


def test_splits_fallback_review_across_comments_when_comment_budget_requires_it() -> None:
    module = load_script()
    files = [file_payload(f"src/file_{idx}.rs", "+" + ("x" * 80)) for idx in range(7)]
    github = FakeGitHub(files=files)
    glm = FakeGLM(response=valid_finding_response("hard evidence finding. " * 12))

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module,
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
    assert all(len(comment) <= 520 for comment in github.posted)
    assert all("## GLM PR Review (part " in comment for comment in github.posted)
    joined = "\n".join(github.posted)
    assert "Chunk 1/" in joined
    assert f"Chunk {len(glm.prompts)}/" in joined
    assert "[truncated to fit GitHub comment limit]" not in joined


def test_large_model_response_is_split_without_truncating_content() -> None:
    module = load_script()
    response = valid_finding_response("X" * 20000)
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(response=response)

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module,
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
    response = "\n".join(
        [
            "Severity: medium",
            "Evidence: `+change`",
            "Issue: fenced evidence needs balanced splitting.",
            "```python",
            *[f"print({idx})" for idx in range(60)],
            "```",
            "Fix / verification: keep fences balanced.",
        ]
    )
    github = FakeGitHub(files=[file_payload("src/lib.rs", "+change")])
    glm = FakeGLM(response=response)

    result = module.run_fallback_review(
        github=github,
        reviewer=glm,
        config=fallback_config(module,
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
            config=fallback_config(module,
                provider="Kimi",
                deliverable_markers=("<!-- ai-pr-reviewer-kimi -->",),
                comment_marker="<!-- ai-pr-reviewer-kimi -->",
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
                config=fallback_config(module,
                    provider="Kimi",
                    deliverable_markers=("<!-- ai-pr-reviewer-kimi -->",),
                    comment_marker="<!-- ai-pr-reviewer-kimi -->",
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


def test_kimi_cli_client_uses_documented_env_auth_path() -> None:
    module = load_script()
    calls: list[dict[str, object]] = []

    class Completed:
        returncode = 0
        stdout = "OK\n"
        stderr = ""

    def fake_run(argv, *, capture_output, text, timeout, env):
        calls.append(
            {
                "argv": argv,
                "capture_output": capture_output,
                "text": text,
                "timeout": timeout,
                "env": env,
            }
        )
        return Completed()

    original_run = module.subprocess.run
    with tempfile.TemporaryDirectory() as kimi_home:
        previous_home = os.environ.get("KIMI_CODE_HOME")
        os.environ["KIMI_CODE_HOME"] = kimi_home
        module.subprocess.run = fake_run
        try:
            client = module.KimiCliClient(
                api_key="fake-kimi-secret",
                api_base="https://api.kimi.com/coding/v1",
                model="configured-kimi-model",
                provider="Kimi",
                provider_type="kimi",
                model_max_context_size=262144,
                default_thinking=True,
                telemetry_disabled=True,
                timeout_seconds=9,
            )
            response = client.review_chunk(system_prompt="system", user_prompt="user")
        finally:
            module.subprocess.run = original_run
            if previous_home is None:
                os.environ.pop("KIMI_CODE_HOME", None)
            else:
                os.environ["KIMI_CODE_HOME"] = previous_home

    assert response == "OK"
    assert len(calls) == 1
    call = calls[0]
    argv = call["argv"]
    assert argv == ["kimi", "-p", "system\n\nuser"]
    assert "fake-kimi-secret" not in " ".join(argv)
    env = call["env"]
    assert env["KIMI_MODEL_NAME"] == "configured-kimi-model"
    assert env["KIMI_MODEL_API_KEY"] == "fake-kimi-secret"
    assert env["KIMI_MODEL_BASE_URL"] == "https://api.kimi.com/coding/v1"
    assert env["KIMI_MODEL_PROVIDER_TYPE"] == "kimi"
    assert env["KIMI_MODEL_MAX_CONTEXT_SIZE"] == "262144"
    assert env["KIMI_MODEL_DEFAULT_THINKING"] == "true"
    assert env["KIMI_DISABLE_TELEMETRY"] == "1"


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
    test_pr_agent_severity_review_without_fallback_labels_suppresses_fallback()
    test_default_pr_agent_sections_without_evidence_do_not_suppress_fallback()
    test_default_pr_agent_sections_with_evidence_suppress_fallback()
    test_unstamped_pr_agent_deliverable_does_not_suppress_fallback()
    test_plain_pr_agent_phrase_does_not_suppress_fallback()
    test_human_pr_agent_marker_comment_does_not_suppress_fallback()
    test_human_kimi_marker_comment_does_not_suppress_fallback()
    test_unexpected_bot_marker_comment_does_not_suppress_fallback()
    test_incremental_pr_agent_deliverable_suppresses_fallback()
    test_prior_glm_fallback_marker_suppresses_later_fallback()
    test_low_quality_marker_comment_does_not_suppress_fallback()
    test_same_round_marker_comment_is_updated_instead_of_posting_new_comment()
    test_previous_round_marker_comment_does_not_get_overwritten()
    test_generated_low_quality_review_is_not_posted_as_deliverable()
    test_github_client_upserts_marker_comments_through_http_api()
    test_github_client_keeps_paginated_marker_comments_distinct()
    test_posts_failure_notice_when_glm_fallback_fails()
    test_sets_failure_notice_output_after_posting_failure_notice()
    test_kimi_fallback_uses_same_chunked_deliverable_contract()
    test_source_label_template_substitutes_configured_model()
    test_stamps_all_pr_agent_review_source_model_comments()
    test_stamping_preserves_existing_marker_as_first_line()
    test_stamping_detects_existing_source_lines_past_initial_comment_window()
    test_splits_fallback_review_across_comments_when_comment_budget_requires_it()
    test_large_model_response_is_split_without_truncating_content()
    test_split_model_response_keeps_markdown_fences_balanced()
    test_posts_failure_notice_when_kimi_fallback_fails()
    test_redacts_kimi_api_key_from_failure_notice()
    test_kimi_cli_client_uses_documented_env_auth_path()
    test_render_notice_redacts_secret_values()
    test_render_notice_redacts_new_provider_secret_env_names()
    print("GLM fallback self-tests OK")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
