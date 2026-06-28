#!/usr/bin/env python3
"""Verify AI review model pins against provider model sources."""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import re
import sys
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

try:
    from ai_review_deliverables import sanitize_detail
except ImportError:  # pragma: no cover - supports importing as scripts.verify_ai_review_model_freshness.
    from scripts.ai_review_deliverables import sanitize_detail


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "ai-review.toml"
KIMI_CODE_MODEL_RE = re.compile(r"\bkimi-k(?P<version>\d+(?:\.\d+)*)-code(?:-highspeed)?\b")
GLM_TEXT_MODEL_RE = re.compile(r"\bGLM-(?P<version>\d+(?:\.\d+)*)(?!\.\d)(?![-\w])", re.IGNORECASE)
PROVIDER_ALL = "all"
PROVIDER_KIMI = "kimi"
PROVIDER_GLM = "glm"


@dataclass(frozen=True)
class ModelPins:
    kimi: str
    glm: str
    glm_pr_agent: str


@dataclass(frozen=True)
class FreshnessSources:
    kimi_chat_docs_url: str
    kimi_models_url: str
    glm_docs_index_url: str
    glm_migration_docs_url: str
    request_timeout_seconds: int
    github_issues_per_page: int
    issue_marker: str
    issue_title: str


def version_key(version: str) -> tuple[int, ...]:
    return tuple(int(part) for part in version.split("."))


def kimi_version_key(version: str) -> tuple[int, ...]:
    if "." in version or len(version) == 1:
        return version_key(version)
    return (int(version[0]), int(version[1:]))


def kimi_code_model_key(model_id: str) -> tuple[int, ...] | None:
    match = KIMI_CODE_MODEL_RE.search(model_id)
    if not match:
        return None
    return kimi_version_key(match.group("version"))


def same_kimi_code_model(left: str, right: str) -> bool:
    left_key = kimi_code_model_key(left)
    right_key = kimi_code_model_key(right)
    return left_key is not None and left_key == right_key


def provider_enabled(scope: str, provider: str) -> bool:
    return scope == PROVIDER_ALL or scope == provider


def freshness_success_message(*, pins: ModelPins, provider: str) -> str:
    if provider == PROVIDER_KIMI:
        return f"AI review model pins are fresh: Kimi={pins.kimi}"
    if provider == PROVIDER_GLM:
        return f"AI review model pins are fresh: GLM={pins.glm}"
    return f"AI review model pins are fresh: Kimi={pins.kimi}, GLM={pins.glm}"


def load_config(path: Path) -> tuple[ModelPins, FreshnessSources]:
    parsed = tomllib.loads(path.read_text(encoding="utf-8"))
    try:
        glm = parsed["glm"]
        kimi = parsed["kimi"]
        freshness = parsed["model_freshness"]
    except KeyError as exc:
        raise ValueError(f"ci/ai-review.toml missing required table/key: {exc}") from exc

    pr_agent = glm.get("pr_agent")
    if not isinstance(pr_agent, dict):
        raise ValueError("ci/ai-review.toml missing [glm.pr_agent]")

    pins = ModelPins(
        kimi=string_value(kimi, "model", "kimi.model"),
        glm=string_value(glm, "model", "glm.model"),
        glm_pr_agent=string_value(pr_agent, "model", "glm.pr_agent.model"),
    )
    sources = FreshnessSources(
        kimi_chat_docs_url=string_value(freshness, "kimi_chat_docs_url", "model_freshness.kimi_chat_docs_url"),
        kimi_models_url=string_value(freshness, "kimi_models_url", "model_freshness.kimi_models_url"),
        glm_docs_index_url=string_value(freshness, "glm_docs_index_url", "model_freshness.glm_docs_index_url"),
        glm_migration_docs_url=string_value(
            freshness,
            "glm_migration_docs_url",
            "model_freshness.glm_migration_docs_url",
        ),
        request_timeout_seconds=int_value(
            freshness,
            "request_timeout_seconds",
            "model_freshness.request_timeout_seconds",
        ),
        github_issues_per_page=int_value(
            freshness,
            "github_issues_per_page",
            "model_freshness.github_issues_per_page",
        ),
        issue_marker=string_value(freshness, "issue_marker", "model_freshness.issue_marker"),
        issue_title=string_value(freshness, "issue_title", "model_freshness.issue_title"),
    )
    return pins, sources


def string_value(table: dict[str, object], key: str, label: str) -> str:
    value = table.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"ci/ai-review.toml {label} must be a non-empty string")
    return value


