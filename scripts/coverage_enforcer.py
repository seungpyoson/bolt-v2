#!/usr/bin/env python3
"""Enforce advisory required CI check coverage without publishing a custom check-run."""

from __future__ import annotations

import argparse
import dataclasses
import os
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
    expected_contexts: tuple[str, ...]
    event_class: str
    policy_reason: str


def load_registry_checks(
    config_path: pathlib.Path = DEFAULT_CONFIG,
) -> dict[str, ci_provenance.RequiredCheckConfig]:
    ci_config = merge_readiness.load_ci_provenance(config_path)
    required_checks = merge_readiness.require_table(
        ci_config, "required_checks", "ci_provenance"
    )
    checks = ci_provenance.load_required_checks(required_checks)
    return {check.context: check for check in checks.values()}


def event_name_for_policy(event: dict[str, object]) -> str:
    if event_pull_request(event) is not None:
        return "pull_request"
    merge_group = event.get("merge_group")
    if isinstance(merge_group, dict):
        return "merge_group"
    raise CoverageEnforcerError("event must be pull_request or merge_group")


def event_action_for_policy(event: dict[str, object]) -> str:
    action = event.get("action")
    if isinstance(action, str):
        return action
    return ""


def event_sender_id_for_policy(event: dict[str, object]) -> int:
    sender = event.get("sender")
    if not isinstance(sender, dict):
        return -1
    sender_id = sender.get("id")
    if isinstance(sender_id, int) and not isinstance(sender_id, bool):
        return sender_id
    return -1


def pull_request_base_changed_for_policy(event: dict[str, object]) -> bool:
    changes = event.get("changes")
    if not isinstance(changes, dict):
        return False
    base = changes.get("base")
    if not isinstance(base, dict):
        return False
    ref = base.get("ref")
    if not isinstance(ref, dict):
        return False
    return isinstance(ref.get("from"), str)


def pull_request_head_ref_for_policy(pr: dict[str, object] | None) -> str:
    if pr is None:
        return ""
    head = pr.get("head")
    if not isinstance(head, dict):
        return ""
    head_ref = head.get("ref")
    if isinstance(head_ref, str):
        return head_ref
    return ""


def policy_result_for_event(
    *,
    config: ci_provenance.ProvenanceConfig,
    event: dict[str, object],
    docs_only: bool = False,
) -> ci_provenance.CiPolicyResult:
    pr = event_pull_request(event)
    event_name = event_name_for_policy(event)
    return ci_provenance.evaluate_ci_policy(
        config,
        event_name=event_name,
        event_action=event_action_for_policy(event),
        pull_request_draft=bool(pr.get("draft")) if pr is not None else False,
        pull_request_head_ref=pull_request_head_ref_for_policy(pr),
        pull_request_base_changed=pull_request_base_changed_for_policy(event),
        docs_only=docs_only,
        event_sender_id=event_sender_id_for_policy(event),
        ref=str(event.get("ref", "")),
    )


def expected_registry_checks_for_policy(
    *,
    config: ci_provenance.ProvenanceConfig,
    policy_result: ci_provenance.CiPolicyResult,
) -> tuple[ci_provenance.RequiredCheckConfig, ...]:
    expected: list[ci_provenance.RequiredCheckConfig] = []
    for check in config.required_checks.values():
        if not check.target or check.reporter == "self":
            continue
        applicable = ci_provenance.required_check_applicable_event_classes(
            check=check,
            policy=config.policy,
            gate_names=config.gate_names,
        )
        carry_forward = ci_provenance.required_check_carry_forward_event_classes(
            check=check,
            policy=config.policy,
            applicable=applicable,
        )
        if policy_result.expected_event_class in applicable - carry_forward:
            expected.append(check)
    return tuple(expected)


