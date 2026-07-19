#!/usr/bin/env python3
"""Run one configured AI reviewer directly and post its exact-head deliverable."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import tomllib
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping


class EvidenceError(RuntimeError):
    """Raised when a direct review cannot produce bound evidence."""


Publisher = Callable[[str, str, str, str, str, int], None]
PUBLISHER_CREDENTIAL_ENV = frozenset({"GITHUB_TOKEN", "GH_TOKEN"})


def required_env(name: str) -> str:
    value = os.environ.get(name, "")
    if not value:
        raise EvidenceError(f"{name} is required")
    return value


def chunk_text(text: str, limit: int) -> tuple[str, ...]:
    if limit <= 0:
        raise EvidenceError("chunk limit must be positive")
    if not text:
        return ("",)
    chunks: list[str] = []
    current = ""
    for line in text.splitlines(keepends=True):
        while len(line) > limit:
            if current:
                chunks.append(current)
                current = ""
            chunks.append(line[:limit])
            line = line[limit:]
        if current and len(current) + len(line) > limit:
            chunks.append(current)
            current = ""
        current += line
    if current or not chunks:
        chunks.append(current)
    return tuple(chunks)


def render_comment(
    *,
    marker: str,
    source: str,
    head_sha: str,
    review: str,
) -> str:
    return (
        f"{marker}\n\n"
        f"**Source:** {source}\n\n"
        f"**Head:** {head_sha}\n\n"
        f"{review.strip()}\n"
    )


def config_table(config: Mapping[str, object], key: str) -> dict[str, object]:
    value = config.get(key)
    if not isinstance(value, dict):
        raise EvidenceError(f"[{key}] is required")
    return value


def config_text(config: Mapping[str, object], key: str) -> str:
    value = config.get(key)
    if not isinstance(value, str) or not value:
        raise EvidenceError(f"{key} must be a non-empty string")
    return value


def config_int(config: Mapping[str, object], key: str) -> int:
    value = config.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise EvidenceError(f"{key} must be a positive integer")
    return value


def config_bool(config: Mapping[str, object], key: str) -> bool:
    value = config.get(key)
    if not isinstance(value, bool):
        raise EvidenceError(f"{key} must be a boolean")
    return value


def source_label(config: Mapping[str, object], model: str) -> str:
    template = config_text(config, "source_label_template")
    if "{model}" not in template:
        raise EvidenceError("source_label_template must contain {model}")
    return template.replace("{model}", model)


def output_contract_text(contract: Mapping[str, object]) -> str:
    if not contract:
        raise EvidenceError("[review.output_contract] must not be empty")
    return "# Required review output contract\n\n" + json.dumps(contract, indent=2, sort_keys=True)


def read_review_diff(path: pathlib.Path, base_sha: str, head_sha: str) -> str:
    if not re.fullmatch(r"[0-9a-f]{40}", base_sha) or not re.fullmatch(r"[0-9a-f]{40}", head_sha):
        raise EvidenceError("base and head identities must be full lowercase Git SHAs")
    try:
        diff = path.read_text(encoding="utf-8")
    except OSError as exc:
        raise EvidenceError(f"immutable review diff could not be read: {path}") from exc
    if not diff:
        raise EvidenceError("pull request diff is empty")
    return diff


def post_comment(repo: str, pr_number: str, token: str, body: str, api_url: str, timeout_seconds: int) -> None:
    request = urllib.request.Request(
        f"{api_url.rstrip('/')}/repos/{repo}/issues/{pr_number}/comments",
        data=json.dumps({"body": body}).encode("utf-8"),
        method="POST",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
            response.read()
    except urllib.error.HTTPError as exc:
        exc.read()
        raise EvidenceError(f"GitHub comment API failed with HTTP {exc.code}") from exc


def publish_bound_review(
    *,
    provider_config: Mapping[str, object],
    review_config: Mapping[str, object],
    repo: str,
    pr_number: str,
    head_sha: str,
    token: str,
    review: str,
    api_url: str,
    api_timeout_seconds: int,
    publisher: Publisher = post_comment,
) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", head_sha):
        raise EvidenceError("published review head must be a full lowercase Git SHA")
    model = config_text(provider_config, "model")
    body = render_comment(
        marker=config_text(provider_config, "deliverable_marker"),
        source=source_label(provider_config, model),
        head_sha=head_sha,
        review=review,
    )
    if len(body) > config_int(review_config, "max_comment_chars"):
        raise EvidenceError("review comment exceeds configured maximum")
    publisher(repo, pr_number, token, body, api_url, api_timeout_seconds)


def read_claude_execution(execution_file: pathlib.Path) -> str:
    try:
        payload = json.loads(execution_file.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise EvidenceError("Claude execution output is missing or invalid") from exc
    if not isinstance(payload, list) or not payload:
        raise EvidenceError("Claude execution output has no final result")
    result = payload[-1]
    if (
        not isinstance(result, dict)
        or result.get("type") != "result"
        or result.get("subtype") != "success"
        or result.get("is_error") is not False
    ):
        raise EvidenceError("Claude execution did not complete successfully")
    review = result.get("result")
    if not isinstance(review, str) or not review.strip():
        raise EvidenceError("Claude execution final result is empty")
    return review.strip()


def publish_claude_execution(
    execution_file: pathlib.Path,
    *,
    provider_config: Mapping[str, object],
    review_config: Mapping[str, object],
    repo: str,
    pr_number: str,
    head_sha: str,
    token: str,
    api_url: str,
    api_timeout_seconds: int,
    publisher: Publisher = post_comment,
) -> None:
    publish_bound_review(
        provider_config=provider_config,
        review_config=review_config,
        repo=repo,
        pr_number=pr_number,
        head_sha=head_sha,
        token=token,
        review=read_claude_execution(execution_file),
        api_url=api_url,
        api_timeout_seconds=api_timeout_seconds,
        publisher=publisher,
    )


def openai_chat_review(prompt: str, config: Mapping[str, object], api_key: str) -> str:
    model = config_text(config, "model")
    payload = {
        "model": model,
        "temperature": config.get("temperature"),
        "messages": [{"role": "user", "content": prompt}],
    }
    request = urllib.request.Request(
        f"{config_text(config, 'api_base').rstrip('/')}/chat/completions",
        data=json.dumps(payload).encode("utf-8"),
        method="POST",
        headers={"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=config_int(config, "api_timeout_seconds")) as response:
            response_payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        exc.read()
        raise EvidenceError(f"GLM API failed with HTTP {exc.code}") from exc
    choices = response_payload.get("choices")
    if not isinstance(choices, list) or not choices:
        raise EvidenceError("GLM response has no choices")
    message = choices[0].get("message")
    content = message.get("content") if isinstance(message, dict) else None
    if not isinstance(content, str) or not content.strip():
        raise EvidenceError("GLM response content is empty")
    return content.strip()


def kimi_review(prompt: str, config: Mapping[str, object], api_key: str) -> str:
    telemetry_disabled = config_bool(config, "telemetry_disabled")
    with tempfile.TemporaryDirectory(prefix="kimi-code-") as tmp:
        home = pathlib.Path(tmp)
        (home / "config.toml").write_text(
            f"telemetry = {'false' if telemetry_disabled else 'true'}\n",
            encoding="utf-8",
        )
        env = {key: value for key, value in os.environ.items() if key not in PUBLISHER_CREDENTIAL_ENV}
        env.update(
            {
                "KIMI_CODE_HOME": str(home),
                "KIMI_DISABLE_TELEMETRY": "1" if telemetry_disabled else "0",
                "KIMI_MODEL_NAME": config_text(config, "model"),
                "KIMI_MODEL_API_KEY": api_key,
                "KIMI_MODEL_BASE_URL": config_text(config, "api_base"),
                "KIMI_MODEL_PROVIDER_TYPE": config_text(config, "provider_type"),
                "KIMI_MODEL_MAX_CONTEXT_SIZE": str(config_int(config, "model_max_context_size")),
                "KIMI_MODEL_DEFAULT_THINKING": str(config_bool(config, "default_thinking")).lower(),
            }
        )
        completed = subprocess.run(
            [config_text(config, "cli_binary"), "-p", prompt],
            capture_output=True,
            text=True,
            timeout=config_int(config, "cli_timeout_seconds"),
            env=env,
            check=False,
        )
    if completed.returncode != 0:
        raise EvidenceError(f"Kimi CLI failed with exit {completed.returncode}")
    if not completed.stdout.strip():
        raise EvidenceError("Kimi CLI response is empty")
    return completed.stdout.strip()


def review_prompt(instructions: str, diff: str, index: int, count: int) -> str:
    return (
        f"{instructions}\n\n"
        f"# Diff chunk {index}/{count}\n\n"
        "Review only hard-evidence issues. Return the configured no-findings contract when none exist.\n\n"
        f"```diff\n{diff}\n```\n"
    )


def review_transaction(
    chunks: tuple[str, ...],
    *,
    instructions: str,
    reviewer: Callable[[str], str],
) -> str:
    findings = tuple(
        reviewer(review_prompt(instructions, chunk, index, len(chunks)))
        for index, chunk in enumerate(chunks, start=1)
    )
    synthesis = (
        f"{instructions}\n\n"
        "# Final synthesis\n\n"
        "The following are the complete ordered chunk-review results for one immutable diff. "
        "Reconcile duplicates and cross-chunk relationships. Return one final review using the required output contract.\n\n"
        + "\n\n".join(
            f"## Chunk {index}/{len(findings)} result\n\n{finding}"
            for index, finding in enumerate(findings, start=1)
        )
    )
    return reviewer(synthesis)


def run(
    provider: str,
    instructions_file: pathlib.Path,
    config_file: pathlib.Path,
    *,
    publisher: Publisher = post_comment,
) -> int:
    runtime = tomllib.loads(config_file.read_text(encoding="utf-8"))
    provider_config = config_table(runtime, provider)
    review_config = config_table(runtime, "review")
    github_config = config_table(runtime, "github")
    repo = required_env("GITHUB_REPOSITORY")
    pr_number = required_env("PR_NUMBER")
    head_sha = required_env("PR_HEAD_SHA")
    base_sha = required_env("PR_BASE_SHA")
    diff_path = pathlib.Path(required_env("PR_DIFF_PATH"))
    token = required_env("GITHUB_TOKEN")
    api_key = required_env(config_text(provider_config, "secret_env"))
    diff = read_review_diff(diff_path, base_sha, head_sha)
    limit = config_int(provider_config, "review_max_chunk_chars")
    chunks = chunk_text(diff, limit)
    instructions = instructions_file.read_text(encoding="utf-8")
    instructions += "\n\n" + output_contract_text(config_table(review_config, "output_contract"))
    api_url = config_text(github_config, "api_url")
    api_timeout_seconds = config_int(github_config, "comment_timeout_seconds")
    adapters = {
        "openai_chat": lambda prompt: openai_chat_review(prompt, provider_config, api_key),
        "kimi_cli": lambda prompt: kimi_review(prompt, provider_config, api_key),
    }
    adapter_name = config_text(provider_config, "adapter")
    try:
        reviewer = adapters[adapter_name]
    except KeyError as exc:
        raise EvidenceError(f"unsupported review adapter: {adapter_name}") from exc
    response = review_transaction(chunks, instructions=instructions, reviewer=reviewer)
    publish_bound_review(
        provider_config=provider_config,
        review_config=review_config,
        repo=repo,
        pr_number=pr_number,
        head_sha=head_sha,
        token=token,
        review=response,
        api_url=api_url,
        api_timeout_seconds=api_timeout_seconds,
        publisher=publisher,
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("provider")
    parser.add_argument("--instructions-file", required=True, type=pathlib.Path)
    parser.add_argument("--config-file", required=True, type=pathlib.Path)
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)
    return run(args.provider, args.instructions_file, args.config_file)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except EvidenceError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        raise SystemExit(1) from None