def int_value(table: dict[str, object], key: str, label: str) -> int:
    value = table.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"ci/ai-review.toml {label} must be a positive integer")
    return value


def model_alias_findings(pins: ModelPins) -> list[str]:
    findings: list[str] = []
    for label, value in (
        ("kimi.model", pins.kimi),
        ("glm.model", pins.glm),
        ("glm.pr_agent.model", pins.glm_pr_agent),
    ):
        if "latest" in value.lower():
            findings.append(f"{label} must be an exact model id, not a latest alias: {value!r}")
    if pins.glm_pr_agent != f"openai/{pins.glm}":
        findings.append(
            "glm.pr_agent.model must wrap the same exact GLM model as glm.model "
            f"({pins.glm_pr_agent!r} != 'openai/{pins.glm}')"
        )
    return findings


def pick_latest_kimi_code_model(model_ids: Iterable[str]) -> str | None:
    matches: list[tuple[tuple[int, ...], bool, bool, str]] = []
    for model_id in model_ids:
        match = KIMI_CODE_MODEL_RE.search(model_id)
        if match:
            version = match.group("version")
            matches.append((kimi_version_key(version), not model_id.endswith("-highspeed"), "." in version, model_id))
    if not matches:
        return None
    matches.sort(reverse=True)
    return matches[0][3]


def parse_kimi_chat_docs_latest(text: str) -> str | None:
    default_match = re.search(r"default:\s*(kimi-k\d+(?:\.\d+)*-code)\b", text)
    if default_match:
        return default_match.group(1)
    return pick_latest_kimi_code_model(match.group(0) for match in KIMI_CODE_MODEL_RE.finditer(text))


def parse_kimi_models_api_latest(text: str) -> str | None:
    payload = json.loads(text)
    data = payload.get("data")
    if not isinstance(data, list):
        raise ValueError("Kimi models API response missing data list")
    model_ids = [item.get("id") for item in data if isinstance(item, dict) and isinstance(item.get("id"), str)]
    return pick_latest_kimi_code_model(model_ids)


def parse_glm_docs_latest(text: str) -> str | None:
    candidates: list[tuple[tuple[int, ...], str]] = []
    for match in GLM_TEXT_MODEL_RE.finditer(text):
        model = f"glm-{match.group('version')}"
        candidates.append((version_key(match.group("version")), model))
    if not candidates:
        return None
    candidates.sort(reverse=True)
    return candidates[0][1]


def parse_glm_migration_model(text: str) -> str | None:
    match = re.search(r"Update\s+`?model`?\s+to\s+`(glm-\d+(?:\.\d+)*)`", text, re.IGNORECASE)
    return match.group(1).lower() if match else parse_glm_docs_latest(text)