def expected_registry_checks(
    *,
    config_path: pathlib.Path = DEFAULT_CONFIG,
    event: dict[str, object],
    docs_only: bool = False,
) -> tuple[ci_provenance.RequiredCheckConfig, ...]:
    config = ci_provenance.load_config(config_path)
    policy_result = policy_result_for_event(
        config=config,
        event=event,
        docs_only=docs_only,
    )
    return expected_registry_checks_for_policy(config=config, policy_result=policy_result)


def expected_registry_contexts(
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
) -> tuple[str, ...]:
    return tuple(check.context for check in checks)


def active_expected_registry_checks(
    *,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    config: ci_provenance.ProvenanceConfig,
    check_runs: list[dict[str, object]] | None,
) -> tuple[ci_provenance.RequiredCheckConfig, ...]:
    active_contexts = merge_readiness.resolve_required_contexts(
        expected_registry_contexts(checks),
        {"gate_names": config.gate_names},
        check_runs,
    )
    active_checks: list[ci_provenance.RequiredCheckConfig] = []
    for check, active_context in zip(checks, active_contexts, strict=True):
        if active_context == check.context:
            active_checks.append(check)
        else:
            active_checks.append(dataclasses.replace(check, context=active_context))
    return tuple(active_checks)


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
        # A backtester noop gate recomputes/proves the BVS lane state directly;
        # only an explicit carry-forward resolver makes the context carry-forward capable.
        return (
            "resolve-gate-carry-forward" in workflow_text
            and "check-backtester-gate" in workflow_text
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


def expected_app_runs(
    check_runs: list[dict[str, object]],
    check: ci_provenance.RequiredCheckConfig,
) -> list[dict[str, object]]:
    return [
        run
        for run in check_runs
        if (
            run.get("name") == check.context
            and app_id_for_run(run) == check.integration_id
        )
    ]


def check_run_attempt_sort_key(run: dict[str, object]) -> tuple[object, int]:
    return (
        merge_readiness.parse_timestamp(run.get("started_at")),
        merge_readiness.positive_int(run.get("id"), "check run id"),
    )


def latest_check_run(runs: list[dict[str, object]]) -> dict[str, object] | None:
    if not runs:
        return None
    return max(runs, key=check_run_attempt_sort_key)


def has_newer_incomplete_expected_app_run(
    *,
    check_runs: list[dict[str, object]],
    check: ci_provenance.RequiredCheckConfig,
    latest_context_run: dict[str, object],
) -> bool:
    latest_context_key = check_run_attempt_sort_key(latest_context_run)
    for run in check_runs:
        if app_id_for_run(run) != check.integration_id:
            continue
        if run.get("status") == "completed":
            continue
        if check_run_attempt_sort_key(run) > latest_context_key:
            return True
    return False


def pending_contexts(
    *,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    check_runs: list[dict[str, object]],
) -> tuple[str, ...]:
    pending: list[str] = []
    for check in checks:
        latest = latest_check_run(expected_app_runs(check_runs, check))
        if latest is None or latest.get("status") != "completed":
            pending.append(check.context)
            continue
        if (
            latest.get("conclusion") != SUCCESS_CONCLUSION
            and has_newer_incomplete_expected_app_run(
                check_runs=check_runs,
                check=check,
                latest_context_run=latest,
            )
        ):
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
                f"{check.context} has no terminal check-run from GitHub App id {expected}"
            )
            continue
        latest_matching = latest_check_run(matching)
        if latest_matching.get("conclusion") != SUCCESS_CONCLUSION:
            findings.append(
                f"{check.context} latest terminal check-run from GitHub App id "
                f"{expected} was not successful: {latest_matching.get('conclusion')!r}"
            )
    return tuple(findings)


