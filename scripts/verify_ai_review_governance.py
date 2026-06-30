#!/usr/bin/env python3
"""Verify AI PR review governance config and workflows stay current."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from enum import Enum
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


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
    'smart = glm.get("smart_trigger") or {}',
    'emit("glm_review_labels", smart.get("review_labels", []))',
    'def notice_marker(table):',
    'marker = table.get("notice_marker")',
    'emit("glm_notice_marker", notice_marker(glm))',
    'raise RuntimeError("notice_marker is required")',
    'emit("glm_pr_agent_model", pr_agent["model"])',
    'emit("glm_pr_agent_fallback_models", pr_agent["fallback_models"])',
    'emit("glm_primary_timeout_minutes", workflow["primary_timeout_minutes"])',
    "scripts/verify_ai_review_model_freshness.py",
    "Check AI review model freshness",
    "--advisory",
    "Decide whether GLM should review",
    "import sys",
    "GLM_REVIEW_LABELS: ${{ steps.runtime-config.outputs.glm_review_labels }}",
    "scripts/ai_review_deliverables.py",
    "retry-needed",
    "--provider",
    "glm",
    "previous-failure-notice",
    "retry-check-unavailable",
    "file=sys.stderr",
    "OPENAI__API_BASE: ${{ steps.runtime-config.outputs.glm_api_base }}",
    "config.model: ${{ steps.runtime-config.outputs.glm_pr_agent_model }}",
    "config.fallback_models: ${{ steps.runtime-config.outputs.glm_pr_agent_fallback_models }}",
    "config.large_patch_policy: ${{ steps.runtime-config.outputs.glm_pr_agent_large_patch_policy }}",
    "Capture GLM review window",
    "id: pr-agent",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.glm_primary_timeout_minutes) }}",
    "OPENAI_KEY: ${{ secrets.GLM_API_KEY }}",
    "Stamp GLM PR-Agent review source",
    "id: glm_stamp",
    "scripts/ai_review_deliverables.py glm-stamp",
    "Ensure GLM deliverable or post split fallback",
    "id: glm_fallback",
    "steps.base-checkout.outcome == 'success'",
    "steps.runtime-config.outcome == 'success'",
    "steps.review-decision.outputs.should_review == 'true'",
    "steps.review-window.outcome == 'success'",
    "continue-on-error: true",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.glm_fallback_timeout_minutes) }}",
    "AI_REVIEW_MODEL_FRESHNESS_WARNING: ${{ steps.model-freshness.outputs.glm_warning }}",
    "scripts/ai_review_deliverables.py glm-fallback",
    "--instructions-file .pr_agent.toml",
    "--config-file ci/ai-review.toml",
    "Post GLM model freshness advisory",
    "scripts/ai_review_deliverables.py model-freshness-notice",
    "Post GLM fallback infrastructure failure notice",
    "&& always()",
    "steps.runtime-config.outcome == 'failure'",
    "steps.review-window.outcome == 'failure'",
    "steps.glm_fallback.outputs.failure_notice_posted != 'true'",
    "steps.glm_stamp.outcome == 'failure' && steps.glm_fallback.outcome != 'success'",
    "review infrastructure failed or timed out before posting a usable failure notice",
    'marker="${{ steps.runtime-config.outputs.glm_notice_marker }}"',
    "scripts/ai_review_deliverables.py notice-env --provider glm --config-file ci/ai-review.toml",
    "AI review notice marker or expected bot login is unavailable.",
    'eval "$config_exports"',
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
    'smart = kimi.get("smart_trigger") or {}',
    'emit("kimi_review_labels", smart.get("review_labels", []))',
    'emit("kimi_deliverable_marker", kimi["deliverable_marker"])',
    'def notice_marker(table):',
    'marker = table.get("notice_marker")',
    'emit("kimi_notice_marker", notice_marker(kimi))',
    'raise RuntimeError("notice_marker is required")',
    'emit("kimi_node_version", workflow["node_version"])',
    "scripts/verify_ai_review_model_freshness.py",
    "Check AI review model freshness",
    "--advisory",
    "Decide whether Kimi should review",
    "import sys",
    "KIMI_REVIEW_LABELS: ${{ steps.runtime-config.outputs.kimi_review_labels }}",
    ".ai-review/base/scripts/ai_review_deliverables.py",
    "retry-needed",
    "--provider",
    "kimi",
    "previous-failure-notice",
    "retry-check-unavailable",
    "file=sys.stderr",
    "id: kimi-review",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.kimi_primary_timeout_minutes) }}",
    "uses: actions/setup-node@2028fbc5c25fe9cf00d9f06a71cc4710d4507903 # v6.0.0",
    "node-version: ${{ steps.runtime-config.outputs.kimi_node_version }}",
    "npm install -g \"$KIMI_CLI_PACKAGE\"",
    "KIMI_CLI_PACKAGE: ${{ steps.runtime-config.outputs.kimi_cli_package }}",
    "KIMI_CODE_HOME: ${{ runner.temp }}/kimi-code",
    "KIMI_MODEL_NAME: ${{ steps.runtime-config.outputs.kimi_model_name }}",
    "KIMI_MODEL_API_KEY: ${{ secrets.KIMI_API_KEY }}",
    "KIMI_MODEL_BASE_URL: ${{ steps.runtime-config.outputs.kimi_model_base_url }}",
    "KIMI_DELIVERABLE_MARKER: ${{ steps.runtime-config.outputs.kimi_deliverable_marker }}",
    "AI_REVIEW_MODEL_FRESHNESS_WARNING: ${{ steps.model-freshness.outputs.kimi_warning }}",
    ".ai-review/base/scripts/ai_review_deliverables.py kimi-review",
    "--config-file .ai-review/base/ci/ai-review.toml",
    "Capture Kimi review window",
    "Ensure Kimi deliverable or post split fallback",
    "id: kimi_fallback",
    "continue-on-error: true",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.kimi_fallback_timeout_minutes) }}",
    ".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback",
    "--instructions-file .ai-review/standards/kimi-review-standards.md",
    "Post Kimi model freshness advisory",
    ".ai-review/base/scripts/ai_review_deliverables.py model-freshness-notice",
    "Post Kimi fallback infrastructure failure notice",
    "&& always()",
    "steps.install-kimi.outcome == 'failure'",
    "steps.runtime-config.outcome == 'failure'",
    "steps.review-window.outcome == 'failure'",
    "steps.kimi_fallback.outputs.failure_notice_posted != 'true'",
    "review infrastructure failed or timed out before posting a usable failure notice",
    'marker="${{ steps.runtime-config.outputs.kimi_notice_marker }}"',
    ".ai-review/base/scripts/ai_review_deliverables.py notice-env --provider kimi --config-file .ai-review/base/ci/ai-review.toml",
    "AI review notice marker or expected bot login is unavailable.",
    'eval "$config_exports"',
    "gh api \"repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments\" --paginate",
    "gh api --method PATCH \"repos/${GITHUB_REPOSITORY}/issues/comments/${existing_id}\"",
    "gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"",
    "GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
)

CLAUDE_WORKFLOW_SNIPPETS = (
    "Claude Code Review",
    "types: [opened, synchronize, labeled]",
    "github.event.pull_request.draft == false",
    "github.event.pull_request.head.repo.full_name == github.repository",
    "github.event.comment.author_association",
    "github.event.review.author_association",
    "ci/ai-review.toml",
    "Load AI review runtime config",
    'emit("claude_model", claude["model"])',
    'emit("github_api_url", github["api_url"])',
    'emit("claude_track_progress", claude["track_progress"])',
    'emit("claude_deliverable_marker", claude["deliverable_marker"])',
    'emit("claude_source_label", claude["source_label_template"].replace("{model}", claude["model"]))',
    'contract = config["review"]["output_contract"]',
    'emit("review_finding_required_labels", contract["finding_required_labels"])',
    'emit("review_no_findings_indicator", contract["no_findings_indicator"])',
    'emit("review_no_findings_required_labels", contract["no_findings_required_labels"])',
    'emit("claude_allowed_tools", claude["allowed_tools"])',
    'emit("claude_max_turns", workflow["max_turns"])',
    'emit("claude_primary_timeout_minutes", workflow["primary_timeout_minutes"])',
    "GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
    "PR_LABEL_NAME: ${{ github.event.label.name }}",
    'if action == "labeled":',
    "review-label",
    "retry-needed",
    "--provider",
    "claude",
    "previous-failure-notice",
    "retry-check-unavailable",
    "fork-pr-manual-mention",
    "scripts/verify_ai_review_model_freshness.py --live --advisory --provider claude",
    "Capture Claude review window",
    "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2",
    "anthropics/claude-code-action@a92e7c70a4da9793dc164451d829089dc057a464 # v1",
    "contents: read",
    "id-token: write",
    "claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}",
    "additional_permissions: |",
    "track_progress: ${{ steps.runtime-config.outputs.claude_track_progress }}",
    "DELIVERABLE MARKER: ${{ steps.runtime-config.outputs.claude_deliverable_marker }}",
    "SOURCE LABEL: ${{ steps.runtime-config.outputs.claude_source_label }}",
    "FINDING REQUIRED LABELS: ${{ steps.runtime-config.outputs.review_finding_required_labels }}",
    "NO-FINDINGS INDICATOR: ${{ steps.runtime-config.outputs.review_no_findings_indicator }}",
    "NO-FINDINGS REQUIRED LABELS: ${{ steps.runtime-config.outputs.review_no_findings_required_labels }}",
    "timeout-minutes: ${{ fromJSON(steps.runtime-config.outputs.claude_primary_timeout_minutes) }}",
    "--max-turns ${{ steps.runtime-config.outputs.claude_max_turns }}",
    "--model ${{ steps.runtime-config.outputs.claude_model }}",
    '--allowedTools "${{ steps.runtime-config.outputs.claude_allowed_tools }}"',
    "Ensure Claude deliverable or post failure notice",
    "scripts/ai_review_deliverables.py claude-deliverable",
    "--execution-file \"${{ steps.claude.outputs.execution_file }}\"",
    "--step-outcome \"${{ steps.claude.outcome }}\"",
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
    "def review_body_has_line_starting_with(",
    "line.strip().startswith(label)",
    "review_body_has_line_starting_with(lowered, label)",
    "def review_body_has_no_findings_contract(",
    "def pr_agent_body_has_substantive_review(",
    "def run_notice_env(",
    "shlex.quote(",
    "review_output_contract(review_config)",
    "def source_label_from_template(",
    "source_label=source_label_from_template(",
    "def validate_review_responses(",
    "def invalid_review_response_detail(",
    "did not meet the hard-evidence output contract",
    "output omitted from PR notice",
    "validate_review_responses(responses, config.output_contract)",
    "def provider_retry_needed(",
    "def provider_retry_decision(",
    "def latest_quality_review_deliverable_time(",
    "def latest_claude_visible_deliverable_time(",
    "def claude_body_is_review_deliverable(",
    "def ensure_claude_deliverable_or_notice(",
    "def run_claude_deliverable_from_env(",
    "deliverable_bot_logins=config_str_tuple(",
    'comment_marker=config_str(claude_config, "deliverable_marker")',
    'claude_deliverable_marker=config_str(claude_config, "deliverable_marker")',
    'require_source_line=provider_key in ("glm", "kimi")',
    "def run_retry_needed_from_env(",
    'print(f"retry_needed=',
    'print(f"reason={reason}")',
    'choices=("glm", "kimi", "claude")',
    "def list_pull_review_comments",
    "def update_pull_review_comment",
    "github.list_pull_review_comments()",
    "github.update_pull_review_comment(",
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


def missing_snippets(label: str, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{label} missing expected snippet: {snippet!r}" for snippet in snippets if snippet not in text]


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

def non_empty_string_list(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(isinstance(item, str) and item for item in value)


class PrAgentMirrorFailure(str, Enum):
    MISSING_TABLE = "missing_table"
    INVALID_NOTE_SNIPPETS = "invalid_note_snippets"
    INVALID_RULES = "invalid_rules"
    INVALID_RULE = "invalid_rule"
    INVALID_RULE_NAME = "invalid_rule_name"
    DUPLICATE_RULE_NAME = "duplicate_rule_name"
    INVALID_RULE_SNIPPETS = "invalid_rule_snippets"


def pr_agent_mirror_contract_failure(reason: PrAgentMirrorFailure, message: str) -> str:
    return f"ci/ai-review.toml pr_agent_mirror contract failure ({reason.value}): {message}"


def verify_pr_agent_mirror(ai_review_toml: str, pr_agent_toml: str) -> list[str]:
    findings: list[str] = []
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError as exc:
        return [f"ci/ai-review.toml invalid TOML: {exc}"]

    mirror = parsed.get("pr_agent_mirror")
    if not isinstance(mirror, dict):
        return [
            pr_agent_mirror_contract_failure(
                PrAgentMirrorFailure.MISSING_TABLE,
                "missing [pr_agent_mirror]",
            )
        ]

    note_snippets = mirror.get("required_note_snippets")
    if not non_empty_string_list(note_snippets):
        return [
            pr_agent_mirror_contract_failure(
                PrAgentMirrorFailure.INVALID_NOTE_SNIPPETS,
                "required_note_snippets must be a non-empty string list",
            )
        ]
    required_note_snippets = tuple(note_snippets)

    rules = mirror.get("rules")
    if not isinstance(rules, list) or not rules:
        return [
            pr_agent_mirror_contract_failure(
                PrAgentMirrorFailure.INVALID_RULES,
                "rules must be a non-empty table array",
            )
        ]
    mirror_rules = tuple(rules)

    extra, extra_findings = pr_agent_extra_instructions(pr_agent_toml)
    findings.extend(extra_findings)
    if extra_findings:
        return findings

    for snippet in required_note_snippets:
        if snippet not in extra:
            findings.append(f".pr_agent.toml missing mirrored governance note: {snippet!r}")

    seen_names: set[str] = set()
    for index, rule in enumerate(mirror_rules):
        if not isinstance(rule, dict):
            findings.append(
                pr_agent_mirror_contract_failure(
                    PrAgentMirrorFailure.INVALID_RULE,
                    f"rules[{index}] must be a table",
                )
            )
            continue
        name = rule.get("name")
        if not isinstance(name, str) or not name:
            findings.append(
                pr_agent_mirror_contract_failure(
                    PrAgentMirrorFailure.INVALID_RULE_NAME,
                    f"rules[{index}].name must be non-empty",
                )
            )
            continue
        if name in seen_names:
            findings.append(
                pr_agent_mirror_contract_failure(
                    PrAgentMirrorFailure.DUPLICATE_RULE_NAME,
                    f"duplicate pr_agent_mirror rule {name!r}",
                )
            )
            continue
        seen_names.add(name)

        snippets = rule.get("snippets")
        if not non_empty_string_list(snippets):
            findings.append(
                pr_agent_mirror_contract_failure(
                    PrAgentMirrorFailure.INVALID_RULE_SNIPPETS,
                    f"rule {name!r} snippets must be non-empty",
                )
            )
            continue
        for snippet in snippets:
            if snippet not in extra:
                findings.append(f".pr_agent.toml missing mirrored governance rule {name!r}: {snippet!r}")

    return findings


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
    expected_finding_labels = (
        "include lines starting exactly with `Severity:`, `Evidence:`, `Issue:`, and `Fix / verification:`",
    )
    if not all(snippet in extra for snippet in expected_finding_labels):
        findings.append(".pr_agent.toml extra_instructions must require the literal finding evidence labels")
    for snippet in (
        "No hard-evidence findings",
        "Coverage reviewed:",
        "Evidence basis:",
        "Risk areas considered:",
    ):
        if snippet not in extra:
            findings.append(".pr_agent.toml extra_instructions must require the no-findings evidence contract")
            break
    for literal in configured_runtime_literals(ai_review_toml):
        if literal in extra:
            findings.append(f".pr_agent.toml extra_instructions must read AI review runtime value from ci/ai-review.toml, not {literal!r}")

    return findings


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


def workflow_literal_block_value(block: str, key: str) -> str | None:
    lines = block.splitlines()
    for index, line in enumerate(lines):
        if line.strip() != f"{key}: |":
            continue
        base_indent = len(line) - len(line.lstrip())
        value_lines: list[str] = []
        for next_line in lines[index + 1 :]:
            stripped = next_line.strip()
            next_indent = len(next_line) - len(next_line.lstrip())
            if stripped and next_indent <= base_indent:
                break
            value_lines.append(next_line[base_indent + 2 :] if len(next_line) >= base_indent + 2 else "")
        return "\n".join(value_lines)
    return None


def yaml_scalar_entries(block: str) -> tuple[tuple[str, str], ...]:
    entries: list[tuple[str, str]] = []
    for line in block.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        key, separator, value = stripped.partition(":")
        if separator:
            entries.append((key.strip(), value.strip()))
    return tuple(entries)


def verify_literal_block_unique_entries(
    step_block: str,
    *,
    block_key: str,
    expected_entries: dict[str, str],
    label: str,
) -> list[str]:
    block = workflow_literal_block_value(step_block, block_key)
    expected_description = ", ".join(
        f"exactly one {key}: {value} entry" for key, value in expected_entries.items()
    )
    if block is None:
        return [f"{label} must set {block_key} to {expected_description}"]

    values_by_key: dict[str, list[str]] = {}
    for key, value in yaml_scalar_entries(block):
        values_by_key.setdefault(key, []).append(value)

    mismatched = {
        key: values_by_key.get(key, [])
        for key, expected_value in expected_entries.items()
        if values_by_key.get(key, []) != [expected_value]
    }
    if mismatched:
        return [f"{label} must set {block_key} to {expected_description}; got {mismatched!r}"]
    return []


def verify_claude_additional_permissions_cap(claude_run_step: str) -> list[str]:
    return verify_literal_block_unique_entries(
        claude_run_step,
        block_key="additional_permissions",
        expected_entries={"contents": "read"},
        label="Claude workflow review step",
    )


def verify_model_freshness_step_contracts(glm_workflow: str, kimi_workflow: str) -> list[str]:
    findings: list[str] = []
    provider_steps = (
        ("GLM workflow", glm_workflow, "glm", "GLM_API_KEY", "glm-secret", "Detect GLM secret"),
        ("Kimi workflow", kimi_workflow, "kimi", "KIMI_API_KEY", "kimi-secret", "Detect Kimi secret"),
    )
    for workflow_name, workflow_text, provider, api_key_name, detector_id, detector_name in provider_steps:
        detector = workflow_step_block(workflow_text, detector_name)
        if not detector:
            findings.append(f"{workflow_name} missing {detector_name} step")
        elif f"id: {detector_id}" not in detector or f"{api_key_name}: ${{{{ secrets.{api_key_name} }}}}" not in detector:
            findings.append(f"{workflow_name} {detector_name} step must be the only {api_key_name} secret detector")
        if workflow_job_env_receives_provider_secret(workflow_text, api_key_name):
            findings.append(f"{workflow_name} must not expose {api_key_name} at job scope")
        block = workflow_step_block(workflow_text, "Check AI review model freshness")
        if not block:
            findings.append(f"{workflow_name} missing Check AI review model freshness step")
            continue
        if f"if: steps.{detector_id}.outputs.configured == 'true'" not in block:
            findings.append(f"{workflow_name} model freshness step must be gated on {detector_id} configured output")
        if "continue-on-error: true" not in block:
            findings.append(f"{workflow_name} model freshness step must be advisory via continue-on-error")
        if f"--provider {provider}" not in block:
            findings.append(f"{workflow_name} model freshness step must check only {provider.upper()} freshness")

    glm_block = workflow_step_block(glm_workflow, "Check AI review model freshness")
    if model_freshness_step_receives_provider_secret(glm_block):
        findings.append("GLM workflow model freshness step must not receive provider API secrets")
    kimi_block = workflow_step_block(kimi_workflow, "Check AI review model freshness")
    if model_freshness_step_receives_provider_secret(kimi_block):
        findings.append("Kimi workflow model freshness step must not receive provider API secrets")
    return findings


def workflow_job_env_receives_provider_secret(workflow_text: str, api_key_name: str) -> bool:
    job_env_blocks = re.findall(r"(?m)^    env:\n((?:      [^\n]*\n?)*)", workflow_text)
    provider_key = re.compile(rf"(?m)^      {re.escape(api_key_name)}:")
    return any(provider_key.search(block) for block in job_env_blocks)


def model_freshness_step_receives_provider_secret(block: str) -> bool:
    secret_patterns = (
        "GLM_API_KEY:",
        "KIMI_API_KEY:",
        "MOONSHOT_API_KEY:",
        "secrets.GLM_API_KEY",
        "secrets.KIMI_API_KEY",
        "secrets.MOONSHOT_API_KEY",
        "${{ env.GLM_API_KEY }}",
        "${{ env.KIMI_API_KEY }}",
        "${{ env.MOONSHOT_API_KEY }}",
    )
    return any(pattern in block for pattern in secret_patterns)


def exact_kimi_model_id(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"kimi-k\d+(?:\.\d+)*-code(?:-highspeed)?", value) is not None


def exact_glm_model_id(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"glm-\d+(?:\.\d+)*", value) is not None


def exact_claude_model_id(value: object) -> bool:
    return isinstance(value, str) and re.fullmatch(r"claude-opus-\d+(?:[-.]\d+)*", value) is not None


def configured_runtime_literals(ai_review_toml: str, *, include_output_contract: bool = False) -> tuple[str, ...]:
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError:
        return ()
    literals: list[str] = []
    review = parsed.get("review")
    if include_output_contract and isinstance(review, dict):
        output_contract = review.get("output_contract")
        if isinstance(output_contract, dict):
            for key in (
                "finding_required_labels",
                "no_findings_indicator",
                "no_findings_intro",
                "no_findings_required_labels",
            ):
                value = output_contract.get(key)
                if isinstance(value, str) and value:
                    literals.append(value)
                elif isinstance(value, list):
                    literals.extend(item for item in value if isinstance(item, str) and item)
    github = parsed.get("github")
    if isinstance(github, dict):
        value = github.get("expected_bot_login")
        if isinstance(value, str) and value:
            literals.append(value)
    model_freshness = parsed.get("model_freshness")
    if isinstance(model_freshness, dict):
        for key in ("github_api_version", "issue_marker", "issue_title", "notice_marker_template", "user_agent"):
            value = model_freshness.get(key)
            if isinstance(value, str) and value:
                literals.append(value)
    for table_name in ("glm", "kimi", "claude"):
        table = parsed.get(table_name)
        if isinstance(table, dict):
            for key in (
                "api_base",
                "model",
                "allowed_tools",
                "deliverable_marker",
                "deliverable_markers",
                "deliverable_bot_logins",
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
                for key in ("model", "source_label_template"):
                    value = pr_agent.get(key)
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


def verify_no_hardcoded_review_labels(ai_review_toml: str, workflow_text: str, provider: str) -> list[str]:
    try:
        parsed = tomllib.loads(ai_review_toml)
    except tomllib.TOMLDecodeError:
        return []
    provider_config = parsed.get(provider)
    if not isinstance(provider_config, dict):
        return []
    smart_trigger = provider_config.get("smart_trigger")
    if not isinstance(smart_trigger, dict):
        return []
    labels = smart_trigger.get("review_labels")
    if not isinstance(labels, list):
        return []
    literal_patterns: list[str] = []
    for label in labels:
        if not isinstance(label, str) or not label:
            continue
        literal_patterns.extend(
            [
                f"github.event.label.name == '{label}'",
                f'github.event.label.name == "{label}"',
                f"contains(github.event.pull_request.labels.*.name, '{label}')",
                f'contains(github.event.pull_request.labels.*.name, "{label}")',
            ]
        )
    for pattern in literal_patterns:
        if pattern in workflow_text:
            return [f"{provider.upper()} workflow must read review labels from ci/ai-review.toml, not hardcode {pattern!r}"]
    return []


def verify_notice_step_guard(
    workflow_text: str,
    *,
    provider_label: str,
    step_name: str,
    helper_snippet: str,
) -> list[str]:
    step = workflow_step_block(workflow_text, step_name)
    if not step:
        return [f"{provider_label} workflow missing {step_name} step"]
    for snippet in (
        helper_snippet,
        "AI review notice marker or expected bot login is unavailable.",
        'eval "$config_exports"',
        'if [ -z "$marker" ] || [ -z "$expected_bot_login" ]; then',
    ):
        if snippet not in step:
            return [f"{provider_label} notice step {step_name!r} must not use empty marker or bot login"]
    return []


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
    model_freshness = table("model_freshness")
    output_contract = review.get("output_contract")
    if not isinstance(output_contract, dict):
        output_contract = {}
    pr_agent_output = review.get("pr_agent_output")
    if not isinstance(pr_agent_output, dict):
        pr_agent_output = {}
    glm = table("glm")
    glm_smart_trigger = glm.get("smart_trigger")
    if not isinstance(glm_smart_trigger, dict):
        findings.append("ci/ai-review.toml missing [glm.smart_trigger]")
        glm_smart_trigger = {}
    glm_pr_agent = glm.get("pr_agent")
    if not isinstance(glm_pr_agent, dict):
        findings.append("ci/ai-review.toml missing [glm.pr_agent]")
        glm_pr_agent = {}
    glm_workflow = glm.get("workflow")
    if not isinstance(glm_workflow, dict):
        findings.append("ci/ai-review.toml missing [glm.workflow]")
        glm_workflow = {}
    kimi = table("kimi")
    kimi_smart_trigger = kimi.get("smart_trigger")
    if not isinstance(kimi_smart_trigger, dict):
        findings.append("ci/ai-review.toml missing [kimi.smart_trigger]")
        kimi_smart_trigger = {}
    kimi_workflow = kimi.get("workflow")
    if not isinstance(kimi_workflow, dict):
        findings.append("ci/ai-review.toml missing [kimi.workflow]")
        kimi_workflow = {}
    claude = table("claude")
    claude_smart_trigger = claude.get("smart_trigger")
    if not isinstance(claude_smart_trigger, dict):
        findings.append("ci/ai-review.toml missing [claude.smart_trigger]")
        claude_smart_trigger = {}
    claude_workflow = claude.get("workflow")
    if not isinstance(claude_workflow, dict):
        findings.append("ci/ai-review.toml missing [claude.workflow]")
        claude_workflow = {}
    smoke = table("smoke")

    expected_values = (
        ("github.api_url", github.get("api_url"), "https://api.github.com"),
        ("github.server_url", github.get("server_url"), "https://github.com"),
        ("github.expected_bot_login", github.get("expected_bot_login"), "github-actions[bot]"),
        ("review.max_comment_chars", review.get("max_comment_chars"), 60000),
        ("review.response_chars_per_chunk", review.get("response_chars_per_chunk"), 8000),
        (
            "model_freshness.user_agent",
            model_freshness.get("user_agent"),
            "bolt-v2-ai-review-model-freshness/1.0",
        ),
        ("model_freshness.github_api_version", model_freshness.get("github_api_version"), "2022-11-28"),
        ("model_freshness.kimi_chat_docs_url", model_freshness.get("kimi_chat_docs_url"), "https://platform.kimi.ai/docs/api/chat"),
        ("model_freshness.glm_docs_index_url", model_freshness.get("glm_docs_index_url"), "https://docs.z.ai/llms.txt"),
        (
            "model_freshness.glm_migration_docs_url",
            model_freshness.get("glm_migration_docs_url"),
            "https://docs.z.ai/guides/overview/migrate-to-glm-new",
        ),
        (
            "model_freshness.claude_models_docs_url",
            model_freshness.get("claude_models_docs_url"),
            "https://docs.anthropic.com/en/docs/about-claude/models/overview",
        ),
        ("model_freshness.request_timeout_seconds", model_freshness.get("request_timeout_seconds"), 30),
        ("model_freshness.github_issues_per_page", model_freshness.get("github_issues_per_page"), 100),
        (
            "model_freshness.issue_marker",
            model_freshness.get("issue_marker"),
            "<!-- ai-review-model-freshness-issue -->",
        ),
        (
            "model_freshness.issue_title",
            model_freshness.get("issue_title"),
            "AI review model pin update available",
        ),
        (
            "model_freshness.notice_marker_template",
            model_freshness.get("notice_marker_template"),
            "<!-- ai-review-model-freshness-notice-{provider} -->",
        ),
        ("glm.api_base", glm.get("api_base"), "https://api.z.ai/api/coding/paas/v4"),
        ("glm.api_timeout_seconds", glm.get("api_timeout_seconds"), 180),
        ("glm.review_max_chunk_chars", glm.get("review_max_chunk_chars"), 60000),
        ("glm.comment_marker", glm.get("comment_marker"), "<!-- ai-pr-reviewer-glm -->"),
        ("glm.notice_marker", glm.get("notice_marker"), "<!-- ai-pr-reviewer-glm-notice -->"),
        ("glm.source_label_template", glm.get("source_label_template"), "GLM direct fallback (`{model}`)"),
        (
            "glm.smart_trigger.review_labels",
            glm_smart_trigger.get("review_labels"),
            ["ai-review", "ai-review-glm"],
        ),
        (
            "glm.pr_agent.source_label_template",
            glm_pr_agent.get("source_label_template"),
            "GLM PR-Agent (`{model}`)",
        ),
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
            [],
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
        ("kimi.provider_type", kimi.get("provider_type"), "kimi"),
        ("kimi.model_max_context_size", kimi.get("model_max_context_size"), 262144),
        ("kimi.default_thinking", kimi.get("default_thinking"), True),
        ("kimi.telemetry_disabled", kimi.get("telemetry_disabled"), True),
        ("kimi.review_max_chunk_chars", kimi.get("review_max_chunk_chars"), 60000),
        ("kimi.deliverable_marker", kimi.get("deliverable_marker"), "<!-- ai-pr-reviewer-kimi -->"),
        ("kimi.notice_marker", kimi.get("notice_marker"), "<!-- ai-pr-reviewer-kimi-notice -->"),
        ("kimi.source_label_template", kimi.get("source_label_template"), "Kimi Code CLI (`{model}`)"),
        ("kimi.cli_package", kimi.get("cli_package"), "@moonshot-ai/kimi-code@0.19.0"),
        (
            "kimi.smart_trigger.review_labels",
            kimi_smart_trigger.get("review_labels"),
            ["ai-review", "ai-review-kimi"],
        ),
        ("kimi.workflow.node_version", kimi_workflow.get("node_version"), "24"),
        ("kimi.workflow.job_timeout_minutes", kimi_workflow.get("job_timeout_minutes"), 45),
        ("kimi.workflow.primary_timeout_minutes", kimi_workflow.get("primary_timeout_minutes"), 20),
        ("kimi.workflow.fallback_timeout_minutes", kimi_workflow.get("fallback_timeout_minutes"), 20),
        ("kimi.workflow.setup_overhead_timeout_minutes", kimi_workflow.get("setup_overhead_timeout_minutes"), 5),
        ("claude.track_progress", claude.get("track_progress"), False),
        ("claude.notice_marker", claude.get("notice_marker"), "<!-- ai-pr-reviewer-claude-notice -->"),
        ("claude.deliverable_marker", claude.get("deliverable_marker"), "<!-- ai-pr-reviewer-claude -->"),
        ("claude.deliverable_bot_logins", claude.get("deliverable_bot_logins"), ["claude[bot]"]),
        ("claude.source_label_template", claude.get("source_label_template"), "Claude Code (`{model}`)"),
        (
            "claude.allowed_tools",
            claude.get("allowed_tools"),
            [
                "mcp__github_inline_comment__create_inline_comment",
                "Bash(gh pr comment:*)",
                "Bash(gh pr diff:*)",
                "Bash(gh pr view:*)",
            ],
        ),
        ("claude.smart_trigger.debounce_seconds", claude_smart_trigger.get("debounce_seconds"), 90),
        (
            "claude.smart_trigger.review_labels",
            claude_smart_trigger.get("review_labels"),
            ["ai-review", "ai-review-claude", "claude-review"],
        ),
        (
            "claude.smart_trigger.review_paths",
            claude_smart_trigger.get("review_paths"),
            [
                "AGENTS.md",
                ".github/**",
                "ci/**",
                "config/**",
                "crates/**",
                "scripts/**",
                "src/**",
                "tests/**",
                "Cargo.toml",
                "Cargo.lock",
            ],
        ),
        ("claude.workflow.job_timeout_minutes", claude_workflow.get("job_timeout_minutes"), 35),
        ("claude.workflow.primary_timeout_minutes", claude_workflow.get("primary_timeout_minutes"), 20),
        ("claude.workflow.max_turns", claude_workflow.get("max_turns"), 10),
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
    claude_model = claude.get("model")
    glm_pr_agent_model = glm_pr_agent.get("model")
    expected_glm_pr_agent_model = f"openai/{glm_model}" if isinstance(glm_model, str) else ""
    if not exact_glm_model_id(glm_model):
        findings.append("ci/ai-review.toml glm.model must be an exact GLM model id, not an alias")
    if not exact_kimi_model_id(kimi_model):
        findings.append("ci/ai-review.toml kimi.model must be an exact Kimi coding model id, not an alias")
    if not exact_claude_model_id(claude_model):
        findings.append("ci/ai-review.toml claude.model must be an exact Claude Opus model id, not an alias")
    if "latest" in str(glm_model).lower() or "latest" in str(kimi_model).lower() or "latest" in str(claude_model).lower():
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
    provider_name = {"glm": "GLM", "kimi": "Kimi", "claude": "Claude", "smoke": "Smoke"}.get(provider, provider)
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
    ai_review_toml: str,
    pr_agent_toml: str,
    ai_review_deliverables: str,
    glm_workflow: str,
    kimi_workflow: str,
    claude_workflow: str,
    smoke_workflow: str,
) -> list[str]:
    findings: list[str] = []

    findings.extend(verify_ai_review_config(ai_review_toml))
    findings.extend(verify_pr_agent_mirror(ai_review_toml, pr_agent_toml))
    findings.extend(verify_pr_agent_config(pr_agent_toml, ai_review_toml))
    findings.extend(missing_snippets("scripts/ai_review_deliverables.py", ai_review_deliverables, AI_REVIEW_DELIVERABLES_SNIPPETS))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, glm_workflow, "glm", setup_required=True))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, kimi_workflow, "kimi", setup_required=True))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, claude_workflow, "claude", setup_required=False))
    findings.extend(verify_review_job_timeout_budget(ai_review_toml, smoke_workflow, "smoke", setup_required=False))
    findings.extend(verify_model_freshness_step_contracts(glm_workflow, kimi_workflow))
    findings.extend(verify_no_hardcoded_review_labels(ai_review_toml, glm_workflow, "glm"))
    findings.extend(verify_no_hardcoded_review_labels(ai_review_toml, kimi_workflow, "kimi"))

    if "pr_reviewer." in glm_workflow:
        findings.append(
            "GLM workflow must not define pr_reviewer.* overrides; keep reviewer behavior in .pr_agent.toml"
        )
    for workflow_name, workflow in (
        ("GLM workflow", glm_workflow),
        ("Kimi workflow", kimi_workflow),
        ("Claude workflow", claude_workflow),
        ("Smoke workflow", smoke_workflow),
    ):
        if "ai_review_deliverables.py notice --" in workflow:
            findings.append(f"{workflow_name} backup/skip notices must not depend on ai_review_deliverables.py")
        if "marker[:-4]" in workflow:
            findings.append(f"{workflow_name} notice markers must use explicit notice_marker config")
        for endpoint in FORBIDDEN_API_ENDPOINTS:
            if endpoint in workflow:
                findings.append(f"{workflow_name} must use coding-plan endpoint/model, not {endpoint!r}")
        for literal in WORKFLOW_FORBIDDEN_RUNTIME_LITERALS:
            if literal in workflow:
                findings.append(f"{workflow_name} must read AI review runtime value from ci/ai-review.toml, not {literal!r}")
        for literal in configured_runtime_literals(ai_review_toml, include_output_contract=True):
            if literal in workflow:
                findings.append(f"{workflow_name} must read AI review runtime value from ci/ai-review.toml, not {literal!r}")

    findings.extend(missing_snippets("GLM workflow", glm_workflow, GLM_DELIVERABLE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_BASE_GOVERNANCE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_DELIVERABLE_SNIPPETS))
    findings.extend(missing_snippets("Claude workflow", claude_workflow, CLAUDE_WORKFLOW_SNIPPETS))
    findings.extend(missing_snippets("Smoke workflow", smoke_workflow, SMOKE_TRUSTED_CONFIG_SNIPPETS))
    if 'track_progress: true' in claude_workflow:
        findings.append("Claude workflow must not enable track_progress; it forces tag mode with edit/git tools")
    if "anthropic_api_key:" in claude_workflow:
        findings.append("Claude workflow must use claude_code_oauth_token only, not anthropic_api_key")
    claude_freshness_step = workflow_step_block(claude_workflow, "Check AI review model freshness")
    if not claude_freshness_step:
        findings.append("Claude workflow missing Check AI review model freshness step")
    else:
        if "continue-on-error: true" not in claude_freshness_step:
            findings.append("Claude workflow model freshness step must be advisory via continue-on-error")
        if "--provider claude" not in claude_freshness_step:
            findings.append("Claude workflow model freshness step must check only Claude freshness")
    claude_run_step = workflow_step_block(claude_workflow, "Run Claude Code review")
    if not claude_run_step:
        findings.append("Claude workflow missing Run Claude Code review step")
    else:
        if "continue-on-error: true" not in claude_run_step:
            findings.append("Claude workflow review step must be advisory via continue-on-error")
        if "claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}" not in claude_run_step:
            findings.append("Claude workflow review step must use CLAUDE_CODE_OAUTH_TOKEN")
        findings.extend(verify_claude_additional_permissions_cap(claude_run_step))
        if "anthropic_api_key:" in claude_run_step:
            findings.append("Claude workflow review step must not set anthropic_api_key alongside OAuth")
        if "--allowedTools" not in claude_run_step:
            findings.append("Claude workflow review step must pass configured allowed tools")
    claude_deliverable_step = workflow_step_block(claude_workflow, "Ensure Claude deliverable or post failure notice")
    if not claude_deliverable_step:
        findings.append("Claude workflow missing Ensure Claude deliverable or post failure notice step")
    else:
        if "continue-on-error: true" in claude_deliverable_step:
            findings.append("Claude deliverable/no-deliverable notice step must fail closed")
        for snippet in (
            "always()",
            "steps.runtime-config.outcome == 'success'",
            "steps.review-window.outcome == 'success'",
            "scripts/ai_review_deliverables.py claude-deliverable",
            "--execution-file \"${{ steps.claude.outputs.execution_file }}\"",
            "--step-outcome \"${{ steps.claude.outcome }}\"",
        ):
            if snippet not in claude_deliverable_step:
                findings.append("Claude deliverable/no-deliverable notice step must inspect action output and post a notice")
                break
    glm_stamp_step = workflow_step_block(glm_workflow, "Stamp GLM PR-Agent review source")
    if not glm_stamp_step:
        findings.append("GLM workflow missing Stamp GLM PR-Agent review source step")
    elif "continue-on-error: true" in glm_stamp_step:
        findings.append("GLM source stamp step must fail closed")
    glm_fallback_step = workflow_step_block(glm_workflow, "Ensure GLM deliverable or post split fallback")
    if not glm_fallback_step:
        findings.append("GLM workflow missing Ensure GLM deliverable or post split fallback step")
    else:
        for snippet in (
            "always()",
            "steps.base-checkout.outcome == 'success'",
            "steps.runtime-config.outcome == 'success'",
            "steps.review-decision.outputs.should_review == 'true'",
            "steps.review-window.outcome == 'success'",
        ):
            if snippet not in glm_fallback_step:
                findings.append(
                    "GLM fallback step must run after stamp failure while requiring usable checkout/runtime context"
                )
                break
    findings.extend(
        verify_notice_step_guard(
            glm_workflow,
            provider_label="GLM",
            step_name="Notice when Z.ai secret is not configured",
            helper_snippet="scripts/ai_review_deliverables.py notice-env --provider glm --config-file ci/ai-review.toml",
        )
    )
    findings.extend(
        verify_notice_step_guard(
            glm_workflow,
            provider_label="GLM",
            step_name="Post GLM fallback infrastructure failure notice",
            helper_snippet="scripts/ai_review_deliverables.py notice-env --provider glm --config-file ci/ai-review.toml",
        )
    )
    findings.extend(
        verify_notice_step_guard(
            kimi_workflow,
            provider_label="Kimi",
            step_name="Notice when Kimi secret is not configured",
            helper_snippet=".ai-review/base/scripts/ai_review_deliverables.py notice-env --provider kimi --config-file .ai-review/base/ci/ai-review.toml",
        )
    )
    findings.extend(
        verify_notice_step_guard(
            kimi_workflow,
            provider_label="Kimi",
            step_name="Post Kimi fallback infrastructure failure notice",
            helper_snippet=".ai-review/base/scripts/ai_review_deliverables.py notice-env --provider kimi --config-file .ai-review/base/ci/ai-review.toml",
        )
    )
    for snippet in KIMI_FORBIDDEN_INPUTS:
        if snippet in kimi_workflow:
            findings.append(f"Kimi workflow must use the official Kimi CLI path, not {snippet!r}")

    return findings


def verify_repo(repo_root: Path) -> list[str]:
    return verify_texts(
        ai_review_toml=read_text(repo_root / "ci/ai-review.toml"),
        pr_agent_toml=read_text(repo_root / ".pr_agent.toml"),
        ai_review_deliverables=read_text(repo_root / "scripts/ai_review_deliverables.py"),
        glm_workflow=read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml"),
        kimi_workflow=read_text(repo_root / ".github/workflows/ai-review-kimi-cli.yml"),
        claude_workflow=read_text(repo_root / ".github/workflows/claude-code-review.yml"),
        smoke_workflow=read_text(repo_root / ".github/workflows/ai-review-coding-plan-smoke.yml"),
    )


def assert_finding(name: str, findings: list[str], expected: str) -> None:
    if not any(expected in finding for finding in findings):
        raise AssertionError(f"{name}: expected finding containing {expected!r}, got {findings!r}")


def bump_model_version(model_id: str) -> str:
    match = re.search(r"\d+(?:\.\d+)*", model_id)
    if not match:
        raise AssertionError(f"model id has no numeric version: {model_id!r}")
    parts = [int(part) for part in match.group(0).split(".")]
    parts[-1] += 1
    bumped = ".".join(str(part) for part in parts)
    return f"{model_id[: match.start()]}{bumped}{model_id[match.end():]}"


def run_self_tests(repo_root: Path) -> None:
    ai_review = read_text(repo_root / "ci/ai-review.toml")
    pr_agent = read_text(repo_root / ".pr_agent.toml")
    ai_review_config = tomllib.loads(ai_review)
    github_config = ai_review_config["github"]
    glm_config = ai_review_config["glm"]
    current_glm_model = ai_review_config["glm"]["model"]
    current_kimi_model = ai_review_config["kimi"]["model"]
    current_claude_model = ai_review_config["claude"]["model"]
    current_glm_pr_agent_model = ai_review_config["glm"]["pr_agent"]["model"]
    current_glm_review_label = ai_review_config["glm"]["smart_trigger"]["review_labels"][-1]
    current_kimi_review_label = ai_review_config["kimi"]["smart_trigger"]["review_labels"][-1]
    current_bot_login = github_config["expected_bot_login"]
    current_glm_comment_marker = glm_config["comment_marker"]
    current_glm_api_base = glm_config["api_base"]
    deliverables = read_text(repo_root / "scripts/ai_review_deliverables.py")
    glm = read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml")
    kimi = read_text(repo_root / ".github/workflows/ai-review-kimi-cli.yml")
    claude = read_text(repo_root / ".github/workflows/claude-code-review.yml")
    smoke = read_text(repo_root / ".github/workflows/ai-review-coding-plan-smoke.yml")

    def verify_variant(
        *,
        ai_review_text: str = ai_review,
        pr_agent_text: str = pr_agent,
        deliverables_text: str = deliverables,
        glm_text: str = glm,
        kimi_text: str = kimi,
        claude_text: str = claude,
        smoke_text: str = smoke,
    ) -> list[str]:
        return verify_texts(
            ai_review_toml=ai_review_text,
            pr_agent_toml=pr_agent_text,
            ai_review_deliverables=deliverables_text,
            glm_workflow=glm_text,
            kimi_workflow=kimi_text,
            claude_workflow=claude_text,
            smoke_workflow=smoke_text,
        )

    baseline = verify_variant()
    if baseline:
        raise AssertionError(f"real repository must satisfy AI review governance check, got {baseline!r}")

    glm_model_freshness_step = workflow_step_block(glm, "Check AI review model freshness")
    glm_secret_detector_step = workflow_step_block(glm, "Detect GLM secret")
    if not glm_secret_detector_step:
        raise AssertionError("missing GLM secret detector step")
    if "id: glm-secret" not in glm_secret_detector_step:
        raise AssertionError("GLM secret detector must expose glm-secret outputs")
    if "GLM_API_KEY: ${{ secrets.GLM_API_KEY }}" not in glm_secret_detector_step:
        raise AssertionError("GLM secret detector must be the only GLM_API_KEY secret detector")
    if workflow_job_env_receives_provider_secret(glm, "GLM_API_KEY"):
        raise AssertionError("GLM workflow must not expose GLM_API_KEY at job scope")
    if not glm_model_freshness_step:
        raise AssertionError("missing GLM model freshness step")
    if "continue-on-error: true" not in glm_model_freshness_step:
        raise AssertionError("GLM model freshness step must be advisory via continue-on-error")
    if "if: steps.glm-secret.outputs.configured == 'true'" not in glm_model_freshness_step:
        raise AssertionError("GLM model freshness step must be gated on glm-secret configured output")
    if "--provider glm" not in glm_model_freshness_step:
        raise AssertionError("GLM model freshness step must check only GLM freshness")
    if model_freshness_step_receives_provider_secret(glm_model_freshness_step):
        raise AssertionError("GLM model freshness step must not receive provider API secrets")

    kimi_model_freshness_step = workflow_step_block(kimi, "Check AI review model freshness")
    kimi_secret_detector_step = workflow_step_block(kimi, "Detect Kimi secret")
    if not kimi_secret_detector_step:
        raise AssertionError("missing Kimi secret detector step")
    if "id: kimi-secret" not in kimi_secret_detector_step:
        raise AssertionError("Kimi secret detector must expose kimi-secret outputs")
    if "KIMI_API_KEY: ${{ secrets.KIMI_API_KEY }}" not in kimi_secret_detector_step:
        raise AssertionError("Kimi secret detector must be the only KIMI_API_KEY secret detector")
    if workflow_job_env_receives_provider_secret(kimi, "KIMI_API_KEY"):
        raise AssertionError("Kimi workflow must not expose KIMI_API_KEY at job scope")
    if not kimi_model_freshness_step:
        raise AssertionError("missing Kimi model freshness step")
    if "continue-on-error: true" not in kimi_model_freshness_step:
        raise AssertionError("Kimi model freshness step must be advisory via continue-on-error")
    if "if: steps.kimi-secret.outputs.configured == 'true'" not in kimi_model_freshness_step:
        raise AssertionError("Kimi model freshness step must be gated on kimi-secret configured output")
    if "--provider kimi" not in kimi_model_freshness_step:
        raise AssertionError("Kimi model freshness step must check only Kimi freshness")
    if model_freshness_step_receives_provider_secret(kimi_model_freshness_step):
        raise AssertionError("Kimi model freshness step must not receive provider API secrets")

    claude_model_freshness_step = workflow_step_block(claude, "Check AI review model freshness")
    if not claude_model_freshness_step:
        raise AssertionError("missing Claude model freshness step")
    if "continue-on-error: true" not in claude_model_freshness_step:
        raise AssertionError("Claude model freshness step must be advisory via continue-on-error")
    if "--provider claude" not in claude_model_freshness_step:
        raise AssertionError("Claude model freshness step must check only Claude freshness")
    claude_review_step = workflow_step_block(claude, "Run Claude Code review")
    if not claude_review_step:
        raise AssertionError("missing Claude review step")
    if "continue-on-error: true" not in claude_review_step:
        raise AssertionError("Claude review step must be advisory via continue-on-error")
    if "claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}" not in claude_review_step:
        raise AssertionError("Claude review step must use CLAUDE_CODE_OAUTH_TOKEN")
    if verify_claude_additional_permissions_cap(claude_review_step):
        raise AssertionError("Claude review step must cap Claude app token contents permission to read")
    if "anthropic_api_key:" in claude_review_step:
        raise AssertionError("Claude review step must not set anthropic_api_key")
    if "track_progress: ${{ steps.runtime-config.outputs.claude_track_progress }}" not in claude_review_step:
        raise AssertionError("Claude track_progress must come from ci/ai-review.toml")
    claude_deliverable_step = workflow_step_block(claude, "Ensure Claude deliverable or post failure notice")
    if not claude_deliverable_step:
        raise AssertionError("missing Claude deliverable/no-deliverable notice step")
    if "continue-on-error: true" in claude_deliverable_step:
        raise AssertionError("Claude deliverable/no-deliverable notice step must fail closed")
    if "scripts/ai_review_deliverables.py claude-deliverable" not in claude_deliverable_step:
        raise AssertionError("Claude deliverable/no-deliverable notice step must call the helper")

    claude_track_progress_true_config = verify_variant(
        ai_review_text=ai_review.replace("track_progress = false", "track_progress = true", 1),
    )
    assert_finding(
        "Claude track_progress true config",
        claude_track_progress_true_config,
        "ci/ai-review.toml claude.track_progress",
    )

    claude_missing_deliverable_bot_login = verify_variant(
        ai_review_text=ai_review.replace('deliverable_bot_logins = ["claude[bot]"]\n', "", 1),
    )
    assert_finding(
        "Claude missing deliverable bot login",
        claude_missing_deliverable_bot_login,
        "ci/ai-review.toml claude.deliverable_bot_logins",
    )

    claude_missing_deliverable_marker = verify_variant(
        ai_review_text=ai_review.replace('deliverable_marker = "<!-- ai-pr-reviewer-claude -->"\n', "", 1),
    )
    assert_finding(
        "Claude missing deliverable marker",
        claude_missing_deliverable_marker,
        "ci/ai-review.toml claude.deliverable_marker",
    )

    claude_hardcoded_model = verify_variant(
        claude_text=claude + f"\n          CLAUDE_MODEL: {current_claude_model}\n",
    )
    assert_finding("Claude hardcoded model", claude_hardcoded_model, "must read AI review runtime value")

    claude_track_progress_literal = verify_variant(
        claude_text=claude.replace(
            "          track_progress: ${{ steps.runtime-config.outputs.claude_track_progress }}",
            "          track_progress: true",
            1,
        ),
    )
    assert_finding("Claude track_progress literal", claude_track_progress_literal, "must not enable track_progress")

    claude_anthropic_api_key = verify_variant(
        claude_text=claude.replace(
            "          claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}",
            "          claude_code_oauth_token: ${{ secrets.CLAUDE_CODE_OAUTH_TOKEN }}\n          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}",
            1,
        ),
    )
    assert_finding("Claude Anthropic API key conflict", claude_anthropic_api_key, "not anthropic_api_key")

    claude_missing_deliverable_step = verify_variant(
        claude_text=claude.replace(
            "      - name: Ensure Claude deliverable or post failure notice\n",
            "      - name: Ensure Claude deliverable or post failure notice disabled\n",
            1,
        ),
    )
    assert_finding(
        "Claude missing deliverable step",
        claude_missing_deliverable_step,
        "Claude workflow missing Ensure Claude deliverable or post failure notice step",
    )

    claude_deliverable_continue_on_error = verify_variant(
        claude_text=claude.replace(
            "      - name: Ensure Claude deliverable or post failure notice\n        id: claude-deliverable\n",
            "      - name: Ensure Claude deliverable or post failure notice\n        id: claude-deliverable\n        continue-on-error: true\n",
            1,
        ),
    )
    assert_finding(
        "Claude deliverable step continue-on-error",
        claude_deliverable_continue_on_error,
        "Claude deliverable/no-deliverable notice step must fail closed",
    )

    claude_missing_additional_permissions = verify_variant(
        claude_text=claude.replace(
            "          additional_permissions: |\n            contents: read\n",
            "",
            1,
        ),
    )
    assert_finding(
        "Claude missing app token contents cap",
        claude_missing_additional_permissions,
        "exactly one contents: read",
    )

    claude_weakened_additional_permissions = verify_variant(
        claude_text=claude.replace("            contents: read", "            contents: write", 1),
    )
    assert_finding(
        "Claude weakened app token contents cap",
        claude_weakened_additional_permissions,
        "exactly one contents: read",
    )

    claude_duplicate_additional_permissions = verify_variant(
        claude_text=claude.replace(
            "          additional_permissions: |\n            contents: read\n",
            "          additional_permissions: |\n            contents: read\n            contents: write\n",
            1,
        ),
    )
    assert_finding(
        "Claude duplicate app token contents cap",
        claude_duplicate_additional_permissions,
        "exactly one contents: read",
    )

    claude_missing_labeled_trigger = verify_variant(
        claude_text=claude.replace("types: [opened, synchronize, labeled]", "types: [opened, synchronize]", 1),
    )
    assert_finding("Claude missing labeled trigger", claude_missing_labeled_trigger, "Claude workflow missing expected snippet")

    claude_missing_draft_guard = verify_variant(
        claude_text=claude.replace("          && github.event.pull_request.draft == false\n", "", 1),
    )
    assert_finding("Claude missing draft guard", claude_missing_draft_guard, "Claude workflow missing expected snippet")

    claude_short_job_timeout = verify_variant(
        claude_text=claude.replace("timeout-minutes: 35", "timeout-minutes: 20", 1),
    )
    assert_finding("Claude short job timeout", claude_short_job_timeout, "Claude workflow job timeout must match")

    glm_blocking_freshness = verify_variant(
        glm_text=glm.replace(
            "        continue-on-error: true\n        run: >-\n          python3 scripts/verify_ai_review_model_freshness.py",
            "        run: >-\n          python3 scripts/verify_ai_review_model_freshness.py",
            1,
        ),
    )
    assert_finding("GLM blocking freshness step", glm_blocking_freshness, "model freshness step must be advisory")

    glm_unscoped_freshness = verify_variant(
        glm_text=glm.replace("          --provider glm", "          --provider all", 1),
    )
    assert_finding("GLM unscoped freshness step", glm_unscoped_freshness, "must check only GLM freshness")

    glm_ungated_freshness = verify_variant(
        glm_text=glm.replace(
            "      - name: Check AI review model freshness\n        id: model-freshness\n        if: steps.glm-secret.outputs.configured == 'true' && steps.pause.outputs.paused != 'true' && steps.review-decision.outputs.should_review == 'true'\n",
            "      - name: Check AI review model freshness\n        id: model-freshness\n",
            1,
        ),
    )
    assert_finding("GLM ungated freshness step", glm_ungated_freshness, "must be gated on glm-secret")

    glm_job_scope_secret_before_pr_number = verify_variant(
        glm_text=glm.replace(
            "    env:\n      PR_NUMBER:",
            "    env:\n      GLM_API_KEY: ${{ secrets.GLM_API_KEY }}\n      PR_NUMBER:",
            1,
        ),
    )
    assert_finding(
        "GLM job-scope secret before PR_NUMBER",
        glm_job_scope_secret_before_pr_number,
        "must not expose GLM_API_KEY at job scope",
    )

    glm_job_scope_secret_after_pr_number = verify_variant(
        glm_text=glm.replace(
            "      PR_NUMBER: ${{ github.event.pull_request.number }}",
            "      PR_NUMBER: ${{ github.event.pull_request.number }}\n      GLM_API_KEY: ${{ secrets.GLM_API_KEY }}",
            1,
        ),
    )
    assert_finding(
        "GLM job-scope secret after PR_NUMBER",
        glm_job_scope_secret_after_pr_number,
        "must not expose GLM_API_KEY at job scope",
    )

    glm_secret_expansion = verify_variant(
        glm_text=glm.replace(
            "        continue-on-error: true\n        run: >-",
            "        continue-on-error: true\n        env:\n          GLM_API_KEY: ${{ secrets.GLM_API_KEY }}\n        run: >-",
            1,
        ),
    )
    assert_finding("GLM model freshness secret expansion", glm_secret_expansion, "must not receive provider API secrets")

    kimi_blocking_freshness = verify_variant(
        kimi_text=kimi.replace(
            "        continue-on-error: true\n        run: >-\n          python3 .ai-review/base/scripts/verify_ai_review_model_freshness.py",
            "        run: >-\n          python3 .ai-review/base/scripts/verify_ai_review_model_freshness.py",
            1,
        ),
    )
    assert_finding("Kimi blocking freshness step", kimi_blocking_freshness, "model freshness step must be advisory")

    kimi_ungated_freshness = verify_variant(
        kimi_text=kimi.replace(
            "      - name: Check AI review model freshness\n        id: model-freshness\n        if: steps.kimi-secret.outputs.configured == 'true' && steps.pause.outputs.paused != 'true' && steps.review-decision.outputs.should_review == 'true'\n",
            "      - name: Check AI review model freshness\n        id: model-freshness\n",
            1,
        ),
    )
    assert_finding("Kimi ungated freshness step", kimi_ungated_freshness, "must be gated on kimi-secret")

    kimi_job_scope_secret_before_pr_number = verify_variant(
        kimi_text=kimi.replace(
            "    env:\n      PR_NUMBER:",
            "    env:\n      KIMI_API_KEY: ${{ secrets.KIMI_API_KEY }}\n      PR_NUMBER:",
            1,
        ),
    )
    assert_finding(
        "Kimi job-scope secret before PR_NUMBER",
        kimi_job_scope_secret_before_pr_number,
        "must not expose KIMI_API_KEY at job scope",
    )

    kimi_job_scope_secret_after_pr_number = verify_variant(
        kimi_text=kimi.replace(
            "      PR_NUMBER: ${{ github.event.pull_request.number }}",
            "      PR_NUMBER: ${{ github.event.pull_request.number }}\n      KIMI_API_KEY: ${{ secrets.KIMI_API_KEY }}",
            1,
        ),
    )
    assert_finding(
        "Kimi job-scope secret after PR_NUMBER",
        kimi_job_scope_secret_after_pr_number,
        "must not expose KIMI_API_KEY at job scope",
    )

    kimi_secret_expansion = verify_variant(
        kimi_text=kimi.replace(
            "        continue-on-error: true\n        run: >-",
            "        continue-on-error: true\n        env:\n          KIMI_API_KEY: ${{ env.KIMI_API_KEY }}\n        run: >-",
            1,
        ),
    )
    assert_finding("Kimi model freshness secret expansion", kimi_secret_expansion, "must not receive provider API secrets")

    future_ai_review = ai_review.replace(
        current_glm_model,
        bump_model_version(current_glm_model),
    ).replace(
        current_kimi_model,
        bump_model_version(current_kimi_model),
    )
    future_model_config = verify_variant(ai_review_text=future_ai_review)
    if future_model_config:
        raise AssertionError(f"future exact model pins must be accepted, got {future_model_config!r}")

    wrong_ai_review_config = verify_variant(
        ai_review_text=ai_review.replace("https://api.z.ai/api/coding/paas/v4", "https://api.z.ai/api/paas/v4"),
    )
    assert_finding("wrong AI review config endpoint", wrong_ai_review_config, "ci/ai-review.toml glm.api_base")

    workflow_runtime_literal = verify_variant(glm_text=glm + f"\n          GLM_MODEL: {current_glm_model}\n")
    assert_finding("workflow runtime literal", workflow_runtime_literal, "must read AI review runtime value")

    workflow_bot_literal = verify_variant(glm_text=glm + f"\n          EXPECTED_BOT_LOGIN: {current_bot_login}\n")
    assert_finding("workflow bot login literal", workflow_bot_literal, "must read AI review runtime value")

    workflow_glm_marker_literal = verify_variant(glm_text=glm + f"\n          GLM_MARKER: {current_glm_comment_marker}\n")
    assert_finding("workflow GLM marker literal", workflow_glm_marker_literal, "must read AI review runtime value")

    glm_review_label_literal = verify_variant(glm_text=glm + f"\n          if: github.event.label.name == '{current_glm_review_label}'\n")
    assert_finding("workflow GLM review label literal", glm_review_label_literal, "must read review labels")

    kimi_review_label_literal = verify_variant(kimi_text=kimi + f"\n          if: github.event.label.name == '{current_kimi_review_label}'\n")
    assert_finding("workflow Kimi review label literal", kimi_review_label_literal, "must read review labels")

    stamp_continue_on_error = verify_variant(
        glm_text=glm.replace(
            "      - name: Stamp GLM PR-Agent review source\n        id: glm_stamp\n        if: steps.pause.outputs.paused != 'true' && steps.review-decision.outputs.should_review == 'true' && steps.glm-secret.outputs.configured == 'true'\n        env:",
            "      - name: Stamp GLM PR-Agent review source\n        id: glm_stamp\n        if: steps.pause.outputs.paused != 'true' && steps.review-decision.outputs.should_review == 'true' && steps.glm-secret.outputs.configured == 'true'\n        continue-on-error: true\n        env:",
            1,
        ),
    )
    assert_finding("stamp continue-on-error", stamp_continue_on_error, "GLM source stamp step must fail closed")

    missing_stamp_step = verify_variant(
        glm_text=glm.replace(
            "      - name: Stamp GLM PR-Agent review source\n",
            "      - name: Stamp GLM PR-Agent review source disabled\n",
            1,
        ),
    )
    assert_finding("missing stamp step", missing_stamp_step, "GLM workflow missing Stamp GLM PR-Agent review source step")

    missing_stamp_failure_notice = verify_variant(
        glm_text=glm.replace(
            "              || (steps.glm_stamp.outcome == 'failure' && steps.glm_fallback.outcome != 'success')\n",
            "",
            1,
        ),
    )
    assert_finding(
        "missing stamp failure notice",
        missing_stamp_failure_notice,
        "GLM workflow missing expected snippet",
    )

    fallback_not_reachable_after_stamp_failure = verify_variant(
        glm_text=glm.replace(
            "            && always()\n",
            "",
            1,
        ),
    )
    assert_finding(
        "fallback not reachable after stamp failure",
        fallback_not_reachable_after_stamp_failure,
        "GLM fallback step must run after stamp failure",
    )

    notice_empty_marker_guard_removed = verify_variant(glm_text=glm.replace('            eval "$config_exports"\n', "", 2))
    assert_finding(
        "notice empty marker guard removed",
        notice_empty_marker_guard_removed,
        "GLM notice step",
    )

    kimi_notice_empty_marker_guard_removed = verify_variant(
        kimi_text=kimi.replace('            eval "$config_exports"\n', "", 2),
    )
    assert_finding(
        "Kimi notice empty marker guard removed",
        kimi_notice_empty_marker_guard_removed,
        "Kimi notice step",
    )

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

    pr_agent_model_literal = verify_variant(
        pr_agent_text=pr_agent.replace(
            "workflow stamps the authoritative configured source/model from `ci/ai-review.toml`",
            f"workflow stamps the authoritative configured source/model from `ci/ai-review.toml` (`{current_glm_pr_agent_model}`)",
        ),
    )
    assert_finding("PR-Agent model literal", pr_agent_model_literal, ".pr_agent.toml extra_instructions must read AI review runtime value")

    pr_agent_missing_finding_labels = verify_variant(
        pr_agent_text=pr_agent.replace(
            "include lines starting exactly with `Severity:`, `Evidence:`, `Issue:`, and `Fix / verification:`",
            "include severity",
        ),
    )
    assert_finding(
        "PR-Agent missing finding labels",
        pr_agent_missing_finding_labels,
        "must require the literal finding evidence labels",
    )

    pr_agent_missing_no_findings_contract = verify_variant(
        pr_agent_text=pr_agent.replace("No hard-evidence findings", "No findings"),
    )
    assert_finding(
        "PR-Agent missing no-findings contract",
        pr_agent_missing_no_findings_contract,
        "must require the no-findings evidence contract",
    )

    workflow_notice_marker_derivation = verify_variant(
        glm_text=glm + '\n              return f"{marker[:-4]}-notice -->"\n',
    )
    assert_finding(
        "workflow notice marker derivation",
        workflow_notice_marker_derivation,
        "notice markers must use explicit notice_marker config",
    )

    workflow_notice_marker_keyerror = verify_variant(
        glm_text=glm.replace(
            'marker = table.get("notice_marker")',
            'marker = table["notice_marker"]',
            1,
        ),
    )
    assert_finding(
        "workflow notice marker explicit error",
        workflow_notice_marker_keyerror,
        "GLM workflow missing expected snippet",
    )

    smoke_runtime_literal = verify_variant(smoke_text=smoke + f"\n          GLM_API_BASE: {current_glm_api_base}\n")
    assert_finding("smoke workflow runtime literal", smoke_runtime_literal, "must read AI review runtime value")

    glm_short_job_timeout = verify_variant(glm_text=glm.replace("timeout-minutes: 35", "timeout-minutes: 20"))
    assert_finding("GLM short job timeout", glm_short_job_timeout, "GLM workflow job timeout must match")

    kimi_short_job_timeout = verify_variant(kimi_text=kimi.replace("timeout-minutes: 45", "timeout-minutes: 35"))
    assert_finding("Kimi short job timeout", kimi_short_job_timeout, "Kimi workflow job timeout must match")

    smoke_job_timeout_drift = verify_variant(smoke_text=smoke.replace("timeout-minutes: 10", "timeout-minutes: 8"))
    assert_finding("Smoke job timeout drift", smoke_job_timeout_drift, "Smoke workflow job timeout must match")

    smoke_head_config = verify_variant(
        smoke_text=smoke.replace(
            "ref: ${{ github.event.pull_request.base.sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
    )
    assert_finding("Smoke head config", smoke_head_config, "Smoke workflow missing expected snippet")

    missing_mirror = verify_variant(
        pr_agent_text=pr_agent.replace("NO HARDCODES: every runtime value comes from TOML config", "NO HARDCODES"),
    )
    assert_finding("missing PR-Agent mirror", missing_mirror, ".pr_agent.toml missing mirrored")

    invalid_mirror_toml = verify_pr_agent_mirror("[pr_agent_mirror\n", pr_agent)
    assert_finding("invalid PR-Agent mirror TOML", invalid_mirror_toml, "ci/ai-review.toml invalid TOML")

    missing_mirror_table = verify_pr_agent_mirror('title = "missing"\n', pr_agent)
    assert_finding("missing PR-Agent mirror table", missing_mirror_table, "missing [pr_agent_mirror]")

    bad_mirror_rules = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note"]
        rules = "not-a-table-array"
        """,
        pr_agent,
    )
    assert_finding("bad PR-Agent mirror rules", bad_mirror_rules, "rules must be a non-empty table array")

    missing_required_note_snippets = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]

        [[pr_agent_mirror.rules]]
        name = "scope discipline"
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]
        """,
        pr_agent,
    )
    assert_finding(
        "missing PR-Agent mirror note snippets",
        missing_required_note_snippets,
        "required_note_snippets must be a non-empty string list",
    )

    empty_required_note_snippets = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = []

        [[pr_agent_mirror.rules]]
        name = "scope discipline"
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]
        """,
        pr_agent,
    )
    assert_finding(
        "empty PR-Agent mirror note snippets",
        empty_required_note_snippets,
        "required_note_snippets must be a non-empty string list",
    )

    invalid_required_note_snippets = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note", 123]

        [[pr_agent_mirror.rules]]
        name = "scope discipline"
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]
        """,
        pr_agent,
    )
    assert_finding(
        "invalid PR-Agent mirror note snippets",
        invalid_required_note_snippets,
        "required_note_snippets must be a non-empty string list",
    )

    missing_github_token_carveout = verify_variant(
        pr_agent_text=pr_agent.replace(
            "GitHub Actions repository automation may use GitHub's ephemeral `GITHUB_TOKEN`",
            "GitHub automation may use the default token",
        ),
    )
    assert_finding(
        "missing GITHUB_TOKEN carve-out mirror",
        missing_github_token_carveout,
        ".pr_agent.toml missing mirrored",
    )

    duplicate_mirror_rule = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note"]

        [[pr_agent_mirror.rules]]
        name = "scope discipline"
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]

        [[pr_agent_mirror.rules]]
        name = "scope discipline"
        snippets = ["flag out-of-scope changes, hidden adjacent work, and missing claimed scope"]
        """,
        pr_agent,
    )
    assert_finding("duplicate PR-Agent mirror rule", duplicate_mirror_rule, "duplicate pr_agent_mirror rule")

    missing_mirror_rule_name = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note"]

        [[pr_agent_mirror.rules]]
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]
        """,
        pr_agent,
    )
    assert_finding("missing PR-Agent mirror rule name", missing_mirror_rule_name, "rules[0].name must be non-empty")

    empty_mirror_rule_name = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note"]

        [[pr_agent_mirror.rules]]
        name = ""
        snippets = ["Scope discipline: one branch or PR may cover only one declared issue, spec, task, or explicitly named slice"]
        """,
        pr_agent,
    )
    assert_finding("empty PR-Agent mirror rule name", empty_mirror_rule_name, "rules[0].name must be non-empty")

    empty_mirror_snippet = verify_pr_agent_mirror(
        """
        [pr_agent_mirror]
        required_note_snippets = ["placeholder governance note"]

        [[pr_agent_mirror.rules]]
        name = "empty snippets"
        snippets = [""]
        """,
        pr_agent,
    )
    assert_finding("empty PR-Agent mirror snippet", empty_mirror_snippet, "snippets must be non-empty")

    glm_split_config = verify_variant(glm_text=glm + "\n          pr_reviewer.num_max_findings: \"6\"\n")
    assert_finding("GLM split config", glm_split_config, "must not define pr_reviewer.*")

    glm_missing_fallback = verify_variant(glm_text=glm.replace("scripts/ai_review_deliverables.py glm-fallback", "echo missing"))
    assert_finding("GLM missing fallback", glm_missing_fallback, "GLM workflow missing expected snippet")

    glm_missing_infrastructure_notice = verify_variant(
        glm_text=glm.replace("gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"", "python3 scripts/ai_review_deliverables.py notice"),
    )
    assert_finding(
        "GLM helper-based fallback infrastructure notice",
        glm_missing_infrastructure_notice,
        "must not depend on ai_review_deliverables.py",
    )

    kimi_head_governance = verify_variant(
        kimi_text=kimi.replace(
            "ref: ${{ github.event.pull_request.base.sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
    )
    assert_finding("Kimi head governance", kimi_head_governance, "Kimi workflow missing expected snippet")

    kimi_misospace_action = verify_variant(kimi_text=kimi + "\n      - uses: misospace/pr-reviewer-action@deadbeef\n")
    assert_finding("Kimi Misospace action", kimi_misospace_action, "must use the official Kimi CLI path")

    kimi_prompt_override = verify_variant(kimi_text=kimi + "\n          system_prompt_mode: append\n")
    assert_finding("Kimi prompt override", kimi_prompt_override, "must use the official Kimi CLI path")

    kimi_missing_fallback = verify_variant(
        kimi_text=kimi.replace(".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback", "echo missing"),
    )
    assert_finding("Kimi missing fallback", kimi_missing_fallback, "Kimi workflow missing expected snippet")

    kimi_missing_infrastructure_notice = verify_variant(
        kimi_text=kimi.replace("gh pr comment \"$PR_NUMBER\" --repo \"$GITHUB_REPOSITORY\"", "python3 .ai-review/base/scripts/ai_review_deliverables.py notice"),
    )
    assert_finding(
        "Kimi helper-based fallback infrastructure notice",
        kimi_missing_infrastructure_notice,
        "must not depend on ai_review_deliverables.py",
    )

    missing_quality_gate = verify_variant(
        deliverables_text=deliverables.replace("def validate_review_responses(", "def removed_validate_review_responses("),
    )
    assert_finding(
        "missing review quality gate",
        missing_quality_gate,
        "scripts/ai_review_deliverables.py missing expected snippet",
    )

    embedded_pr_agent_label_gate = verify_variant(
        deliverables_text=deliverables.replace(
            "line.strip().startswith(label)",
            "label in line",
        ),
    )
    assert_finding(
        "embedded PR-Agent label gate",
        embedded_pr_agent_label_gate,
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