def fetch_text(url: str, token: str | None = None, *, timeout_seconds: int) -> str:
    headers = {"User-Agent": "bolt-v2-ai-review-model-freshness/1.0"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=timeout_seconds) as response:
            return response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise RuntimeError(f"GET {url} failed with HTTP {exc.code}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"GET {url} failed: {exc.reason}") from exc


class GitHubIssueClient:
    def __init__(
        self,
        *,
        repo: str,
        token: str,
        api_url: str,
        request_timeout_seconds: int,
        issues_per_page: int,
    ) -> None:
        self.repo = repo
        self.token = token
        self.api_url = api_url.rstrip("/")
        self.request_timeout_seconds = request_timeout_seconds
        self.issues_per_page = issues_per_page

    def _request_json(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, str] | None = None,
        payload: dict[str, object] | None = None,
    ) -> object:
        query = f"?{urllib.parse.urlencode(params)}" if params else ""
        data = None if payload is None else json.dumps(payload).encode("utf-8")
        request = urllib.request.Request(
            f"{self.api_url}/repos/{self.repo}/{path}{query}",
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
            with urllib.request.urlopen(request, timeout=self.request_timeout_seconds) as response:
                body = response.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            raise RuntimeError(f"GitHub API {method} {path} failed with HTTP {exc.code}: {detail}") from exc
        if not body.strip():
            return {}
        return json.loads(body)

    def list_issues(self, *, state: str) -> list[dict[str, object]]:
        items: list[dict[str, object]] = []
        page = 1
        while True:
            payload = self._request_json(
                "GET",
                "issues",
                params={"state": state, "per_page": str(self.issues_per_page), "page": str(page)},
            )
            if not isinstance(payload, list):
                raise RuntimeError("GitHub issues API returned non-list payload")
            page_items = [item for item in payload if isinstance(item, dict) and "pull_request" not in item]
            items.extend(page_items)
            if len(payload) < self.issues_per_page:
                return items
            page += 1

    def create_issue(self, *, title: str, body: str) -> None:
        self._request_json("POST", "issues", payload={"title": title, "body": body})

    def update_issue(self, issue_number: int, **fields: str) -> None:
        self._request_json("PATCH", f"issues/{issue_number}", payload=dict(fields))


def live_latest_models(
    sources: FreshnessSources,
    *,
    provider: str = PROVIDER_ALL,
) -> tuple[str | None, str | None, list[str]]:
    warnings: list[str] = []
    kimi_latest: str | None = None
    if provider_enabled(provider, PROVIDER_KIMI):
        kimi_token = os.environ.get("KIMI_API_KEY")
        if kimi_token:
            try:
                kimi_latest = parse_kimi_models_api_latest(
                    fetch_text(
                        sources.kimi_models_url,
                        token=kimi_token,
                        timeout_seconds=sources.request_timeout_seconds,
                    )
                )
            except Exception as exc:  # noqa: BLE001 - keep fallback diagnostic actionable.
                warnings.append(
                    "Kimi models API unavailable; falling back to public docs: "
                    f"{sanitize_detail(str(exc))}"
                )
        if kimi_latest is None:
            kimi_latest = parse_kimi_chat_docs_latest(
                fetch_text(sources.kimi_chat_docs_url, timeout_seconds=sources.request_timeout_seconds)
            )

    glm_latest: str | None = None
    if provider_enabled(provider, PROVIDER_GLM):
        glm_latest = parse_glm_docs_latest(
            fetch_text(sources.glm_docs_index_url, timeout_seconds=sources.request_timeout_seconds)
        )
        migration_model = parse_glm_migration_model(
            fetch_text(sources.glm_migration_docs_url, timeout_seconds=sources.request_timeout_seconds)
        )
        if glm_latest and migration_model and glm_latest != migration_model:
            warnings.append(
                "Z.AI docs disagree on latest GLM text model: "
                f"index={glm_latest!r}, migration={migration_model!r}"
            )
        glm_latest = migration_model or glm_latest
    return kimi_latest, glm_latest, warnings


def check_pins_against_latest(
    pins: ModelPins,
    kimi_latest: str | None,
    glm_latest: str | None,
    *,
    provider: str = PROVIDER_ALL,
) -> list[str]:
    findings = model_alias_findings(pins)
    if provider_enabled(provider, PROVIDER_KIMI):
        if kimi_latest is None:
            findings.append("Could not determine latest Kimi coding model from provider sources")
        elif not same_kimi_code_model(pins.kimi, kimi_latest):
            findings.append(f"ci/ai-review.toml kimi.model is stale: {pins.kimi!r}; latest is {kimi_latest!r}")

    if provider_enabled(provider, PROVIDER_GLM):
        if glm_latest is None:
            findings.append("Could not determine latest GLM text coding model from provider sources")
        elif pins.glm != glm_latest:
            findings.append(f"ci/ai-review.toml glm.model is stale: {pins.glm!r}; latest is {glm_latest!r}")
    return findings


def model_update_warning(*, provider: str, config_key: str, current: str, latest: str) -> str:
    return (
        f"{provider} model update available: ci/ai-review.toml {config_key} uses `{current}`, "
        f"but official provider sources report `{latest}` as the latest coding model. "
        f"The review continues with the pinned model; update the model pin in a reviewed PR."
    )


def build_advisory_outputs(
    *,
    pins: ModelPins,
    kimi_latest: str | None,
    glm_latest: str | None,
    warnings: list[str],
    provider: str = PROVIDER_ALL,
) -> dict[str, str]:
    kimi_warning = ""
    glm_warning = ""
    if provider_enabled(provider, PROVIDER_KIMI) and kimi_latest and not same_kimi_code_model(pins.kimi, kimi_latest):
        kimi_warning = model_update_warning(
            provider="Kimi",
            config_key="kimi.model",
            current=pins.kimi,
            latest=kimi_latest,
        )
    if provider_enabled(provider, PROVIDER_GLM) and glm_latest and pins.glm != glm_latest:
        glm_warning = model_update_warning(
            provider="GLM",
            config_key="glm.model",
            current=pins.glm,
            latest=glm_latest,
        )
    stale = bool(kimi_warning or glm_warning)
    return {
        "stale": "true" if stale else "false",
        "kimi_warning": kimi_warning,
        "glm_warning": glm_warning,
        "source_warnings": "\n".join(sanitize_detail(warning) for warning in warnings),
    }


def render_model_freshness_issue_body(*, pins: ModelPins, advisory: dict[str, str], issue_marker: str) -> str:
    warning_lines = [
        warning
        for warning in (advisory.get("kimi_warning", ""), advisory.get("glm_warning", ""))
        if warning
    ]
    if warning_lines:
        summary = "\n".join(f"- {sanitize_detail(warning)}" for warning in warning_lines)
        next_step = "Open a reviewed PR that updates `ci/ai-review.toml` to the provider-confirmed model pins."
    else:
        summary = "- Kimi and GLM model pins match the latest provider-confirmed coding models."
        next_step = "No model pin update is currently required."

    source_warnings = advisory.get("source_warnings", "").strip()
    source_warning_section = ""
    if source_warnings:
        source_warning_section = "\n\nSource warnings:\n" + "\n".join(
            f"- {sanitize_detail(line)}" for line in source_warnings.splitlines()
        )

    return (
        f"{issue_marker}\n\n"
        "## AI review model freshness\n\n"
        f"{summary}\n\n"
        "Current pins:\n"
        f"- Kimi: `{pins.kimi}`\n"
        f"- GLM: `{pins.glm}`\n"
        f"- GLM PR-Agent: `{pins.glm_pr_agent}`"
        f"{source_warning_section}\n\n"
        f"Next step: {next_step}\n"
    )


def model_freshness_issue_number(issue: dict[str, object], *, issue_marker: str) -> int | None:
    number = issue.get("number")
    body = issue.get("body")
    if isinstance(number, int) and isinstance(body, str) and issue_marker in body:
        return number
    return None


def model_freshness_issues(issues: Iterable[dict[str, object]], *, issue_marker: str) -> list[dict[str, object]]:
    return [issue for issue in issues if model_freshness_issue_number(issue, issue_marker=issue_marker) is not None]


def issue_number(issue: dict[str, object], *, issue_marker: str) -> int:
    number = model_freshness_issue_number(issue, issue_marker=issue_marker)
    if number is None:
        raise ValueError("issue is missing model freshness marker or integer number")
    return number


def sync_model_freshness_issue(
    *,
    github: object,
    pins: ModelPins,
    advisory: dict[str, str],
    sources: FreshnessSources,
) -> str:
    body = render_model_freshness_issue_body(pins=pins, advisory=advisory, issue_marker=sources.issue_marker)
    existing_issues = model_freshness_issues(  # type: ignore[attr-defined]
        github.list_issues(state="all"),
        issue_marker=sources.issue_marker,
    )
    existing_issues.sort(key=lambda issue: issue_number(issue, issue_marker=sources.issue_marker), reverse=True)
    existing_issue = existing_issues[0] if existing_issues else None

    if advisory.get("stale") == "true":
        if existing_issue is not None:
            existing_number = issue_number(existing_issue, issue_marker=sources.issue_marker)
            github.update_issue(existing_number, state="open", body=body)  # type: ignore[attr-defined]
            for duplicate in existing_issues[1:]:
                if duplicate.get("state") == "open":
                    github.update_issue(  # type: ignore[attr-defined]
                        issue_number(duplicate, issue_marker=sources.issue_marker),
                        state="closed",
                        body=body,
                    )
            if existing_issue.get("state") == "closed":
                return "issue-reopened"
            return "issue-updated"
        github.create_issue(title=sources.issue_title, body=body)  # type: ignore[attr-defined]
        return "issue-created"

    open_issues = [issue for issue in existing_issues if issue.get("state") == "open"]
    for issue in open_issues:
        github.update_issue(  # type: ignore[attr-defined]
            issue_number(issue, issue_marker=sources.issue_marker),
            state="closed",
            body=body,
        )
    if open_issues:
        return "issue-closed"
    return "issue-not-needed"


def github_issue_client_from_env(sources: FreshnessSources) -> GitHubIssueClient:
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    token = os.environ.get("GITHUB_TOKEN", "")
    api_url = os.environ.get("GITHUB_API_URL", "https://api.github.com")
    if not repo:
        raise RuntimeError("GITHUB_REPOSITORY is required to sync the model freshness issue")
    if not token:
        raise RuntimeError("GITHUB_TOKEN is required to sync the model freshness issue")
    return GitHubIssueClient(
        repo=repo,
        token=token,
        api_url=api_url,
        request_timeout_seconds=sources.request_timeout_seconds,
        issues_per_page=sources.github_issues_per_page,
    )


def write_github_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT", "")
    if not output_path:
        return
    with Path(output_path).open("a", encoding="utf-8") as output:
        if "\n" in value:
            delimiter = f"EOF_{name.upper()}"
            output.write(f"{name}<<{delimiter}\n{value}\n{delimiter}\n")
        else:
            output.write(f"{name}={value}\n")


def write_github_outputs(values: dict[str, str]) -> None:
    for name, value in values.items():
        write_github_output(name, value)


def run_self_test() -> None:
    old_kimi_version = "9.1"
    current_kimi_version = "9.2"
    next_kimi_version = "9.3"
    old_kimi = f"kimi-k{old_kimi_version}-code"
    current_kimi = f"kimi-k{current_kimi_version}-code"
    next_kimi = f"kimi-k{next_kimi_version}-code"
    old_glm_version = "9.1"
    current_glm_version = "9.2"
    old_glm = f"glm-{old_glm_version}"
    current_glm = f"glm-{current_glm_version}"
    current_pr_agent_glm = f"openai/{current_glm}"
    kimi_doc = f"model\ndefault:{current_kimi}\nAvailable options: `{current_kimi}-highspeed`"
    assert parse_kimi_chat_docs_latest(kimi_doc) == current_kimi
    kimi_api = json.dumps({"data": [{"id": old_kimi}, {"id": f"{current_kimi}-highspeed"}, {"id": current_kimi}]})
    assert parse_kimi_models_api_latest(kimi_api) == current_kimi
    kimi_alias_doc = "Available options: `kimi-k2.7-code` `kimi-k2.7-code-highspeed` `kimi-k27-code`"
    assert parse_kimi_chat_docs_latest(kimi_alias_doc) == "kimi-k2.7-code"
    glm_index = f"[GLM-{old_glm_version}](/guides/llm/glm-old.md) [GLM-{current_glm_version}](/guides/llm/glm-current.md)"
    assert parse_glm_docs_latest(glm_index) == current_glm
    assert parse_glm_docs_latest("Variants: GLM-5.2-flash GLM-4.5-Air GLM-4.5V") is None
    assert parse_glm_docs_latest("Variants: GLM-5.2-flash; current standalone GLM-4.6") == "glm-4.6"
    assert parse_glm_docs_latest("Current model is GLM-5.2.") == "glm-5.2"
    migration = f"Migration Checklist\n* Update model identifier to `{current_glm}`"
    assert parse_glm_migration_model(migration) == current_glm
    uppercase_migration = f"Migration Checklist\n* Update model identifier to `{current_glm.upper()}`"
    assert parse_glm_migration_model(uppercase_migration) == current_glm
    explicit_uppercase_migration = f"Migration Checklist\n* Update `model` to `{current_glm.upper()}`"
    assert parse_glm_migration_model(explicit_uppercase_migration) == current_glm
    pins = ModelPins(kimi=old_kimi, glm=old_glm, glm_pr_agent=f"openai/{old_glm}")
    findings = check_pins_against_latest(pins, current_kimi, current_glm)
    assert any("kimi.model is stale" in finding for finding in findings), findings
    assert any("glm.model is stale" in finding for finding in findings), findings
    alias_findings = check_pins_against_latest(
        ModelPins(kimi="kimi-k2.7-code", glm=current_glm, glm_pr_agent=current_pr_agent_glm),
        "kimi-k27-code",
        current_glm,
    )
    assert not any("kimi.model is stale" in finding for finding in alias_findings), alias_findings
    alias_findings = model_alias_findings(ModelPins(kimi="kimi-latest", glm=current_glm, glm_pr_agent=current_pr_agent_glm))
    assert any("latest alias" in finding for finding in alias_findings), alias_findings
    advisory = build_advisory_outputs(
        pins=ModelPins(kimi=current_kimi, glm=current_glm, glm_pr_agent=current_pr_agent_glm),
        kimi_latest=next_kimi,
        glm_latest=current_glm,
        warnings=[],
    )
    assert advisory["stale"] == "true", advisory
    assert advisory["kimi_warning"], advisory
    assert advisory["glm_warning"] == "", advisory
    assert current_kimi in advisory["kimi_warning"], advisory
    assert next_kimi in advisory["kimi_warning"], advisory
    alias_advisory = build_advisory_outputs(
        pins=ModelPins(kimi="kimi-k2.7-code", glm=current_glm, glm_pr_agent=current_pr_agent_glm),
        kimi_latest="kimi-k27-code",
        glm_latest=current_glm,
        warnings=[],
    )
    assert alias_advisory["stale"] == "false", alias_advisory
    assert alias_advisory["kimi_warning"] == "", alias_advisory
    config_text = f"""
[model_freshness]
kimi_chat_docs_url = "https://example.invalid/kimi-chat"
kimi_models_url = "https://example.invalid/kimi-models"
glm_docs_index_url = "https://example.invalid/glm-index"
glm_migration_docs_url = "https://example.invalid/glm-migration"
request_timeout_seconds = 30
github_issues_per_page = 100
issue_marker = "<!-- test-ai-review-model-freshness-issue -->"
issue_title = "Test AI review model pin update available"

[glm]
model = "{current_glm}"

[glm.pr_agent]
model = "{current_pr_agent_glm}"

[kimi]
model = "{current_kimi}"
"""
    original_live_latest_models = live_latest_models
    try:
        globals()["live_latest_models"] = lambda sources, *, provider=PROVIDER_ALL: (next_kimi, current_glm, [])
        with tempfile.TemporaryDirectory() as tmpdir:
            config_path = Path(tmpdir) / "ai-review.toml"
            config_path.write_text(config_text, encoding="utf-8")
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                assert main(["--config-file", str(config_path), "--live", "--advisory"]) == 0
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(io.StringIO()):
                assert main(["--config-file", str(config_path), "--live", "--advisory", "--provider", PROVIDER_GLM]) == 0
            assert f"GLM={current_glm}" in stdout.getvalue(), stdout.getvalue()
            assert "Kimi=" not in stdout.getvalue(), stdout.getvalue()
            stderr = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
                assert main(["--config-file", str(config_path), "--advisory", "--github-notice"]) == 1
            assert "--github-notice requires --live provider freshness data" in stderr.getvalue(), stderr.getvalue()
    finally:
        globals()["live_latest_models"] = original_live_latest_models

    secret = "model-freshness-secret-value"
    previous_secret = os.environ.get("MODEL_FRESHNESS_TEST_API_KEY")
    os.environ["MODEL_FRESHNESS_TEST_API_KEY"] = secret
    try:
        def failing_live_latest_models(
            sources: FreshnessSources,
            *,
            provider: str = PROVIDER_ALL,
        ) -> tuple[str | None, str | None, list[str]]:
            del sources, provider
            raise RuntimeError(f"provider echoed {secret}")

        globals()["live_latest_models"] = failing_live_latest_models
        with tempfile.TemporaryDirectory() as tmpdir:
            config_path = Path(tmpdir) / "ai-review.toml"
            config_path.write_text(config_text, encoding="utf-8")
            stderr = io.StringIO()
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
                assert main(["--config-file", str(config_path), "--live", "--advisory"]) == 0
            assert secret not in stderr.getvalue(), stderr.getvalue()
            assert "***" in stderr.getvalue(), stderr.getvalue()
    finally:
        globals()["live_latest_models"] = original_live_latest_models
        if previous_secret is None:
            os.environ.pop("MODEL_FRESHNESS_TEST_API_KEY", None)
        else:
            os.environ["MODEL_FRESHNESS_TEST_API_KEY"] = previous_secret

    sources = FreshnessSources(
        kimi_chat_docs_url="https://example.invalid/kimi-chat",
        kimi_models_url="https://example.invalid/kimi-models",
        glm_docs_index_url="https://example.invalid/glm-index",
        glm_migration_docs_url="https://example.invalid/glm-migration",
        request_timeout_seconds=30,
        github_issues_per_page=100,
        issue_marker="<!-- test-ai-review-model-freshness-issue -->",
        issue_title="Test AI review model pin update available",
    )
    original_fetch_text = fetch_text
    try:
        def fake_fetch_text(url: str, token: str | None = None, *, timeout_seconds: int) -> str:
            del timeout_seconds
            del token
            if url == sources.kimi_chat_docs_url:
                return f"default:{current_kimi}"
            if url == sources.glm_docs_index_url:
                return f"Archive: [GLM-99.0](/legacy/unavailable.md) Current migration: [GLM-{current_glm_version}](/current.md)"
            if url == sources.glm_migration_docs_url:
                return f"Migration Checklist\n* Update model identifier to `{current_glm}`"
            raise AssertionError(f"unexpected URL {url}")

        globals()["fetch_text"] = fake_fetch_text
        _, glm_from_migration, source_warnings = live_latest_models(sources)
        assert glm_from_migration == current_glm
        assert any("docs disagree" in warning for warning in source_warnings), source_warnings
    finally:
        globals()["fetch_text"] = original_fetch_text

    provider_calls: list[str] = []
    try:
        def fake_provider_fetch_text(url: str, token: str | None = None, *, timeout_seconds: int) -> str:
            del timeout_seconds
            del token
            provider_calls.append(url)
            if url == sources.kimi_chat_docs_url:
                return f"default:{current_kimi}"
            if url == sources.glm_docs_index_url:
                return f"[GLM-{current_glm_version}](/current.md)"
            if url == sources.glm_migration_docs_url:
                return f"Migration Checklist\n* Update model identifier to `{current_glm}`"
            raise AssertionError(f"unexpected URL {url}")

        globals()["fetch_text"] = fake_provider_fetch_text
        scoped_kimi, scoped_glm, _ = live_latest_models(sources, provider=PROVIDER_GLM)
        assert scoped_kimi is None
        assert scoped_glm == current_glm
        assert provider_calls == [sources.glm_docs_index_url, sources.glm_migration_docs_url], provider_calls
    finally:
        globals()["fetch_text"] = original_fetch_text

    class FakeIssueClient:
        def __init__(self, issues: list[dict[str, object]] | None = None) -> None:
            self.issues = list(issues or [])
            self.created: list[dict[str, str]] = []
            self.updated: list[tuple[int, dict[str, str]]] = []

        def list_issues(self, *, state: str) -> list[dict[str, object]]:
            if state == "all":
                return list(self.issues)
            return [issue for issue in self.issues if issue.get("state") == state]

        def create_issue(self, *, title: str, body: str) -> None:
            self.created.append({"title": title, "body": body})

        def update_issue(self, issue_number: int, **fields: str) -> None:
            self.updated.append((issue_number, fields))

    stale_advisory = build_advisory_outputs(
        pins=ModelPins(kimi=current_kimi, glm=old_glm, glm_pr_agent=f"openai/{old_glm}"),
        kimi_latest=current_kimi,
        glm_latest=current_glm,
        warnings=["Z.AI docs disagree on latest GLM text model"],
    )
    fake_issues = FakeIssueClient()
    assert sync_model_freshness_issue(github=fake_issues, pins=pins, advisory=stale_advisory, sources=sources) == "issue-created"
    assert len(fake_issues.created) == 1
    assert sources.issue_marker in fake_issues.created[0]["body"]
    assert "GLM model update available" in fake_issues.created[0]["body"]

    previous_secret = os.environ.get("MODEL_FRESHNESS_TEST_API_KEY")
    os.environ["MODEL_FRESHNESS_TEST_API_KEY"] = secret
    try:
        redacted_advisory = dict(stale_advisory)
        redacted_advisory["source_warnings"] = f"provider echoed {secret}"
        redacted_body = render_model_freshness_issue_body(
            pins=pins,
            advisory=redacted_advisory,
            issue_marker=sources.issue_marker,
        )
        assert secret not in redacted_body, redacted_body
        assert "***" in redacted_body, redacted_body
    finally:
        if previous_secret is None:
            os.environ.pop("MODEL_FRESHNESS_TEST_API_KEY", None)
        else:
            os.environ["MODEL_FRESHNESS_TEST_API_KEY"] = previous_secret

    stale_issue = {
        "number": 321,
        "state": "open",
        "body": f"{sources.issue_marker}\n\nold stale body",
    }
    fake_issues = FakeIssueClient([stale_issue])
    assert sync_model_freshness_issue(github=fake_issues, pins=pins, advisory=stale_advisory, sources=sources) == "issue-updated"
    assert fake_issues.created == []
    assert fake_issues.updated == [
        (
            321,
            {
                "state": "open",
                "body": render_model_freshness_issue_body(
                    pins=pins,
                    advisory=stale_advisory,
                    issue_marker=sources.issue_marker,
                ),
            },
        )
    ]

    closed_stale_issue = {
        "number": 322,
        "state": "closed",
        "body": f"{sources.issue_marker}\n\nclosed stale body",
    }
    fake_issues = FakeIssueClient([closed_stale_issue])
    assert sync_model_freshness_issue(github=fake_issues, pins=pins, advisory=stale_advisory, sources=sources) == "issue-reopened"
    assert fake_issues.created == []
    assert fake_issues.updated == [
        (
            322,
            {
                "state": "open",
                "body": render_model_freshness_issue_body(
                    pins=pins,
                    advisory=stale_advisory,
                    issue_marker=sources.issue_marker,
                ),
            },
        )
    ]

    duplicate_issues = [
        {
            "number": 323,
            "state": "open",
            "body": f"{sources.issue_marker}\n\nolder duplicate",
        },
        {
            "number": 324,
            "state": "closed",
            "body": f"{sources.issue_marker}\n\nnewer closed issue",
        },
    ]
    fake_issues = FakeIssueClient(duplicate_issues)
    assert sync_model_freshness_issue(github=fake_issues, pins=pins, advisory=stale_advisory, sources=sources) == "issue-reopened"
    assert fake_issues.created == []
    stale_body = render_model_freshness_issue_body(
        pins=pins,
        advisory=stale_advisory,
        issue_marker=sources.issue_marker,
    )
    assert fake_issues.updated == [
        (324, {"state": "open", "body": stale_body}),
        (323, {"state": "closed", "body": stale_body}),
    ]

    fresh_advisory = build_advisory_outputs(
        pins=ModelPins(kimi=current_kimi, glm=current_glm, glm_pr_agent=current_pr_agent_glm),
        kimi_latest=current_kimi,
        glm_latest=current_glm,
        warnings=[],
    )
    fake_issues = FakeIssueClient([stale_issue])
    assert sync_model_freshness_issue(github=fake_issues, pins=pins, advisory=fresh_advisory, sources=sources) == "issue-closed"
    assert fake_issues.updated == [
        (
            321,
            {
                "state": "closed",
                "body": render_model_freshness_issue_body(
                    pins=pins,
                    advisory=fresh_advisory,
                    issue_marker=sources.issue_marker,
                ),
            },
        )
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config-file", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--live", action="store_true", help="Fetch provider sources and compare pinned models")
    parser.add_argument("--advisory", action="store_true", help="Emit stale-model outputs but exit zero for drift")
    parser.add_argument("--github-notice", action="store_true", help="Create, update, or close the durable GitHub stale-model issue")
    parser.add_argument(
        "--provider",
        choices=(PROVIDER_ALL, PROVIDER_KIMI, PROVIDER_GLM),
        default=PROVIDER_ALL,
        help="Limit live freshness checks to one review provider",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        run_self_test()
        print("AI review model freshness self-tests OK")
        return 0

    try:
        pins, sources = load_config(args.config_file)
        findings = model_alias_findings(pins)
        kimi_latest: str | None = None
        glm_latest: str | None = None
        warnings: list[str] = []
        if args.live:
            try:
                kimi_latest, glm_latest, warnings = live_latest_models(sources, provider=args.provider)
            except Exception as exc:
                if not args.advisory:
                    raise
                warnings = [f"AI review model freshness source check unavailable: {sanitize_detail(str(exc))}"]
            for warning in warnings:
                print(f"warning: {sanitize_detail(warning)}", file=sys.stderr)
            findings = check_pins_against_latest(pins, kimi_latest, glm_latest, provider=args.provider)
        if args.advisory:
            advisory = build_advisory_outputs(
                pins=pins,
                kimi_latest=kimi_latest,
                glm_latest=glm_latest,
                warnings=warnings,
                provider=args.provider,
            )
            write_github_outputs(advisory)
            if args.github_notice:
                if not args.live:
                    raise RuntimeError("--github-notice requires --live provider freshness data")
                print(
                    sync_model_freshness_issue(
                        github=github_issue_client_from_env(sources),
                        pins=pins,
                        advisory=advisory,
                        sources=sources,
                    )
                )
            if advisory["stale"] == "true":
                for warning in (advisory["kimi_warning"], advisory["glm_warning"]):
                    if warning:
                        print(f"warning: {sanitize_detail(warning)}", file=sys.stderr)
                return 0
            if findings:
                for finding in findings:
                    print(f"warning: {sanitize_detail(finding)}", file=sys.stderr)
                return 0
        if findings:
            for finding in findings:
                print(f"ERROR: {sanitize_detail(finding)}", file=sys.stderr)
            return 1
    except Exception as exc:  # noqa: BLE001 - script should report clean verifier errors.
        print(f"ERROR: {sanitize_detail(str(exc))}", file=sys.stderr)
        return 1

    if args.live:
        print(freshness_success_message(pins=pins, provider=args.provider))
    else:
        print("AI review model pins use exact, internally consistent model ids")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