def poll_required_check_runs(
    *,
    repo: str,
    token: str,
    head_sha: str,
    checks: tuple[ci_provenance.RequiredCheckConfig, ...],
    config: ci_provenance.ProvenanceConfig,
    settings: merge_readiness.MergeReadinessSettings,
    api_json=merge_readiness.github_api_json,
    monotonic: Callable[[], float] = time.monotonic,
    sleep: Callable[[float], None] = time.sleep,
) -> tuple[list[dict[str, object]], tuple[str, ...], tuple[ci_provenance.RequiredCheckConfig, ...]]:
    deadline = monotonic() + settings.max_watch_seconds
    latest_runs: list[dict[str, object]] = []
    latest_checks = checks
    latest_pending: tuple[str, ...] = tuple(check.context for check in checks)
    while True:
        latest_runs = merge_readiness.check_runs_for_sha(
            repo=repo,
            token=token,
            sha=head_sha,
            settings=settings,
            api_json=api_json,
        )
        latest_checks = active_expected_registry_checks(
            checks=checks,
            config=config,
            check_runs=latest_runs,
        )
        latest_pending = pending_contexts(checks=latest_checks, check_runs=latest_runs)
        if not latest_pending:
            return latest_runs, (), latest_checks
        if monotonic() >= deadline:
            return latest_runs, latest_pending, latest_checks
        sleep(settings.poll_seconds)


def summary_for_findings(findings: tuple[str, ...]) -> str:
    if not findings:
        return "All required checks reported from the registered GitHub App on the exact head SHA."
    return "\n".join(f"- {finding}" for finding in findings)


def step_summary_text(
    *,
    result: CoverageEnforcementResult,
) -> str:
    expected = ", ".join(result.expected_contexts) or "(none)"
    return "\n".join(
        (
            "# coverage-enforcer",
            "",
            f"- conclusion: {result.conclusion}",
            f"- head SHA: {result.head_sha}",
            f"- event class: {result.event_class}",
            f"- policy reason: {result.policy_reason}",
            f"- expected contexts: {expected}",
            "- mode: advisory native job; no custom check-run is published",
            "",
            result.summary,
            "",
        )
    )


def write_step_summary(result: CoverageEnforcementResult) -> str | None:
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if not summary_path:
        return None
    try:
        with pathlib.Path(summary_path).open("a", encoding="utf-8") as handle:
            handle.write(step_summary_text(result=result))
    except OSError as exc:
        return f"coverage-enforcer warning: could not write GITHUB_STEP_SUMMARY: {exc}"
    return None


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
    ci_config = ci_provenance.load_config(config_path)
    policy_result = policy_result_for_event(config=ci_config, event=event)
    required_checks = expected_registry_checks_for_policy(
        config=ci_config,
        policy_result=policy_result,
    )
    settings = merge_readiness.merge_settings(config_path)

    findings = list(
        registry_workflow_derivation_findings(
            checks=all_checks,
            workflow_dir=workflow_dir,
        )
    )
    check_runs, pending, active_required_checks = poll_required_check_runs(
        repo=repo,
        token=token,
        head_sha=head_sha,
        checks=required_checks,
        config=ci_config,
        settings=settings,
        api_json=api_json,
        monotonic=monotonic,
        sleep=sleep,
    )
    findings.extend(
        drift_findings_for_terminal_runs(checks=active_required_checks, check_runs=check_runs)
    )
    if pending:
        findings.append(
            "timed out waiting for terminal check-runs: " + ", ".join(pending)
        )

    conclusion = FAILURE_CONCLUSION if findings else SUCCESS_CONCLUSION
    summary = summary_for_findings(tuple(findings))
    return CoverageEnforcementResult(
        conclusion=conclusion,
        head_sha=head_sha,
        findings=tuple(findings),
        summary=summary,
        expected_contexts=expected_registry_contexts(active_required_checks),
        event_class=policy_result.expected_event_class,
        policy_reason=policy_result.reason,
    )


def parser() -> argparse.ArgumentParser:
    argument_parser = argparse.ArgumentParser(
        description="Enforce advisory required-check coverage"
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

    summary_warning = write_step_summary(result)
    if summary_warning is not None:
        print(summary_warning, file=sys.stderr)
    print(result.summary)
    return 0 if result.conclusion == SUCCESS_CONCLUSION else 1


if __name__ == "__main__":
    sys.exit(main())
