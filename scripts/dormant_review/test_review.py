#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import stat
import subprocess
import tempfile
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = pathlib.Path(__file__).with_name("review.py")
CONFIG_PATH = pathlib.Path(__file__).with_name("review-config")
WORKFLOW_PATH = ROOT / ".github" / "workflows" / "ai-reviews.yml"
TOGGLE_PATH = pathlib.Path(__file__).with_name("toggle")


def load_module() -> object:
    spec = importlib.util.spec_from_file_location("dormant_ai_review", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("review module could not be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class FakeResponse:
    def __init__(self, payload: object) -> None:
        self.payload = json.dumps(payload).encode("utf-8")

    def __enter__(self) -> "FakeResponse":
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self) -> bytes:
        return self.payload


def assert_publish_rejects_a_moved_pr_head() -> None:
    module = load_module()
    base = "0" * 40
    expected = "1" * 40
    observed: list[str] = []

    def opener(request: object, **_kwargs: object) -> FakeResponse:
        method = request.get_method()
        observed.append(method)
        if method == "GET":
            return FakeResponse({"state": "open", "base": {"sha": base}, "head": {"sha": "2" * 40}})
        raise AssertionError("stale review reached comment POST")

    try:
        module.publish_bound_review(
            provider_config={"model": "model", "source_label_template": "Source ({model})", "deliverable_marker": "<!-- marker -->"},
            review_config={"max_comment_chars": 60000},
            github_config={"api_url": "https://api.github.invalid", "comment_timeout_seconds": 30},
            repo="owner/repo",
            pr_number="7",
            base_sha=base,
            head_sha=expected,
            token="token",
            review="review",
            opener=opener,
        )
    except module.ReviewError as exc:
        if "head moved" not in str(exc):
            raise
    else:
        raise AssertionError("stale review was published")
    if observed != ["GET"]:
        raise AssertionError(observed)


def assert_publish_checks_head_then_posts() -> None:
    module = load_module()
    base = "0" * 40
    expected = "1" * 40
    observed: list[str] = []

    def opener(request: object, **_kwargs: object) -> FakeResponse:
        method = request.get_method()
        observed.append(method)
        if method == "GET":
            return FakeResponse({"state": "open", "base": {"sha": base}, "head": {"sha": expected}})
        if method == "POST":
            body = json.loads(request.data.decode("utf-8"))
            if expected not in body["body"] or "review" not in body["body"]:
                raise AssertionError(body)
            return FakeResponse({"id": 1})
        raise AssertionError(method)

    module.publish_bound_review(
        provider_config={"model": "model", "source_label_template": "Source ({model})", "deliverable_marker": "<!-- marker -->"},
        review_config={"max_comment_chars": 60000},
        github_config={"api_url": "https://api.github.invalid", "comment_timeout_seconds": 30},
        repo="owner/repo",
        pr_number="7",
        base_sha=base,
        head_sha=expected,
        token="token",
        review="review",
        opener=opener,
    )
    if observed != ["GET", "POST"]:
        raise AssertionError(observed)


def assert_publish_rejects_closed_or_retargeted_pr() -> None:
    module = load_module()
    base = "0" * 40
    head = "1" * 40
    for payload, expected_error in (
        ({"state": "closed", "base": {"sha": base}, "head": {"sha": head}}, "not open"),
        ({"state": "open", "base": {"sha": "2" * 40}, "head": {"sha": head}}, "base moved"),
    ):
        observed: list[str] = []

        def opener(request: object, **_kwargs: object) -> FakeResponse:
            observed.append(request.get_method())
            if request.get_method() == "GET":
                return FakeResponse(payload)
            raise AssertionError("invalid review reached comment POST")

        try:
            module.publish_bound_review(
                provider_config={"model": "model", "source_label_template": "Source ({model})", "deliverable_marker": "<!-- marker -->"},
                review_config={"max_comment_chars": 60000},
                github_config={"api_url": "https://api.github.invalid", "comment_timeout_seconds": 30},
                repo="owner/repo",
                pr_number="7",
                base_sha=base,
                head_sha=head,
                token="token",
                review="review",
                opener=opener,
            )
        except module.ReviewError as exc:
            if expected_error not in str(exc):
                raise
        else:
            raise AssertionError("invalid review was published")
        if observed != ["GET"]:
            raise AssertionError(observed)


def assert_every_provider_is_text_only() -> None:
    module = load_module()
    config = tomllib.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    providers = config["review"]["providers"]
    if [entry["name"] for entry in providers] != ["claude", "glm", "kimi"]:
        raise AssertionError(providers)
    for entry in providers:
        provider = config[entry["name"]]
        if provider["adapter"] != "openai_chat":
            raise AssertionError(provider)
        payload = module.chat_payload(provider, "trusted policy", "untrusted diff")
        if "tools" in payload or payload["messages"] != [
            {"role": "system", "content": "trusted policy"},
            {"role": "user", "content": "untrusted diff"},
        ]:
            raise AssertionError(payload)


def assert_workflow_is_default_off_and_never_checks_out_pr_code() -> None:
    workflow = WORKFLOW_PATH.read_text(encoding="utf-8")
    required = (
        "pull_request_target:",
        "if: ${{ vars['AI_REVIEWS_ENABLED'] == 'true' }}",
        "ref: ${{ github.sha }}",
        "secrets[matrix.review.secret]",
        "scripts/dormant_review/review.py",
        "timeout-minutes: ${{ fromJSON(needs.prepare.outputs.review_timeout) }}",
    )
    for fragment in required:
        if fragment not in workflow:
            raise AssertionError(f"missing workflow boundary: {fragment}")
    forbidden = (
        "claude-code-action",
        "ref: ${{ github.event.pull_request.head.sha }}",
        "ref: ${{ needs.prepare.outputs.head_sha }}",
        "cargo ",
        "nextest",
        "clippy",
        "pytest",
        "npm ",
        "timeout-minutes: 10",
        "timeout-minutes: 45",
    )
    for fragment in forbidden:
        if fragment in workflow:
            raise AssertionError(f"forbidden review execution surface: {fragment}")
    if workflow.count("uses: actions/checkout@") != workflow.count("ref: ${{ github.sha }}"):
        raise AssertionError("every review checkout must remain pinned to the trusted base")


def assert_toggle_has_one_authority() -> None:
    toggle = TOGGLE_PATH.read_text(encoding="utf-8")
    if not TOGGLE_PATH.stat().st_mode & stat.S_IXUSR:
        raise AssertionError("toggle must be executable")
    if "AI_REVIEWS_ENABLED" not in toggle or "gh variable set" not in toggle:
        raise AssertionError(toggle)
    for forbidden in ("gh workflow enable", "gh workflow disable", "AI_REVIEW_PAUSED"):
        if forbidden in toggle:
            raise AssertionError(f"dual toggle authority: {forbidden}")


def assert_toggle_off_cancels_active_runs() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        log = root / "calls.log"
        count = root / "list-count"
        fake_gh = root / "gh"
        fake_gh.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$FAKE_GH_LOG"
if [ "$1 $2" = "variable get" ]; then
  printf 'false\\n'
elif [ "$1 $2" = "run list" ]; then
  current=0
  if [ -f "$FAKE_GH_COUNT" ]; then current="$(cat "$FAKE_GH_COUNT")"; fi
  current=$((current + 1))
  printf '%s\\n' "$current" > "$FAKE_GH_COUNT"
  if [ "$current" -eq 1 ]; then printf '101\\n102\\n'; fi
fi
""",
            encoding="utf-8",
        )
        fake_gh.chmod(0o755)
        environment = dict(os.environ)
        environment.update(
            {
                "PATH": f"{root}:{environment['PATH']}",
                "FAKE_GH_LOG": str(log),
                "FAKE_GH_COUNT": str(count),
            }
        )
        subprocess.run([str(TOGGLE_PATH), "off"], check=True, env=environment, capture_output=True, text=True)
        calls = log.read_text(encoding="utf-8")
        for fragment in ("variable set AI_REVIEWS_ENABLED --body false", "run cancel 101", "run cancel 102"):
            if fragment not in calls:
                raise AssertionError(calls)
        if calls.count("run list") < 2:
            raise AssertionError("off did not verify that active runs were gone")


def main() -> int:
    assert_publish_rejects_a_moved_pr_head()
    assert_publish_checks_head_then_posts()
    assert_publish_rejects_closed_or_retargeted_pr()
    assert_every_provider_is_text_only()
    assert_workflow_is_default_off_and_never_checks_out_pr_code()
    assert_toggle_has_one_authority()
    assert_toggle_off_cancels_active_runs()
    print("OK: isolated AI review tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
