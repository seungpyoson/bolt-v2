#!/usr/bin/env python3
"""Publish an advisory check proving required CI check coverage has not drifted."""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import sys
import time
from collections.abc import Callable


SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import ci_provenance
import merge_readiness


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
DEFAULT_WORKFLOW_DIR = REPO_ROOT / ".github" / "workflows"
COVERAGE_CHECK_NAME = ci_provenance.TARGET_REQUIRED_CHECK_CONTEXT
SUCCESS_CONCLUSION = "success"
FAILURE_CONCLUSION = "failure"


class CoverageEnforcerError(RuntimeError):
    """Raised when coverage-enforcer cannot safely complete."""


@dataclasses.dataclass(frozen=True)
class DerivedWorkflowFlags:
    runs_on_tags: bool
    supports_carry_forward: bool


@dataclasses.dataclass(frozen=True)
class CoverageEnforcementResult:
    conclusion: str
    head_sha: str
    findings: tuple[str, ...]
    summary: str
    published: bool
    publish_reason: str


def load_registry_checks(
    config_path: pathlib.Path = DEFAULT_CONFIG,
) -> dict[str, ci_provenance.RequiredCheckConfig]:
    ci_config = merge_readiness.load_ci_provenance(config_path)
    required_checks = merge_readiness.require_table(
        ci_config, "required_checks", "ci_provenance"
    )
    checks = ci_provenance.load_required_checks(required_checks)
    return {check.context: check for check in checks.values()}


def required_registry_checks(
    config_path: pathlib.Path = DEFAULT_CONFIG,
) -> tuple[ci_provenance.RequiredCheckConfig, ...]:
    checks = load_registry_checks(config_path)
    required = []
    for context in merge_readiness.required_contexts(config_path):
        check = checks.get(context)
        if check is None:
            raise CoverageEnforcerError(f"required context {context!r} is missing from registry")
        required.append(check)
    return tuple(required)


def workflow_path_for_check(check: ci_provenance.RequiredCheckConfig) -> str:
    if check.reporter == "self":
        return "coverage-enforcer.yml"
    reporter_workflow = check.reporter.split(" ", 1)[0]
    if reporter_workflow.endswith(".yml"):
        return reporter_workflow
    raise CoverageEnforcerError(
        f"ci_provenance.required_checks.{check.key}.reporter does not name a workflow"
    )


def workflow_text_for_check(
    check: ci_provenance.RequiredCheckConfig, workflow_dir: pathlib.Path
) -> str:
    workflow_path = workflow_dir / workflow_path_for_check(check)
    try:
        return workflow_path.read_text(encoding="utf-8")
    except FileNotFoundError as exc:
        raise CoverageEnforcerError(f"workflow for {check.context} missing: {workflow_path}") from exc
    except OSError as exc:
        raise CoverageEnforcerError(f"workflow for {check.context} could not be read: {exc}") from exc


def strip_comment(line: str) -> str:
    return line.split("#", 1)[0].rstrip()


def top_level_block(workflow_text: str, key: str) -> list[str]:
    lines = [strip_comment(line) for line in workflow_text.splitlines()]
    for index, line in enumerate(lines):
        if line != f"{key}:":
            continue
        block: list[str] = []
        for child in lines[index + 1 :]:
            if child and not child.startswith((" ", "\t")):
                break
            block.append(child)
        return block
    return []


def workflow_trigger_block(workflow_text: str, trigger: str) -> list[str]:
    on_block = top_level_block(workflow_text, "on")
    trigger_line = f"  {trigger}:"
    for index, line in enumerate(on_block):
        if line.strip() != trigger_line.strip():
            continue
        block: list[str] = []
        for child in on_block[index + 1 :]:
            if re.match(r"^  [^ \t:#][^:#]*:", child):
                break
            block.append(child)
        return block
    return []


def workflow_declares_tag_push(workflow_text: str) -> bool:
    push_block = workflow_trigger_block(workflow_text, "push")
    return any(line.strip().startswith("tags:") for line in push_block)


