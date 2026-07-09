#!/usr/bin/env python3
"""Fence scripts against retyping governed config strings."""

from __future__ import annotations

import ast
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]

GOVERNED_CONFIG_ARTIFACTS = (
    "ci/ai-review.toml",
    "ci/bolt-v3-boundary-exemptions.toml",
    "ci/chainlink-reference-fixture-capture-provenance.toml",
    "ci/developer-tool-storage-hygiene.toml",
    "ci/doc-decoupling-residuals.toml",
    "ci/fail-closed-contracts.toml",
    "ci/fail-closed-exceptions.toml",
    "ci/github-actions-runners.toml",
    "ci/nextest-fingerprint.toml",
    "ci/rust-ci-inputs.toml",
    "ci/rust-verification.toml",
    "ci/storage-tripwire.toml",
    "config/clean-merged.toml",
    "config/deploy.toml",
    "config/profiles/prod-btc-5m.overlay.toml",
    "config/root.toml",
    "config/strategies/binary_oracle_bnb.toml",
    "config/strategies/binary_oracle_btc.toml",
    "config/strategies/binary_oracle_doge.toml",
    "config/strategies/binary_oracle_eth.toml",
    "config/strategies/binary_oracle_sol.toml",
    "config/strategies/binary_oracle_xrp.toml",
    "crates/backtesting-vertical-slice/ci/rust-verification.toml",
)
GOVERNED_CONFIG_ARTIFACT_PATTERNS = (
    "ci/*.toml",
    "config/**/*.toml",
    "crates/backtesting-vertical-slice/ci/*.toml",
)

# Strict mode covers Python files touched by #1301 plus this PR. Non-strict
# files stay under the ratchet so old debt can only shrink.
STRICT_RETYPE_PATHS = frozenset(
    {
        "scripts/ci_provenance.py",
        "scripts/clean_merged_artifacts.py",
        "scripts/cargo-shim",
        "scripts/cargo_command_analysis.py",
        "scripts/ci_workflow_hygiene_test_helpers.py",
        "scripts/git_remote_utils.py",
        "scripts/governance_diff_analysis.py",
        "scripts/merge_queue_preflight.py",
        "scripts/merge_queue_operator.py",
        "scripts/minimal_toml.py",
        "scripts/run_ci_lint_suites.py",
        "scripts/rust_verification.py",
        "scripts/sandbox_safe_push.py",
        "scripts/shell_dataflow_analysis.py",
        "scripts/test_cargo_shim.py",
        "scripts/test_cargo_command_analysis.py",
        "scripts/test_clean_merged_artifacts.py",
        "scripts/test_governance_diff_analysis.py",
        "scripts/test_host_health_sampler.py",
        "scripts/test_merge_queue_operator.py",
        "scripts/test_merge_queue_preflight.py",
        "scripts/test_run_ci_lint_suites.py",
        "scripts/test_rust_verification.py",
        "scripts/test_rust_verification_decoupling.py",
        "scripts/test_sandbox_safe_push.py",
        "scripts/test_shell_dataflow_analysis.py",
        "scripts/test_verifier_io.py",
        "scripts/test_verify_bolt_v3_core_boundary.py",
        "scripts/test_verify_bolt_v3_dependency_direction.py",
        "scripts/test_verify_bolt_v3_naming.py",
        "scripts/test_verify_ci_workflow_hygiene.py",
        "scripts/test_verify_no_config_retype.py",
        "scripts/test_verify_probability_typed_pilot.py",
        "scripts/test_verify_ra_notebook_read_only_boundary.py",
        "scripts/test_verify_ra_single_engine_import_boundary.py",
        "scripts/test_workflow_expression_analysis.py",
        "scripts/verifier_io.py",
        "scripts/verify_ai_review_governance.py",
        "scripts/verify_bolt_v3_core_boundary.py",
        "scripts/verify_bolt_v3_naming.py",
        "scripts/verify_ci_workflow_hygiene.py",
        "scripts/verify_dashboard_read_only_contract.py",
        "scripts/verify_no_config_retype.py",
        "scripts/verify_probability_typed_pilot.py",
        "scripts/verify_ra_notebook_read_only_boundary.py",
        "scripts/verify_ra_single_engine_import_boundary.py",
        "scripts/workflow_expression_analysis.py",
    }
)

