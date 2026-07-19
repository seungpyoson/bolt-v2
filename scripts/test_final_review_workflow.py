#!/usr/bin/env python3
"""Static contract for the sole fixed final-review workflow."""

from __future__ import annotations

import pathlib
import re
import tomllib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
FINAL_REVIEW = REPO_ROOT / ".github/workflows/final-review.yml"
WORKFLOW_ROOT = REPO_ROOT / ".github/workflows"
RUNNERS_CONFIG = REPO_ROOT / "ci/github-actions-runners.toml"
ALTERNATE_REVIEW_WORKFLOWS = (
    REPO_ROOT / ".github/workflows/ai-review-coding-plan-smoke.yml",
    REPO_ROOT / ".github/workflows/debug-test.yml",
)
WORKERS = (
    REPO_ROOT / ".github/workflows/claude-code-review.yml",
    REPO_ROOT / ".github/workflows/ai-review-kimi-cli.yml",
    REPO_ROOT / ".github/workflows/ai-review-glm.yml",
)
FIXED_JOBS = (
    "capture-head",
    "evidence",
    "claude-review",
    "kimi-review",
    "glm-review",
)
FORBIDDEN_SELECTION_TEXT = (
    "paths-ignore",
    "changed-path",
    "full_ci_required",
    "isDraft",
    "github.event.label",
    "CERTIFIED",
    "UNCERTIFIED",
    "cache-hit ==",
)


def assert_final_review_has_one_fixed_graph() -> None:
    if not FINAL_REVIEW.exists():
        raise AssertionError("final-review.yml is missing")
    text = FINAL_REVIEW.read_text(encoding="utf-8")
    on_block = text.split("concurrency:", 1)[0]
    if "workflow_dispatch:" not in on_block:
        raise AssertionError("final-review must expose workflow_dispatch")
    for trigger in ("pull_request:", "pull_request_review:", "workflow_call:"):
        if trigger in on_block:
            raise AssertionError(f"final-review exposes alternate trigger: {trigger}")
    for job in FIXED_JOBS:
        if not re.search(rf"^  {re.escape(job)}:\s*$", text, flags=re.MULTILINE):
            raise AssertionError(f"fixed final-review job missing: {job}")
    for forbidden in FORBIDDEN_SELECTION_TEXT:
        if forbidden in text:
            raise AssertionError(f"conditional job selection remains: {forbidden}")
    if_lines = [line.strip() for line in text.splitlines() if line.lstrip().startswith("if:")]
    if if_lines:
        raise AssertionError(f"final-review must have one unconditional fixed sequence, got {if_lines!r}")
    evidence = text.split("  evidence:\n", 1)[1].split("  claude-review:\n", 1)[0]
    evidence_permissions = evidence.split("    runs-on:", 1)[0]
    if "actions: write" in evidence_permissions or "pull-requests: write" in evidence_permissions:
        raise AssertionError("subject evidence job must not receive write-capable GitHub permissions")
    if "ref: ${{ github.sha }}" not in evidence:
        raise AssertionError("evidence machinery must execute from the governance workflow SHA")
    if "path: subject" not in evidence or "ref: ${{ needs.capture-head.outputs.head_sha }}" not in evidence:
        raise AssertionError("the immutable subject tree must be checked out only as data")
    if "scripts/final_review_runner.py" not in evidence:
        raise AssertionError("evidence must use the single governance-owned runner")
    if "timeout-minutes: ${{ fromJSON(needs.capture-head.outputs.evidence_timeout) }}" not in evidence:
        raise AssertionError("evidence does not enforce its configured timeout")
    if "--obligation-timeout-seconds" not in evidence or "obligation_timeout_seconds" not in text:
        raise AssertionError("evidence does not propagate the configured per-obligation timeout")
    if "Assert every evidence obligation passed" not in evidence:
        raise AssertionError("reviewers are not gated on complete successful evidence")
    if '"failed_tests": []' in evidence:
        raise AssertionError("production evidence must not claim an always-empty failure inventory")
    for worker in WORKERS:
        relative = worker.relative_to(REPO_ROOT).as_posix()
        if f"uses: ./{relative}" not in text:
            raise AssertionError(f"final-review does not invoke {relative}")
    config = tomllib.loads((REPO_ROOT / "ci/ai-review.toml").read_text(encoding="utf-8"))
    per_obligation = config["final_review"]["obligation_timeout_seconds"]
    outer = config["final_review"]["evidence_timeout_minutes"] * 60
    from final_review_runner import FINAL_REVIEW_OBLIGATIONS

    if per_obligation <= 0 or len(FINAL_REVIEW_OBLIGATIONS) * per_obligation >= outer:
        raise AssertionError("configured obligation timeouts can exhaust the evidence job before inventory completion")