def workflow_job_text(workflow_text: str, job_id: str) -> str:
    jobs_block = top_level_block(workflow_text, "jobs")
    job_header = f"  {job_id}:"
    for index, line in enumerate(jobs_block):
        if line.strip() != job_header.strip():
            continue
        job_lines: list[str] = []
        for child in jobs_block[index + 1 :]:
            if re.match(r"^  [^ \t:#][^:#]*:", child):
                break
            job_lines.append(child)
        return "\n".join(job_lines)
    return ""


def job_excludes_tag_refs(job_text: str) -> bool:
    return (
        "!startsWith(github.ref, 'refs/tags/" in job_text
        or '!startsWith(github.ref, "refs/tags/' in job_text
    )


def check_supports_policy_tag_reuse(
    check: ci_provenance.RequiredCheckConfig, workflow_text: str
) -> bool:
    if check.context == "gate":
        return (
            "name: ${{ needs.ci-policy.outputs.gate_name }}" in workflow_text
            and '--ref "${{ github.ref }}"' in workflow_text
        )
    if check.context == "backtester-gate":
        return (
            "name: ${{ needs.ci-policy.outputs.backtester_gate_name }}" in workflow_text
            and '--ref "${{ github.ref }}"' in workflow_text
        )
    return False


def check_runs_on_tags(
    check: ci_provenance.RequiredCheckConfig, workflow_text: str
) -> bool:
    if check_supports_policy_tag_reuse(check, workflow_text):
        return True
    if not workflow_declares_tag_push(workflow_text):
        return False
    return not job_excludes_tag_refs(workflow_job_text(workflow_text, check.context))


def check_supports_policy_carry_forward(
    check: ci_provenance.RequiredCheckConfig, workflow_text: str
) -> bool:
    if check.context == "gate":
        return (
            "resolve-gate-carry-forward" in workflow_text
            and "check-ci-gate" in workflow_text
            and "needs.ci-policy.outputs.ci_policy_path == 'noop'" in workflow_text
            and "needs.ci-policy.outputs.full_ci_deferred == 'true'" in workflow_text
        )
    if check.context == "backtester-gate":
        return (
            "check-backtester-gate" in workflow_text
            and "needs.ci-policy.outputs.ci_policy_path == 'noop'" in workflow_text
            and "needs.ci-policy.outputs.full_ci_deferred == 'true'" in workflow_text
        )
    return False


def derive_registry_workflow_flags(
    checks: dict[str, ci_provenance.RequiredCheckConfig],
    workflow_dir: pathlib.Path = DEFAULT_WORKFLOW_DIR,
) -> dict[str, DerivedWorkflowFlags]:
    """Derive registry booleans from the workflow that reports each context."""

    derived: dict[str, DerivedWorkflowFlags] = {}
    for context, check in checks.items():
        workflow_text = workflow_text_for_check(check, workflow_dir)
        derived[context] = DerivedWorkflowFlags(
            runs_on_tags=check_runs_on_tags(check, workflow_text),
            supports_carry_forward=check_supports_policy_carry_forward(
                check, workflow_text
            ),
        )
    return derived


def registry_workflow_derivation_findings(
    *,
    checks: dict[str, ci_provenance.RequiredCheckConfig],
    workflow_dir: pathlib.Path,
) -> tuple[str, ...]:
    derived = derive_registry_workflow_flags(checks, workflow_dir)
    findings: list[str] = []
    for context, check in checks.items():
        flags = derived[context]
        if flags.runs_on_tags != check.runs_on_tags:
            findings.append(
                "registry/YAML derivation mismatch for "
                f"{context}: runs_on_tags registry={check.runs_on_tags} "
                f"derived={flags.runs_on_tags}"
            )
        if flags.supports_carry_forward != check.supports_carry_forward:
            findings.append(
                "registry/YAML derivation mismatch for "
                f"{context}: supports_carry_forward registry={check.supports_carry_forward} "
                f"derived={flags.supports_carry_forward}"
            )
    return tuple(findings)