# Recompute by running verify_no_config_retype.py after intentional ratchet
# removals/additions; this value is the exact current non-strict count.
RATCHET_BASELINE_COUNT = 2571


@dataclass(frozen=True)
class RegisteredRetype:
    path: str
    value: str
    reason: str


@dataclass(frozen=True)
class LiteralHit:
    path: str
    line: int
    value: str


@dataclass(frozen=True)
class ProtectedString:
    value: str
    source: str


STRICT_BOOTSTRAP_REASON = (
    "pre-existing strict-file governance fixture or mirror retained while introducing the no-config-retype fence"
)

STRICT_BOOTSTRAP_RETYPE_VALUES_BY_PATH = {
    "scripts/ci_provenance.py": (
        "]",
        "actionlint",
        "backtester-gate",
        "build",
        "check-aarch64",
        "ci",
        "clippy",
        "coverage-enforcer",
        "defer",
        "deny",
        "detector",
        "docs",
        "full",
        "gate",
        "host-health",
        "iteration",
        "merge_group",
        "meter",
        "nextest-fingerprint",
        "noop",
        "pull_request",
        "push",
        "source-fence",
        "tag_reuse",
        "test",
        "test-archive",
        "workflow_dispatch",
    ),
    "scripts/clean_merged_artifacts.py": (
        "]",
    ),
    "scripts/cargo-shim": (
        "]",
        "ci",
    ),
    "scripts/cargo_command_analysis.py": (
        "build",
        "clippy",
        "deny",
        "test",
        "test-archive",
    ),
    "scripts/ci_workflow_hygiene_test_helpers.py": (
        ".github/workflows/ci.yml",
        "AGENTS.md",
        "[local_compile_policy]",
        "[local_lane_policy]",
        "[remote_verification]",
        'acquire_timeout_seconds = 1800',
        'allowed_ci_env = "GITHUB_ACTIONS"',
        'break_glass_env = "BOLT_ALLOW_LOCAL_RUST"',
        "checks_appear_timeout_seconds = 300",
        "ci",
        "diagnostic_log_max_bytes = 20000",
        "diagnostic_log_max_lines = 160",
        "diagnostic_unavailable_notice_interval_polls = 4",
        "enabled = true",
        "heartbeat_seconds = 15",
        'lock_dir = "/tmp/rust-verification-lanes"',
        "main",
        "overall_timeout_seconds = 3600",
        "poll_interval_seconds = 1",
        "poll_interval_seconds = 15",
        'project_id = "backtesting-vertical-slice"',
        'project_id = "bolt-v2"',
        'refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]',
        'refused_managed_commands = ["test", "clippy", "build"]',
        "schema_version = 2",
        'target_namespace = "backtesting-vertical-slice"',
        'target_namespace = "bolt-v2"',
    ),
    "scripts/governance_diff_analysis.py": (
        "AGENTS.md",
        "]",
    ),
    "scripts/merge_queue_preflight.py": (
        "]",
        "ci",
    ),
    "scripts/workflow_expression_analysis.py": (
        "build",
        "check-aarch64",
        "clippy",
        "deny",
        "nextest-fingerprint",
        "source-fence",
        "test",
        "test-archive",
    ),
    "scripts/merge_queue_operator.py": (
        "ci",
    ),
    "scripts/minimal_toml.py": (
        "]",
    ),
    "scripts/run_ci_lint_suites.py": (
        "coverage-enforcer",
        "nextest-fingerprint",
    ),
    "scripts/rust_verification.py": (
        "]",
        "build",
        "clippy",
        "pull_request",
        "test",
    ),
    "scripts/sandbox_safe_push.py": (
        "push",
    ),
    "scripts/test_cargo_shim.py": (
        '"test",',
        "[clean-merged.backups]",
        "[clean-merged.daily_maintenance_launch_agent]",
        "[clean-merged.lane_r]",
        "[clean-merged.lane_w]",
        "[clean-merged.logging]",
        "[clean-merged]",
        "[local_compile_policy]",
        "[remote_verification]",
        "]",
        'allowed_ci_env = "GITHUB_ACTIONS"',
        "archive_timeout_s = 120",
        "archive_verify_timeout_s = 30",
        'audit_format = "jsonl"',
        'audit_path = "<git-common-dir>/clean-merged.log"',
        'break_glass_env = "BOLT_ALLOW_LOCAL_RUST"',
        "build",
        "cache_ttl_s = 300",
        "ci",
        "discard_hidden_index_bits = false",
        "discard_ignored = false",
        "enabled = false",
        "enabled = true",
        "gh_limit = 100",
        "gh_timeout_s = 5",
        'heartbeat_path = "<git-common-dir>/clean-merged.heartbeat"',
        "heartbeat_stale_days = 7",
        'lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"',
        "main",
        "max_log_bytes = 1048576",
        "poll_interval_seconds = 15",
        'project_id = "bolt-v2"',
        "prune_after_days = 30",
        'quarantine_dir = "<git-common-dir>/clean-merged-quarantine"',
        "quarantine_grace_days = 30",
        'remote_name = "origin"',
        "remove_nested_repos = false",
        "report_error_max_chars = 200",
        "rotated_log_retention_days = 30",
        "schema_version = 1",
        "schema_version = 2",
        'target_namespace = "bolt-v2"',
        "test",
        'trunk_branch = "main"',
    ),
    "scripts/test_cargo_command_analysis.py": (
        "ci",
        "test",
    ),
    "scripts/test_governance_diff_analysis.py": (
        ".github/workflows/ci.yml",
        "AGENTS.md",
        "]",
    ),
    "scripts/test_workflow_expression_analysis.py": (
        ".github/workflows/ci.yml",
        "actionlint.yml",
        "merge_group",
        'merge_group = "full"',
        "nextest-fingerprint",
        "test-archive",
    ),
    "scripts/test_clean_merged_artifacts.py": (
        "[clean-merged.backups]",
        "[clean-merged.daily_maintenance_launch_agent]",
        "[clean-merged.lane_r]",
        "[clean-merged.lane_t]",
        "[clean-merged.lane_w]",
        "[clean-merged.logging]",
        "[clean-merged]",
        "]",
        "archive_timeout_s = 120",
        "archive_verify_timeout_s = 30",
        'audit_format = "jsonl"',
        'audit_path = "<git-common-dir>/clean-merged.log"',
        "cache_ttl_s = 300",
        "discard_hidden_index_bits = false",
        "discard_ignored = false",
        "docs",
        "enabled = true",
        "gh_limit = 100",
        "gh_timeout_s = 5",
        'heartbeat_path = "<git-common-dir>/clean-merged.heartbeat"',
        "heartbeat_stale_days = 7",
        'lane_r_log_path = "<git-common-dir>/clean-merged.lane-r.log"',
        "main",
        "max_log_bytes = 1048576",
        "prune_after_days = 30",
        "push",
        'quarantine_dir = "<git-common-dir>/clean-merged-quarantine"',
        "quarantine_grace_days = 30",
        'remote_name = "origin"',
        "remove_nested_repos = false",
        "report_error_max_chars = 200",
        "rotated_log_retention_days = 30",
        "schema_version = 1",
        'target_dir_name = "target"',
        'trunk_branch = "main"',
    ),
    "scripts/test_host_health_sampler.py": (
        "[persistence]",
    ),
    "scripts/test_merge_queue_operator.py": (
        "[merge_queue_preflight.operator]",
        "[merge_queue_preflight.timeouts]",
        "[merge_queue_preflight.verifier_profiles.local]",
        "[merge_queue_preflight.verifier_profiles.static]",
        "[merge_queue_preflight]",
        'base = "main"',
        'commands = ["just fmt-check", "just source-fence-static", "just ci-lint-workflow"]',
        'commands = ["just source-fence-static"]',
        'default_verifier_profile = "static"',
        'origin = "origin"',
        'queue_command = "@mergifyio queue"',
    ),
    "scripts/test_merge_queue_preflight.py": (
        '"actionlint" = "actionlint"',
        '"backtester-gate" = "Backtester CI"',
        '"gate" = "CI"',
        '"host-health" = "CI"',
        '"just source-fence-static" = "just source-fence-static-fences-only"',
        "CI",
        "[fail_closed_contracts]",
        "[fail_closed_exceptions]",
        "[local_lane_policy]",
        "[merge_queue_preflight.output]",
        "[merge_queue_preflight.required_check_workflows]",
        "[merge_queue_preflight.source_check_aliases]",
        "[merge_queue_preflight.source_fence_fences_only_rewrites]",
        "[merge_queue_preflight.timeouts]",
        "[merge_queue_preflight]",
        "]",
        "actionlint",
        "backtester-gate",
        "backtester-gate-iteration",
        'base = "main"',
        "ci",
        "commands = []",
        "gate",
        "gate-iteration",
        "host-health",
        "main",
        'origin = "origin"',
        "push",
        "source_fence_full_profile_pathspecs = [",
    ),
    "scripts/test_run_ci_lint_suites.py": (
        "noop",
    ),
    "scripts/test_rust_verification.py": (
        '"backtester-gate" = "Backtester CI"',
        '"ci/rust-verification.toml",',
        '"host-health" = "CI"',
        '"justfile",',
        '"scripts",',
        "AGENTS.md",
        "CI",
        "CI [dispatch:iteration]",
        "[ci_provenance.dispatch]",
        "[ci_provenance.gate_names]",
        "[ci_provenance]",
        "[merge_queue_preflight.required_check_workflows]",
        "[merge_queue_preflight]",
        "[remote_fast_linker]",
        "]",
        "backtester-gate",
        "backtester-gate-iteration",
        "build",
        "ci",
        'ci_env = "GITHUB_ACTIONS"',
        "docs",
        "enabled = true",
        "gate",
        "gate-iteration",
        'gate_required = "gate"',
        "host-health",
        'linker_env = "BOLT_RUST_FAST_LINKER"',
        "poll_interval_seconds = 1",
        'programs = ["mold", "lld"]',
        'proof_gate_job = "gate"',
        "pull_request",
        'run_name_iteration = "CI [dispatch:iteration]"',
        "schema_version = 1",
        "source_fence_full_profile_pathspecs = [",
        "workflow_dispatch",
        'workflow_name = "CI"',
        'workflow_path = ".github/workflows/ci.yml"',
    ),
    "scripts/test_sandbox_safe_push.py": (
        "[sandbox_safe_push]",
        "ci",
        'project_id = "bolt-v2"',
        "push",
        "schema_version = 2",
        'target_namespace = "bolt-v2"',
    ),
    "scripts/test_verify_bolt_v3_dependency_direction.py": (
        "main",
        "push",
        "source-fence",
    ),
    "scripts/test_verify_ci_workflow_hygiene.py": (
        '".github/actions/setup-environment/**",',
        '".github/workflows/backtester-ci.yml",',
        '".gitignore",',
        '"backtester_ci",',
        '"ci/rust-ci-inputs.toml",',
        '"gated_source_roots.manifest",',
        '"scripts/ci_input_sets.py",',
        '"scripts/rust_test_targets.py",',
        '"specs/023-nt-research-analytics-platform/reference/**",',
        "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}",
        ".github/workflows/ci.yml",
        "[artifact_retention.classes.reuse-bound]",
        "[artifact_retention.classes.transient]",
        "[artifact_retention.lookback_bindings.build_deploy]",
        '[artifact_retention.uploads.".github/workflows/ci.yml::build::upload-bolt-v2-binary"]',
        "[ci_provenance.dispatch]",
        "[ci_provenance.full_ci.jobs.test]",
        "[ci_provenance.mergify]",
        "[meter.api_limits]",
        "[sets.backtester_detect]",
        "[storage_audit.cleanup_feasibility_alert]",
        "]",
        'artifact_class = "capture-provenance"',
        'artifact_class = "deployable"',
        'artifact_class = "reuse-bound"',
        "artifact_lookback_age_seconds = 259200",
        'artifact_name_config_file = "ci/github-actions-runners.toml"',
        'artifact_name_config_ref = "ci_provenance.deploy.artifact_name"',
        'artifact_name_template = "ci-provenance-attempt-{run_attempt}"',
        'artifact_name_template_config_file = "ci/github-actions-runners.toml"',
        'artifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
        'artifact_name_template_vars_config_ref = "backtester.issue_789.artifact_name_template_vars"',
        'artifact_name_template_vars_config_ref = "ci_provenance.artifact_name_template_vars"',
        "artifact_retention_days = 3",
        'artifact_upload_if = "${{ github.event_name == \'push\' && github.ref == \'refs/heads/main\' }}"',
        "backtester-gate",
        "backtester-gate-iteration",
        'backtester_iteration = "backtester-gate-iteration"',
        "branch_pull_requests_per_page = 20",
        "build",
        "check-aarch64",
        'check_name = "test"',
        "ci",
        "clippy",
        'config_file = "ci/github-actions-runners.toml"',
        'converted_to_draft = "iteration"',
        "defer",
        "deny",
        "detector",
        "docs",
        'draft_pr_synchronize = "iteration"',
        "draft_timeline_items = 100",
        'fingerprint_artifact_prefix = "nextest-archive-fingerprint-"',
        'fingerprint_source = "meter"',
        "force_full_ci = false",
        "full",
        "gate",
        "gate-iteration",
        'gate_iteration = "gate-iteration"',
        'gate_required = "gate"',
        "ignore_emit_failure = false",
        "iteration",
        'lookback_ref = "ci_provenance.deploy.artifact_lookback_age_seconds"',
        "main",
        'main_push = "full"',
        "max_lookback_age_seconds = 1209600",
        "max_retention_days = 14",
        "max_retention_days = 7",
        "merge_group",
        'merge_group = "full"',
        'mergify_temp_pr = "full"',
        "nextest-fingerprint",
        'nextest-fingerprint = "managed_heavy"',
        "noop",
        'proof_gate_job = "gate"',
        "pull_request",
        "push",
        'ready_for_review = "full"',
        'ready_pr = "full"',
        'ready_pr_edited_no_base = "noop"',
        'ready_pr_reopened = "noop"',
        'required_if_config_file = "ci/github-actions-runners.toml"',
        'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
        "retention_days = 14",
        'retention_days_config_file = "ci/chainlink-reference-fixture-capture-provenance.toml"',
        'retention_days_config_ref = "ci_provenance.artifacts.retention_days"',
        'retention_ref = "ci_provenance.deploy.artifact_retention_days"',
        "run_artifacts_per_page = 100",
        'run_attempt = "${{ github.run_attempt }}"',
        "run_jobs_per_page = 100",
        "schema_version = 1",
        "schema_version = 2",
        "source-fence",
        "tag_reuse",
        "test",
        "test-archive",
        'unknown_event = "full"',
        'upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"',
        "workflow_dispatch",
        'workflow_dispatch = "iteration"',
        'workflow_dispatch_full_ci = "full"',
        'workflow_name = "CI"',
        "workflow_runs_per_page = 100",
    ),
    "scripts/test_verify_no_config_retype.py": (
        "[ci_provenance]",
        "ci",
        "gate",
        "schema_version = 1",
    ),
    "scripts/test_verify_probability_typed_pilot.py": (
        "]",
    ),
    "scripts/verifier_io.py": (
        "main",
    ),
    "scripts/verify_ai_review_governance.py": (
        "AGENTS.md",
        "[[pr_agent_mirror.rules]]",
        "[pr_agent_mirror]",
        "]",
        'deliverable_bot_logins = ["claude[bot]"]',
        'deliverable_marker = "<!-- ai-pr-reviewer-claude -->"',
        'name = "scope discipline"',
        "track_progress = false",
    ),
    "scripts/verify_ci_workflow_hygiene.py": (
        ".claude/rust-verification.toml",
        ".github/workflows/ci.yml",
        "]",
        "actionlint",
        "actionlint.yml",
        "backtester-gate",
        "build",
        "capture",
        "check-aarch64",
        "ci",
        "clippy",
        "coverage-enforcer",
        "deny",
        "detector",
        "detector.build_required",
        "docs",
        "full",
        "gate",
        "iteration",
        "main",
        "merge_group",
        "mergify/merge-queue/",
        "meter",
        "nextest archive",
        "nextest-fingerprint",
        "pull_request",
        "push",
        "source-fence",
        "tag_reuse",
        "test",
        "test-archive",
        "workflow_dispatch",
    ),
    "scripts/verify_no_config_retype.py": (
        "]",
    ),
}