def assert_provider_workers_are_triggerless() -> None:
    for path in WORKERS:
        text = path.read_text(encoding="utf-8")
        on_block = text.split("concurrency:", 1)[0]
        if "workflow_call:" not in on_block:
            raise AssertionError(f"{path.name} must expose workflow_call")
        for trigger in ("pull_request:", "pull_request_review:", "workflow_dispatch:"):
            if trigger in on_block:
                raise AssertionError(f"{path.name} exposes alternate trigger: {trigger}")
        for forbidden in ("final-ai-review", "review_readiness", "fallback_model", "fallback_client"):
            if forbidden in text:
                raise AssertionError(f"{path.name} contains {forbidden}")


def assert_reviewers_use_exact_diff_and_compatible_permissions() -> None:
    final = FINAL_REVIEW.read_text(encoding="utf-8")
    required_job_permissions = {
        "claude-review": ("contents: read", "pull-requests: write", "issues: write", "id-token: write"),
        "kimi-review": ("contents: read", "pull-requests: read", "issues: write"),
        "glm-review": ("contents: read", "pull-requests: write", "issues: write"),
    }
    for job, permissions in required_job_permissions.items():
        block = final.split(f"  {job}:\n", 1)[1]
        for permission in permissions:
            if permission not in block.split("  ", 1)[0] and permission not in block[:1200]:
                raise AssertionError(f"{job} does not grant {permission}")
    kimi_job = final.split("  kimi-review:\n", 1)[1].split("  glm-review:\n", 1)[0]
    if "pull-requests: write" in kimi_job:
        raise AssertionError("Kimi caller retains permission to submit pull-request reviews")
    kimi_worker = WORKERS[1].read_text(encoding="utf-8")
    kimi_permissions = kimi_worker.split("permissions:\n", 1)[1].split("\njobs:\n", 1)[0]
    if "pull-requests: read" not in kimi_permissions or "pull-requests: write" in kimi_permissions:
        raise AssertionError("Kimi worker must have read-only pull-request access")
    checkout_counts = {
        "claude-code-review.yml": 2,
        "ai-review-kimi-cli.yml": 1,
        "ai-review-glm.yml": 1,
    }
    for worker in WORKERS:
        text = worker.read_text(encoding="utf-8")
        if "base_sha" not in text or "head_sha" not in text:
            raise AssertionError(f"{worker.name} is not bound to both exact SHAs")
        if "ref: ${{ inputs.head_sha }}" in text:
            raise AssertionError(f"{worker.name} checks out untrusted subject code in a secret-bearing job")
        checkout_count = text.count("uses: actions/checkout@")
        governance_ref_count = text.count("ref: ${{ inputs.governance_sha }}")
        if checkout_count != checkout_counts[worker.name] or governance_ref_count != checkout_count:
            raise AssertionError(f"{worker.name} jobs must check out only protected governance")
        if "gh pr diff" in text:
            raise AssertionError(f"{worker.name} may read the moving live PR diff")
        if "job_timeout_minutes" not in text or "timeout-minutes: ${{ inputs.job_timeout_minutes }}" not in text:
            raise AssertionError(f"{worker.name} does not enforce its configured job timeout")
    claude = WORKERS[0].read_text(encoding="utf-8")
    if "  analysis:\n" not in claude or "  publish:\n" not in claude:
        raise AssertionError("Claude analysis and publication must be separate jobs")
    analysis_job = claude.split("  analysis:\n", 1)[1].split("  publish:\n", 1)[0]
    publish_job = claude.split("  publish:\n", 1)[1]
    for forbidden in ("pull-requests: write", "issues: write", "GITHUB_TOKEN"):
        if forbidden in analysis_job:
            raise AssertionError(f"Claude analysis retains publisher authority: {forbidden}")
    for required in ("pull-requests: write", "issues: write", "needs: analysis"):
        if required not in publish_job:
            raise AssertionError(f"Claude publisher is missing {required}")
    for forbidden in ("mcp__github_inline_comment__create_inline_comment", "Bash(gh pr comment:*)"):
        if forbidden in claude:
            raise AssertionError(f"Claude analysis retains comment capability: {forbidden}")
    for required in (
        "id: analysis",
        "steps.analysis.outputs.execution_file",
        "python3 scripts/publish_claude_review.py",
    ):
        if required not in claude:
            raise AssertionError(f"Claude final publisher is missing {required}")
    if claude.count("python3 scripts/publish_claude_review.py") != 1:
        raise AssertionError("Claude must invoke exactly one repository-owned final publisher")
    if "Begin the comment with DELIVERABLE MARKER" in claude:
        raise AssertionError("Claude analysis duplicates publisher-owned comment metadata")
    if "Return only the review body" not in claude:
        raise AssertionError("Claude analysis is not separated from final comment rendering")