def app_id_for_run(run: dict[str, object]) -> int | None:
    app = run.get("app")
    if not isinstance(app, dict):
        return None
    app_id = app.get("id")
    if isinstance(app_id, int) and not isinstance(app_id, bool):
        return app_id
    return None


def terminal_runs_for_context(
    check_runs: list[dict[str, object]], context: str
) -> list[dict[str, object]]:
    return [
        run
        for run in check_runs
        if run.get("name") == context and run.get("status") == "completed"
    ]


def pending_contexts(
    *,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    check_runs: list[dict[str, object]],
) -> tuple[str, ...]:
    pending: list[str] = []
    for check in checks:
        if not terminal_runs_for_context(check_runs, check.context):
            pending.append(check.context)
    return tuple(pending)


def drift_findings_for_terminal_runs(
    *,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    check_runs: list[dict[str, object]],
) -> tuple[str, ...]:
    findings: list[str] = []
    for check in checks:
        terminal = terminal_runs_for_context(check_runs, check.context)
        if not terminal:
            continue
        app_ids = [app_id_for_run(run) for run in terminal]
        expected = check.integration_id
        matching = [run for run in terminal if app_id_for_run(run) == expected]
        unexpected = [app_id for app_id in app_ids if app_id != expected]
        if unexpected:
            findings.append(
                f"{check.context} was reported by wrong GitHub App id(s) "
                f"{unexpected!r}; expected {expected}"
            )
        if not matching:
            findings.append(
                f"{check.context} must have at least one terminal check-run from "
                f"GitHub App id {expected}; found 0"
            )
        distinct_app_ids = list(dict.fromkeys(app_ids))
        if len(distinct_app_ids) != 1:
            findings.append(
                f"{check.context} has unexpected duplicate terminal reporters: app ids {app_ids!r}"
            )
    return tuple(findings)