REGISTERED_RETYPE_PAYLOADS: tuple[RegisteredRetype, ...] = tuple(
    RegisteredRetype(path=path, value=value, reason=STRICT_BOOTSTRAP_REASON)
    for path, values in STRICT_BOOTSTRAP_RETYPE_VALUES_BY_PATH.items()
    for value in values
)

REGISTERED_PAYLOAD_ASSIGNMENTS = frozenset(
    {
        "REGISTERED_RETYPE_PAYLOADS",
        "STRICT_BOOTSTRAP_RETYPE_VALUES_BY_PATH",
    }
)


def rel_path(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def non_comment_config_lines(path: Path, source_rel: str) -> Iterable[ProtectedString]:
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            yield ProtectedString(stripped, f"{source_rel}:non-comment-line")


def flatten_string_values(value: object, field_path: str) -> Iterable[str]:
    if isinstance(value, str):
        yield value
        return
    if isinstance(value, dict):
        for key, child in value.items():
            if not isinstance(key, str):
                raise TypeError(f"{field_path}: unhandled non-string key type {type(key).__name__}")
            yield from flatten_string_values(child, f"{field_path}.{key}")
        return
    if isinstance(value, list):
        for index, child in enumerate(value):
            yield from flatten_string_values(child, f"{field_path}[{index}]")
        return
    # TOML numeric/bool values are not no-config string retype risks.
    if isinstance(value, (bool, int, float)):
        return
    raise TypeError(f"{field_path}: unhandled container type {type(value).__name__}")


def ci_provenance_strings(path: Path, source_rel: str) -> Iterable[ProtectedString]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    table = data.get("ci_provenance")
    if table is None:
        return
    for value in flatten_string_values(table, "ci_provenance"):
        stripped = value.strip()
        if stripped:
            yield ProtectedString(stripped, f"{source_rel}:ci_provenance-string")


def discover_governed_config_artifacts(root: Path) -> tuple[str, ...]:
    discovered: set[str] = set()
    for pattern in GOVERNED_CONFIG_ARTIFACT_PATTERNS:
        for path in root.glob(pattern):
            if path.is_file():
                discovered.add(rel_path(path, root))
    return tuple(sorted(discovered))


def protected_strings(root: Path, governed_config_artifacts: tuple[str, ...]) -> tuple[ProtectedString, ...]:
    protected: list[ProtectedString] = []
    declared = frozenset(governed_config_artifacts)
    for rel in governed_config_artifacts:
        path = root / rel
        if not path.is_file():
            raise FileNotFoundError(f"governed config artifact missing: {rel}")
    unlisted = sorted(set(discover_governed_config_artifacts(root)) - declared)
    if unlisted:
        raise ValueError(f"governed config artifact unlisted: {unlisted[0]}")
    for rel in governed_config_artifacts:
        path = root / rel
        protected.extend(non_comment_config_lines(path, rel))
        protected.extend(ci_provenance_strings(path, rel))
    return tuple(protected)


def string_literal_candidates(value: str) -> tuple[str, ...]:
    candidates = {value.strip()}
    candidates.update(line.strip() for line in value.splitlines() if line.strip())
    candidates.discard("")
    return tuple(sorted(candidates))


def assignment_names(node: ast.AST) -> frozenset[str]:
    targets: list[ast.expr] = []
    if isinstance(node, ast.Assign):
        targets.extend(node.targets)
    elif isinstance(node, ast.AnnAssign):
        targets.append(node.target)
    else:
        return frozenset()
    names: set[str] = set()
    for target in targets:
        if isinstance(target, ast.Name):
            names.add(target.id)
    return frozenset(names)


def registered_payload_literal_ranges(tree: ast.AST) -> tuple[tuple[int, int], ...]:
    ranges: list[tuple[int, int]] = []
    for node in ast.walk(tree):
        if assignment_names(node) & REGISTERED_PAYLOAD_ASSIGNMENTS:
            start = getattr(node, "lineno", 1)
            end = getattr(node, "end_lineno", start)
            ranges.append((start, end))
    return tuple(ranges)


def line_in_ranges(line: int, ranges: tuple[tuple[int, int], ...]) -> bool:
    return any(start <= line <= end for start, end in ranges)


def script_source_paths(root: Path) -> tuple[Path, ...]:
    scripts = root / "scripts"
    return tuple(
        sorted(
            path
            for path in scripts.rglob("*")
            if path.is_file() and (path.suffix == ".py" or path.suffix == "")
        )
    )


def script_literals(root: Path, *, strict_paths: frozenset[str] = STRICT_RETYPE_PATHS) -> Iterable[LiteralHit]:
    for path in script_source_paths(root):
        rel = rel_path(path, root)
        source = path.read_text(encoding="utf-8")
        try:
            tree = ast.parse(source, filename=rel)
        except SyntaxError:
            if path.suffix == "" and rel not in strict_paths:
                continue
            raise
        skipped_ranges = (
            registered_payload_literal_ranges(tree)
            if rel == "scripts/verify_no_config_retype.py"
            else ()
        )
        for node in ast.walk(tree):
            if isinstance(node, ast.Constant) and isinstance(node.value, str):
                line = getattr(node, "lineno", 1)
                if line_in_ranges(line, skipped_ranges):
                    continue
                for candidate in string_literal_candidates(node.value):
                    yield LiteralHit(rel, line, candidate)


def registration_index(registered_retypes: tuple[RegisteredRetype, ...]) -> dict[tuple[str, str], str]:
    index: dict[tuple[str, str], str] = {}
    for registration in registered_retypes:
        if not registration.path or not registration.value or not registration.reason.strip():
            raise ValueError("registered no-config retype payloads require path, value, and reason")
        if any(marker in registration.path for marker in "*?["):
            raise ValueError(f"wildcard registered no-config retype path is not allowed: {registration.path}")
        key = (registration.path, registration.value)
        if key in index:
            raise ValueError(f"duplicate registered no-config retype payload: {registration.path}:{registration.value!r}")
        index[key] = registration.reason
    return index


def is_registered(hit: LiteralHit, index: dict[tuple[str, str], str]) -> bool:
    return (hit.path, hit.value) in index


def collect_violations(
    root: Path,
    *,
    governed_config_artifacts: tuple[str, ...] = GOVERNED_CONFIG_ARTIFACTS,
    registered_retypes: tuple[RegisteredRetype, ...] = REGISTERED_RETYPE_PAYLOADS,
    strict_paths: frozenset[str] = STRICT_RETYPE_PATHS,
) -> tuple[tuple[LiteralHit, ProtectedString], ...]:
    protected = protected_strings(root, governed_config_artifacts)
    protected_by_value: dict[str, ProtectedString] = {}
    for item in protected:
        protected_by_value.setdefault(item.value, item)
    registered = registration_index(registered_retypes)
    violations: list[tuple[LiteralHit, ProtectedString]] = []
    for hit in script_literals(root, strict_paths=strict_paths):
        if is_registered(hit, registered):
            continue
        protected_source = protected_by_value.get(hit.value)
        if protected_source is None:
            continue
        violations.append((hit, protected_source))
    return tuple(violations)


def collect_findings(
    root: Path = REPO_ROOT,
    *,
    governed_config_artifacts: tuple[str, ...] = GOVERNED_CONFIG_ARTIFACTS,
    strict_paths: frozenset[str] = STRICT_RETYPE_PATHS,
    ratchet_baseline_count: int = RATCHET_BASELINE_COUNT,
    registered_retypes: tuple[RegisteredRetype, ...] = REGISTERED_RETYPE_PAYLOADS,
) -> list[str]:
    try:
        violations = collect_violations(
            root,
            governed_config_artifacts=governed_config_artifacts,
            registered_retypes=registered_retypes,
            strict_paths=strict_paths,
        )
    except FileNotFoundError as exc:
        return [str(exc)]
    except (OSError, SyntaxError, tomllib.TOMLDecodeError, TypeError, ValueError) as exc:
        return [f"no-config-retype verifier failed: {exc}"]

    strict_violations = tuple(item for item in violations if item[0].path in strict_paths)
    ratchet_violations = tuple(item for item in violations if item[0].path not in strict_paths)
    findings: list[str] = []
    for hit, protected in strict_violations:
        findings.append(
            f"{hit.path}:{hit.line}: strict no-config-retype violation: {hit.value!r} "
            f"retypes {protected.source}"
        )
    if len(ratchet_violations) > ratchet_baseline_count:
        findings.append(
            "ratchet no-config-retype count increased: "
            f"{len(ratchet_violations)} > {ratchet_baseline_count}"
        )
        for hit, protected in ratchet_violations[:20]:
            findings.append(f"  {hit.path}:{hit.line}: {hit.value!r} retypes {protected.source}")
    return findings


def main() -> int:
    findings = collect_findings(REPO_ROOT)
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1
    print("OK: no governed config strings are retyped outside registered payloads.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
