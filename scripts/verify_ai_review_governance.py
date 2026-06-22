#!/usr/bin/env python3
"""Verify AI PR review governance mirrors stay current."""

from __future__ import annotations

import argparse
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
            "required reviewer approved",
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
    "standards_file: .ai-review/standards/kimi-review-standards.md",
)

GLM_DELIVERABLE_SNIPPETS = (
    "Capture GLM review window",
    "id: pr-agent",
    "timeout-minutes: 8",
    "Ensure GLM deliverable or post split fallback",
    "scripts/ai_review_deliverables.py glm-fallback",
    "GLM_REVIEW_MAX_CHUNK_CHARS",
    "AI_REVIEW_MAX_COMMENT_CHARS",
    "--instructions-file .pr_agent.toml",
)

KIMI_DELIVERABLE_SNIPPETS = (
    "id: kimi-review",
    "timeout-minutes: 8",
    "Capture Kimi review window",
    "Ensure Kimi deliverable or post split fallback",
    ".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback",
    'comment_marker: "<!-- ai-pr-reviewer-kimi -->"',
    'KIMI_DELIVERABLE_MARKER: "<!-- ai-pr-reviewer-kimi -->"',
    "KIMI_REVIEW_MAX_CHUNK_CHARS",
    "AI_REVIEW_MAX_COMMENT_CHARS",
    "--instructions-file .ai-review/standards/kimi-review-standards.md",
)

KIMI_FORBIDDEN_INPUTS = (
    "system_prompt:",
    "system_prompt_file:",
    "system_prompt_mode:",
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


def missing_snippets(label: str, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{label} missing expected snippet: {snippet!r}" for snippet in snippets if snippet not in text]


def verify_texts(
    *,
    agents_md: str,
    pr_agent_toml: str,
    glm_workflow: str,
    kimi_workflow: str,
) -> list[str]:
    findings: list[str] = []

    extra, extra_findings = pr_agent_extra_instructions(pr_agent_toml)
    findings.extend(extra_findings)
    if extra_findings:
        return findings

    findings.extend(missing_snippets("AGENTS.md", agents_md, AGENTS_BACKPOINTER_SNIPPETS))
    findings.extend(missing_snippets(".pr_agent.toml extra_instructions", extra, PR_AGENT_MIRROR_NOTE_SNIPPETS))

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

    findings.extend(missing_snippets("GLM workflow", glm_workflow, GLM_DELIVERABLE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_BASE_GOVERNANCE_SNIPPETS))
    findings.extend(missing_snippets("Kimi workflow", kimi_workflow, KIMI_DELIVERABLE_SNIPPETS))
    for snippet in KIMI_FORBIDDEN_INPUTS:
        if snippet in kimi_workflow:
            findings.append(f"Kimi workflow must not override the bundled prompt with {snippet!r}")

    return findings


def verify_repo(repo_root: Path) -> list[str]:
    return verify_texts(
        agents_md=read_text(repo_root / "AGENTS.md"),
        pr_agent_toml=read_text(repo_root / ".pr_agent.toml"),
        glm_workflow=read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml"),
        kimi_workflow=read_text(repo_root / ".github/workflows/ai-review-kimi-misospace.yml"),
    )


def assert_finding(name: str, findings: list[str], expected: str) -> None:
    if not any(expected in finding for finding in findings):
        raise AssertionError(f"{name}: expected finding containing {expected!r}, got {findings!r}")


def run_self_tests(repo_root: Path) -> None:
    agents = read_text(repo_root / "AGENTS.md")
    pr_agent = read_text(repo_root / ".pr_agent.toml")
    glm = read_text(repo_root / ".github/workflows/ai-review-glm-pr-agent.yml")
    kimi = read_text(repo_root / ".github/workflows/ai-review-kimi-misospace.yml")

    baseline = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm,
        kimi_workflow=kimi,
    )
    if baseline:
        raise AssertionError(f"real repository must satisfy AI review governance check, got {baseline!r}")

    missing_mirror = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent.replace("NO HARDCODES: every runtime value comes from TOML config", "NO HARDCODES"),
        glm_workflow=glm,
        kimi_workflow=kimi,
    )
    assert_finding("missing PR-Agent mirror", missing_mirror, ".pr_agent.toml missing mirrored")

    changed_source = verify_texts(
        agents_md=agents.replace("**NO DUAL PATHS**", "**NO MULTI PATHS**"),
        pr_agent_toml=pr_agent,
        glm_workflow=glm,
        kimi_workflow=kimi,
    )
    assert_finding("changed AGENTS source", changed_source, "AGENTS.md source for mirrored rule 'no dual paths'")

    glm_split_config = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm + "\n          pr_reviewer.num_max_findings: \"6\"\n",
        kimi_workflow=kimi,
    )
    assert_finding("GLM split config", glm_split_config, "must not define pr_reviewer.*")

    glm_missing_fallback = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm.replace("scripts/ai_review_deliverables.py glm-fallback", "echo missing"),
        kimi_workflow=kimi,
    )
    assert_finding("GLM missing fallback", glm_missing_fallback, "GLM workflow missing expected snippet")

    kimi_head_governance = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm,
        kimi_workflow=kimi.replace(
            "ref: ${{ github.event.pull_request.base.sha }}",
            "ref: ${{ github.event.pull_request.head.sha }}",
        ),
    )
    assert_finding("Kimi head governance", kimi_head_governance, "Kimi workflow missing expected snippet")

    kimi_prompt_override = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm,
        kimi_workflow=kimi + "\n          system_prompt_mode: append\n",
    )
    assert_finding("Kimi prompt override", kimi_prompt_override, "must not override the bundled prompt")

    kimi_missing_fallback = verify_texts(
        agents_md=agents,
        pr_agent_toml=pr_agent,
        glm_workflow=glm,
        kimi_workflow=kimi.replace(".ai-review/base/scripts/ai_review_deliverables.py kimi-fallback", "echo missing"),
    )
    assert_finding("Kimi missing fallback", kimi_missing_fallback, "Kimi workflow missing expected snippet")


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
