#!/usr/bin/env python3
"""Verify AI PR review governance mirrors stay current."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class MirrorRule:
    name: str
    agents_snippets: tuple[str, ...]
    pr_agent_snippets: tuple[str, ...]


AGENTS_BACKPOINTER_SNIPPETS = (
    ".pr_agent.toml` mirrors the critical AI-review subset for PR-Agent",
    "scripts/verify_ai_review_governance.py` checks the mirror in CI",
)

PR_AGENT_MIRROR_NOTE_SNIPPETS = (
    "This mirror is checked by scripts/verify_ai_review_governance.py",
    "update this block when the mirrored AGENTS.md rules change",
)

MIRRORED_RULES = (
    MirrorRule(
        "scope discipline",
        (
            "One branch or PR may cover only one declared issue, spec, task, or an explicitly named slice",
            "Reviewers must flag out-of-scope changes, hidden adjacent issue work, and missing claimed scope",
        ),
        (
            "Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice",
            "flag out-of-scope changes, hidden adjacent work, and missing claimed scope",
        ),
    ),
    MirrorRule(
        "no hardcodes",
        (
            "**NO HARDCODES**",
            "every runtime value comes from TOML config",
        ),
        (
            "NO HARDCODES: every runtime value comes from TOML config",
            "no string literals for IDs, quantities, timeouts, or any runtime value in code",
        ),
    ),
    MirrorRule(
        "no dual paths",
        (
            "**NO DUAL PATHS**",
            "one way to do each thing",
        ),
        (
            "NO DUAL PATHS: one way to do each thing",
            "one config format, one secret source, one build path",
        ),
    ),
    MirrorRule(
        "no debts",
        (
            "**NO DEBTS**",
            "no TODO, no \"fix later\", no unpinned dependencies, no uncommitted work",
        ),
        (
            "NO DEBTS: no TODO, no \"fix later\", no unpinned dependencies, no uncommitted work",
        ),
    ),
    MirrorRule(
        "no credential display",
        (
            "**NO CREDENTIAL DISPLAY**",
            "never cat/print/log API keys, private keys, secrets",
        ),
        (
            "NO CREDENTIAL DISPLAY: never cat, print, or log API keys, private keys, or secrets",
        ),
    ),
    MirrorRule(
        "ssm secret source",
        (
            "**SSM IS THE SINGLE SECRET SOURCE**",
            "No AWS CLI subprocess, no 1Password CLI, no environment variable fallbacks",
        ),
        (
            "SSM is the single secret source for runtime credentials",
            "do not add environment variable fallbacks or alternate secret backends in product code",
        ),
    ),
    MirrorRule(
        "evidence-driven verification",
        (
            "Every claim must map to evidence",
            "External review: only after local findings are resolved and exact-head CI or the user-approved equivalent is green",
        ),
        (
            "Evidence-driven verification: every claim must map to tests",
            "External review happens only after local findings are resolved and exact-head CI or a user-approved equivalent is green",
        ),
    ),
    MirrorRule(
        "remote-first rust verification",
        (
            "Do not run local compile-heavy Rust verification by default",
            "Use local non-compile gates for fast feedback",
        ),
        (
            "Remote-first Rust verification: do not request local compile-heavy Rust checks by default",
            "request remote CI or allowed static checks",
        ),
    ),
    MirrorRule(
        "required human review",
        (
            "Agents must not merge, squash, rebase-merge, or otherwise land code until the PR has approval",
        ),
        (
            "Required human review must be preserved",
            "agents must not merge or bypass the required reviewer gate",
        ),
    ),
)

KIMI_BASE_GOVERNANCE_SNIPPETS = (
    "ref: ${{ github.event.pull_request.base.sha }}",
    "path: .ai-review/base",
    "sparse-checkout: |",
    "AGENTS.md",
    "cat .ai-review/base/AGENTS.md",
)

GLM_DELIVERABLE_SNIPPETS = (
    "ci/ai-review.toml",
    "Load AI review runtime config",
    "ref: ${{ github.event.pull_request.base.sha }}",
    'emit("expected_bot_login", github["expected_bot_login"])',
    'emit("glm_api_base", glm["api_base"])',
    'def notice_marker(table, marker_key):',
    'emit("glm_notice_marker", notice_marker(glm, "comment_marker"))',
    'emit("glm_pr_agent_model", pr_agent["model"])',
    'emit("glm_pr_agent_fallback_models", pr_agent["fallback_models"])',
    'emit("glm_primary_timeout_minutes", workflow["primary_timeout_minutes"])',
    "OPENAI__API_BASE: ${{ steps.runtime-config.outputs.glm_api_base }}",
    "config.model: ${{ steps.runtime-config.outputs.glm_pr_agent_model }}",
    "config.fallback_models: ${{ steps.runtime-config.outputs.glm_pr_agent_fallback_models }}",
    "config.large_patch_policy: ${{ steps.runtime-config.outputs.glm_pr_agent_large_patch_policy }}",
    "Capture GLM review window",
    "id: pr-agent",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.glm_primary_timeout_minutes) }}",
    "OPENAI_KEY: ${{ env.GLM_API_KEY }}",
    "Stamp GLM PR-Agent review source",
    "id: glm_stamp",
    "scripts/ai_review_deliverables.py glm-stamp",
    "Ensure GLM deliverable or post split fallback",
    "id: glm_fallback",
    "continue-on-error: true",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.glm_fallback_timeout_minutes) }}",
    "scripts/ai_review_deliverables.py glm-fallback",
    "--instructions-file .pr_agent.toml",
    "--config-file ci/ai-review.toml",
    "Post GLM fallback infrastructure failure notice",
    "&& always()",
    "steps.runtime-config.outcome == 'failure'",
    "steps.glm_fallback.outputs.failure_notice_posted != 'true'",
    "review infrastructure failed or timed out before posting a usable failure notice",
    'marker="${{ steps.runtime-config.outputs.glm_notice_marker }}"',
    "gh api \"repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments\" --paginate",
    "gh api --method PATCH \"repos/${GITHUB_REPOSITORY}/issues/comments/${existing_id}\"",
    "gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"",
    "GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
)

KIMI_DELIVERABLE_SNIPPETS = (
    "AI Review - Kimi CLI",
    "group: ai-review-kimi-cli-pr-${{ github.event.pull_request.number }}",
    "kimi-cli-review:",
    "name: Kimi CLI review",
    "ci/ai-review.toml",
    "Load AI review runtime config",
    'emit("expected_bot_login", github["expected_bot_login"])',
    'emit("kimi_model_name", kimi["model"])',
    'emit("kimi_model_base_url", kimi["api_base"])',
    'emit("kimi_deliverable_marker", kimi["deliverable_marker"])',
    'def notice_marker(table, marker_key):',
    'emit("kimi_notice_marker", notice_marker(kimi, "deliverable_marker"))',
    'emit("kimi_node_version", workflow["node_version"])',
    "id: kimi-review",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.kimi_primary_timeout_minutes) }}",
    "uses: actions/setup-node@2028fbc5c25fe9cf00d9f06a71cc4710d4507903 # v6.0.0",
    "node-version: ${{ steps.runtime-config.outputs.kimi_node_version }}",
    "npm install -g \"$KIMI_CLI_PACKAGE\"",
    "KIMI_CLI_PACKAGE: ${{ steps.runtime-config.outputs.kimi_cli_package }}",
    "KIMI_CODE_HOME: ${{ runner.temp }}/kimi-code",
    "KIMI_MODEL_NAME: ${{ steps.runtime-config.outputs.kimi_model_name }}",
    "KIMI_MODEL_API_KEY: ${{ env.KIMI_API_KEY }}",
    "KIMI_MODEL_BASE_URL: ${{ steps.runtime-config.outputs.kimi_model_base_url }}",
    "KIMI_DELIVERABLE_MARKER: ${{ steps.runtime-config.outputs.kimi_deliverable_marker }}",
    ".ai-review/base/scripts/ai_review_deliverables.py kimi-review",
    "--config-file .ai-review/base/ci/ai-review.toml",
    "Capture Kimi review window",
    "Ensure Kimi deliverable or post split fallback",
    "id: kimi_fallback",
    "continue-on-error: true",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.kimi_fallback_timeout_minutes) }}",
    ".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback",
    "--instructions-file .ai-review/standards/kimi-review-standards.md",
    "Post Kimi fallback infrastructure failure notice",
    "&& always()",
    "steps.install-kimi.outcome == 'failure'",
    "steps.runtime-config.outcome == 'failure'",
    "steps.kimi_fallback.outputs.failure_notice_posted != 'true'",
    "review infrastructure failed or timed out before posting a usable failure notice",
    'marker="${{ steps.runtime-config.outputs.kimi_notice_marker }}"',
    "gh api \"repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments\" --paginate",
    "gh api --method PATCH \"repos/${GITHUB_REPOSITORY}/issues/comments/${existing_id}\"",
    "gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"",
    "GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
)

SMOKE_TRUSTED_CONFIG_SNIPPETS = (
    "ref: ${{ github.event.pull_request.base.sha }}",
    "path: .ai-review/smoke-base",
    "sparse-checkout: ci/ai-review.toml",
    "id: trusted-config",
    "Path(\".ai-review/smoke-base/ci/ai-review.toml\")",
    "steps.trusted-config.outputs.available == 'true'",
    "Future smoke runs use the base-branch config before sending provider secrets",
)

AI_REVIEW_DELIVERABLES_SNIPPETS = (
    "def review_body_is_quality_deliverable(",
    "output_contract.finding_required_labels",
    "output_contract.finding_guidance",
    "output_contract.no_findings_required_labels",
    "output_contract.no_findings_guidance",
    "output_contract.non_deliverable_indicators",
    "output_contract.pr_agent_deliverable_headings",
    "output_contract.pr_agent_disabled_noise",
    "review_output_contract(review_config)",
    "def source_label_from_template(",
    "source_label=source_label_from_template(",
    "def validate_review_responses(",
    "did not meet the hard-evidence output contract",
    "validate_review_responses(responses, config.output_contract)",
)

KIMI_FORBIDDEN_INPUTS = (
    "misospace/pr-reviewer-action",
    "Kimi Misospace",
    "Run Misospace review",
    "ai_base_url:",
    "ai_api_format:",
    "ai_api_key:",
    "KIMI_API_BASE:",
    "system_prompt:",
    "system_prompt_file:",
    "system_prompt_mode:",
)

FORBIDDEN_API_ENDPOINTS = (
    "https://api.z.ai/api/paas/v4",
    "https://api.moonshot.ai/v1",
)

WORKFLOW_FORBIDDEN_RUNTIME_LITERALS = (
    "https://api.z.ai/api/coding/paas/v4",
    "https://api.kimi.com/coding/v1",
    "@moonshot-ai/kimi-code@0.19.0",
    "<!-- ai-pr-reviewer-kimi -->",
    "<!-- ai-pr-reviewer-glm -->",
    "node-version: \"24\"",
    "node-version: '24'",
    "node-version: 24",
)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def pr_agent_extra_instructions(pr_agent_toml: str) -> tuple[str, list[str]]:
    try:
        parsed = tomllib.loads(pr_agent_toml)
    except tomllib.TOMLDecodeError as exc:
        return "", [f".pr_agent.toml invalid TOML: {exc}"]

    reviewer = parsed.get("pr_reviewer")
    if not isinstance(reviewer, dict):
        return "", [".pr_agent.toml missing [pr_reviewer]"]

    extra = reviewer.get("extra_instructions")
    if not isinstance(extra, str) or not extra.strip():
        return "", [".pr_agent.toml missing non-empty pr_reviewer.extra_instructions"]

    return extra, []


def exact_glm_model_id(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"glm-\d+(?:\.\d+)*", value) is not None


def configured_runtime_literals(ai_review_toml: str) -> tuple[str, ...]:
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError:
        return ()
    literals: list[str] = []
    github = parsed.get("github")
    if isinstance(github, dict):
        value = github.get("expected_bot_login")
        if isinstance(value, str) and value:
            literals.append(value)
    for table_name in ("glm", "kimi"):
        table = parsed.get(table_name)
        if isinstance(table, dict):
            for key in (
                "api_base",
                "model",
                "deliverable_marker",
                "deliverable_markers",
                "comment_marker",
                "notice_marker",
                "source_label_template",
                "cli_package",
            ):
                value = table.get(key)
                if isinstance(value, str) and value:
                    literals.append(value)
                elif isinstance(value, list):
                    literals.extend(item for item in value if isinstance(item, str) and item)
            pr_agent = table.get("pr_agent")
            if isinstance(pr_agent, dict):
                value = pr_agent.get("model")
                if isinstance(value, str) and value:
                    literals.append(value)
                value = pr_agent.get("source_label_template")
                if isinstance(value, str) and value:
                    literals.append(value)
                fallback_models = pr_agent.get("fallback_models")
                if isinstance(fallback_models, list):
                    literals.extend(value for value in fallback_models if isinstance(value, str) and value)
            workflow = table.get("workflow")
            if isinstance(workflow, dict):
                node_version = workflow.get("node_version")
                if isinstance(node_version, str) and node_version:
                    literals.append(f'node-version: "{node_version}"')
                    literals.append(f"node-version: '{node_version}'")
                    literals.append(f"node-version: {node_version}")
    return tuple(dict.fromkeys(literals))


def verify_pr_agent_config(pr_agent_toml: str, ai_review_toml: str) -> list[str]:
    findings: list[str] = []
    try:
        parsed = tomllib.loads(pr_agent_toml)
    except tomllib.TOMLDecodeError as exc:
        return [f".pr_agent.toml invalid TOML: {exc}"]

    reviewer = parsed.get("pr_reviewer")
    if not isinstance(reviewer, dict):
        return [".pr_agent.toml missing [pr_reviewer]"]

    expected = (
        ("require_ticket_analysis_review", True),
        ("require_can_be_split_review", True),
    )
    for key, value in expected:
        if reviewer.get(key) is not value:
            findings.append(f".pr_agent.toml pr_reviewer.{key} must be {value!r}")

    expected_source = "workflow stamps the authoritative configured source/model from `ci/ai-review.toml`"
    extra = reviewer.get("extra_instructions", "")
    if expected_source not in extra:
        findings.append(".pr_agent.toml extra_instructions must delegate source/model stamping to ci/ai-review.toml")
    for literal in configured_runtime_literals(ai_review_toml):
        if literal in extra:
            findings.append(f".pr_agent.toml extra_instructions must read AI review runtime value from ci/ai-review.toml, not {literal!r}")

    return findings


def missing_snippets(label: str, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{label} missing expected snippet: {snippet!r}" for snippet in snippets if snippet not in text]


def workflow_step_block(workflow_text: str, step_name: str) -> str:
    lines = workflow_text.splitlines()
    for index, line in enumerate(lines):
        if line.strip() != f"- name: {step_name}":
            continue
        indent = len(line) - len(line.lstrip())
        block = [line]
        for next_line in lines[index + 1 :]:
            stripped = next_line.strip()
            next_indent = len(next_line) - len(next_line.lstrip())
            if stripped and next_indent < indent:
                break
            if stripped.startswith("- name:") and next_indent == indent:
                break
            block.append(next_line)
        return "\n".join(block)
    return ""


def verify_ai_review_config(ai_review_toml: str) -> list[str]:
    findings: list[str] = []
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError as exc:
        return [f"ci/ai-review.toml invalid TOML: {exc}"]

    def table(name: str) -> dict[str, object]:
        value = parsed.get(name)
        if not isinstance(value, dict):
            findings.append(f"ci/ai-review.toml missing [{name}]")
            return {}
        return value

    github = table("github")
    review = table("review")
    output_contract = review.get("output_contract")
    if not isinstance(output_contract, dict):
        output_contract = {}
    pr_agent_output = review.get("pr_agent_output")
    if not isinstance(pr_agent_output, dict):
        pr_agent_output = {}
    glm = table("glm")
    glm_pr_agent = glm.get("pr_agent")
    if not isinstance(glm_pr_agent, dict):
        findings.append("ci/ai-review.toml missing [glm.pr_agent]")
        glm_pr_agent = {}
    glm_workflow = glm.get("workflow")
    if not isinstance(glm_workflow, dict):
        findings.append("ci/ai-review.toml missing [glm.workflow]")
        glm_workflow = {}
    kimi = table("kimi")
    kimi_workflow = kimi.get("workflow")
    if not isinstance(kimi_workflow, dict):
        findings.append("ci/ai-review.toml missing [kimi.workflow]")
        kimi_workflow = {}
    smoke = table("smoke")

    expected_values = (
        ("github.api_url", github.get("api_url"), "https://api.github.com"),
        ("github.server_url", github.get("server_url"), "https://github.com"),
        ("github.expected_bot_login", github.get("expected_bot_login"), "github-actions[bot]"),
        ("review.max_comment_chars", review.get("max_comment_chars"), 60000),
        ("review.response_chars_per_chunk", review.get("response_chars_per_chunk"), 8000),
        (
            "review.output_contract.finding_required_labels",
            output_contract.get("finding_required_labels"),
            ["Severity:", "Evidence:", "Issue:", "Fix / verification:"],
        ),
        (
            "review.output_contract.finding_guidance",
            output_contract.get("finding_guidance"),
            [
                "blocking, high, medium, or low",
                "the smallest relevant snippet or line reference from the supplied chunk",
                "why this is a real behavior, safety, governance, or verification problem",
                "the concrete next step",
            ],
        ),
        (
            "review.output_contract.no_findings_indicator",
            output_contract.get("no_findings_indicator"),
            "No hard-evidence findings",
        ),
        (
            "review.output_contract.no_findings_intro",
            output_contract.get("no_findings_intro"),
            "No hard-evidence findings in this chunk.",
        ),
        (
            "review.output_contract.no_findings_required_labels",
            output_contract.get("no_findings_required_labels"),
            ["Coverage reviewed:", "Evidence basis:", "Risk areas considered:"],
        ),
        (
            "review.output_contract.no_findings_guidance",
            output_contract.get("no_findings_guidance"),
            [
                "<specific changed files or diff areas reviewed in this chunk>.",
                "supplied diff only; no omitted files, logs, or external state were assumed.",
                "correctness, security, workflow safety, verification, and repo-governance impact visible in this chunk.",
            ],
        ),
        (
            "review.output_contract.non_deliverable_indicators",
            output_contract.get("non_deliverable_indicators"),
            ["review did not produce a deliverable", "review notice"],
        ),
        (
            "review.pr_agent_output.deliverable_headings",
            pr_agent_output.get("deliverable_headings"),
            ["## PR Reviewer Guide", "## Incremental PR Reviewer Guide"],
        ),
        (
            "review.pr_agent_output.disabled_noise",
            pr_agent_output.get("disabled_noise"),
            ["ticket compliance analysis", "estimated effort to review", "can be split"],
        ),
        ("glm.api_base", glm.get("api_base"), "https://api.z.ai/api/coding/paas/v4"),
        ("glm.api_timeout_seconds", glm.get("api_timeout_seconds"), 180),
        ("glm.review_max_chunk_chars", glm.get("review_max_chunk_chars"), 60000),
        ("glm.comment_marker", glm.get("comment_marker"), "<!-- ai-pr-reviewer-glm -->"),
        ("glm.notice_marker", glm.get("notice_marker"), "<!-- ai-pr-reviewer-glm-notice -->"),
        ("glm.source_label_template", glm.get("source_label_template"), "GLM direct fallback (`{model}`)"),
        (
            "glm.pr_agent.source_label_template",
            glm_pr_agent.get("source_label_template"),
            "GLM PR-Agent (`{model}`)",
        ),
        ("glm.pr_agent.custom_model_max_tokens", glm_pr_agent.get("custom_model_max_tokens"), 128000),
        ("glm.pr_agent.large_patch_policy", glm_pr_agent.get("large_patch_policy"), "clip"),
        ("glm.pr_agent.timeout_seconds", glm_pr_agent.get("timeout_seconds"), 300),
        ("glm.pr_agent.auto_review", glm_pr_agent.get("auto_review"), True),
        ("glm.pr_agent.auto_describe", glm_pr_agent.get("auto_describe"), False),
        ("glm.pr_agent.auto_improve", glm_pr_agent.get("auto_improve"), False),
        ("glm.workflow.job_timeout_minutes", glm_workflow.get("job_timeout_minutes"), 35),
        ("glm.workflow.primary_timeout_minutes", glm_workflow.get("primary_timeout_minutes"), 8),
        ("glm.workflow.fallback_timeout_minutes", glm_workflow.get("fallback_timeout_minutes"), 20),
        ("glm.workflow.setup_overhead_timeout_minutes", glm_workflow.get("setup_overhead_timeout_minutes"), 7),
        ("kimi.api_base", kimi.get("api_base"), "https://api.kimi.com/coding/v1"),
        ("kimi.model", kimi.get("model"), "kimi-for-coding"),
        ("kimi.provider_type", kimi.get("provider_type"), "kimi"),
        ("kimi.model_max_context_size", kimi.get("model_max_context_size"), 262144),
        ("kimi.default_thinking", kimi.get("default_thinking"), True),
        ("kimi.telemetry_disabled", kimi.get("telemetry_disabled"), True),
        ("kimi.review_max_chunk_chars", kimi.get("review_max_chunk_chars"), 60000),
        ("kimi.deliverable_marker", kimi.get("deliverable_marker"), "<!-- ai-pr-reviewer-kimi -->"),
        ("kimi.notice_marker", kimi.get("notice_marker"), "<!-- ai-pr-reviewer-kimi-notice -->"),
        ("kimi.source_label_template", kimi.get("source_label_template"), "Kimi Code CLI (`{model}`)"),
        ("kimi.cli_package", kimi.get("cli_package"), "@moonshot-ai/kimi-code@0.19.0"),
        ("kimi.workflow.node_version", kimi_workflow.get("node_version"), "24"),
        ("kimi.workflow.job_timeout_minutes", kimi_workflow.get("job_timeout_minutes"), 45),
        ("kimi.workflow.primary_timeout_minutes", kimi_workflow.get("primary_timeout_minutes"), 20),
        ("kimi.workflow.fallback_timeout_minutes", kimi_workflow.get("fallback_timeout_minutes"), 20),
        ("kimi.workflow.setup_overhead_timeout_minutes", kimi_workflow.get("setup_overhead_timeout_minutes"), 5),
        ("smoke.max_tokens", smoke.get("max_tokens"), 16),
        ("smoke.workflow.job_timeout_minutes", (smoke.get("workflow") if isinstance(smoke.get("workflow"), dict) else {}).get("job_timeout_minutes"), 10),
    )
    for name, actual, expected in expected_values:
        if actual != expected:
            findings.append(f"ci/ai-review.toml {name} must be {expected!r}, got {actual!r}")

    expected_glm_markers = [
        "## PR Reviewer Guide",
        "## Incremental PR Reviewer Guide",
        "<!-- ai-pr-reviewer-glm -->",
    ]
    if glm.get("deliverable_markers") != expected_glm_markers:
        findings.append("ci/ai-review.toml glm.deliverable_markers must include PR-Agent and GLM fallback markers")
    glm_model = glm.get("model")
    kimi_model = kimi.get("model")
    glm_pr_agent_model = glm_pr_agent.get("model")
    expected_glm_pr_agent_model = f"openai/{glm_model}" if isinstance(glm_model, str) else ""
    if not exact_glm_model_id(glm_model):
        findings.append("ci/ai-review.toml glm.model must be an exact GLM model id, not an alias")
    if "latest" in str(glm_model).lower() or "latest" in str(kimi_model).lower():
        findings.append("ci/ai-review.toml AI review models must not use latest aliases")
    if glm_pr_agent_model != expected_glm_pr_agent_model:
        findings.append("ci/ai-review.toml glm.pr_agent.model must wrap the same exact GLM model as glm.model")
    if glm_pr_agent.get("fallback_models") != [expected_glm_pr_agent_model]:
        findings.append("ci/ai-review.toml glm.pr_agent.fallback_models must contain only glm.pr_agent.model")

    return findings


def verify_review_job_timeout_budget(
    ai_review_toml: str,
    workflow_text: str,
    provider: str,
    setup_required: bool,
) -> list[str]:
    findings: list[str] = []
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError:
        return findings

    provider_config = parsed.get(provider)
    if not isinstance(provider_config, dict):
        return findings
    workflow = provider_config.get("workflow")
    if not isinstance(workflow, dict):
        return findings

    job_timeout = workflow.get("job_timeout_minutes")
    primary_timeout = workflow.get("primary_timeout_minutes")
    fallback_timeout = workflow.get("fallback_timeout_minutes")
    setup_overhead = workflow.get("setup_overhead_timeout_minutes")
    provider_name = {"glm": "GLM", "kimi": "Kimi", "smoke": "Smoke"}.get(provider, provider)
    expected_line = f"    timeout-minutes: {job_timeout}"
    if isinstance(job_timeout, int) and expected_line not in workflow_text:
        findings.append(
            f"{provider_name} workflow job timeout must match ci/ai-review.toml "
            f"{provider}.workflow.job_timeout_minutes ({job_timeout})"
        )

    if not setup_required:
        return findings

    if not all(isinstance(value, int) for value in (job_timeout, primary_timeout, fallback_timeout, setup_overhead)):
        return findings

    required_timeout = primary_timeout + fallback_timeout + setup_overhead
    if job_timeout < required_timeout:
        findings.append(
            f"ci/ai-review.toml {provider}.workflow.job_timeout_minutes must cover "
            "primary_timeout_minutes + fallback_timeout_minutes + setup_overhead_timeout_minutes"
        )

    return findings


def verify_texts(
    *,
    agents_md: str,
    pr_agent_toml: str,
    ai_review_toml: str,
    ai_review_deliverables: str,
    glm_workflow: str,
    kimi_workflow: str,
    smoke_workflow: str,
) -> list[str]:
    findings: list[str] = []

    extra, extra_findings = pr_agent_extra_instructions(pr_agent_toml)
    findings.extend(extra_findings)
    if extra_findings:
        return findings

    findings.extend(missing_snippets("AGENTS.md", agents_md, AGENTS_BACKPOINTER_SNIPPETS))
    findings.extend(missing_snippets(".pr_agent.toml extra_instructions", extra, PR_AGENT_MIRROR_NOTE_SNIPPETS))
    findings.extend(verify_ai_review_config(ai_review_toml))
    findings.extend(verify_pr_agent_config(pr_agent_toml, ai_review_toml))
    findings.extend(missing_snippets("scripts/ai_review_deliverables.py", ai_review_deliverables, AI_REVIEW_DELIVERABLES_SNIPPETS))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, glm_workflow, "glm", setup_required=True))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, kimi_workflow, "kimi", setup_required=True))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, smoke_workflow, "smoke", setup_required=False))

    for rule in MIRRORED_RULES:
        for snippet in rule.agents_snippets:
            if snippet not in agents_md:
                findings.append(
                    f"AGENTS.md source for mirrored rule {rule.name!r} changed or disappeared: {snippet!r}"
                )
        for snippet in rule.pr_agent_snippets:
            if snippet not in extra:
                findings.append(
                    f".pr_agent.toml missing mirrored AGENTS.md rule {rule.name!r}: {snippet!r}"
                )

    if "pr_reviewer." in glm_workflow:
        findings.append(
            "GLM workflow must not define pr_reviewer.* overrides; keep reviewer behavior in .pr_agent.toml"
        )
    for workflow_name, workflow in (
        ("GLM workflow", glm_workflow),
        ("Kimi workflow", kimi_workflow),
        ("Smoke workflow", smoke_workflow),
    ):
        if "ai_review_deliverables.py notice" in workflow:
            findings.append(f"{workflow_name} backup/skip notices must not depend on ai_review_deliverables.py")
        for endpoint in FORBIDDEN_API_ENDPOINTS:
            if endpoint in workflow:
                findings.append(f"{workflow_name} must use coding-plan endpoint/model, not {endpoint!r}")
        for literal in WORKFLOW_FORBIDDEN_RUNTIME_LITERALS:
            if literal in workflow:
                findings.append(f"{workflow_name} must read AI review runtime value from ci/ai-review.toml, not {literal!r}")
        for literal in configured_runtime_literals(ai_review_toml):
            if literal in workflow:
                findings.append(f"{workflow_name} must read AI review runtime value from ci/ai-review.toml, not {literal!r}")

    findings.extend(missing_snippets("GLM workflow", glm_workflow, GLM_DELIVERABLE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_BASE_GOVERNANCE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_DELIVERABLE_SNIPPETS))
    findings.extend(missing_snippets("Smoke workflow", smoke_workflow, SMOKE_TRUSTED_CONFIG_SNIPPETS))
    glm_stamp_step = workflow_step_block(glm_workflow, "Stamp GLM PR-Agent review source")
    if not glm_stamp_step:
        findings.append("GLM workflow missing Stamp GLM PR-Agent review source step")
    elif "continue-on-error: true" in glm_stamp_step:
        findings.append("GLM source stamp step must fail closed")
    for snippet in KIMI_FORBIDDEN_INPUTS:
        if snippet in kimi_workflow:
            findings.append(f"Kimi workflow must use the official Kimi CLI path, not {snippet!r}")

    return findings


def verify_repo(repo_root: Path) -> list[str]:
    return verify_texts(
        agents_md=read_text(repo_root / "AGENTS.md"),
        pr_agent_toml=read_text(repo_root / ".pr_agent.toml"),
        ai_review_toml=read_text(repo_root / "ci/ai-review.toml"),
        ai_review_deliverables=read_text(repo_root / "scripts/ai_review_deliverables.py"),
        glm_workflow=read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml"),
        kimi_workflow=read_text(repo_root / ".github/workflows/ai-review-kimi-cli.yml"),
        smoke_workflow=read_text(repo_root / ".github/workflows/ai-review-coding-plan-smoke.yml"),
    )


def assert_finding(name: str, findings: list[str], expected: str) -> None:
    if not any(expected in finding for finding in findings):
        raise AssertionError(f"{name}: expected finding containing {expected!r}, got {findings!r}")


def run_self_tests(repo_root: Path) -> None:
    agents = read_text(repo_root / "AGENTS.md")
    pr_agent = read_text(repo_root / ".pr_agent.toml")
    ai_review = read_text(repo_root / "ci/ai-review.toml")
    ai_review_config = tomllib.loads(ai_review)
    github_config = ai_review_config["github"]
    glm_config = ai_review_config["glm"]
    current_glm_model = ai_review_config["glm"]["model"]
    current_glm_pr_agent_model = ai_review_config["glm"]["pr_agent"]["model"]
    current_bot_login = github_config["expected_bot_login"]
    current_glm_comment_marker = glm_config["comment_marker"]
    current_glm_api_base = glm_config["api_base"]
    deliverables = read_text(repo_root / "scripts/ai_review_deliverables.py")
    glm = read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml")
    kimi = read_text(repo_root / ".github/workflows/ai-review-kimi-cli.yml")
    smoke = read_text(repo_root / ".github/workflows/ai-review-coding-plan-smoke.yml")

    baseline = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    if baseline:
        raise AssertionError(f"real repository must satisfy AI review governance check, got {baseline!r}")

    wrong_ai_review_config = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review.replace("https://api.z.ai/api/coding/paas/v4", "https://api.z.ai/api/paas/v4"),
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("wrong AI review config endpoint", wrong_ai_review_config, "ci/ai-review.toml glm.api_base")

    workflow_runtime_literal = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm + f"\n          GLM_MODEL: {current_glm_model}\n",
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("workflow runtime literal", workflow_runtime_literal, "must read AI review runtime value")

    workflow_bot_literal = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm + f"\n          EXPECTED_BOT_LOGIN: {current_bot_login}\n",
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("workflow bot login literal", workflow_bot_literal, "must read AI review runtime value")

    workflow_glm_marker_literal = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm + f"\n          GLM_MARKER: {current_glm_comment_marker}\n",
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("workflow GLM marker literal", workflow_glm_marker_literal, "must read AI review runtime value")

    stamp_continue_on_error = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm.replace(
            "      - name: Stamp GLM PR-Agent review source\n        id: glm_stamp\n        if: env.GLM_API_KEY != ''\n        env:",
            "      - name: Stamp GLM PR-Agent review source\n        id: glm_stamp\n        if: env.GLM_API_KEY != ''\n        continue-on-error: true\n        env:",
            1,
        ),
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("stamp continue-on-error", stamp_continue_on_error, "GLM source stamp step must fail closed")

    missing_stamp_step = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm.replace(
            "      - name: Stamp GLM PR-Agent review source\n",
            "      - name: Stamp GLM PR-Agent review source disabled\n",
            1,
        ),
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("missing stamp step", missing_stamp_step, "GLM workflow missing Stamp GLM PR-Agent review source step")

    last_step_then_later_job = "\n".join(
        [
            "jobs:",
            "  review:",
            "    steps:",
            "      - name: Stamp GLM PR-Agent review source",
            "        run: echo stamp",
            "  later:",
            "    steps:",
            "      - name: Later allowed step",
            "        continue-on-error: true",
        ]
    )
    stamp_block = workflow_step_block(last_step_then_later_job, "Stamp GLM PR-Agent review source")
    if "continue-on-error: true" in stamp_block:
        raise AssertionError("workflow_step_block included a later job in the stamp step block")

    pr_agent_model_literal = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent.replace(
            "the workflow stamps the authoritative configured source/model from `ci/ai-review.toml`",
            f"the workflow stamps the authoritative configured source/model from `ci/ai-review.toml` (`{current_glm_pr_agent_model}`)",
        ),
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("PR-Agent model literal", pr_agent_model_literal, ".pr_agent.toml extra_instructions must read AI review runtime value")

    smoke_runtime_literal = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke + f"\n          GLM_API_BASE: {current_glm_api_base}\n",
    )
    assert_finding("smoke workflow runtime literal", smoke_runtime_literal, "must read AI review runtime value")

    glm_short_job_timeout = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm.replace("timeout-minutes: 35", "timeout-minutes: 20"),
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("GLM short job timeout", glm_short_job_timeout, "GLM workflow job timeout must match")

    kimi_short_job_timeout = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi.replace("timeout-minutes: 45", "timeout-minutes: 35"),
        smoke_workflow=smoke,
    )
    assert_finding("Kimi short job timeout", kimi_short_job_timeout, "Kimi workflow job timeout must match")

    smoke_job_timeout_drift = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke.replace("timeout-minutes: 10", "timeout-minutes: 8"),
    )
    assert_finding("Smoke job timeout drift", smoke_job_timeout_drift, "Smoke workflow job timeout must match")

    smoke_head_config = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke.replace(
            "ref: ${{ github.event.pull_request.base.sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
    )
    assert_finding("Smoke head config", smoke_head_config, "Smoke workflow missing expected snippet")

    missing_mirror = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent.replace("NO HARDCODES: every runtime value comes from TOML config", "NO HARDCODES"),
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("missing PR-Agent mirror", missing_mirror, ".pr_agent.toml missing mirrored")

    changed_source = verify_texts(
        agents_md=agents.replace("**NO DUAL PATHS**", "**NO MULTI PATHS**"),
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("changed AGENTS source", changed_source, "AGENTS.md source for mirrored rule 'no dual paths'")

    glm_split_config = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm + "\n          pr_reviewer.num_max_findings: \"6\"\n",
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("GLM split config", glm_split_config, "must not define pr_reviewer.*")

    glm_missing_fallback = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm.replace("scripts/ai_review_deliverables.py glm-fallback", "echo missing"),
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding("GLM missing fallback", glm_missing_fallback, "GLM workflow missing expected snippet")

    glm_missing_infrastructure_notice = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm.replace("gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"", "python3 scripts/ai_review_deliverables.py notice"),
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding(
        "GLM helper-based fallback infrastructure notice",
        glm_missing_infrastructure_notice,
        "must not depend on ai_review_deliverables.py",
    )

    kimi_head_governance = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi.replace(
            "ref: ${{ github.event.pull_request.base.sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
        smoke_workflow=smoke,
    )
    assert_finding("Kimi head governance", kimi_head_governance, "Kimi workflow missing expected snippet")

    kimi_misospace_action = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi + "\n      - uses: misospace/pr-reviewer-action@deadbeef\n",
        smoke_workflow=smoke,
    )
    assert_finding("Kimi Misospace action", kimi_misospace_action, "must use the official Kimi CLI path")

    kimi_prompt_override = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi + "\n          system_prompt_mode: append\n",
        smoke_workflow=smoke,
    )
    assert_finding("Kimi prompt override", kimi_prompt_override, "must use the official Kimi CLI path")

    kimi_missing_fallback = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi.replace(".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback", "echo missing"),
        smoke_workflow=smoke,
    )
    assert_finding("Kimi missing fallback", kimi_missing_fallback, "Kimi workflow missing expected snippet")

    kimi_missing_infrastructure_notice = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables,
        glm_workflow=glm,
        kimi_workflow=kimi.replace("gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"", "python3 .ai-review/base/scripts/ai_review_deliverables.py notice"),
        smoke_workflow=smoke,
    )
    assert_finding(
        "Kimi helper-based fallback infrastructure notice",
        kimi_missing_infrastructure_notice,
        "must not depend on ai_review_deliverables.py",
    )

    missing_quality_gate = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        ai_review_toml=ai_review,
        ai_review_deliverables=deliverables.replace("def validate_review_responses(", "def removed_validate_review_responses("),
        glm_workflow=glm,
        kimi_workflow=kimi,
        smoke_workflow=smoke,
    )
    assert_finding(
        "missing review quality gate",
        missing_quality_gate,
        "scripts/ai_review_deliverables.py missing expected snippet",
    )


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    repo_root = args.repo_root.resolve()

    if args.self_test:
        run_self_tests(repo_root)
        print("AI review governance self-tests OK")
        return 0

    findings = verify_repo(repo_root)
    if findings:
        for finding in findings:
            print(finding, file=sys.stderr)
        return 1

    print("AI review governance mirror OK")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