def assert_no_alternate_review_or_archive_reuse_workflow() -> None:
    surviving = [path.relative_to(REPO_ROOT).as_posix() for path in ALTERNATE_REVIEW_WORKFLOWS if path.exists()]
    if surviving:
        raise AssertionError(f"alternate review/reuse workflows remain: {', '.join(surviving)}")


def assert_public_command_surface_is_single_path() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    headers = {
        line.split(":", 1)[0].split(" ", 1)[0]
        for line in justfile.splitlines()
        if line and not line[0].isspace() and ":" in line
    }
    if "final-review" not in headers:
        raise AssertionError("just final-review is missing")
    surviving = sorted(headers & {"certify", "review-ready", "verify-remote"})
    if surviving:
        raise AssertionError(f"legacy public commands remain: {', '.join(surviving)}")


def assert_rust_linker_has_one_mandatory_route() -> None:
    action = (REPO_ROOT / ".github/actions/setup-environment/action.yml").read_text(encoding="utf-8")
    policy = (REPO_ROOT / "ci/rust-verification.toml").read_text(encoding="utf-8")
    for forbidden in ("continuing without", "for rust_linker_program in", 'programs = ["mold", "lld"]'):
        if forbidden in action or forbidden in policy:
            raise AssertionError(f"Rust linker fallback remains: {forbidden}")
    if 'programs = ["mold"]' not in policy:
        raise AssertionError("one mandatory Rust linker is not configured")


def assert_setup_checks_configured_actionlint() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    setup = justfile.split("\nsetup:\n", 1)[1]
    if "ci/ai-review.toml" not in setup or "actionlint -version" not in setup:
        raise AssertionError("just setup does not verify the configured actionlint version")


def assert_upload_artifact_uses_one_configured_pin() -> None:
    config = tomllib.loads(RUNNERS_CONFIG.read_text(encoding="utf-8"))
    expected = config["action_pins"]["upload_artifact"]
    if not isinstance(expected, str) or not re.fullmatch(
        r"actions/upload-artifact@[0-9a-f]{40}", expected
    ):
        raise AssertionError("configured upload-artifact action must use an immutable commit SHA")
    storage_mirror = config["storage_audit"]["cleanup_feasibility_alert"]["workflow"][
        "json_artifact_action"
    ]
    if storage_mirror != expected:
        raise AssertionError("storage workflow upload-artifact pin must match action_pins")
    mismatches: list[str] = []
    references = 0
    workflow_paths = sorted((*WORKFLOW_ROOT.glob("*.yml"), *WORKFLOW_ROOT.glob("*.yaml")))
    for path in workflow_paths:
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(
            r"uses:\s*[\"']?(actions/upload-artifact@[^\"'\s#]+)", text
        ):
            references += 1
            actual = match.group(1)
            if actual != expected:
                mismatches.append(f"{path.name}: {actual}")
    if references == 0:
        raise AssertionError("repository workflows contain no upload-artifact action")
    if mismatches:
        raise AssertionError(
            "upload-artifact actions must match the configured pin: " + ", ".join(mismatches)
        )


def main() -> int:
    assert_final_review_has_one_fixed_graph()
    assert_provider_workers_are_triggerless()
    assert_reviewers_use_exact_diff_and_compatible_permissions()
    assert_no_alternate_review_or_archive_reuse_workflow()
    assert_public_command_surface_is_single_path()
    assert_rust_linker_has_one_mandatory_route()
    assert_setup_checks_configured_actionlint()
    assert_upload_artifact_uses_one_configured_pin()
    print("OK: fixed final-review workflow tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