def poll_required_check_runs(
    *,
    repo: str,
    token: str,
    head_sha: str,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    settings: merge_readiness.MergeReadinessSettings,
    api_json=merge_readiness.github_api_json,
    monotonic: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> tuple[list[dict[str, object]], tuple[str, ...]]:
    deadline = monotonic() + settings.max_watch_seconds
    latest_runs: list[dict[str, object]] = []
    latest_pending: tuple[str, ...] = tuple(check.context for check in checks)
    while True:
        latest_runs = merge_readiness.check_runs_for_sha(
            repo=repo,
            token=token,
            sha=head_sha,
            settings=settings,
            api_json=api_json,
        )
        latest_pending = pending_contexts(checks=checks, check_runs=latest_runs)
        if not latest_pending:
            return latest_runs, ()
        if monotonic() >= deadline:
            return latest_runs, latest_pending
        sleep(settings.poll_seconds)


def summary_for_findings(findings: tuple[str, ...]) -> str:
    if not findings:
        return "All required checks reported from the registered GitHub App on the exact head SHA."
    return "\n".join(f"- {finding}" for finding in findings)


def publish_coverage_check_run(
    *,
    repo: str,
    token: str,
    head_sha: str,
    conclusion: str,
    summary: str,
    api_json=merge_readiness.github_api_json,
) -> None:
    api_json(
        repo,
        token,
        "check-runs",
        method="POST",
        data={
            "name": COVERAGE_CHECK_NAME,
            "head_sha": head_sha,
            "status": "completed",
            "conclusion": conclusion,
            "output": {
                "title": "Coverage enforcer",
                "summary": summary,
            },
        },
    )


def event_pull_request(event: dict[str, object]) -> dict[str, object] | None:
    if not isinstance(event, dict):
        raise CoverageEnforcerError("event payload is malformed")
    pr = event.get("pull_request")
    if pr is None:
        return None
    if not isinstance(pr, dict):
        raise CoverageEnforcerError("pull_request payload is malformed")
    return pr


def head_sha_from_event(event: dict[str, object]) -> str:
    pr = event_pull_request(event)
    if pr is not None:
        return merge_readiness.pull_request_head_sha(pr)
    merge_group = event.get("merge_group")
    if isinstance(merge_group, dict):
        head_sha = merge_group.get("head_sha")
        if isinstance(head_sha, str) and merge_readiness.SHA_RE.fullmatch(head_sha):
            return head_sha
        raise CoverageEnforcerError("merge_group.head_sha is missing or malformed")
    raise CoverageEnforcerError("event must be pull_request or merge_group")


def is_fork_event(event: dict[str, object]) -> bool:
    pr = event_pull_request(event)
    return pr is not None and merge_readiness.is_fork_pull_request(pr)


def enforce_coverage(
    *,
    repo: str,
    token: str,
    event: dict[str, object],
    config_path: pathlib.Path = DEFAULT_CONFIG,
    workflow_dir: pathlib.Path = DEFAULT_WORKFLOW_DIR,
    api_json=merge_readiness.github_api_json,
    monotonic: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> CoverageEnforcementResult:
    head_sha = head_sha_from_event(event)
    all_checks = load_registry_checks(config_path)
    required_checks = required_registry_checks(config_path)
    settings = merge_readiness.merge_settings(config_path)

    findings = list(
        registry_workflow_derivation_findings(
            checks=all_checks,
            workflow_dir=workflow_dir,
        )
    )
    check_runs, pending = poll_required_check_runs(
        repo=repo,
        token=token,
        head_sha=head_sha,
        checks=required_checks,
        settings=settings,
        api_json=api_json,
        monotonic=monotonic,
        sleep=sleep,
    )
    findings.extend(
        drift_findings_for_terminal_runs(checks=required_checks, check_runs=check_runs)
    )
    if pending:
        findings.append(
            "timed out waiting for terminal check-runs: " + ", ".join(pending)
        )

    conclusion = FAILURE_CONCLUSION if findings else SUCCESS_CONCLUSION
    summary = summary_for_findings(tuple(findings))
    if is_fork_event(event):
        return CoverageEnforcementResult(
            conclusion=conclusion,
            head_sha=head_sha,
            findings=tuple(findings),
            summary=summary,
            published=False,
            publish_reason="skipping coverage-enforcer check-run publish for fork PR",
        )

    publish_coverage_check_run(
        repo=repo,
        token=token,
        head_sha=head_sha,
        conclusion=conclusion,
        summary=summary,
        api_json=api_json,
    )
    return CoverageEnforcementResult(
        conclusion=conclusion,
        head_sha=head_sha,
        findings=tuple(findings),
        summary=summary,
        published=True,
        publish_reason="published coverage-enforcer check-run",
    )


def parser() -> argparse.ArgumentParser:
    argument_parser = argparse.ArgumentParser(
        description="Publish advisory coverage-enforcer check-run"
    )
    argument_parser.add_argument(
        "--config",
        type=pathlib.Path,
        default=DEFAULT_CONFIG,
        help="Path to ci/github-actions-runners.toml",
    )
    argument_parser.add_argument(
        "--workflow-dir",
        type=pathlib.Path,
        default=DEFAULT_WORKFLOW_DIR,
        help="Path to .github/workflows",
    )
    return argument_parser


def main(argv: list[str] | None = None, *, api_json=merge_readiness.github_api_json) -> int:
    args = parser().parse_args(argv)
    try:
        repo = merge_readiness.require_env("GITHUB_REPOSITORY")
        token = merge_readiness.require_env("GITHUB_TOKEN")
        event_path = pathlib.Path(merge_readiness.require_env("GITHUB_EVENT_PATH"))
        event = merge_readiness.load_event(event_path)
        result = enforce_coverage(
            repo=repo,
            token=token,
            event=event,
            config_path=args.config,
            workflow_dir=args.workflow_dir,
            api_json=api_json,
        )
    except (CoverageEnforcerError, merge_readiness.MergeReadinessError, ci_provenance.ProvenanceError) as exc:
        print(f"coverage-enforcer failed: {exc}", file=sys.stderr)
        return 1

    print(result.publish_reason)
    print(result.summary)
    return 0 if result.conclusion == SUCCESS_CONCLUSION else 1


if __name__ == "__main__":
    sys.exit(main())
