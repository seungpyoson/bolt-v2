#!/usr/bin/env python3
"""Verify CI workflow hygiene invariants for the current workflow topology."""

from __future__ import annotations

import argparse
import ast
from collections.abc import Callable, Iterable, Mapping
import difflib
import hashlib
import json
import pathlib
import re
import shlex
import subprocess
import sys
import tomllib
from typing import Any, NamedTuple, cast

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from workflow_expression_analysis import (
    ELSE_RE,
    FI_RE,
    GATE_NAME_OUTPUT,
    IF_OR_ELIF_RE,
    KNOWN_SAFE_CANCEL_FORMS,
    SAFE_CANCEL_EVENT_RE,
    TAG_SKIPPED_JOBS,
    YAML_ANCHOR_PATTERN,
    YAML_KEY_PATTERN,
    _cancel_in_progress_value,
    _normalize_concurrency_text,
    cancel_in_progress_is_merge_group_safe,
    collect_if_chain_bodies,
    gate_checks_nextest_fingerprint_reuse,
    gate_checks_same_sha_reuse,
    gate_policy_truth_table_errors,
    if_chain_bodies,
    one_indexed_sequence,
    simple_bte_run_block_partition_denominators,
    simple_shell_lines,
    strip_comment,
    unquote_yaml_scalar,
)
from cargo_command_analysis import (
    CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT,
    CARGO_PROCESS_SUBCOMMANDS,
    CI_INSTALL_ACTION_COMMANDS,
    CI_SOURCE_BUILD_TOOLS,
    ENV_OPTIONS_WITHOUT_ARGUMENT,
    ENV_OPTIONS_WITH_ARGUMENT,
    ENV_SIGNAL_OPTIONS,
    FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS,
    FLOCK_OPTIONS_WITHOUT_ARGUMENT,
    FLOCK_OPTIONS_WITH_ARGUMENT,
    RECURSIVE_WRAPPER_EXECUTABLES,
    SHELL_ASSIGNMENT_RE,
    SHELL_COMMAND_BOUNDARIES,
    SHELL_PUNCTUATION_CHARS,
    SHELL_PUNCTUATION_OPERATORS,
    SHELL_PUNCTUATION_OPERATORS_BY_LENGTH,
    SHELL_REDIRECTION_OPERATORS,
    SUDO_OPTIONS_WITHOUT_ARGUMENT,
    SUDO_OPTIONS_WITH_ARGUMENT,
    SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
    SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
    SU_SG_OPTIONS_WITHOUT_ARGUMENT,
    SU_SG_OPTIONS_WITH_ARGUMENT,
    TIME_OPTIONS_WITHOUT_ARGUMENT,
    TIME_OPTIONS_WITH_ARGUMENT,
    _command_tokens_cached,
    backtick_command_payloads,
    cargo_config_has_storage_override,
    cargo_config_looks_like_path,
    cargo_config_storage_override_message,
    cargo_install_source_build_tools,
    cargo_install_source_build_tools_from_tokens,
    cargo_install_source_build_tools_in_text,
    cargo_target_routing_scan_tokens,
    cargo_token_is_command,
    chroot_inner_tokens,
    cd_source_tool,
    command_has_raw_cargo,
    command_prefix_allows_cargo,
    command_tokens,
    command_tokens_with_line_boundaries,
    consume_assignment_words,
    consume_cargo_global_options,
    consume_option_prefix,
    consume_rust_verification_repo_option,
    container_inner_tokens,
    container_rust_payload_from_tokens,
    decode_toml_unicode_escapes,
    direct_raw_cargo_storage_override_messages,
    env_assignment_argument,
    env_command_prefix_index,
    env_inner_tokens,
    env_short_cluster_next_index,
    env_short_split_tokens,
    exec_inner_tokens,
    executable_name,
    expand_cargo_aliases,
    expand_known_shell_assignment_name,
    expand_known_shell_assignment_names,
    expand_known_shell_assignment_value,
    expand_known_shell_command_variables,
    expand_known_shell_variables,
    export_assignment_values_from_tokens,
    find_exec_payloads,
    flock_command_option_tokens,
    flock_inner_tokens,
    inline_command_substitution_payloads,
    managed_rust_verification_cargo_args,
    managed_rust_verification_command_tokens,
    managed_rust_verification_tokens,
    merge_split_shell_parameter_assignment_tokens,
    nice_command_index,
    no_mistakes_inner_tokens,
    path_executable_looks_like_cargo,
    path_executable_looks_like_rustc,
    path_invocation_has_cargo_subcommand,
    path_invocation_may_have_cargo_subcommand,
    path_name_looks_like_renamed_cargo,
    path_name_looks_like_renamed_rustc,
    normalized_source_path,
    persistent_shell_assignment_values,
    python_rust_verification_script_index,
    raw_cargo_storage_override_messages_from_tokens,
    raw_rust_tool_token,
    rust_tool_name_has_script_extension,
    rustup_run_inner_tokens,
    shell_alias_payloads,
    shell_array_assignment_values_from_tokens,
    shell_assignment_from_tokens,
    shell_assignment_name,
    shell_assignment_values_from_tokens,
    shell_assignment_word,
    shell_command,
    shell_command_substitution_payloads,
    shell_command_substitution_at,
    shell_quotes_are_balanced,
    shell_declaration_assignment_values_from_tokens,
    shell_identifier_fragment,
    shell_logical_lines,
    shell_name_word,
    shell_redirection_next_index,
    shell_variable_reference_token,
    short_cluster_consumes_option_argument,
    simple_cargo_aliases,
    source_build_clone_path_tools,
    source_build_tool_for_path,
    source_build_tool_from_token,
    source_build_tools_from_depth_exceeded_tokens,
    split_shell_punctuation_tokens,
    storage_strip_quotes,
    strip_shell_redirections,
    su_sg_command_option_tokens,
    target_routing_cargo_args,
    text_has_path_style_cargo_config,
    tokens_have_raw_cargo,
    tokens_have_raw_cargo_launch,
    tokens_have_top_level_shell_boundary,
    tokens_have_target_routing_override,
    wrapper_inner_tokens,
)
from shell_dataflow_analysis import (
    ACTIVE_TARGET_STDOUT_COMMANDS,
    AWS_S3_OPTIONS_WITH_ARGUMENT,
    AWS_S3_TRANSFER_COMMANDS,
    RUSTFLAGS_OUTPUT_OVERRIDE_KEYS,
    S3_ACTIVE_TARGET_CACHE_MESSAGE,
    STORAGE_ROLE_ACTIVE_TARGET,
    STORAGE_ROLE_S3,
    TAR_SHORT_OPTIONS_WITH_ARGUMENT,
    TAR_SHORT_OPTION_CLUSTER_FLAGS,
    aws_s3_transfer_touches_active_target,
    aws_s3_transfer_streams_s3_to_stdout,
    aws_service_index,
    aws_s3_operands,
    cd_option_token,
    command_copies_s3_path_to_active_target,
    command_operand_roles,
    command_output_redirects_to_active_target,
    command_prefix_before_token,
    command_streams_active_target_to_stdout,
    command_tail_until_boundary,
    command_writes_s3_stdin_to_active_target,
    consume_storage_option,
    directory_wrapper_chdir_value,
    dynamic_env_assignment_message,
    dynamic_env_segment_messages,
    dynamic_env_target_override_messages,
    dynamic_env_tokens_messages,
    env_chdir_value,
    github_env_assignments_from_cat_heredocs,
    github_env_assignments_from_echo_tokens,
    github_env_assignments_from_printf_tokens,
    github_env_assignment_from_echo_tokens,
    github_env_assignment_from_printf_tokens,
    github_env_assignment_line,
    github_env_assignment_lines,
    github_env_assignments_from_line,
    github_env_cat_heredoc_spec,
    github_env_line_assignments_around_cat_heredoc,
    github_env_assignments_from_logical_text,
    github_env_payload_assignments,
    local_transfer_operands,
    operand_has_s3_path_role,
    output_redirection_targets,
    printf_rendered_payload,
    record_aws_s3_download_paths,
    record_local_transfer_paths,
    record_tar_archive_paths,
    record_zip_archive_paths,
    rustflags_value_has_output_override,
    shell_assignment_alias_value,
    shell_assignment_tracking_value,
    shell_command_segments_from_tokens,
    shell_directory_change_target,
    shell_group_end_index,
    shell_heredoc_quoted_delimiters,
    skip_shell_redirections,
    storage_assignment_values,
    storage_command_substitution_has_target,
    storage_path_is_inside_active_path,
    storage_path_key,
    storage_stdout_roles_from_tokens,
    storage_transfer_policy_errors,
    storage_transfer_policy_errors_from_tokens,
    storage_value_has_target_component,
    storage_value_roles,
    storage_value_without_substitutions,
    storage_variable_names,
    storage_variable_roles,
    storage_without_trailing_current_dir,
    sudo_chdir_value,
    tar_archive_creation,
    tar_archive_inputs,
    tar_cluster_looks_like_options,
    tar_extracts_s3_archive_to_active_target,
    tar_extracts_to_active_target,
    tar_option_parts,
    tar_writes_archive_to_stdout,
    target_env_key_alias,
    target_env_key_from_assignment_name,
    unzip_extracts_s3_archive_to_active_target,
    zip_archive_operands,
)
from governance_diff_analysis import (
    SELF_AUTHORIZING_ALLOWLIST_ENTRY_PATHS,
    SELF_AUTHORIZING_CAPABILITY_PATHS,
    SELF_AUTHORIZING_GITHUB_AUTOMATION_PREFIXES,
    SELF_AUTHORIZING_GOVERNANCE_PATHS,
    SELF_AUTHORIZING_SECRETS_INHERIT_RE,
    SELF_AUTHORIZING_SECRET_REF_RE,
    SelfAuthorizingCapabilitySignal,
    SelfAuthorizingDiffError,
    dedupe_self_authorizing_signals,
    is_github_automation_path,
    non_comment_line,
    repo_git_bytes,
    repo_git_text_at_ref,
    self_authorizing_added_lines,
    self_authorizing_allowlist_signals,
    self_authorizing_capability_signals,
    self_authorizing_changed_paths,
    self_authorizing_governance_diff_errors,
    self_authorizing_new_active_secret_signals,
    self_authorizing_permission_grant_signals,
    self_authorizing_permission_signals,
    self_authorizing_secret_inherit_signals,
    self_authorizing_secret_ref_detail,
    self_authorizing_secret_ref_signals,
    yaml_flow_mapping_grants,
    yaml_permissions_block_exists,
    yaml_permissions_block_grants,
    yaml_permissions_block_scopes,
    yaml_permissions_grants,
    yaml_permissions_scoped_grants,
)
from merge_queue_preflight import (
    MERGIFY_DYNAMIC_BATCH_KEYS,
    MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS,
    MERGIFY_MERGE_QUEUE_KEYS,
    MERGIFY_PRIORITY_RULE_KEYS,
    MERGIFY_QUEUE_RULE_KEYS,
    MERGIFY_REQUIRED_MERGE_CONDITIONS,
    MERGIFY_REQUIRED_PRIORITY_RULES,
    MERGIFY_REQUIRED_QUEUE_RULES,
    MERGIFY_TOP_LEVEL_KEYS,
    MERGIFY_YAML_PARSER_RUBY,
    expect_scalar,
    mergify_condition_list,
    mergify_list,
    mergify_mapping,
    mergify_required_conditions,
    named_mergify_rules,
    parse_mergify_yaml,
    required_mergify_list,
    required_mergify_mapping,
    scalar_equals,
    unsupported_mapping_keys,
    verify_mergify_config,
    yaml_display,
)

from ci_provenance import (
    GATE_NAME_KEYS,
    MERGIFY_CONFIG_EXPECTATIONS,
    MERGIFY_TEMP_PR_TRANSIENT_PREFIX,
    POLICY_ROWS,
    POLICY_VALUES,
    REUSE_RELEVANT_WORKFLOW_ENV_KEYS,
    ProvenanceError,
    check_lookback_le_retention,
    gate_name_collision_errors,
    github_actions_output_safe_check_name,
    policy_contract_errors,
    evaluate_ci_policy as provenance_evaluate_ci_policy,
    docs_safe_path_contract_errors,
    ProvenanceConfig,
    load_config,
    mergify_temp_pr_matches,
    reuse_scoped_env_value_uses_single_line_scalar,
    top_level_block_lines,
    top_level_env_entry_key_value,
    top_level_env_immediate_entry_lines,
    workflow_line_starts_block_scalar,
    workflow_structural_mapping_value,
    workflow_structural_sequence_value,
    workflow_yaml_structural_line,
)

# Keep the former verifier-local helper families module-scoped so parity tests
# prove the old helper surface now points at the shared path.
from command_understanding import (
    CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT,
    cargo_args_for_target_routing_scan,
    cargo_subcommand,
    cargo_subcommand_with_index,
    nextest_subcommand_with_index,
    python_call_command_argument,
    python_call_name,
    python_command_string,
    python_constant_string,
    python_inline_command_payloads,
)
from ci_test_manifest import CiTestManifest, _mask_rust_non_code, build_test_manifest
from rust_verification import CARGO_ALIAS_SUBCOMMANDS, CARGO_DISK_PREFLIGHT_SUBCOMMANDS
import ci_storage_tripwire
import ci_input_sets
from verifier_io import require_nonempty


COMMAND_UNDERSTANDING_PARITY_EXPORTS = (
    cargo_subcommand_with_index,
    nextest_subcommand_with_index,
    python_call_command_argument,
    python_call_name,
    python_command_string,
    python_constant_string,
)


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKFLOW_DIR = REPO_ROOT / ".github" / "workflows"
DEFAULT_WORKFLOW = DEFAULT_WORKFLOW_DIR / "ci.yml"
DEFAULT_WORKFLOW_GLOBS = ("*.yml", "*.yaml")
DEFAULT_SETUP_ACTION = REPO_ROOT / ".github" / "actions" / "setup-environment" / "action.yml"
DEFAULT_NEXTEST_CONFIG = REPO_ROOT / ".config" / "nextest.toml"
DEFAULT_NO_MISTAKES_CONFIG = REPO_ROOT / ".no-mistakes.yaml"
DEFAULT_MERGIFY_CONFIG = REPO_ROOT / ".mergify.yml"
DEFAULT_RUNNERS_CONFIG = REPO_ROOT / "ci" / "github-actions-runners.toml"
DEFAULT_ACTIONLINT_CONFIG = REPO_ROOT / ".github" / "actionlint.yaml"
DEFAULT_RUST_VERIFICATION_POLICY = REPO_ROOT / "ci" / "rust-verification.toml"
DEFAULT_BVS_RUST_VERIFICATION_POLICY = REPO_ROOT / "crates" / "backtesting-vertical-slice" / "ci" / "rust-verification.toml"
RUNNERS_CONFIG_LABEL = "ci/github-actions-runners.toml"
JOB_RUNS_ON_VAR_RE = re.compile(r"^    runs-on:\s*\$\{\{\s*vars\.([A-Z0-9_]+)\s*\}\}\s*$")
CONFIG_TEMPLATE_PLACEHOLDER_RE = re.compile(r"\{([A-Za-z_][A-Za-z0-9_]*)\}")
ARTIFACT_RETENTION_WORKFLOW_SOURCE_RE = re.compile(r"\.github/workflows/[^/]+\.ya?ml")
ARTIFACT_RETENTION_ACTION_SOURCE_RE = re.compile(
    r"\.github/actions/[^/]+(?:/[^/]+)*/action\.ya?ml"
)
WORKFLOW_RUNNER_CONFIG_KEYS = {
    "ci.yml": "ci",
    ".github/workflows/ci.yml": "ci",
    "backtester-ci.yml": "backtester_ci",
    ".github/workflows/backtester-ci.yml": "backtester_ci",
    "flaky-test-detection.yml": "flaky_test_detection",
    ".github/workflows/flaky-test-detection.yml": "flaky_test_detection",
    "flaky-test-smoke.yml": "flaky_test_smoke",
    ".github/workflows/flaky-test-smoke.yml": "flaky_test_smoke",
    "dispatch-ci-cancel.yml": "dispatch_ci_cancel",
    ".github/workflows/dispatch-ci-cancel.yml": "dispatch_ci_cancel",
    "merge-readiness-finalizer.yml": "merge_readiness_finalizer",
    ".github/workflows/merge-readiness-finalizer.yml": "merge_readiness_finalizer",
    "coverage-enforcer.yml": "coverage_enforcer",
    ".github/workflows/coverage-enforcer.yml": "coverage_enforcer",
    "ci-storage-tripwire.yml": "ci_storage_tripwire",
    ".github/workflows/ci-storage-tripwire.yml": "ci_storage_tripwire",
    "ci-storage-cleanup-alert.yml": "ci_storage_cleanup_alert",
    ".github/workflows/ci-storage-cleanup-alert.yml": "ci_storage_cleanup_alert",
    "ci-runner-debug.yml": "ci_runner_debug",
    ".github/workflows/ci-runner-debug.yml": "ci_runner_debug",
    "debug-test.yml": "debug_test",
    ".github/workflows/debug-test.yml": "debug_test",
    "rust-probe.yml": "rust_probe",
    ".github/workflows/rust-probe.yml": "rust_probe",
    "actionlint.yml": "actionlint",
    ".github/workflows/actionlint.yml": "actionlint",
    "ai-review-glm-pr-agent.yml": "ai_review_glm_pr_agent",
    ".github/workflows/ai-review-glm-pr-agent.yml": "ai_review_glm_pr_agent",
    "ai-review-kimi-cli.yml": "ai_review_kimi_cli",
    ".github/workflows/ai-review-kimi-cli.yml": "ai_review_kimi_cli",
    "ai-review-coding-plan-smoke.yml": "ai_review_coding_plan_smoke",
    ".github/workflows/ai-review-coding-plan-smoke.yml": "ai_review_coding_plan_smoke",
    "ai-review-model-freshness.yml": "ai_review_model_freshness",
    ".github/workflows/ai-review-model-freshness.yml": "ai_review_model_freshness",
    "claude-code-review.yml": "claude_code_review",
    ".github/workflows/claude-code-review.yml": "claude_code_review",
    "advisory.yml": "advisory",
    ".github/workflows/advisory.yml": "advisory",
    "summary.yml": "summary",
    ".github/workflows/summary.yml": "summary",
    "stale.yml": "stale",
    ".github/workflows/stale.yml": "stale",
    "weekly-cleanup.yml": "weekly_cleanup",
    ".github/workflows/weekly-cleanup.yml": "weekly_cleanup",
    "performance-improver.yml": "performance_improver",
    ".github/workflows/performance-improver.yml": "performance_improver",
    "tech-debt-review.yml": "tech_debt_review",
    ".github/workflows/tech-debt-review.yml": "tech_debt_review",
}
STORAGE_TRIPWIRE_RUNNER_CONFIG_KEY = "ci_storage_tripwire"
SSH_RUNNER_ACTION_RE = re.compile(r"^ubicloud/ssh-runner@[0-9a-f]{40}$")
DEFAULT_REPO_AUTOMATION_FILES = (
    REPO_ROOT / "justfile",
    REPO_ROOT / "ci" / "rust-ci-inputs.toml",
)
DEFAULT_REPO_AUTOMATION_GLOBS = (
    (REPO_ROOT / "scripts", "*.sh"),
    (REPO_ROOT / "tests", "*.sh"),
    (REPO_ROOT / ".github" / "scripts", "*.sh"),
    (REPO_ROOT / ".github" / "actions", "**/action.yml"),
    (REPO_ROOT / ".github" / "actions", "**/action.yaml"),
)
JULES_ADVISORY_WORKFLOW_PATHS = frozenset(
    (
        ".github/workflows/weekly-cleanup.yml",
        ".github/workflows/performance-improver.yml",
        ".github/workflows/tech-debt-review.yml",
    )
)
JULES_ADVISORY_ENDPOINT_VARIABLE = "JULES_SESSIONS_ENDPOINT"
JULES_ADVISORY_TIMEOUT_VARIABLE = "JULES_SESSION_TIMEOUT_MINUTES"
JULES_ADVISORY_SECRET = "JULES_API_KEY"
JULES_AWS_COMMAND_RE = re.compile(r"(^|[\s;&|])aws([ \t\r\n;&|]|$)")
GITHUB_SECRET_REF_RE = re.compile(r"secrets\.([A-Z0-9_]+)")
LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS = {"build", "clippy", "test"}
LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS = set(CARGO_DISK_PREFLIGHT_SUBCOMMANDS) | set(CARGO_ALIAS_SUBCOMMANDS)
YAML_STEP_ITEM_RE = re.compile(rf"^-\s+(?:{YAML_ANCHOR_PATTERN}(?:\s+|$))?")
YAML_RUN_LINE_RE = re.compile(rf"^(\s*)(?:-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?)?run:\s*(.*?)\s*$")
YAML_FOLDED_RUN_LINE_RE = re.compile(
    rf"^(\s*)(?:-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?)?run:\s*>[+-]?\s*(?:#.*)?$"
)


class PolicyError(RuntimeError):
    pass


class CiPolicyResult(NamedTuple):
    ci_policy_path: str
    full_ci_required: bool
    full_ci_deferred: bool
    gate_name: str
    backtester_gate_name: str
    expected_event_class: str
    reason: str


class ArtifactRetentionClass(NamedTuple):
    max_retention_days: int


class ArtifactRetentionUploadSite(NamedTuple):
    artifact_name: str
    artifact_class: str
    retention_days: int
    retention_config_file: str | None
    retention_config_ref: str | None
    required_if: str | None


class ArtifactRetentionLookbackBinding(NamedTuple):
    upload: str
    config_file: str
    retention_ref: str
    lookback_ref: str


class ArtifactRetentionPolicy(NamedTuple):
    classes: dict[str, ArtifactRetentionClass]
    uploads: dict[str, ArtifactRetentionUploadSite]
    lookback_bindings: dict[str, ArtifactRetentionLookbackBinding]


class ArtifactRetentionResolvedInt(NamedTuple):
    value: int
    config_file: str | None
    config_ref: str | None


RUNNERS_CONFIG_REF = "ci/github-actions-runners.toml"
DEPLOYABLE_ARTIFACT_CLASS = "deployable"
DEPLOY_ARTIFACT_UPLOAD_KEY = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"
DEPLOY_ARTIFACT_NAME_REF = "ci_provenance.deploy.artifact_name"
DEPLOY_ARTIFACT_RETENTION_REF = "ci_provenance.deploy.artifact_retention_days"
DEPLOY_ARTIFACT_REQUIRED_IF_REF = "ci_provenance.deploy.artifact_upload_if"
DEPLOY_ARTIFACT_LOOKBACK_REF = "ci_provenance.deploy.artifact_lookback_age_seconds"


ArtifactRetentionSourceResolver = Callable[
    [dict[str, object], pathlib.Path, dict[str, object], str],
    object,
]


class ArtifactRetentionSourceMode(NamedTuple):
    name: str
    keys: tuple[str, ...]
    resolver: ArtifactRetentionSourceResolver


REQUIRED_JOBS = (
    "ci-policy",
    "detector",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "nextest-fingerprint-reuse",
    "test",
    "build",
    "ci-provenance-emit",
    "same-sha-main-evidence",
    "gate",
    "deploy",
)
GATE_REQUIRED = (
    "detector",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test",
    "build",
)
DEPLOY_REQUIRED_NEEDS = (
    "gate",
    "same-sha-main-evidence",
    "build",
    "detector",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test",
)
CI_PROVENANCE_REQUIRED_JOBS = (
    "detector",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "test",
)
CI_PROVENANCE_POLICY_VALUES = POLICY_VALUES
CI_PROVENANCE_POLICY_ROWS = POLICY_ROWS
CI_PROVENANCE_GATE_NAME_KEYS = GATE_NAME_KEYS


class PolicyRowSemantics(NamedTuple):
    changes_head_sha: bool = False
    changes_base: bool = False
    changes_target: bool = False
    changes_queue_origin: bool = False
    changes_required_context: bool = False
    mergeable_without_queue: bool = True
    queue_covered: bool = False


CI_POLICY_ROW_SEMANTICS = {
    "draft_pr_synchronize": PolicyRowSemantics(changes_head_sha=True, mergeable_without_queue=False),
    "draft_pr_opened": PolicyRowSemantics(changes_head_sha=True, changes_target=True, mergeable_without_queue=False),
    "draft_pr_reopened": PolicyRowSemantics(mergeable_without_queue=False),
    "draft_pr_edited": PolicyRowSemantics(changes_base=True, mergeable_without_queue=False),
    "converted_to_draft": PolicyRowSemantics(changes_required_context=True, mergeable_without_queue=False),
    "ready_pr": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_target=True),
    "ready_pr_edited_no_base": PolicyRowSemantics(),
    "ready_pr_reopened": PolicyRowSemantics(),
    "ready_for_review": PolicyRowSemantics(changes_required_context=True),
    "docs": PolicyRowSemantics(),
    "workflow_dispatch": PolicyRowSemantics(changes_required_context=True, mergeable_without_queue=False),
    "main_push": PolicyRowSemantics(changes_head_sha=True, changes_target=True),
    "merge_group": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_queue_origin=True),
    "mergify_temp_pr": PolicyRowSemantics(changes_head_sha=True, changes_queue_origin=True),
    "tag": PolicyRowSemantics(changes_target=True),
    "unknown_event": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_target=True),
}
PR_BASE_CHANGED_EXPR = "github.event.changes.base.ref.from && true || false"
READY_PR_NOOP_EXPR = (
    "github.event.pull_request.draft == false"
    " && (github.event.action == 'reopened'"
    " || (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false)))"
)
TAG_SKIP_REQUIRED_JOBS = (
    "deny",
    "clippy",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "nextest-fingerprint-reuse",
    "test",
    "ci-provenance-emit",
)
TARGET_DIR_JOBS = ("clippy", "check-aarch64", "source-fence", "test-archive", "build")
CACHE_KEY_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test-archive", "build")
JOB_REQUIRED_JUST_RECIPE = {
    "deny": "deny",
    "clippy": "clippy",
    "check-aarch64": "check-aarch64",
    "source-fence": "source-fence",
    "build": "build",
}
LIVE_NODE_TEST_GROUP = "live-node"
LIVE_NODE_UNIT_TEST_FILTERS = (
    "binary(=bolt_v2)",
    "test(~bolt_v3_client_registration::tests::)",
    "test(~bolt_v3_live_node::tests::)",
)
LIVE_NODE_NEXTEST_BINARIES = (
    "bolt_v3_adapter_mapping",
    "bolt_v3_client_registration",
    "bolt_v3_controlled_connect",
    "bolt_v3_credential_log_suppression",
    "bolt_v3_readiness",
    "bolt_v3_strategy_registration",
    "bolt_v3_submit_admission",
    "chainlink_startup_boot",
    "config_parsing",
    "lake_batch",
    "nt_runtime_capture",
    "venue_contract",
)
EXPECTED_HARNESS_COUNT = 13
DECLARED_TOP_LEVEL_TEST_HELPERS = {"bolt_v3_iv_support"}
RUST_TEST_ATTR_RE = re.compile(r"#\s*\[\s*(?:tokio::)?test(?:\s*\([^]]*\))?\s*\]")
RUST_INNER_ATTR_RE = re.compile(r"#!\s*\[\s*([A-Za-z_][A-Za-z0-9_]*)")
BANNED_RUST_INNER_ATTRS = {
    "feature",
    "no_std",
    "no_main",
    "crate_name",
    "crate_type",
    "crate_id",
}
NEXTEST_BINARY_FILTER_RE = re.compile(r"\bbinary\(=([A-Za-z0-9_-]+)\)")
# Equality tail for an audited binary target: binary(=name). Anything else after
# `binary(` (regex form binary(/.../), spaced forms, etc.) is unparseable by the
# guardrail and must fail closed rather than collapse to an empty binary set.
NEXTEST_BINARY_EQ_TAIL_RE = re.compile(r"=[A-Za-z0-9_-]+\)")
# Harness-scoped live-node test prefix: test(/^member::/).
NEXTEST_TEST_PREFIX_RE = re.compile(r"test\(/\^([A-Za-z0-9_]+)::/\)")
NEXTEST_SENSITIVE_OVERRIDE_KEYS = {
    "test-group",
    "retries",
    "slow-timeout",
    "leak-timeout",
    "timeout",
}
LIVE_NODE_NEXTEST_FILTER = " | ".join(f"binary(={binary})" for binary in LIVE_NODE_NEXTEST_BINARIES)
CHECK_AARCH64_JOB_LEVEL_IF_RE = re.compile(r"^    if:\s*.*$")
CHECK_AARCH64_STANDALONE_IF_RE = re.compile(
    r"^\s+(?:-\s*)?if:\s*(?:\$\{\{\s*)?needs\.detector\.outputs\.build_required\s*!=\s*['\"]true['\"]\s*(?:\}\})?\s*$"
)
TAG_SKIP_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
TAG_SKIP_ALWAYS_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*(?:"
    r"!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*always\(\)"
    r"|always\(\)\s*&&\s*!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)"
    r")\s*\}\}\s*$"
)
SAME_SHA_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
FULL_CI_REQUIRED_EXPR = "needs.ci-policy.outputs.full_ci_required == 'true'"
DOCS_POLICY_EXPR = "needs.ci-policy.outputs.ci_policy_path == 'docs'"
TAG_REUSE_POLICY_EXPR = "needs.ci-policy.outputs.ci_policy_path == 'tag_reuse'"
SOURCE_FENCE_JOB_IF_VALUE = f"${{{{ {FULL_CI_REQUIRED_EXPR} || {DOCS_POLICY_EXPR} }}}}"
SOURCE_FENCE_POLICY_SWITCH = """
if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
  just source-fence
else
  just source-fence-static
fi
"""
SOURCE_FENCE_CHECKOUT_REF = (
    "${{ needs.ci-policy.outputs.ci_policy_path == 'docs' && "
    "github.event.pull_request.head.sha || github.sha }}"
)
NEXTEST_REUSE_MISS_EXPR = "needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true'"
MAIN_BRANCH_SKIP_EXPR = "github.ref != 'refs/heads/main'"
BUILD_REQUIRED_EXPR = "needs.detector.outputs.build_required == 'true'"
FINGERPRINT_REUSE_ALLOWED_EXPR = "needs.detector.outputs.fingerprint_reuse_allowed == 'true'"
FINGERPRINT_REUSE_CONSUMER_EVENTS_EXPR = (
    "contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name)"
)
# A key may be listed here only if it must not influence compiler/test-runner
# behavior or archive content; when in doubt classify reuse-relevant
# (fails toward rebuild). Build-affecting keys must instead go into
# ci_provenance.REUSE_RELEVANT_WORKFLOW_ENV_KEYS so they invalidate reuse.
REUSE_NEUTRAL_TOP_LEVEL_ENV_KEYS = frozenset(
    {"CARGO_TERM_COLOR", "S3_DEPLOY_PATH"}
)
FINGERPRINT_REUSE_JOB_IF_VALUE = (
    "${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' "
    "&& contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name) "
    "&& needs.detector.outputs.fingerprint_reuse_allowed == 'true' "
    "&& github.ref != 'refs/heads/main' }}"
)
FINGERPRINT_REUSE_ALLOWED_OUTPUT = (
    "fingerprint_reuse_allowed: ${{ steps.fingerprint_reuse_allowed.outputs.value }}"
)
FINGERPRINT_REUSE_REASON_OUTPUT = (
    "fingerprint_reuse_reason: ${{ steps.fingerprint_reuse_allowed.outputs.reason }}"
)
DETECTOR_REFS_STEP_ALLOWED_KEYS = frozenset(("name", "id", "if", "shell", "env", "run"))
DETECTOR_REFS_STEP_SCALARS = {
    "id": "pr_refs",
    "if": "github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'",
    "shell": "bash",
    "env": "",
    "run": "|",
}
DETECTOR_REFS_STEP_ENV = {
    "EVENT_NAME": "${{ github.event_name }}",
    "PR_NUMBER": "${{ github.event.pull_request.number || github.run_id }}",
    "PR_BASE_REF": "${{ github.event.pull_request.base.ref || '' }}",
    "DISPATCH_BASE_REF": "${{ github.event.repository.default_branch }}",
    "MERGE_GROUP_BASE_REF": "${{ github.event.merge_group.base_ref || '' }}",
}
FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_ALLOWED_KEYS = frozenset(
    ("name", "id", "if", "shell", "run")
)
FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_SCALARS = {
    "id": "fingerprint_reuse_inputs_changed",
    "if": "github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'",
    "shell": "bash",
    "run": "|",
}
FINGERPRINT_REUSE_ALLOWED_STEP_ALLOWED_KEYS = frozenset(("name", "id", "shell", "run"))
FINGERPRINT_REUSE_ALLOWED_STEP_SCALARS = {
    "id": "fingerprint_reuse_allowed",
    "shell": "bash",
    "run": "|",
}
SELF_AUTHORIZING_GOVERNANCE_STEP_ALLOWED_KEYS = frozenset(
    ("name", "id", "if", "shell", "run")
)
SELF_AUTHORIZING_GOVERNANCE_STEP_SCALARS = {
    "id": "self_authorizing_governance",
    "if": "github.event_name == 'pull_request'",
    "shell": "bash",
    "run": "|",
}
NEXTEST_FINGERPRINT_REUSE_RESOLVER_STEP_ALLOWED_KEYS = frozenset(
    ("name", "id", "shell", "env", "run")
)
NEXTEST_FINGERPRINT_REUSE_RESOLVER_STEP_SCALARS = {
    "id": "reuse",
    "shell": "bash",
    "env": "",
    "run": "|",
}
NEXTEST_FINGERPRINT_REUSE_RESOLVER_ENV = {"GITHUB_TOKEN": "${{ github.token }}"}
NEXTEST_FINGERPRINT_REUSE_BASE_STEP_ALLOWED_KEYS = frozenset(
    ("name", "id", "if", "shell", "env", "run")
)
NEXTEST_FINGERPRINT_REUSE_BASE_STEP_SCALARS = {
    "id": "reuse_provenance_base",
    "if": "github.event_name == 'pull_request' || github.event_name == 'merge_group'",
    "shell": "bash",
    "env": "",
    "run": "|",
}
NEXTEST_FINGERPRINT_REUSE_BASE_ENV = {
    "EVENT_NAME": "${{ github.event_name }}",
    "PR_NUMBER": "${{ github.event.pull_request.number || github.run_id }}",
    "PR_BASE_REF": "${{ github.event.pull_request.base.ref || '' }}",
    "PR_BASE_SHA": "${{ github.event.pull_request.base.sha || '' }}",
    "MERGE_GROUP_BASE_REF": "${{ github.event.merge_group.base_ref || '' }}",
    "MERGE_GROUP_BASE_SHA": "${{ github.event.merge_group.base_sha || '' }}",
}
TRUSTED_BASE_STEP_ALLOWED_KEYS = frozenset(("name", "id", "if", "shell", "env", "run"))
CI_PROVENANCE_BASE_STEP_SCALARS = {
    "id": "provenance_base",
    "if": "github.event_name == 'pull_request' || github.event_name == 'merge_group'",
    "shell": "bash",
    "env": "",
    "run": "|",
}
VERDICT_BASE_STEP_SCALARS = {
    "id": "verdict_base",
    "if": "github.event_name == 'pull_request' || github.event_name == 'merge_group'",
    "shell": "bash",
    "env": "",
    "run": "|",
}
TRUSTED_BASE_ENV = {
    "EVENT_NAME": "${{ github.event_name }}",
    "PR_NUMBER": "${{ github.event.pull_request.number || github.run_id }}",
    "PR_BASE_REF": "${{ github.event.pull_request.base.ref || '' }}",
    "PR_BASE_SHA": "${{ github.event.pull_request.base.sha || '' }}",
    "MERGE_GROUP_BASE_REF": "${{ github.event.merge_group.base_ref || '' }}",
    "MERGE_GROUP_BASE_SHA": "${{ github.event.merge_group.base_sha || '' }}",
}
DETECTOR_REFS_RUN = '''if [[ "$EVENT_NAME" == "pull_request" ]]; then
  base_branch="$PR_BASE_REF"
  base_ref="refs/remotes/origin/pr-base-${PR_NUMBER}"
  head_ref="refs/remotes/origin/pr-head-${PR_NUMBER}"
  git check-ref-format "refs/heads/$base_branch"
  git fetch --no-tags origin \\
    "+refs/heads/${base_branch}:${base_ref}" \\
    "+refs/pull/${PR_NUMBER}/head:${head_ref}"
elif [[ "$EVENT_NAME" == "workflow_dispatch" ]]; then
  base_branch="$DISPATCH_BASE_REF"
  if [[ "$base_branch" == refs/* ]]; then
    echo "unsupported workflow_dispatch default_branch: $base_branch" >&2
    exit 1
  fi
  base_ref="refs/remotes/origin/dispatch-base-${GITHUB_RUN_ID}"
  head_ref="HEAD"
  git check-ref-format "refs/heads/$base_branch"
  git fetch --no-tags origin "+refs/heads/${base_branch}:${base_ref}"
elif [[ "$EVENT_NAME" == "merge_group" ]]; then
  merge_group_base="$MERGE_GROUP_BASE_REF"
  if [[ "$merge_group_base" == refs/heads/* ]]; then
    base_branch="${merge_group_base#refs/heads/}"
  elif [[ "$merge_group_base" == refs/* ]]; then
    echo "unsupported merge_group base_ref: $merge_group_base" >&2
    exit 1
  else
    base_branch="$merge_group_base"
  fi
  base_ref="refs/remotes/origin/pr-base-merge-group-${GITHUB_RUN_ID}"
  head_ref="HEAD"
  git check-ref-format "refs/heads/$base_branch"
  git fetch --no-tags origin "+refs/heads/${base_branch}:${base_ref}"
else
  echo "unsupported detector refs event: $EVENT_NAME" >&2
  exit 1
fi
echo "base_ref=${base_ref}" >> "$GITHUB_OUTPUT"
echo "head_ref=${head_ref}" >> "$GITHUB_OUTPUT"'''
FINGERPRINT_REUSE_INPUTS_CHANGED_RUN = """base_ref="${{ steps.pr_refs.outputs.base_ref }}"
head_ref="${{ steps.pr_refs.outputs.head_ref }}"
if [[ "${{ github.event_name }}" == "workflow_dispatch" ]]; then
  diff_range="${base_ref}..${head_ref}"
else
  diff_range="${base_ref}...${head_ref}"
fi
changed="$(git diff --name-only "$diff_range" -- \\
  .github/actions/setup-environment/action.yml \\
  ci/nextest-fingerprint.toml \\
  ci/github-actions-runners.toml \\
  scripts/nextest_fingerprint.py \\
  scripts/test_nextest_fingerprint.py \\
  scripts/root_bin_sidecars.py \\
  scripts/test_root_bin_sidecars.py \\
  scripts/config_validators.py \\
  scripts/ci_provenance.py \\
  scripts/test_ci_provenance.py \\
  scripts/verify_ci_workflow_hygiene.py \\
  scripts/test_verify_ci_workflow_hygiene.py)"
if [[ -n "$changed" ]]; then
  echo "any_changed=true" >> "$GITHUB_OUTPUT"
else
  echo "any_changed=false" >> "$GITHUB_OUTPUT"
fi"""
FINGERPRINT_REUSE_ALLOWED_RUN = """if [[ "${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true" ]]; then
  echo "value=false" >> "$GITHUB_OUTPUT"
  echo "reason=governance-changed" >> "$GITHUB_OUTPUT"
elif [[ "${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group" ]]; then
  echo "value=true" >> "$GITHUB_OUTPUT"
  echo "reason=consumer-event" >> "$GITHUB_OUTPUT"
else
  echo "value=false" >> "$GITHUB_OUTPUT"
  echo "reason=non-consumer-event" >> "$GITHUB_OUTPUT"
fi"""
SELF_AUTHORIZING_GOVERNANCE_RUN = """set -euo pipefail
base_ref="${{ steps.pr_refs.outputs.base_ref }}"
head_ref="${{ steps.pr_refs.outputs.head_ref }}"
if [[ -z "$base_ref" || -z "$head_ref" ]]; then
  echo "self-authorizing governance detector missing PR diff context"
  exit 1
fi
changed="$(git diff --name-only "${base_ref}...${head_ref}" -- \\
  AGENTS.md \\
  .specify/memory/constitution.md \\
  .pr_agent.toml \\
  ci/ai-review.toml)"
if [[ -z "$changed" ]]; then
  exit 0
fi
base_tree="$RUNNER_TEMP/self-authorizing-governance-base-tree"
mkdir -p "$base_tree"
git archive "$base_ref" \\
  .github/ \\
  .config/ \\
  ci/ \\
  crates/backtesting-vertical-slice/ci/ \\
  scripts/ \\
  tests/ \\
  AGENTS.md \\
  Cargo.toml \\
  justfile \\
  .mergify.yml \\
  .no-mistakes.yaml \\
  .pr_agent.toml \\
  | tar -x -C "$base_tree"
python3 "$base_tree/scripts/verify_ci_workflow_hygiene.py" self-authorizing-governance \\
  --repo "$GITHUB_WORKSPACE" \\
  --base "$base_ref" \\
  --head "$head_ref\""""
NEXTEST_FINGERPRINT_REUSE_BASE_RUN = '''if [[ "$EVENT_NAME" == "pull_request" ]]; then
  base_branch="$PR_BASE_REF"
  base_sha="$PR_BASE_SHA"
  base_ref="refs/remotes/origin/ci-provenance-reuse-base-${PR_NUMBER}"
elif [[ "$EVENT_NAME" == "merge_group" ]]; then
  merge_group_base="$MERGE_GROUP_BASE_REF"
  if [[ "$merge_group_base" == refs/heads/* ]]; then
    base_branch="${merge_group_base#refs/heads/}"
  elif [[ "$merge_group_base" == refs/* ]]; then
    echo "unsupported merge_group base_ref: $merge_group_base" >&2
    exit 1
  else
    base_branch="$merge_group_base"
  fi
  base_sha="$MERGE_GROUP_BASE_SHA"
  base_ref="refs/remotes/origin/ci-provenance-reuse-base-merge-group-${GITHUB_RUN_ID}"
else
  echo "unsupported trusted base event: $EVENT_NAME" >&2
  exit 1
fi
git check-ref-format "refs/heads/$base_branch"
if [[ ! "$base_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "trusted base SHA is missing or malformed: $base_sha" >&2
  exit 1
fi
git fetch --no-tags origin "+${base_sha}:${base_ref}"
base_tree="$RUNNER_TEMP/ci-provenance-reuse-base-tree"
mkdir -p "$base_tree"
git archive "$base_ref" scripts/ | tar -x -C "$base_tree"
provenance_script="$base_tree/scripts/ci_provenance.py"
if [[ ! -f "$provenance_script" || -L "$provenance_script" ]]; then
  echo "trusted base provenance script is missing or not a regular file: $provenance_script" >&2
  exit 1
fi
echo "script=$provenance_script" >> "$GITHUB_OUTPUT"'''
NEXTEST_FINGERPRINT_REUSE_RESOLVER_RUN = """required_emitter="scripts/ci_provenance.py"
trusted_base_emitter="${{ steps.reuse_provenance_base.outputs.script }}"
if [[ -n "$trusted_base_emitter" ]]; then
  required_emitter="$trusted_base_emitter"
fi
python3 scripts/ci_provenance.py resolve-fingerprint \\
  --current-run-id "${{ github.run_id }}" \\
  --current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}" \\
  --require-inherited-emitter "$required_emitter" \\
  | tee -a "$GITHUB_OUTPUT\""""
CI_PROVENANCE_BASE_RUN = '''if [[ "$EVENT_NAME" == "pull_request" ]]; then
  base_branch="$PR_BASE_REF"
  base_sha="$PR_BASE_SHA"
  base_ref="refs/remotes/origin/ci-provenance-base-${PR_NUMBER}"
elif [[ "$EVENT_NAME" == "merge_group" ]]; then
  merge_group_base="$MERGE_GROUP_BASE_REF"
  if [[ "$merge_group_base" == refs/heads/* ]]; then
    base_branch="${merge_group_base#refs/heads/}"
  elif [[ "$merge_group_base" == refs/* ]]; then
    echo "unsupported merge_group base_ref: $merge_group_base" >&2
    exit 1
  else
    base_branch="$merge_group_base"
  fi
  base_sha="$MERGE_GROUP_BASE_SHA"
  base_ref="refs/remotes/origin/ci-provenance-base-merge-group-${GITHUB_RUN_ID}"
else
  echo "unsupported trusted base event: $EVENT_NAME" >&2
  exit 1
fi
git check-ref-format "refs/heads/$base_branch"
if [[ ! "$base_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "trusted base SHA is missing or malformed: $base_sha" >&2
  exit 1
fi
git fetch --no-tags origin "+${base_sha}:${base_ref}"
base_tree="$RUNNER_TEMP/ci-provenance-base-tree"
mkdir -p "$base_tree"
git archive "$base_ref" scripts/ ci/github-actions-runners.toml | tar -x -C "$base_tree"
tested_workflow="$GITHUB_WORKSPACE/.github/workflows/ci.yml"
if [[ ! -f "$tested_workflow" || -L "$tested_workflow" ]]; then
  echo "tested workflow file is missing or not a regular file: $tested_workflow" >&2
  exit 1
fi
mkdir -p "$base_tree/.github/workflows"
cp "$tested_workflow" "$base_tree/.github/workflows/ci.yml"
{
  echo "script=$base_tree/scripts/ci_provenance.py"
  echo "config=$base_tree/ci/github-actions-runners.toml"
  echo "workflow=$base_tree/.github/workflows/ci.yml"
} >> "$GITHUB_OUTPUT"'''
VERDICT_BASE_RUN = '''if [[ "$EVENT_NAME" == "pull_request" ]]; then
  base_branch="$PR_BASE_REF"
  base_sha="$PR_BASE_SHA"
  base_ref="refs/remotes/origin/ci-gate-base-${PR_NUMBER}"
elif [[ "$EVENT_NAME" == "merge_group" ]]; then
  merge_group_base="$MERGE_GROUP_BASE_REF"
  if [[ "$merge_group_base" == refs/heads/* ]]; then
    base_branch="${merge_group_base#refs/heads/}"
  elif [[ "$merge_group_base" == refs/* ]]; then
    echo "unsupported merge_group base_ref: $merge_group_base" >&2
    exit 1
  else
    base_branch="$merge_group_base"
  fi
  base_sha="$MERGE_GROUP_BASE_SHA"
  base_ref="refs/remotes/origin/ci-gate-base-merge-group-${GITHUB_RUN_ID}"
else
  echo "unsupported trusted base event: $EVENT_NAME" >&2
  exit 1
fi
git check-ref-format "refs/heads/$base_branch"
if [[ ! "$base_sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "trusted base SHA is missing or malformed: $base_sha" >&2
  exit 1
fi
git fetch --no-tags origin "+${base_sha}:${base_ref}"
base_tree="$RUNNER_TEMP/ci-gate-base-tree"
mkdir -p "$base_tree"
git archive "$base_ref" scripts/ ci/github-actions-runners.toml | tar -x -C "$base_tree"
echo "script=$base_tree/scripts/ci_provenance.py" >> "$GITHUB_OUTPUT"'''
GATE_NEXTEST_FINGERPRINT_REUSE_BRANCH = """if [[ "${{ needs.nextest-fingerprint-reuse.result }}" != "success" ]]; then
  echo "nextest fingerprint reuse resolver did not succeed"
  exit 1
fi
if [[ "${{ needs.ci-provenance-emit.result }}" != "success" ]]; then
  echo "ci-provenance-emit did not succeed during nextest fingerprint reuse"
  exit 1
fi
echo "nextest archive reused from run ${{ needs.nextest-fingerprint-reuse.outputs.source_run_id }} at ${{ needs.nextest-fingerprint-reuse.outputs.source_sha }}\""""
FINGERPRINT_REUSE_GOVERNANCE_PATHS = (
    ".github/actions/setup-environment/action.yml",
    "ci/nextest-fingerprint.toml",
    "ci/github-actions-runners.toml",
    "scripts/nextest_fingerprint.py",
    "scripts/test_nextest_fingerprint.py",
    "scripts/root_bin_sidecars.py",
    "scripts/test_root_bin_sidecars.py",
    "scripts/config_validators.py",
    "scripts/ci_provenance.py",
    "scripts/test_ci_provenance.py",
    "scripts/verify_ci_workflow_hygiene.py",
    "scripts/test_verify_ci_workflow_hygiene.py",
)
BUILD_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*"
    r"needs\.ci-policy\.outputs\.full_ci_required\s*==\s*['\"]true['\"]\s*&&\s*"
    r"needs\.detector\.outputs\.build_required\s*==\s*['\"]true['\"]\s*\}\}\s*$"
)
PR_CONCURRENCY_EVENT_RE = re.compile(r"github\.event_name\s*==\s*['\"]pull_request['\"]")
PR_CONCURRENCY_PULL_REQUEST_BRANCH_RE = re.compile(
    r"github\.event_name\s*==\s*['\"]pull_request['\"]\s*&&\s*"
    r"format\(\s*['\"]pr-\{0\}['\"]\s*,\s*github\.event\.number\s*\)"
)
PR_CONCURRENCY_NON_PR_FALLBACK_RE = re.compile(
    r"\|\|\s*format\(\s*['\"]\{0\}-\{1\}['\"]\s*,\s*github\.ref_name\s*,\s*github\.sha\s*\)"
)
PR_CONCURRENCY_CANCEL_SCOPE_ERROR = (
    "cancel-in-progress must apply only to pull_request and workflow_dispatch runs"
)
GATE_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?always\(\)\s*(?:\}\})?\s*$")
DEPLOY_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*always\(\)\s*&&\s*startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*"
    r"needs\.gate\.result\s*==\s*['\"]success['\"]\s*&&\s*"
    r"needs\.same-sha-main-evidence\.result\s*==\s*['\"]success['\"]\s*\}\}\s*$"
)
EXIT_RE = re.compile(r"^\s*exit(?:\s+([0-9]+))?\s*$", re.MULTILINE)
TARGET_DIR_OPT_IN_RE = re.compile(r"^\s+include-managed-target-dir:\s*(['\"])true\1\s*$")
SETUP_TARGET_DIR_EXPORT_RE = re.compile(r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir\s*\}\}\s*$")
SETUP_TARGET_DIR_RELATIVE_EXPORT_RE = re.compile(
    r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir_relative\s*\}\}\s*$"
)
SETUP_TARGET_DIR_RELATIVE_OUTPUT_RE = re.compile(
    r'^\s*echo\s+"managed_target_dir_relative=\$managed_target_dir_relative"\s*>>\s*"\$GITHUB_OUTPUT"\s*$'
)
SETUP_CARGO_BUILD_JOBS_ENV_OUTPUT_RE = re.compile(
    r'^\s*echo\s+"CARGO_BUILD_JOBS=\$cargo_build_jobs"\s*>>\s*"\$GITHUB_ENV"\s*$'
)
INLINE_CARGO_BUILD_JOBS_RE = re.compile(r"\bCARGO_BUILD_JOBS\b")
CARGO_BUILD_JOBS_COMPILE_COMMAND_RE = re.compile(
    r"(?:^|[\s;&|()])(?:"
    r"cargo\s+(?:build|check|clippy|test|nextest|zigbuild)\b|"
    r"cargo\s+--repo\b|"
    r"just\s+(?:"
    r"build|check-aarch64|clippy|source-fence|source-fence-static|cargo-shim-tests|"
    r"test-archive|test-archive-run|test|"
    r"bte-clippy|bte-test-archive|bte-test-archive-run|bte-test"
    r")\b"
    r")"
)
SPIKE_PROBE_MARKER_RE = re.compile(r"\bBOLT_SPIKE_[A-Z0-9_]*\b")
SETUP_TARGET_DIR_RELATIVE_COMPUTE = (
    "managed_target_dir_relative=\"$(python3 -c 'import os, sys; "
    "print(os.path.relpath(sys.argv[2], sys.argv[1]))' \"$GITHUB_WORKSPACE\" \"$managed_target_dir\")\""
)
SETUP_TARGET_DIR_IF_RE = re.compile(
    r"^\s+if:\s*\$\{\{\s*inputs\.include-managed-target-dir\s*==\s*['\"]true['\"]\s*\}\}\s*$"
)
SETUP_ACTION_REQUIRED_LITERALS = (
    "inputs.just-version",
    "inputs.include-deny-version",
    "inputs.include-nextest-version",
    "inputs.include-build-values",
    "inputs.lint-workflow-contract",
    "inputs.install-rust-linker",
    "inputs.build-jobs-key",
    "just ci-lint-workflow",
    "awk -F'\\\"' '/^channel = / {print $2}' rust-toolchain.toml",
    "just --evaluate deny_version",
    "just --evaluate nextest_version",
    "just --evaluate target",
    "just --evaluate zig_version",
    "just --evaluate zigbuild_version",
    "just --evaluate rust_verification_owner",
    "ci/github-actions-runners.toml",
    "cargo_build_jobs=$cargo_build_jobs",
    'python3.12 "${{ steps.shared.outputs.rust_verification_owner }}" fast-linker-programs --repo "$GITHUB_WORKSPACE"',
    'command -v "$rust_linker_program" >/dev/null',
    "BOLT_RUST_FAST_LINKER=$rust_linker_program",
    'echo "::warning::failed to install any configured Rust linker; continuing without fast linker"',
    'target-dir --repo "$GITHUB_WORKSPACE"',
    "os.path.relpath",
)
SETUP_FAST_LINKER_FAIL_OPEN_WARNING = (
    'echo "::warning::failed to install any configured Rust linker; continuing without fast linker"'
)
SETUP_ACTION_OUTPUT_MAPPINGS = {
    "rust_toolchain": "steps.shared.outputs.rust_toolchain",
    "deny_version": "steps.shared.outputs.deny_version",
    "nextest_version": "steps.shared.outputs.nextest_version",
    "target": "steps.shared.outputs.target",
    "zig_version": "steps.shared.outputs.zig_version",
    "zigbuild_version": "steps.shared.outputs.zigbuild_version",
    "rust_verification_owner": "steps.shared.outputs.rust_verification_owner",
    "managed_target_dir": "steps.target_dir.outputs.managed_target_dir",
    "managed_target_dir_relative": "steps.target_dir.outputs.managed_target_dir_relative",
    "cargo_build_jobs": "steps.shared.outputs.cargo_build_jobs",
}
SETUP_ACTION_ORDERED_STEPS = (
    "Lint workflow contract",
    "Read shared values",
    "Install Rust linker",
    "Resolve managed target dir",
    "Setup Rust toolchain",
)
CI_RUST_FAST_LINKER_JOBS = {"build", "clippy", "source-fence", "test-archive"}
TEST_PARTITION_COMMAND = (
    'just test-archive-run "$NEXTEST_ARCHIVE_PATH" '
    '"$RUNNER_TEMP/nextest-archive-extract" '
    '--partition "count:${shard}/${shards}"'
)
TEST_REPRODUCTION_COMMAND = (
    "just test-archive-run .nextest-archive/nextest-archive.tar.zst "
    "<extract-root> "
    "--partition count:${shard}/${shards}"
)
TEST_REPRODUCTION_ECHO = f'echo "reproduce locally: {TEST_REPRODUCTION_COMMAND}"'
TEST_ARCHIVE_EXTRACT_ROOT_INIT = 'mkdir -p "$RUNNER_TEMP/nextest-archive-extract"'
TEST_ARCHIVE_SHARDS_ASSIGNMENT = 'shards="${{ needs.nextest-fingerprint.outputs.nextest_shards }}"'
TEST_ARCHIVE_SHARDS_ASSERT = 'if [[ ! "$shards" =~ ^[1-9][0-9]*$ ]]; then'
TEST_ARCHIVE_PARTITION_LOOP = 'for shard in $(seq 1 "$shards"); do'
TEST_ARCHIVE_PARTITION_GROUP = 'echo "::group::nextest archive partition ${shard}/${shards}"'
TEST_ARCHIVE_PARTITION_STATUS_INIT = "status=0"
TEST_ARCHIVE_PARTITION_STATUS_MARK = "status=1"
TEST_ARCHIVE_PARTITION_STATUS_EXIT = 'exit "$status"'
TEST_ARCHIVE_PARTITION_LOG_ASSIGN = 'partition_log="$RUNNER_TEMP/nextest-archive-partition-${shard}.log"'
TEST_ARCHIVE_PARTITION_TEE = f'{TEST_PARTITION_COMMAND} 2>&1 | tee "$partition_log"'
TEST_ARCHIVE_PARTITION_RC_CAPTURE = 'rc="${PIPESTATUS[0]}"'
TEST_ARCHIVE_PARTITION_ERROR_ANNOTATION = (
    'echo "::error title=nextest archive partition failed::shard=${shard}/${shards} exit=${rc}"'
)
TEST_ARCHIVE_PARTITION_LOG_TAIL = 'tail -80 "$partition_log"'
TEST_ARCHIVE_PARTITION_FAILURE_WRAPPER = (
    f"            {TEST_ARCHIVE_PARTITION_LOG_ASSIGN}\n"
    "            set +e\n"
    f"            {TEST_ARCHIVE_PARTITION_TEE}\n"
    "            rc=\"${PIPESTATUS[0]}\"\n"
    "            set -e\n"
    "            if [[ \"$rc\" -ne 0 ]]; then\n"
    "              status=1\n"
)
ROOT_TEST_ARCHIVE_JOB_SHA256 = "682af87a5a168c034b281d100f47da66693f24320ca5bb64ea41b16520b8fe5c"
CI_CLASSIFICATION_SUMMARY_LINE = (
    'echo "CI classification: class=${class} policy=${CI_POLICY_PATH:-unknown} '
    'full_ci_required=${FULL_CI_REQUIRED:-false} deferred=${FULL_CI_DEFERRED:-false} '
    'event_class=${EXPECTED_EVENT_CLASS:-unknown} reason=${POLICY_REASON:-missing}" >> "$GITHUB_STEP_SUMMARY"'
)
NEXTEST_REUSE_SUMMARY_LINE = (
    'echo "Nextest reuse: decision=${decision} detector_allowed=${detector_allowed} '
    'reuse_found=${reuse_found} source_run=${source_run:-none} source_sha=${source_sha:-none} '
    'artifact=${artifact:-none} reason=${reason:-none}" >> "$GITHUB_STEP_SUMMARY"'
)
NEXTEST_REUSE_SUMMARY_ENV_LINES = (
    "DETECTOR_ALLOWED: ${{ needs.detector.outputs.fingerprint_reuse_allowed || 'false' }}",
    "DETECTOR_REASON: ${{ needs.detector.outputs.fingerprint_reuse_reason || 'unknown' }}",
    "REUSE_FOUND: ${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || 'false' }}",
    "REUSE_SOURCE_RUN: ${{ needs.nextest-fingerprint-reuse.outputs.source_run_id || 'none' }}",
    "REUSE_SOURCE_SHA: ${{ needs.nextest-fingerprint-reuse.outputs.source_sha || 'none' }}",
    "REUSE_ARTIFACT: ${{ needs.nextest-fingerprint-reuse.outputs.source_artifact_id || 'none' }}",
    "REUSE_REASON: ${{ needs.nextest-fingerprint-reuse.outputs.reason || '' }}",
)
NEXTEST_REUSE_SUMMARY_ASSIGNMENTS = (
    'detector_allowed="${DETECTOR_ALLOWED:-false}"',
    'detector_reason="${DETECTOR_REASON:-unknown}"',
    'reuse_found="${REUSE_FOUND:-false}"',
    'source_run="${REUSE_SOURCE_RUN:-none}"',
    'source_sha="${REUSE_SOURCE_SHA:-none}"',
    'artifact="${REUSE_ARTIFACT:-none}"',
    'reason="${REUSE_REASON:-}"',
)
BVS_PARTITION_LOG_ASSIGN = 'partition_log="$RUNNER_TEMP/bvs-nextest-archive-partition-${shard}.log"'
BVS_PARTITION_COMMAND = (
    'just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" '
    '"$RUNNER_TEMP/bvs-nextest-archive-extract" '
    '--partition "count:${shard}/${BVS_NEXTEST_SHARDS}" '
    "-- --skip issue_789_first_real_free_data_taker_pl --skip backtesting_vertical_slice_s3_catalog_smoke"
)
BVS_PARTITION_TEE = f'{BVS_PARTITION_COMMAND} 2>&1 | tee "$partition_log"'
BVS_PARTITION_FAILURE_WRAPPER = (
    f"            {BVS_PARTITION_LOG_ASSIGN}\n"
    "            set +e\n"
    f"            {BVS_PARTITION_TEE}\n"
    "            rc=\"${PIPESTATUS[0]}\"\n"
    "            set -e\n"
)
BVS_TEST_ARCHIVE_JOB_SHA256 = "e18b0205846df6f4a7def0f24959477697ed5d0e35d3db56ca97547de696dd6c"
TEST_ARCHIVE_CACHE_KEY = (
    "${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}"
    "v${{ needs.nextest-fingerprint.outputs.nextest_schema }}"
    "-${{ runner.os }}-${{ runner.arch }}"
    "-${{ needs.nextest-fingerprint.outputs.nextest_profile }}"
    "-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}"
    "-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
)
TEST_ARCHIVE_FINGERPRINT_PATH = ".nextest-archive-fingerprint/cache-key.txt"
TEST_ARCHIVE_FINGERPRINT_OUTPUT = "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"
TEST_ARCHIVE_FINGERPRINT_JOB_OUTPUT = (
    "nextest_fingerprint: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint }}"
)
TEST_ARCHIVE_FINGERPRINT_REQUIRED_JOB_OUTPUTS = (
    "nextest_digest: ${{ steps.nextest-fingerprint.outputs.nextest_digest }}",
    TEST_ARCHIVE_FINGERPRINT_JOB_OUTPUT,
    "nextest_fingerprint_artifact_name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}",
    "nextest_archive_prefix: ${{ steps.nextest-fingerprint.outputs.nextest_archive_prefix }}",
    "nextest_schema: ${{ steps.nextest-fingerprint.outputs.nextest_schema }}",
    "nextest_profile: ${{ steps.nextest-fingerprint.outputs.nextest_profile }}",
    "nextest_shards: ${{ steps.nextest-fingerprint.outputs.nextest_shards }}",
)
TEST_ARCHIVE_FINGERPRINT_STEP_ID = "id: nextest-fingerprint"
TEST_ARCHIVE_FINGERPRINT_SCRIPT = "python3 scripts/nextest_fingerprint.py"
TEST_ARCHIVE_FINGERPRINT_SCRIPT_ARGS = (
    '--repo-root "$GITHUB_WORKSPACE"',
    "--config ci/nextest-fingerprint.toml",
    "--runners-config ci/github-actions-runners.toml",
    '--runner-os "${{ runner.os }}"',
    '--runner-arch "${{ runner.arch }}"',
    "--output-path .nextest-archive-fingerprint/cache-key.txt",
)
TEST_ARCHIVE_FINGERPRINT_ARTIFACT_NAME_OUTPUT = (
    "name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}"
)
FORBIDDEN_MANAGED_TARGET_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
    "'ci/rust-verification.toml'",
    "'justfile'",
    "'scripts/command_understanding.py'",
    "'scripts/rust_verification.py'",
)
TEST_ARCHIVE_PATH = "NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst"
TEST_ARCHIVE_SIDECAR_PATH = "ROOT_BIN_SIDECARS_PATH: .nextest-archive/root-bin-sidecars.tar.gz"
TEST_ARCHIVE_CACHE_PATH = "path: ${{ env.NEXTEST_ARCHIVE_PATH }}"
TEST_ARCHIVE_SIDECAR_CACHE_PATH = "path: ${{ env.ROOT_BIN_SIDECARS_PATH }}"
TEST_ARCHIVE_SIDECAR_CACHE_KEY = (
    "root-bin-sidecars-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}"
    "-${{ runner.os }}-${{ runner.arch }}"
    "-${{ needs.nextest-fingerprint.outputs.nextest_profile }}"
    "-profile-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
)
TEST_ARCHIVE_CACHE_HIT_GUARD = "if: steps.nextest-archive-cache.outputs.cache-hit != 'true'"
TEST_ARCHIVE_SCCACHE_OPT_IN = (
    "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}"
)
SCCACHE_SETUP_ACTION_PATH = "./.github/actions/sccache-setup"
SCCACHE_SETUP_ACTION_FILE = ".github/actions/sccache-setup/action.yml"
SCCACHE_ELIGIBILITY_SCRIPT_FILE = "scripts/sccache_eligibility.py"
SCCACHE_STATS_ACTION_PATH = "./.github/actions/sccache-stats"
SCCACHE_STATS_ACTION_FILE = ".github/actions/sccache-stats/action.yml"
SCCACHE_LOCATION_CONFIG_PATH = "ci/sccache-location.toml"
SCCACHE_LOCATION_CONFIG_DEFAULT = f"default: {SCCACHE_LOCATION_CONFIG_PATH}"
SCCACHE_READONLY_ROLE_INPUT = "role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}"
# Value, not mere presence: the fail-open flag must be literally "1", and the
# Rust verification owner must own the retry so workflows do not grow a second
# retry/test-execution path.
TEST_ARCHIVE_SCCACHE_IGNORE_IO = "SCCACHE_IGNORE_SERVER_IO_ERROR=1"
TEST_ARCHIVE_OWNER_COMMAND = 'just test-archive "$NEXTEST_ARCHIVE_PATH"'
TEST_ARCHIVE_SCCACHE_ACTIVE_INPUT = "active: ${{ steps.nextest-archive-cache.outputs.cache-hit != 'true' && 'true' || 'false' }}"
TEST_ARCHIVE_SCCACHE_WRITE_ROLE_INPUT = "write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}"
TEST_ARCHIVE_SIDECAR_CACHE_HIT_GUARD = "if: steps.root-bin-sidecars-cache.outputs.cache-hit == 'true'"
TEST_ARCHIVE_SIDECAR_CACHE_MISS_GUARD = "if: steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'"
TEST_ARCHIVE_SIDECAR_BUILD_GUARD = (
    "if: steps.nextest-archive-cache.outputs.cache-hit == 'true' "
    "&& steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'"
)
TEST_ARCHIVE_SIDECAR_PACK_GUARD = (
    "if: steps.nextest-archive-cache.outputs.cache-hit != 'true' "
    "&& steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'"
)
TEST_ARCHIVE_TARGET_CACHE_RESTORE_GUARD = "if: steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'"
TEST_ARCHIVE_TARGET_CACHE_SAVE_GUARD = "if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}"
TEST_ARCHIVE_TARGET_CACHE_KEY = (
    "managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-"
    "${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
)
TEST_ARCHIVE_CACHE_AUDIT_STEP = "Resolve root nextest cache keys"
TEST_ARCHIVE_CACHE_AUDIT_STEP_ID = "id: root-nextest-cache-keys"
TEST_ARCHIVE_CACHE_KEY_OUTPUT = "${{ steps.root-nextest-cache-keys.outputs.nextest_archive_cache_key }}"
TEST_ARCHIVE_SIDECAR_CACHE_KEY_OUTPUT = "${{ steps.root-nextest-cache-keys.outputs.root_bin_sidecars_cache_key }}"
TEST_ARCHIVE_TARGET_CACHE_KEY_OUTPUT = "${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}"
TEST_ARCHIVE_CACHE_AUDIT_OUTPUTS = (
    f"nextest_archive_cache_key: {TEST_ARCHIVE_CACHE_KEY_OUTPUT}",
    f"root_bin_sidecars_cache_key: {TEST_ARCHIVE_SIDECAR_CACHE_KEY_OUTPUT}",
    f"archive_build_target_cache_key: {TEST_ARCHIVE_TARGET_CACHE_KEY_OUTPUT}",
    "nextest_archive_cache_hit: ${{ steps.nextest-archive-cache.outcome == 'skipped' && 'skipped' || (steps.nextest-archive-cache.outputs.cache-hit || 'false') }}",
    "root_bin_sidecars_cache_hit: ${{ steps.root-bin-sidecars-cache.outcome == 'skipped' && 'skipped' || (steps.root-bin-sidecars-cache.outputs.cache-hit || 'false') }}",
    "archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || (steps.test-target-cache.outputs.cache-hit || 'false') }}",
)
TEST_ARCHIVE_CACHE_AUDIT_SAVE_OUTCOME_OUTPUTS = (
    "nextest_archive_cache_save_outcome: ${{ steps.nextest-archive-cache-save.outputs.save-status || (steps.nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
    "root_bin_sidecars_cache_save_outcome: ${{ steps.root-bin-sidecars-cache-save.outputs.save-status || (steps.root-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
    "archive_build_target_cache_save_outcome: ${{ steps.test-target-cache-save.outcome }}",
)
TEST_ARCHIVE_CACHE_SAVE_STEP_IDS = (
    ("Save nextest archive to S3", "id: nextest-archive-cache-save"),
    ("Save root binary sidecars to S3", "id: root-bin-sidecars-cache-save"),
    ("Save archive build target cache", "id: test-target-cache-save"),
)
TEST_ARCHIVE_CACHE_RESTORE_STEP_IDS = (
    ("Restore nextest archive from S3", "id: nextest-archive-cache"),
    ("Restore root binary sidecars from S3", "id: root-bin-sidecars-cache"),
    ("Restore archive build target cache", "id: test-target-cache"),
)
TEST_ARCHIVE_CACHE_AUDIT_KEY_OUTPUTS = (
    "nextest_archive_cache_key=",
    "root_bin_sidecars_cache_key=",
    "archive_build_target_cache_key=",
)
CACHE_PERSISTENCE_AUDIT_PROBE_STEP = "Probe saved cache keys"
CACHE_PERSISTENCE_AUDIT_NEEDS = ("ci-policy", "nextest-fingerprint-reuse", "test-archive")
CACHE_PERSISTENCE_AUDIT_CACHE_KEYS = (
    '--cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"',
)
CACHE_PERSISTENCE_AUDIT_CACHE_REFS = (
    '--github-event-name "$GITHUB_EVENT_NAME"',
    '--github-ref "$GITHUB_REF"',
    '--github-base-ref "$GITHUB_BASE_REF"',
    '--github-default-branch "${{ github.event.repository.default_branch }}"',
)
CACHE_PERSISTENCE_AUDIT_SUMMARY_ARG = '--github-step-summary "$GITHUB_STEP_SUMMARY"'
CACHE_PERSISTENCE_AUDIT_ANNOTATIONS_ARG = "--github-annotations"
CACHE_PERSISTENCE_AUDIT_RESTORE_HIT_ARGS = (
    '--restore-hit "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_hit }}"',
    '--restore-hit "root binary sidecars=${{ needs.test-archive.outputs.root_bin_sidecars_cache_hit }}"',
    '--restore-hit "archive build target=${{ needs.test-archive.outputs.archive_build_target_cache_hit }}"',
)
CACHE_PERSISTENCE_AUDIT_SAVE_OUTCOME_ARGS = (
    '--save-outcome "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_save_outcome }}"',
    '--save-outcome "root binary sidecars=${{ needs.test-archive.outputs.root_bin_sidecars_cache_save_outcome }}"',
    '--save-outcome "archive build target=${{ needs.test-archive.outputs.archive_build_target_cache_save_outcome }}"',
)
CACHE_PERSISTENCE_AUDIT_PROBE_SCALAR_REQUIREMENTS = (
    (
        "cache-persistence-audit must use the workflow token for cache API reads",
        "GH_TOKEN",
        "${{ github.token }}",
    ),
)
CACHE_PERSISTENCE_AUDIT_PROBE_COMMAND_REQUIREMENTS = (
    (
        "cache-persistence-audit must probe all root nextest cache keys",
        CACHE_PERSISTENCE_AUDIT_CACHE_KEYS,
    ),
    (
        "cache-persistence-audit must limit exact-key probes to restorable cache refs",
        CACHE_PERSISTENCE_AUDIT_CACHE_REFS,
    ),
    (
        "cache-persistence-audit must write probe results to the job summary",
        (CACHE_PERSISTENCE_AUDIT_SUMMARY_ARG,),
    ),
    (
        "cache-persistence-audit must emit audit annotations from ci_storage_audit",
        (CACHE_PERSISTENCE_AUDIT_ANNOTATIONS_ARG,),
    ),
    (
        "cache-persistence-audit must summarize cache restore hits",
        CACHE_PERSISTENCE_AUDIT_RESTORE_HIT_ARGS,
    ),
    (
        "cache-persistence-audit must summarize cache save outcomes",
        CACHE_PERSISTENCE_AUDIT_SAVE_OUTCOME_ARGS,
    ),
)
CACHE_PERSISTENCE_AUDIT_ARGV_PREFIX = ("python3", "scripts/ci_storage_audit.py")
TEST_ARCHIVE_TEST_PROFILE_ENV = 'CARGO_PROFILE_TEST_DEBUG: "0"'
TEST_ARCHIVE_SIDECAR_PROFILE_ENV = 'CARGO_PROFILE_DEV_DEBUG: "0"'
TEST_ARCHIVE_SIDECAR_BUILD_COMMAND = (
    'python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo "$GITHUB_WORKSPACE" -- build --locked --bins'
)
TEST_ARCHIVE_SIDECAR_PACK_COMMAND = "python3 scripts/root_bin_sidecars.py pack"
TEST_ARCHIVE_RESTORE_ACTION = "uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_SAVE_ACTION = "uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_S3_AWS_CONFIG_STEP = "Configure AWS credentials for nextest artifact cache"
TEST_ARCHIVE_S3_ELIGIBILITY_STEP = "Resolve nextest artifact cache eligibility"
TEST_ARCHIVE_S3_RESTORE_GUARD = "if: steps.nextest-artifact-cache.outputs.eligible == 'true' && steps.nextest-artifact-cache-aws.outcome == 'success'"
TEST_ARCHIVE_S3_MAIN_SAVE_GUARD = "github.event_name == 'push' && github.ref == 'refs/heads/main'"
TEST_ARCHIVE_S3_PREFIX_ENV = "NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}"
TEST_ARCHIVE_S3_ENABLED_ENV = "NEXTEST_ARTIFACT_CACHE_ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}"
TEST_ARCHIVE_S3_BUCKET_ENV = "NEXTEST_ARTIFACT_CACHE_BUCKET: ${{ vars.CI_SCCACHE_BUCKET }}"
TEST_ARCHIVE_S3_REGION_ENV = "NEXTEST_ARTIFACT_CACHE_REGION: ${{ vars.CI_SCCACHE_REGION }}"
TEST_ARCHIVE_RESTORE_RESULT_OUTPUT = 'echo "restore-result='
TEST_ARCHIVE_RESTORE_REASON_OUTPUT = 'echo "restore-reason='
TEST_ARCHIVE_S3_SUMMARY_STEP = "Summarize nextest archive S3 state"
TEST_ARCHIVE_S3_SUMMARY_AWS_ENV = "S3_AWS_OUTCOME: ${{ steps.nextest-artifact-cache-aws.outcome }}"
TEST_ARCHIVE_S3_SUMMARY_RESTORE_STATE = "restore_state()"
TEST_ARCHIVE_S3_SUMMARY_NEXT_LINE = (
    'echo "Root nextest archive S3: eligible=${S3_ELIGIBLE:-false} '
    'mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} '
    'restore=${archive_restore} reason=${archive_reason}"'
)
TEST_ARCHIVE_S3_SUMMARY_SIDECAR_LINE = (
    'echo "Root binary sidecars S3: eligible=${S3_ELIGIBLE:-false} '
    'mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} '
    'restore=${sidecar_restore} reason=${sidecar_reason}"'
)
TEST_ARCHIVE_DOWNLOAD_ACTION = "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c"
UPLOAD_ARTIFACT_SHA_RE = re.compile(r"^\s*(?:-\s*)?uses:\s*([\"']?)actions/upload-artifact@[0-9a-fA-F]{40}\1\s*$")
CACHE_KEY_RE = re.compile(r"^\s+(?:key|shared-key):\s*\S+.*$")
SHARED_REGISTRY_CACHE_KEY = "cargo-registry-git-v1"
SHARED_REGISTRY_SAVE_IF = "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}"
REGISTRY_CACHE_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test-archive", "build")
# Jobs that opt into the managed-target actions/cache. Each value is the
# job-specific key prefix segment between `managed-target-v1-${runner.os}-
# ${runner.arch}-` and the hashFiles suffix. Adding a new job that uses
# `steps.setup.outputs.managed_target_dir` requires (a) registering its
# expected prefix here so `managed_target_cache_errors` enforces key isolation
# AND a matching `restore-keys` prefix fallback (#400), and (b) updating the
# self-test fixture in `scripts/test_verify_ci_workflow_hygiene.py`.
MANAGED_TARGET_CACHE_KEYS = {
    "clippy": "clippy-host",
    "check-aarch64": "check-aarch64-dev",
    "source-fence": "source-fence-test",
    "test-archive": "test-archive-test",
    "build": "build-aarch64-release",
}
JUST_LANE_RE = re.compile(
    r"(^|[^A-Za-z0-9_./-])just\s+"
    r"(deny|deny-advisories|clippy|test-archive-run|test-archive|test|build|check-aarch64|source-fence)"
    r"([^A-Za-z0-9_]|$)"
)
REPO_LOCAL_ARTIFACT_RE = re.compile(r"(^|[^A-Za-z0-9_./-])target/(?:.*/)?release/bolt-v2(?:\.sha256)?([^A-Za-z0-9_./-]|$)")
BINARY_PATH_COMMAND = 'python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
# taiki-e/install-action must be pinned to a 40-hex commit SHA (mutable tags
# like @v2 are rejected). The specific SHA is NOT enforced here — Dependabot
# opens a PR with release notes for every bump and PR review is the human
# gate. See tj-actions/changed-files (CVE-2025-30066, March 2025) for why
# SHA-pinning matters and why hardcoding a specific SHA here adds maintenance
# burden without real supply-chain value.
#
# Two regexes intentionally:
#   * TAIKI_INSTALL_ACTION_RE matches well-formed pinned single-line `uses:`
#     references. Optional matching quotes (single OR double, enforced by
#     backreference so mismatched quotes still fail) are accepted around the
#     reference. Uppercase hex is allowed in the match so the consistency
#     check can normalize via .lower() rather than silently rejecting valid
#     uppercase pins. The SHA is captured in group(2); group(1) is the
#     (possibly empty) opening quote used by the backreference.
#   * TAIKI_INSTALL_ACTION_MENTION_RE finds candidate action refs, while
#     the uses-key regexes scope those candidates to real `uses:` values.
#     This preserves mutable-tag and multi-line-scalar rejection without
#     treating prose in `name:`/comments as an action invocation.
TAIKI_INSTALL_ACTION_RE = re.compile(
    r"""^\s*(?:-\s*)?uses:\s*(['"]?)taiki-e/install-action@([0-9a-fA-F]{40})\1\s*$"""
)
TAIKI_INSTALL_ACTION_MENTION_RE = re.compile(r"\btaiki-e/install-action@")
TAIKI_INSTALL_ACTION_USES_LINE_RE = re.compile(r"^\s*(?:-\s*)?uses\s*:")
TAIKI_INSTALL_ACTION_BARE_USES_KEY_RE = re.compile(r"^\s*(?:-\s*)?uses\s*:\s*(?:[>|][0-9+-]*)?\s*$")
SETUP_JUST_TOOL = "just@${{ inputs.just-version }}"
CI_INSTALL_ACTION_TOOLS = {
    "deny": ("cargo-deny", "steps.setup.outputs.deny_version"),
    "advisories": ("cargo-deny", "steps.setup.outputs.deny_version"),
    "test-archive": ("cargo-nextest", "steps.setup.outputs.nextest_version"),
    "build": ("cargo-zigbuild", "steps.setup.outputs.zigbuild_version"),
}


def extract_paths_ignore_for_trigger(
    workflow_text: str, trigger: str
) -> tuple[str, ...] | None:
    """Return the paths-ignore list under `on.<trigger>`, or None if absent.

    Parses the block-style YAML this repo uses; flow-style maps are not supported.
    """

    lines = [strip_comment(line).rstrip() for line in workflow_text.splitlines()]

    def section_index(start: int, header: str, max_indent: int) -> int | None:
        i = start
        while i < len(lines):
            line = lines[i]
            if line and len(line) - len(line.lstrip(" ")) <= max_indent and line != header:
                return None
            if line == header:
                return i
            i += 1
        return None

    on_idx = section_index(0, "on:", max_indent=-1)
    if on_idx is None:
        return None
    trigger_idx = section_index(on_idx + 1, f"  {trigger}:", max_indent=0)
    if trigger_idx is None:
        return None
    pi_idx = section_index(trigger_idx + 1, "    paths-ignore:", max_indent=2)
    if pi_idx is None:
        return None

    items: list[str] = []
    for i in range(pi_idx + 1, len(lines)):
        line = lines[i]
        if line and len(line) - len(line.lstrip(" ")) <= 4:
            break
        stripped = line.lstrip()
        if stripped.startswith("- "):
            items.append(stripped[2:].strip().strip("'").strip('"'))
    return tuple(items)


def parse_jobs(workflow_text: str) -> dict[str, list[str]]:
    """Parse this repo's strict GitHub Actions job subset.

    Top-level job ids must be indented by exactly two spaces under `jobs:`.
    The verifier reports required job ids that drift to another indentation.
    """

    lines = workflow_text.splitlines()
    jobs: dict[str, list[str]] = {}
    in_jobs = False
    current: str | None = None

    for line in lines:
        clean = strip_comment(line)
        if clean == "jobs:":
            in_jobs = True
            current = None
            continue
        if not in_jobs:
            continue
        if clean and not clean.startswith((" ", "\t")):
            break
        match = re.match(r"^  ([^ \t:#][^:#]*):(?:\s+&[A-Za-z0-9_.-]+)?\s*$", clean)
        if match:
            current = match.group(1).strip().strip("'\"")
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(clean)

    return jobs


def top_level_block(workflow_text: str, key: str) -> list[str]:
    lines = workflow_text.splitlines()
    start_line = f"{key}:"
    for index, line in enumerate(lines):
        clean = strip_comment(line)
        if clean != start_line:
            continue
        block: list[str] = []
        for child_line in lines[index + 1 :]:
            child_clean = strip_comment(child_line)
            if child_clean and not child_clean.startswith((" ", "\t")):
                break
            block.append(child_clean)
        return block
    return []


def yaml_scalar(value: str) -> str:
    stripped = value.strip()
    if len(stripped) >= 2 and stripped[0] == stripped[-1] and stripped[0] in {"'", '"'}:
        return stripped[1:-1]
    return stripped


def scalar_mapping(block_lines: list[str]) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in block_lines:
        clean = strip_comment(line).strip()
        match = re.fullmatch(r"([A-Za-z-]+):\s*(.+)", clean)
        if match:
            values[match.group(1)] = yaml_scalar(match.group(2))
    return values


def block_run_commands(lines: list[str]) -> list[str]:
    commands: list[str] = []
    run_indent: int | None = None
    for line in lines:
        clean = strip_comment(line).rstrip()
        if run_indent is not None:
            if clean and len(clean) - len(clean.lstrip(" ")) <= run_indent:
                run_indent = None
            else:
                command = clean.strip()
                if command:
                    commands.append(command)
                continue
        match = re.fullmatch(r"(\s*)run:\s*(.*)", clean)
        if match is None:
            continue
        value = match.group(2).strip()
        if value == "|":
            run_indent = len(match.group(1))
        elif value:
            commands.append(yaml_scalar(value))
    return commands


# The merge_group concurrency group must isolate every queue entry on its own
# ref. We do NOT analyze the group expression to decide that: GitHub's expression
# language is too expressive for a regex contract to reason about safely. Every
# analysis attempt was fail-open — an unkeyed arm can select merge_group by an
# unbounded set of syntaxes (github['event_name'] index form, startsWith on the
# queue ref, a negation), `&&` and gate text can hide inside string literals,
# github.ref can be buried inside a constant-collapsing function (startsWith /
# endsWith / contains) so the value is the same for every entry, and a duplicate
# top-level `group:` key (GitHub takes the last) escapes any single-expression
# parse. The only fail-closed contract with no analysis surface is a positive
# allowlist of the exact group expressions that are known merge_group-safe,
# compared after whitespace normalization. These are INDEPENDENT constants, not
# derived from the workflows, so an unsafe edit to a workflow is rejected rather
# than silently blessed. A new or edited merge_group workflow must add its
# normalized group expression here after review — that review is the gate.
#
# Known textual-scanning residual (liveness-only, tracked #879): the block
# extractor strips per-line YAML comments, which is faithful for real comments
# but treats a `#` inside a block scalar (`>-`/`|`, where GitHub keeps it as
# literal content) as a comment too. Such a workflow can normalize to an approved
# form and be accepted here even though GitHub's expression evaluation breaks on
# the literal `#`. This cannot admit an unvalidated commit — the broken
# expression errors the run, and actionlint (a required merge_group check this
# verifier already enforces) rejects it (verified exit 1). It is the same class
# as the duplicate-key / exotic-encoding residual below; the encoding-proof fix
# is a YAML-faithful parse, deferred to avoid a new runtime dependency for a
# liveness-only hardening.


MERGIFY_PROOF_PR_BASE_HEAD_REF_PREFIX = "mergify/merge-queue/"
MERGIFY_PROOF_PR_TRANSIENT_HEAD_REF_PREFIX = (
    f"{MERGIFY_TEMP_PR_TRANSIENT_PREFIX}{MERGIFY_PROOF_PR_BASE_HEAD_REF_PREFIX}"
)
MERGIFY_PROOF_PR_HEAD_REF_PREDICATE = (
    f"(startsWith(github.event.pull_request.head.ref, '{MERGIFY_PROOF_PR_BASE_HEAD_REF_PREFIX}') "
    f"|| startsWith(github.event.pull_request.head.ref, '{MERGIFY_PROOF_PR_TRANSIENT_HEAD_REF_PREFIX}'))"
)
MERGIFY_PROOF_PR_CANCEL_GUARD = f"!{MERGIFY_PROOF_PR_HEAD_REF_PREDICATE}"
MERGIFY_PROOF_PR_GROUP_TOKEN = "mergify-proof"

# Shared predicates used by advisory jobs only. Merge-readiness progress waits
# for required proof gates, so it is limited to boundary proof PRs. The
# coverage-enforcer is event-aware and must not reuse that boundary-only gate.
MERGIFY_PROOF_PR_READY_PREDICATE = (
    "github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false "
    "&& " + MERGIFY_PROOF_PR_HEAD_REF_PREDICATE
)
MERGIFY_PROOF_PR_METADATA_ONLY_EDIT_PREDICATE = (
    "github.event.action == 'edited' "
    "&& !(github.event.changes.base.ref.from && true || false)"
)
MERGIFY_PROOF_PR_HEAD_SHA = "github.event.pull_request.head.sha"

EXPECTED_MERGE_READINESS_PROGRESS_IF = (
    "${{ " + MERGIFY_PROOF_PR_READY_PREDICATE + " "
    "&& !(" + MERGIFY_PROOF_PR_METADATA_ONLY_EDIT_PREDICATE + ") }}"
)

EXPECTED_COVERAGE_ENFORCER_IF = ""

EXPECTED_COVERAGE_ENFORCER_PERMISSIONS = {
    "checks": "read",
    "contents": "read",
    "pull-requests": "read",
}

EXPECTED_COVERAGE_ENFORCER_CHECKOUT_WITH = {
    "ref": "${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}",
    "persist-credentials": "false",
}

EXPECTED_COVERAGE_ENFORCER_SETUP_PYTHON_WITH = {
    "python-version": "3.12",
}

EXPECTED_COVERAGE_ENFORCER_ENV = {
    "GITHUB_TOKEN": "${{ github.token }}",
    "GITHUB_EVENT_PATH": "${{ github.event_path }}",
    "GITHUB_REPOSITORY": "${{ github.repository }}",
}

EXPECTED_COVERAGE_ENFORCER_RUN_BODY = (
    "if [ ! -f scripts/coverage_enforcer.py ]; then",
    '  echo "coverage-enforcer bootstrap fail-closed: trusted base tree lacks scripts/coverage_enforcer.py"',
    "  exit 1",
    "fi",
    'if ! grep -q "def expected_registry_checks_for_policy" scripts/coverage_enforcer.py; then',
    '  echo "coverage-enforcer bootstrap fail-closed: trusted base tree lacks event-aware scripts/coverage_enforcer.py"',
    "  exit 1",
    "fi",
    "python3 scripts/coverage_enforcer.py",
)


MERGE_GROUP_SAFE_GROUP_FORMS = frozenset({
    # .github/workflows/ci.yml — merge_group arm format('mq-{0}', github.ref)
    # wins under merge_group (the PR/workflow_dispatch arms are false then), before
    # the per-ref/sha fallback; PR-draft-deferral arms are gated off merge_group.
    "group: >- ${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha) "
    "|| github.event_name == 'pull_request' && github.event.pull_request.draft == true "
    "&& contains(fromJSON('[\"opened\",\"synchronize\",\"reopened\",\"converted_to_draft\",\"edited\"]'), github.event.action) "
    "&& format('pr-{0}-deferred', github.event.number) || github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false && (github.event.action == 'reopened' "
    "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false))) "
    "&& format('pr-{0}-noop', github.event.number) || github.event_name == 'pull_request' "
    "&& format('pr-{0}-full', github.event.number) || github.event_name == 'workflow_dispatch' "
    "&& format('{0}-dispatch-iteration', github.ref_name) "
    "|| github.event_name == 'merge_group' "
    "&& format('mq-{0}', github.ref) || format('{0}-{1}', github.ref_name, github.sha) }}",
    # .github/workflows/actionlint.yml — simpler prefixed shape, same merge_group
    # arm before the per-ref/sha fallback.
    "group: >- actionlint-${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha) "
    "|| github.event_name == 'pull_request' && format('pr-{0}', github.event.number) "
    "|| github.event_name == 'merge_group' && format('mq-{0}', github.ref) "
    "|| format('{0}-{1}', github.ref_name, github.sha) }}",
    # .github/workflows/backtester-ci.yml — same draft/full PR split as ci.yml
    # with a backtester-prefixed merge_group arm before the per-ref/sha fallback.
    "group: >- ${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('bvs-pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha) "
    "|| github.event_name == 'pull_request' && github.event.pull_request.draft == true "
    "&& contains(fromJSON('[\"opened\",\"synchronize\",\"reopened\",\"converted_to_draft\",\"edited\"]'), github.event.action) "
    "&& format('bvs-pr-{0}-deferred', github.event.number) || github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false && (github.event.action == 'reopened' "
    "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false))) "
    "&& format('bvs-pr-{0}-noop', github.event.number) || github.event_name == 'pull_request' "
    "&& format('bvs-pr-{0}-full', github.event.number) || github.event_name == 'workflow_dispatch' "
    "&& format('bvs-{0}-dispatch-iteration', github.ref_name) "
    "|| github.event_name == 'merge_group' && format('bvs-mq-{0}', github.ref) "
    "|| format('bvs-{0}-{1}', github.ref_name, github.sha) }}",
    # .github/workflows/coverage-enforcer.yml — coverage-specific namespace with
    # the same PR class split as ci.yml and a merge_group arm before fallback.
    "group: >- coverage-${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha) "
    "|| github.event_name == 'pull_request' && github.event.pull_request.draft == true "
    "&& contains(fromJSON('[\"opened\",\"synchronize\",\"reopened\",\"converted_to_draft\",\"edited\"]'), github.event.action) "
    "&& format('pr-{0}-deferred', github.event.number) || github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false && (github.event.action == 'reopened' "
    "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false))) "
    "&& format('pr-{0}-noop', github.event.number) || github.event_name == 'pull_request' "
    "&& format('pr-{0}-full', github.event.number) || github.event_name == 'merge_group' "
    "&& format('mq-{0}', github.ref) || format('{0}-{1}', github.ref_name, github.sha) }}",
})

# cancel-in-progress is fail-closed for merge_group only when it is provably
# false for the merge_group event. A bare substring check missed `true` and
# negations (`!= 'push'`, `!startsWith(github.ref, ...)`) that evaluate true for
# the queue ref and so cancel a queue validation. A positive allowlist — the
# literal false, or solely pull_request/workflow_dispatch equality arms — is the
# only form we can prove never cancels a merge_group run.


def concurrency_group_and_cancel(workflow_text: str) -> tuple[str, str] | None:
    """Split a workflow's top-level concurrency block into (group_text,
    cancel_text). group_text is the group expression joined to one line;
    cancel_text is the cancel-in-progress expression. Returns None when no
    concurrency block is defined. Shared by verify_pr_concurrency and
    verify_merge_group_concurrency so both read the block identically.

    Lines are bucketed by which key (group: / cancel-in-progress:) they fall
    under, not by their order, so a block that writes cancel-in-progress before
    group still splits correctly (YAML allows either order). Splitting by first
    cancel-in-progress occurrence would misclassify the whole group expression
    as cancel text and emit a misleading diagnostic."""
    block = top_level_block(workflow_text, "concurrency")
    if not block:
        return None
    group_lines: list[str] = []
    cancel_lines: list[str] = []
    current: list[str] | None = None
    for line in block:
        stripped = line.strip()
        if stripped.startswith("group:"):
            current = group_lines
        elif stripped.startswith("cancel-in-progress:"):
            current = cancel_lines
        if current is not None:
            current.append(line)
    group_text = " ".join(line.strip() for line in group_lines if line.strip())
    cancel_text = "\n".join(cancel_lines)
    return group_text, cancel_text


def merge_group_concurrency_errors(group_text: str, cancel_text: str) -> list[str]:
    """Assert the concurrency block isolates merge_group queue validations on
    their own ref and never cancels them. Both checks are fail-closed:

    * Group: the group expression (whitespace-normalized) must be one of the
      approved MERGE_GROUP_SAFE_GROUP_FORMS. This is a positive allowlist, not an
      analysis of the expression — GitHub's expression language defeats every
      regex contract (an unkeyed arm can select merge_group by unbounded
      syntaxes, `&&`/gate text can hide in string literals, github.ref can be
      buried in a constant-collapsing function, and a duplicate `group:` key
      escapes single-expression parsing). Matching a known-safe form has no
      analysis surface to bypass; an unrecognized form fails closed.
    * Cancel: cancel-in-progress must be provably false for merge_group (a
      positive pull_request/workflow_dispatch allowlist, not a 'merge_group'
      deny-list which missed `true` and negations).

    Either gap would let two queue entries share a concurrency group and cancel
    each other, dropping a required-check report and blocking or corrupting the
    merge."""
    errors: list[str] = []
    if _normalize_concurrency_text(group_text) not in MERGE_GROUP_SAFE_GROUP_FORMS:
        errors.append(
            "concurrency group must exactly match an approved merge_group-safe form "
            "(add the normalized group expression to MERGE_GROUP_SAFE_GROUP_FORMS after review)"
        )
    if not cancel_in_progress_is_merge_group_safe(cancel_text):
        errors.append("cancel-in-progress must not cancel merge_group queue validations")
    return errors


def mergify_proof_pr_concurrency_errors(
    group_text: str,
    cancel_text: str,
    *,
    head_ref_predicate: str = MERGIFY_PROOF_PR_HEAD_REF_PREDICATE,
    cancel_guard: str = MERGIFY_PROOF_PR_CANCEL_GUARD,
) -> list[str]:
    errors: list[str] = []
    normalized_group = _normalize_concurrency_text(group_text)
    normalized_cancel = _normalize_concurrency_text(cancel_text)
    if (
        head_ref_predicate not in normalized_group
        or MERGIFY_PROOF_PR_GROUP_TOKEN not in normalized_group
        or MERGIFY_PROOF_PR_HEAD_SHA not in normalized_group
    ):
        errors.append("concurrency group must isolate Mergify proof PR runs")
    if "github.run_id" in normalized_group:
        errors.append("Mergify proof PR concurrency group must key on head SHA, not run_id")
    # Split on event arms, not every `||`: the Mergify head-ref predicate itself
    # contains an inner OR between stable and transient queue prefixes.
    group_arms = re.split(r"\s+\|\|\s+github\.event_name\b", normalized_group)
    if any(
        head_ref_predicate in arm
        and MERGIFY_PROOF_PR_METADATA_ONLY_EDIT_PREDICATE in arm
        for arm in group_arms
    ):
        errors.append("queue-branch metadata-only edits must use the Mergify proof group")
    if cancel_guard not in normalized_cancel:
        errors.append("cancel-in-progress must not cancel Mergify proof PR validations")
    return errors


def _workflow_proof_pr_prefixes() -> frozenset[str]:
    """The head-ref prefixes the workflow concurrency layer isolates, extracted from
    MERGIFY_PROOF_PR_HEAD_REF_PREDICATE: startsWith(<ref>, '<prefix>')."""
    prefixes = frozenset(re.findall(r"startsWith\([^,]+,\s*'([^']*)'\)", MERGIFY_PROOF_PR_HEAD_REF_PREDICATE))
    if not prefixes:
        raise ProvenanceError(
            "MERGIFY_PROOF_PR_HEAD_REF_PREDICATE must embed at least one startsWith(ref, '<prefix>') literal"
        )
    return prefixes


def mergify_proof_prefix_alignment_errors(config: ProvenanceConfig) -> list[str]:
    """Fail loud if the CI policy resolver and the workflow concurrency layer ever
    disagree on which head-ref prefixes mark a Mergify proof PR.

    The workflow isolates a proof-PR run (own per-run concurrency group, never
    cancelled) when the head ref satisfies one of the documented Mergify proof-PR
    prefixes. The resolver must promote exactly that same set for the bound actor.
    If either layer handles a form the other does not, the required gate can be
    skipped or cancelled and the queue can deadlock."""
    errors: list[str] = []
    workflow_prefixes = _workflow_proof_pr_prefixes()
    resolver_prefixes = frozenset(
        {
            config.mergify_temp_pr_head_ref_prefix,
            f"{MERGIFY_TEMP_PR_TRANSIENT_PREFIX}{config.mergify_temp_pr_head_ref_prefix}",
        }
    )
    if workflow_prefixes != resolver_prefixes:
        errors.append(
            "mergify proof-PR prefix drift: resolver prefixes "
            f"{sorted(resolver_prefixes)!r} must equal workflow concurrency predicate prefixes "
            f"{sorted(workflow_prefixes)!r}"
        )
    actor_id = config.mergify_temp_pr_actor_id
    suffix = "83d4b0be7e"
    expected_head_refs = {prefix: prefix + suffix for prefix in resolver_prefixes}
    resolver_promoted_prefixes: set[str] = set()
    workflow_isolated_prefixes: set[str] = set()
    for prefix, head_ref in expected_head_refs.items():
        workflow_isolates = any(head_ref.startswith(workflow_prefix) for workflow_prefix in workflow_prefixes)
        resolver_promotes = mergify_temp_pr_matches(
            event_name="pull_request",
            event_action="opened",
            pull_request_draft=True,
            pull_request_head_ref=head_ref,
            temp_pr_head_ref_prefix=config.mergify_temp_pr_head_ref_prefix,
            event_sender_id=actor_id,
            temp_pr_actor_id=actor_id,
        )
        if resolver_promotes:
            resolver_promoted_prefixes.add(prefix)
        if workflow_isolates:
            workflow_isolated_prefixes.add(prefix)
        if resolver_promotes and not workflow_isolates:
            errors.append(
                "mergify proof-PR drift: resolver promotes head ref "
                f"{head_ref!r} that the workflow concurrency layer does not isolate (it would land "
                "in the cancellable group and deadlock the queue)"
            )
        if workflow_isolates and not resolver_promotes:
            errors.append(
                "mergify proof-PR drift: workflow concurrency layer isolates head ref "
                f"{head_ref!r} that the resolver does not promote (the required gate would not report)"
            )
    unrelated_ref = "feature/x"
    unrelated_workflow_isolates = any(
        unrelated_ref.startswith(workflow_prefix) for workflow_prefix in workflow_prefixes
    )
    unrelated_resolver_promotes = mergify_temp_pr_matches(
        event_name="pull_request",
        event_action="opened",
        pull_request_draft=True,
        pull_request_head_ref=unrelated_ref,
        temp_pr_head_ref_prefix=config.mergify_temp_pr_head_ref_prefix,
        event_sender_id=actor_id,
        temp_pr_actor_id=actor_id,
    )
    if unrelated_workflow_isolates or unrelated_resolver_promotes:
        errors.append(
            "mergify proof-PR drift: unrelated head ref "
            f"{unrelated_ref!r} must be neither workflow-isolated nor resolver-promoted"
        )
    if resolver_promoted_prefixes != workflow_isolated_prefixes:
        errors.append(
            "mergify proof-PR prefix drift: resolver-promoted prefixes "
            f"{sorted(resolver_promoted_prefixes)!r} must equal workflow-isolated prefixes "
            f"{sorted(workflow_isolated_prefixes)!r}"
        )
    return errors


def jobs_with_job_level_concurrency(workflow_text: str) -> list[str]:
    """Job ids that define a job-level `concurrency:` key. GitHub evaluates
    job-level concurrency in addition to the workflow-level block, so a required
    merge_group job under a shared/cancelling job-level group collapses queue
    entries even when the workflow-level block is safe — and nothing else in this
    verifier inspects the job level.

    parse_jobs preserves each body line's indentation. A job-level key sits at
    the shallowest indentation in the job body; deeper lines are step/run content
    (which may legitimately contain the word `concurrency:`). Match `concurrency:`
    only at that shallowest key indentation so run-block text is not misread."""
    result: list[str] = []
    for job_id, lines in parse_jobs(workflow_text).items():
        body = [line for line in lines if line.strip()]
        if not body:
            continue
        key_indent = min(len(line) - len(line.lstrip()) for line in body)
        for line in body:
            indent = len(line) - len(line.lstrip())
            if indent == key_indent and line.strip().startswith("concurrency:"):
                result.append(job_id)
                break
    return result


def merge_group_concurrency_workflow_errors(workflow_text: str) -> list[str]:
    """Fail-closed merge_group check that needs the whole workflow, not just the
    extracted top-level group/cancel text: job-level `concurrency:`.

    GitHub evaluates job-level concurrency in addition to the workflow-level
    block, so a shared/cancelling job-level group on a required merge_group job
    collapses queue entries even when the workflow-level group is allowlist-safe —
    and nothing else in this verifier (nor actionlint) inspects the job level
    (verified: actionlint exits 0 on a job-level shared/cancelling group). The
    group allowlist's fail-closed property does not reach this layer, so the check
    must live here. Realistic drift — a block or flow-style job-level concurrency
    key — is rejected outright.

    Two related divergences are deliberately NOT re-implemented here:

    * Duplicate top-level `concurrency:` keys (GitHub resolves last-wins, so a
      first-match line scan could bless the discarded block). actionlint — a
      required merge_group check this verifier already enforces — rejects
      duplicate keys in every form (block, flow, and quoted: verified exit 1), so
      detecting them here too would be incomplete (quoted/anchored YAML key forms
      defeat any line scan) and duplicate logic actionlint already owns
      completely. Single source of truth: actionlint owns duplicate-key
      detection.
    * Exotic YAML key encodings of a job-level concurrency key (a quoted
      `"concurrency":`, or a fully flow-style job) are out of this textual
      checker's scope; parse_jobs is a strict-subset parser and such forms break
      its other required-job checks first.

    Scope of the residual: a concurrency misconfiguration that slips past every
    check disrupts queue LIVENESS (queue entries cancel each other, a required
    check reports cancelled, and that entry's merge is blocked/requeued). It does
    NOT admit an unvalidated commit — the merge_group heavy CI on the exact
    to-be-merged commit is the safety gate. The complete, encoding-proof fix is a
    YAML-faithful parse of the resolved concurrency structure; it is tracked as
    follow-up rather than carried here to avoid a new runtime dependency for a
    liveness-only hardening."""
    errors: list[str] = []
    for job_id in jobs_with_job_level_concurrency(workflow_text):
        errors.append(
            f"job '{job_id}' must not define job-level concurrency in a merge_group "
            "workflow (job-level concurrency is evaluated in addition to the "
            "workflow-level block and bypasses the merge_group isolation check; "
            "use workflow-level concurrency only)"
        )
    return errors


def verify_merge_group_concurrency(workflow_text: str) -> list[str]:
    """Standalone merge_group concurrency check for required-check workflows that
    do not use ci.yml's full PR-deferral concurrency shape (e.g. actionlint.yml),
    so they get the same fail-closed merge_group isolation as ci.yml."""
    split = concurrency_group_and_cancel(workflow_text)
    if split is None:
        return ["workflow must define concurrency for merge_group isolation"]
    group_text, cancel_text = split
    errors = merge_group_concurrency_errors(group_text, cancel_text)
    errors.extend(mergify_proof_pr_concurrency_errors(group_text, cancel_text))
    errors.extend(merge_group_concurrency_workflow_errors(workflow_text))
    return errors


def verify_pr_concurrency(workflow_text: str) -> list[str]:
    split = concurrency_group_and_cancel(workflow_text)
    if split is None:
        return ["workflow must define PR-only concurrency"]
    group_text, cancel_text = split
    errors: list[str] = []
    if not PR_CONCURRENCY_EVENT_RE.search(group_text):
        errors.append("concurrency group must branch on pull_request event")
    if "needs." in group_text or "needs." in cancel_text:
        errors.append("workflow-level concurrency must not reference job outputs")
    normalized_group = _normalize_concurrency_text(group_text)
    normalized_cancel = _normalize_concurrency_text(cancel_text)
    if "pr-{0}-deferred" not in group_text or "pr-{0}-full" not in group_text:
        errors.append("concurrency group must split deferred PR runs from full CI runs")
    if "pr-{0}-noop" not in group_text:
        errors.append("concurrency group must split noop PR runs from full CI runs")
    if READY_PR_NOOP_EXPR not in normalized_group:
        errors.append("concurrency group must use the canonical ready PR noop predicate")
    if "dispatch-iteration" not in group_text:
        errors.append("workflow_dispatch runs must use the iteration concurrency group")
    if "github.event.inputs.full_ci" in group_text or "dispatch-full" in group_text:
        errors.append("workflow_dispatch runs must not define a full-CI concurrency group")
    if not PR_CONCURRENCY_NON_PR_FALLBACK_RE.search(group_text):
        errors.append("concurrency group must keep non-PR runs isolated by ref and SHA")
    cancel_has_pull_request = (
        "github.event_name == 'pull_request'" in cancel_text
        or 'github.event_name == "pull_request"' in cancel_text
    )
    cancel_has_dispatch = (
        "github.event_name == 'workflow_dispatch'" in cancel_text
        or 'github.event_name == "workflow_dispatch"' in cancel_text
    )
    if not cancel_has_pull_request or not cancel_has_dispatch:
        errors.append(PR_CONCURRENCY_CANCEL_SCOPE_ERROR)
    elif READY_PR_NOOP_EXPR not in normalized_cancel or "!(" not in normalized_cancel:
        errors.append("cancel-in-progress must not cancel noop PR runs")
    if (
        "github.event_name == 'push'" in cancel_text
        or 'github.event_name == "push"' in cancel_text
        or "refs/tags" in cancel_text
        or "startsWith(github.ref" in cancel_text
    ):
        errors.append("cancel-in-progress must not cancel push, tag, or deploy flows")
    errors.extend(merge_group_concurrency_errors(group_text, cancel_text))
    errors.extend(mergify_proof_pr_concurrency_errors(group_text, cancel_text))
    errors.extend(merge_group_concurrency_workflow_errors(workflow_text))
    return errors


def evaluate_ci_policy(
    policy: dict[str, object],
    gate_names: dict[str, str],
    *,
    event_name: str,
    action: str,
    pull_request_draft: bool,
    pull_request_head_ref: str = "",
    pull_request_base_changed: bool = False,
    mergify_temp_pr_head_ref_prefix: str = "",
    mergify_temp_pr_actor_id: int = -1,
    event_sender_id: int = -1,
    pull_request_author_id: int = -1,
    ref: str,
) -> CiPolicyResult:
    override = policy.get("override")
    force_full_ci = isinstance(override, dict) and override.get("force_full_ci") is True
    # Queue-only rework (#981): the runtime resolver now reads
    # config.mergify_temp_pr_actor_id and an event_sender_id to bind the mergify temp
    # PR to its actor. This static mirror delegates to that same resolver, so it must
    # supply the bound actor id (or a sentinel that never matches a real sender) and
    # thread the sender id through, or it would crash on the missing attribute.
    config = type(
        "StaticPolicyConfig",
        (),
        {
            "policy": {key: str(value) for key, value in policy.items() if key != "override"},
            "gate_names": dict(gate_names),
            "mergify_temp_pr_head_ref_prefix": mergify_temp_pr_head_ref_prefix,
            "mergify_temp_pr_actor_id": mergify_temp_pr_actor_id,
            "force_full_ci": force_full_ci,
        },
    )()
    try:
        result = provenance_evaluate_ci_policy(
            config,
            event_name=event_name,
            event_action=action,
            pull_request_draft=pull_request_draft,
            pull_request_head_ref=pull_request_head_ref,
            pull_request_base_changed=pull_request_base_changed,
            docs_only=False,
            event_sender_id=event_sender_id,
            pull_request_author_id=pull_request_author_id,
            ref=ref,
        )
    except ProvenanceError as exc:
        raise ValueError(str(exc)) from exc
    return CiPolicyResult(
        ci_policy_path=result.ci_policy_path,
        full_ci_required=result.full_ci_required,
        full_ci_deferred=result.full_ci_deferred,
        gate_name=result.gate_name,
        backtester_gate_name=result.backtester_gate_name,
        expected_event_class=result.expected_event_class,
        reason=result.reason,
    )


def policy_row_is_proof_affecting(semantics: PolicyRowSemantics) -> bool:
    return semantics.mergeable_without_queue and (
        semantics.changes_head_sha
        or semantics.changes_base
        or semantics.changes_target
        or semantics.changes_queue_origin
        or semantics.changes_required_context
    )


def policy_proof_invariant_errors(policy: dict[str, object]) -> list[str]:
    errors: list[str] = []
    missing_semantics = sorted(set(CI_PROVENANCE_POLICY_ROWS) - set(CI_POLICY_ROW_SEMANTICS))
    if missing_semantics:
        errors.append(
            "ci_provenance.policy rows must define proof-affecting semantics: "
            + ", ".join(missing_semantics)
        )
    for row in CI_PROVENANCE_POLICY_ROWS:
        if row not in CI_POLICY_ROW_SEMANTICS:
            continue
        value = policy.get(row)
        semantics = CI_POLICY_ROW_SEMANTICS[row]
        if semantics.queue_covered and value == "iteration":
            continue
        if row == "tag":
            if value != "tag_reuse":
                errors.append("ci_provenance.policy.tag is proof-affecting and must be tag_reuse")
            continue
        if policy_row_is_proof_affecting(semantics) and value != "full":
            errors.append(
                f"ci_provenance.policy.{row} is proof-affecting and must be full "
                "or queue-covered iteration"
            )
    return errors


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


def parse_inline_yaml_list(value: str) -> set[str]:
    stripped = value.strip()
    if not (stripped.startswith("[") and stripped.endswith("]")):
        return set()
    return {item.strip().strip("'\"") for item in stripped[1:-1].split(",") if item.strip()}


def workflow_pull_request_types(workflow_text: str) -> set[str]:
    block = workflow_trigger_block(workflow_text, "pull_request")
    types: set[str] = set()
    for index, line in enumerate(block):
        stripped = line.strip()
        if stripped.startswith("types:"):
            after = stripped.split(":", 1)[1].strip()
            types.update(parse_inline_yaml_list(after))
            for child in block[index + 1 :]:
                child_stripped = child.strip()
                if not child_stripped.startswith("- "):
                    break
                types.add(child_stripped.removeprefix("- ").strip().strip("'\""))
    return types


def workflow_pull_request_type_errors(
    workflow_text: str,
    required_types: tuple[str, ...] = ("ready_for_review", "converted_to_draft", "edited"),
) -> list[str]:
    types = workflow_pull_request_types(workflow_text)
    errors: list[str] = []
    for required_type in required_types:
        if required_type not in types:
            errors.append(f"pull_request types must include {required_type}")
    return errors


CI_POLICY_SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "(", "{", ")", "}"}
PYTHON3_EXECUTABLE_RE = re.compile(r"^python3(?:\.\d+)?$")




def token_assigns_event_sender_id(token: str) -> bool:
    return shell_assignment_name(token) == "EVENT_SENDER_ID"


def token_executable_name(token: str) -> str:
    return pathlib.Path(token).name


def token_is_python3_executable(token: str) -> bool:
    return PYTHON3_EXECUTABLE_RE.fullmatch(token_executable_name(token)) is not None


def command_segments(tokens: list[str]) -> list[list[str]]:
    segments: list[list[str]] = []
    current: list[str] = []
    for token in tokens:
        if token in CI_POLICY_SHELL_COMMAND_BOUNDARIES:
            if current:
                segments.append(current)
                current = []
            continue
        current.append(token)
    if current:
        segments.append(current)
    return segments


def ci_policy_resolver_command_index(segment: list[str]) -> int | None:
    index = consume_assignment_words(segment, 0)
    while index < len(segment) and token_executable_name(segment[index]) == "env":
        index = env_command_prefix_index(segment, index + 1)
        if index is None:
            return None
    if index + 2 >= len(segment):
        return None
    if not token_is_python3_executable(segment[index]) or segment[index + 2] != "ci-policy":
        return None
    script = segment[index + 1]
    if script != "$policy_script" and not script.endswith("/ci_provenance.py") and script != "scripts/ci_provenance.py":
        return None
    return index


def command_passes_event_sender_id_arg(tokens: list[str]) -> bool:
    for index, token in enumerate(tokens):
        if token.startswith("--event-sender-id"):
            return True
        candidate = token
        for continuation in tokens[index + 1 : index + 5]:
            candidate += continuation
            if candidate.startswith("--event-sender-id"):
                return True
            if not "--event-sender-id".startswith(candidate):
                break
    return False


def segment_overrides_event_sender_id_inline(segment: list[str], command_index: int) -> bool:
    return any(token_assigns_event_sender_id(token) for token in segment[:command_index])


def segment_persists_event_sender_id_override(segment: list[str]) -> bool:
    if any(token in CI_POLICY_SHELL_COMMAND_BOUNDARIES for token in segment):
        return False
    assignment_index = consume_assignment_words(segment, 0)
    if assignment_index == len(segment):
        return any(token_assigns_event_sender_id(token) for token in segment)
    if assignment_index < len(segment) and token_executable_name(segment[assignment_index]) == "export":
        return any(token_assigns_event_sender_id(token) for token in segment[assignment_index + 1 :])
    return False


def yaml_structural_key_count(lines: list[str], key: str) -> int:
    count = 0
    skip_scalar_indent: int | None = None
    key_re = re.compile(rf"^\s*{re.escape(key)}\s*:")
    for line in lines:
        clean = strip_comment(line).rstrip()
        if skip_scalar_indent is not None:
            if not clean.strip():
                continue
            indent = len(clean) - len(clean.lstrip(" "))
            if indent > skip_scalar_indent:
                continue
            skip_scalar_indent = None
        run_match = YAML_RUN_LINE_RE.match(clean)
        if run_match is not None and run_match.group(2).strip().startswith(("|", ">")):
            skip_scalar_indent = len(run_match.group(1))
            continue
        if key_re.match(clean):
            count += 1
    return count


def ci_policy_event_sender_command_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    if yaml_structural_key_count(job_lines, "EVENT_SENDER_ID") > 1:
        errors.append("ci-policy must declare EVENT_SENDER_ID env exactly once")
    # Defense-in-depth only: same-repo PRs control their workflow run blocks, so
    # sender id hygiene is not an unspoofable trust boundary. The merge boundary
    # remains trusted-base check-ci-gate plus branch-protection sp-reviewer approval;
    # this tokenized check blocks known command-level injections.
    for block in step_blocks(job_lines):
        event_sender_id_overridden = False
        tokens = command_tokens_with_line_boundaries(block_run_body(block))
        for segment in command_segments(tokens):
            command_index = ci_policy_resolver_command_index(segment)
            if command_index is not None and segment_overrides_event_sender_id_inline(segment, command_index):
                errors.append("ci-policy must not override EVENT_SENDER_ID inline on the resolver command line")
            if command_index is not None and event_sender_id_overridden:
                errors.append("ci-policy must not override EVENT_SENDER_ID before the resolver command")
            if command_index is not None:
                if command_passes_event_sender_id_arg(segment[command_index + 3 :]):
                    errors.append("ci-policy must not pass --event-sender-id on the resolver command line")
                continue
            if segment_persists_event_sender_id_override(segment):
                event_sender_id_overridden = True
    return errors


def ci_policy_job_errors(job_lines: list[str]) -> list[str]:
    text = uncommented_text(job_lines)
    errors: list[str] = []
    for output in (
        "ci_policy_path",
        "full_ci_required",
        "full_ci_deferred",
        "gate_name",
        "backtester_gate_name",
        "expected_event_class",
        "reason",
        "ignore_emit_failure",
    ):
        if f"{output}: ${{{{ steps.policy.outputs.{output} }}}}" not in text:
            errors.append(f"ci-policy must expose {output}")
    if 'tee -a "$GITHUB_OUTPUT"' not in text:
        errors.append("ci-policy must write script output to GITHUB_OUTPUT")
    if (
        CI_CLASSIFICATION_SUMMARY_LINE not in text
        or 'class="heavy proof"' not in text
        or 'class="promoted-cheap"' not in text
        or 'class="iteration lane"' not in text
    ):
        errors.append("ci-policy must summarize CI classification")
    for required in (
        "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
        "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
        'git check-ref-format "refs/heads/$base_branch"',
        "git archive \"$base_ref\" scripts/ ci/github-actions-runners.toml",
        "steps.policy_base.outputs.script",
        'python3 "$policy_script" ci-policy',
    ):
        if required not in text:
            errors.append(f"ci-policy must run ci_provenance.py ci-policy from trusted base tree ({required})")
    if '--event-name "${{ github.event_name }}"' not in text:
        errors.append("ci-policy must pass github.event_name")
    if '--event-action "${{ github.event.action || \'\' }}"' not in text:
        errors.append("ci-policy must pass github.event.action")
    if '--pull-request-draft "${{ github.event.pull_request.draft || false }}"' not in text:
        errors.append("ci-policy must pass pull_request draft state")
    if "PR_HEAD_REF: ${{ github.event.pull_request.head.ref || '' }}" not in text:
        errors.append("ci-policy must pass pull_request head ref through an env var")
    if '--pull-request-head-ref "$PR_HEAD_REF"' not in text:
        errors.append("ci-policy must pass pull_request head ref")
    if "PR_AUTHOR_ID: ${{ github.event.pull_request.user.id || '' }}" not in text:
        errors.append("ci-policy must pass pull_request author id through an env var")
    for required in (
        "author_args=()",
        'python3 "$policy_script" ci-policy --help | grep -q -- "--pull-request-author-id"',
        'author_args=(--pull-request-author-id "$PR_AUTHOR_ID")',
        '"${author_args[@]}"',
    ):
        if required not in text:
            errors.append(f"ci-policy must feature-detect pull_request author id support ({required})")
    if f'--pull-request-base-changed "${{{{ {PR_BASE_CHANGED_EXPR} }}}}"' not in text:
        errors.append("ci-policy must pass pull_request base-change state")
    if "--workflow-dispatch-full-ci" in text or "github.event.inputs.full_ci" in text:
        errors.append("ci-policy must not pass workflow_dispatch full_ci input")
    if "--docs-only" not in text and "name: ci-policy" in text:
        errors.append("ci-policy must pass detector docs_only output")
    if '--ref "${{ github.ref }}"' not in text:
        errors.append("ci-policy must pass github.ref")
    if "EVENT_SENDER_ID: ${{ github.event.sender.id }}" not in text:
        errors.append("ci-policy must set EVENT_SENDER_ID env for the mergify actor binding")
    errors.extend(ci_policy_event_sender_command_errors(job_lines))
    return errors


def job_header_indent_errors(workflow_text: str) -> list[str]:
    errors: list[str] = []
    required_job_re = re.compile(rf"^(?P<indent>\s+)({'|'.join(re.escape(job) for job in REQUIRED_JOBS)}):\s*$")
    in_jobs = False

    for line in workflow_text.splitlines():
        clean = strip_comment(line)
        if clean == "jobs:":
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if clean and not clean.startswith((" ", "\t")):
            break
        match = required_job_re.match(clean)
        if match and match.group("indent") != "  ":
            job = clean.strip()[:-1]
            errors.append(f"job {job} must use two-space top-level indentation")

    return errors


def workflow_steps_alias_errors(workflow_text: str) -> list[str]:
    in_steps = False
    steps_indent: int | None = None
    for line in workflow_text.splitlines():
        clean = strip_comment(line)
        stripped = clean.lstrip()
        if not stripped:
            continue
        indent = len(clean) - len(stripped)
        if re.match(r"^\s*steps:\s*\*[A-Za-z0-9_.-]+\s*$", clean):
            return ["workflow steps must be explicit; YAML steps aliases are unsupported"]
        if re.match(r"^\s*steps:\s*$", clean):
            in_steps = True
            steps_indent = indent
            continue
        if in_steps and steps_indent is not None:
            is_item = stripped.startswith("-")
            if indent <= steps_indent and not (indent == steps_indent and is_item):
                in_steps = False
                steps_indent = None
                continue
            if re.match(r"^-\s*\*[A-Za-z0-9_.-]+\s*$", stripped):
                return ["workflow steps must be explicit; YAML steps aliases are unsupported"]
    return []


def parse_inline_needs(value: str) -> set[str]:
    value = value.strip()
    if not value:
        return set()
    if value.startswith("[") and value.endswith("]"):
        return {part.strip().strip("'\"") for part in value[1:-1].split(",") if part.strip()}
    return {value.strip().strip("'\"")}


def extract_needs(job_lines: list[str]) -> set[str]:
    needs: set[str] = set()
    index = 0
    while index < len(job_lines):
        clean = strip_comment(job_lines[index])
        match = re.match(r"^    needs:\s*(.*)$", clean)
        if not match:
            index += 1
            continue
        rest = match.group(1).strip()
        if rest:
            needs.update(parse_inline_needs(rest))
            index += 1
            continue
        index += 1
        while index < len(job_lines):
            nested = strip_comment(job_lines[index])
            if re.match(r"^    [A-Za-z0-9_.-]+:", nested):
                break
            item = re.match(r"^\s*-\s*([A-Za-z0-9_.-]+)\s*$", nested)
            if item:
                needs.add(item.group(1))
            index += 1
    return needs


def step_blocks(job_lines: list[str]) -> list[list[str]]:
    blocks: list[list[str]] = []
    current: list[str] | None = None
    in_steps = False
    steps_indent: int | None = None
    step_indent: int | None = None

    for line in job_lines:
        clean = strip_comment(line)
        stripped = clean.lstrip()
        if not in_steps:
            if re.match(r"^\s*steps:\s*$", clean):
                in_steps = True
                steps_indent = len(clean) - len(stripped)
            continue
        if not stripped:
            if current is not None:
                current.append(line)
            continue
        indent = len(clean) - len(stripped)
        is_step_item = YAML_STEP_ITEM_RE.match(stripped) is not None
        if steps_indent is not None and indent <= steps_indent and not (
            indent == steps_indent and is_step_item
        ):
            break
        if step_indent is None and is_step_item:
            step_indent = indent
        if step_indent is not None and indent == step_indent and is_step_item:
            if current is not None:
                blocks.append(current)
            current = [line]
            continue
        if current is not None:
            current.append(line)
    if current is not None:
        blocks.append(current)
    return blocks


def setup_action_blocks(job_lines: list[str]) -> list[list[str]]:
    return [block for block in step_blocks(job_lines) if any("./.github/actions/setup-environment" in line for line in block)]


def line_uses_action(line: str, action: str) -> bool:
    match = re.match(r"^\s*(?:-\s*)?uses:\s*(['\"]?)(?P<value>[^'\"\s#]+)", strip_comment(line))
    return match is not None and match.group("value").startswith(action)


def action_blocks(job_lines: list[str], action: str) -> list[list[str]]:
    return [block for block in step_blocks(job_lines) if any(line_uses_action(line, action) for line in block)]


def upload_artifact_pin_errors(job_lines: list[str]) -> list[str]:
    for block in action_blocks(job_lines, "actions/upload-artifact@"):
        if not any(UPLOAD_ARTIFACT_SHA_RE.match(strip_comment(line)) for line in block):
            return ["actions/upload-artifact must be pinned to a 40-character SHA"]
    return []


def rust_cache_blocks(job_lines: list[str]) -> list[list[str]]:
    return action_blocks(job_lines, "Swatinem/rust-cache@")


def github_cache_blocks(job_lines: list[str]) -> list[list[str]]:
    return (
        action_blocks(job_lines, "actions/cache@")
        + action_blocks(job_lines, "actions/cache/restore@")
        + action_blocks(job_lines, "actions/cache/save@")
    )


def block_run_command_count(block: list[str], command: str) -> int:
    for index, line in enumerate(block):
        clean = strip_comment(line)
        inline = YAML_RUN_LINE_RE.match(clean)
        if inline is None:
            continue
        value = inline.group(2).strip().strip("'\"")
        if value == command:
            return 1
        if value not in {"|", ">"}:
            continue
        return sum(1 for nested in block_run_body_lines(block) if nested.strip() == command)
    return 0


def shell_line_is_control_flow(line: str) -> bool:
    stripped = line.strip()
    return (
        re.match(
            r"^(if|then|elif|else|fi|for|while|until|case|esac|select|do|done)\b",
            stripped,
        )
        is not None
    )


def shell_line_is_function_definition(line: str) -> bool:
    stripped = line.strip()
    name = r"[A-Za-z_][A-Za-z0-9_]*"
    return (
        re.match(rf"^(?:function\s+)?{name}\s*\(\)\s*(?:[{{(].*)?$", stripped) is not None
        or re.match(rf"^function\s+{name}\b", stripped) is not None
    )


def shell_line_has_unclosed_quote(line: str) -> bool:
    try:
        shlex.split(line)
    except ValueError:
        return True
    return False


def run_body_required_command_count(lines: list[str], command: str) -> int:
    for line in lines:
        stripped = line.strip()
        if not stripped:
            continue
        if (
            "<<"
            in stripped
            or shell_line_is_control_flow(stripped)
            or shell_line_is_function_definition(stripped)
            or shell_line_has_unclosed_quote(stripped)
        ):
            return 0
    return top_level_shell_commands(lines).count(command)


def block_required_run_command_count(block: list[str], command: str) -> int:
    for line in block:
        clean = strip_comment(line)
        inline = YAML_RUN_LINE_RE.match(clean)
        if inline is None:
            continue
        value = inline.group(2).strip().strip("'\"")
        if value == command:
            return 1
        if value != "|":
            continue
        return run_body_required_command_count(block_run_body_lines(block), command)
    return 0


def block_runs_command(block: list[str], command: str) -> bool:
    return block_run_command_count(block, command) > 0


def job_run_command_count(job_lines: list[str], command: str) -> int:
    return sum(block_run_command_count(block, command) for block in step_blocks(job_lines))


def step_is_unconditional(block: list[str]) -> bool:
    items = block_top_level_items(block)
    return items is not None and "if" not in items


def job_unconditional_run_command_count(job_lines: list[str], command: str) -> int:
    if job_if_value(job_lines) != "":
        return 0
    return sum(
        block_required_run_command_count(block, command)
        for block in step_blocks(job_lines)
        if step_is_unconditional(block)
    )


def job_runs_command(job_lines: list[str], command: str) -> bool:
    return job_run_command_count(job_lines, command) > 0


def workflow_run_command_count(workflow_text: str, command: str) -> int:
    return sum(
        job_unconditional_run_command_count(job_lines, command)
        for job_lines in parse_jobs(workflow_text).values()
    )


def block_has_target_dir_opt_in(block: list[str]) -> bool:
    return any(TARGET_DIR_OPT_IN_RE.match(strip_comment(line)) for line in block)




def block_input_items(block: list[str]) -> list[tuple[str, str]]:
    items: list[tuple[str, str]] = []
    with_indent: int | None = None
    input_indent: int | None = None
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        if with_indent is None:
            match = re.match(r"^(\s*)with:\s*$", clean)
            if match is not None:
                with_indent = len(match.group(1))
                input_indent = with_indent + 2
            continue

        indent = len(clean) - len(clean.lstrip(" "))
        if indent <= with_indent:
            break
        if indent != input_indent:
            continue
        match = re.match(rf"^\s{{{input_indent}}}([A-Za-z0-9_.-]+):\s*(.*)$", clean)
        if match is not None:
            items.append((match.group(1), match.group(2).strip()))
    return items


def block_has_input(block: list[str], name: str, value: str | None = None) -> bool:
    expected = None if value is None else unquote_yaml_scalar(value)
    for item_name, item_value in block_input_items(block):
        if item_name != name:
            continue
        if expected is None or unquote_yaml_scalar(item_value) == expected:
            return True
    return False


def block_input_values(block: list[str], name: str) -> list[str]:
    return [
        unquote_yaml_scalar(item_value)
        for item_name, item_value in block_input_items(block)
        if item_name == name
    ]


def block_has_scalar(block: list[str], name: str, value: str) -> bool:
    expected = f"{name}: {value}"
    return any(strip_comment(line).strip() == expected for line in block)


def mapping_child_block(lines: list[str], name: str) -> list[str]:
    expected = f"{name}:"
    for index, line in enumerate(lines):
        clean = strip_comment(line).rstrip()
        if clean.strip() != expected:
            continue
        parent_indent = line_indent(clean)
        block: list[str] = []
        for nested in lines[index + 1:]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                if block:
                    block.append(nested_clean)
                continue
            if line_indent(nested_clean) <= parent_indent:
                break
            block.append(nested_clean)
        return block
    return []


def block_input_value(block: list[str], name: str) -> str | None:
    for item_name, item_value in block_input_items(block):
        if item_name == name:
            return unquote_yaml_scalar(item_value)
    return None


def artifact_retention_upload_key(source_name: str, job_id: str, step_id: str) -> str:
    return f"{source_name}::{job_id}::{step_id}"


def artifact_retention_source_is_canonical(source_name: str) -> bool:
    return bool(
        ARTIFACT_RETENTION_WORKFLOW_SOURCE_RE.fullmatch(source_name)
        or ARTIFACT_RETENTION_ACTION_SOURCE_RE.fullmatch(source_name)
    )


def block_step_id(block: list[str]) -> str | None:
    items = block_top_level_items(block)
    if items is None:
        return None
    step_id = items.get("id")
    return step_id if step_id else None


def artifact_retention_upload_matches(site: ArtifactRetentionUploadSite, artifact_name: str) -> bool:
    return artifact_name == site.artifact_name


def artifact_retention_upload_name_expectation(site: ArtifactRetentionUploadSite) -> str:
    return f"configured name {site.artifact_name}"


def texts_have_upload_artifact_action(texts: Iterable[str]) -> bool:
    return any(
        "actions/upload-artifact@" in strip_comment(line)
        for text in texts
        for line in text.splitlines()
    )


def upload_artifact_retention_errors(
    policy: ArtifactRetentionPolicy,
    source_name: str,
    job_id: str,
    job_lines: list[str],
    seen_upload_keys: set[str] | None = None,
) -> list[str]:
    errors: list[str] = []
    seen_step_ids: set[str] = set()
    for block in action_blocks(job_lines, "actions/upload-artifact@"):
        step_id = block_step_id(block)
        if step_id is None:
            errors.append(f"{source_name} {job_id} upload-artifact step must set id for artifact retention policy")
            continue
        if step_id in seen_step_ids:
            errors.append(f"{source_name} {job_id} upload-artifact step id {step_id} is duplicated")
            continue
        seen_step_ids.add(step_id)
        upload_key = artifact_retention_upload_key(source_name, job_id, step_id)
        if seen_upload_keys is not None:
            seen_upload_keys.add(upload_key)
        site = policy.uploads.get(upload_key)
        if site is None:
            errors.append(f"{upload_key} missing from artifact retention policy")
            continue
        artifact_names = block_input_values(block, "name")
        if len(artifact_names) != 1:
            errors.append(f"{upload_key} upload-artifact step must set exactly one name")
            continue
        artifact_name = artifact_names[0]
        label = f"{source_name} {job_id} {step_id} artifact {artifact_name}"
        if not artifact_retention_upload_matches(site, artifact_name):
            errors.append(
                f"{label} does not match {artifact_retention_upload_name_expectation(site)}"
            )
            continue
        if site.required_if is not None:
            items = block_top_level_items(block)
            if items is None:
                errors.append(f"{label} must have parseable step keys for configured if")
                continue
            actual_if = items.get("if")
            if actual_if is None:
                errors.append(f"{label} must set if to configured if {site.required_if}")
                continue
            if actual_if != site.required_if:
                errors.append(
                    f"{label} if {actual_if} does not match configured if {site.required_if}"
                )
                continue
        class_policy = policy.classes[site.artifact_class]
        retention_values = block_input_values(block, "retention-days")
        if not retention_values:
            errors.append(f"{label} must set retention-days")
            continue
        if len(retention_values) != 1:
            errors.append(f"{label} must set exactly one retention-days")
            continue
        try:
            retention_days = int(retention_values[0])
        except ValueError:
            errors.append(f"{label} retention-days must be a positive integer")
            continue
        if retention_days <= 0:
            errors.append(f"{label} retention-days must be a positive integer")
            continue
        if retention_values[0] != str(site.retention_days):
            errors.append(
                f"{label} retention-days {retention_values[0]} "
                f"does not match configured retention-days {site.retention_days}"
            )
            continue
        if retention_days > class_policy.max_retention_days:
            errors.append(
                f"{label} retention-days {retention_days} "
                f"exceeds configured max {class_policy.max_retention_days}"
            )
    return errors


def verify_artifact_retention_policy(
    workflows: dict[str, str],
    composite_actions: dict[str, str],
) -> list[str]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None

    policy = config["artifact_retention"]
    if not isinstance(policy, ArtifactRetentionPolicy):
        return ["github-actions runner config invalid: artifact_retention policy did not load"]

    errors: list[str] = []
    seen_sources: set[str] = set()
    seen_upload_keys: set[str] = set()
    for file_name, workflow_text in sorted(workflows.items()):
        seen_sources.add(file_name)
        for job_id, job_lines in sorted(parse_jobs(workflow_text).items()):
            errors.extend(upload_artifact_retention_errors(
                policy,
                file_name,
                job_id,
                job_lines,
                seen_upload_keys,
            ))

    for file_name, automation_text in sorted(composite_actions.items()):
        seen_sources.add(file_name)
        errors.extend(upload_artifact_retention_errors(
            policy,
            file_name,
            "__composite__",
            automation_text.splitlines(),
            seen_upload_keys,
        ))

    for upload_key in sorted(policy.uploads):
        source_name = upload_key.split("::", 1)[0]
        if source_name not in seen_sources:
            errors.append(
                f"artifact retention policy upload {upload_key} "
                f"source {source_name} is missing from scanned sources"
            )
        elif upload_key not in seen_upload_keys:
            errors.append(f"artifact retention policy upload {upload_key} has no matching upload-artifact step")

    return errors


def job_has_setup_input(job_lines: list[str], name: str, value: str | None = None) -> bool:
    return any(block_has_input(block, name, value) for block in setup_action_blocks(job_lines))


def step_if_condition(block: list[str]) -> str | None:
    for line in block:
        clean = strip_comment(line).strip()
        if clean.startswith("if:"):
            return clean[3:].strip()
    return None


def block_has_cargo_build_jobs_compile_command(block: list[str]) -> bool:
    return CARGO_BUILD_JOBS_COMPILE_COMMAND_RE.search(uncommented_text(block)) is not None


def cargo_build_jobs_setup_order_errors(job_lines: list[str], expected_key: str) -> list[str]:
    setup_conditions: set[str | None] = set()
    for block in step_blocks(job_lines):
        if any("./.github/actions/setup-environment" in line for line in block) and block_has_input(
            block, "build-jobs-key", expected_key
        ):
            setup_conditions.add(step_if_condition(block))
            continue
        if not block_has_cargo_build_jobs_compile_command(block):
            continue
        compile_condition = step_if_condition(block)
        if None in setup_conditions or compile_condition in setup_conditions:
            continue
        if setup_conditions:
            return [
                "build-jobs-key setup-environment step must be unconditional "
                "or match the cargo/just compile step condition"
            ]
        return ["build-jobs-key setup-environment step must run before cargo/just compile commands"]
    return []


def job_has_toolchain_component(job_lines: list[str], component: str) -> bool:
    for block in setup_action_blocks(job_lines):
        for value in block_input_values(block, "toolchain-components"):
            components = {item.strip() for item in value.split(",") if item.strip()}
            if component in components:
                return True
    return False


def job_uses_managed_target_dir(job_lines: list[str]) -> bool:
    return any(
        "steps.setup.outputs.managed_target_dir" in strip_comment(line)
        or "steps.setup.outputs.managed_target_dir_relative" in strip_comment(line)
        for line in job_lines
    )


def job_opts_into_managed_target_dir(job_lines: list[str]) -> bool:
    return any(block_has_target_dir_opt_in(block) for block in setup_action_blocks(job_lines))


def uncommented_text(lines: list[str]) -> str:
    return "\n".join(strip_comment(line) for line in lines)


def append_missing_text_requirements(
    errors: list[str],
    text: str,
    requirements: tuple[tuple[str, tuple[str, ...]], ...],
) -> None:
    for error, fragments in requirements:
        if not all(fragment in text for fragment in fragments):
            errors.append(error)


def append_failed_contracts(errors: list[str], contracts: Iterable[tuple[str, bool]]) -> None:
    errors.extend(error for error, passed in contracts if not passed)


def block_run_body_lines(block: list[str]) -> list[str]:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        value = match.group(2).strip().strip("'\"")
        if value not in {"|", ">"}:
            return [value] if value else []
        run_indent = len(clean) - len(clean.lstrip(" "))
        body_indent: int | None = None
        body: list[str] = []
        for nested in block[index + 1:]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                body.append("")
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= run_indent:
                break
            if body_indent is None:
                body_indent = indent
            body.append(nested_clean[body_indent:] if indent >= body_indent else nested_clean.lstrip())
        return body
    return []


def line_indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def top_level_shell_commands(lines: list[str]) -> list[str]:
    commands: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index].rstrip()
        if not line.strip() or line_indent(line) != 0:
            index += 1
            continue
        parts = [line.strip()]
        while parts[-1].endswith("\\") and index + 1 < len(lines):
            index += 1
            continuation = lines[index].strip()
            if continuation:
                parts.append(continuation)
        commands.append(" ".join(parts))
        index += 1
    return commands


def ordered_command_match(
    commands: list[str],
    predicates: tuple[Callable[[str], bool], ...],
) -> bool:
    cursor = -1
    for predicate in predicates:
        for index in range(cursor + 1, len(commands)):
            if predicate(commands[index]):
                cursor = index
                break
        else:
            return False
    return True


def run_body_has_top_level_command(lines: list[str], command: str) -> bool:
    return command in top_level_shell_commands(lines)


def shell_line_exits(line: str) -> bool:
    stripped = line.strip()
    if not stripped:
        return False
    command = stripped.split(maxsplit=1)[0]
    return command in {"exit", "return"}


def run_body_has_single_terminal_exit(lines: list[str], command: str) -> bool:
    significant_lines = [(index, line) for index, line in enumerate(lines) if line.strip()]
    if not significant_lines:
        return False
    last_index, last_line = significant_lines[-1]
    if line_indent(last_line) != 0 or last_line.strip() != command:
        return False
    exit_lines = [
        (index, line)
        for index, line in significant_lines
        if shell_line_exits(line)
    ]
    return exit_lines == [(last_index, last_line)]


def command_argv_has_prefix(command: str, expected_prefix: tuple[str, ...]) -> bool:
    try:
        argv = shlex.split(command)
    except ValueError:
        return False
    return tuple(argv[: len(expected_prefix)]) == expected_prefix


CACHE_PERSISTENCE_AUDIT_FAILURE_MASKING_OPERATORS = {";", "&", "&&", "||", "|"}


def command_has_failure_masking_shell_control(command: str) -> bool:
    tokens = command_tokens_with_line_boundaries(command)
    return any(token in CACHE_PERSISTENCE_AUDIT_FAILURE_MASKING_OPERATORS for token in tokens)


def append_missing_cache_persistence_probe_structure(
    errors: list[str],
    run_lines: list[str],
) -> None:
    commands = top_level_shell_commands(run_lines)
    command_text = "\n".join(commands)
    append_failed_contracts(
        errors,
        (
            (
                "cache-persistence-audit must delegate audit policy to ci_storage_audit",
                len(commands) == 1,
            ),
            (
                "cache-persistence-audit must run ci_storage_audit exact-key probes",
                command_argv_has_prefix(command_text, CACHE_PERSISTENCE_AUDIT_ARGV_PREFIX),
            ),
            (
                "cache-persistence-audit must not suppress audit contract failures",
                len(commands) == 1 and not command_has_failure_masking_shell_control(command_text),
            ),
        ),
    )
    append_missing_text_requirements(
        errors,
        command_text,
        CACHE_PERSISTENCE_AUDIT_PROBE_COMMAND_REQUIREMENTS,
    )


def first_step_uses_checkout(step_blocks_: list[list[str]]) -> bool:
    return len(step_blocks_) == 2 and any(line_uses_action(line, "actions/checkout@") for line in step_blocks_[0])


def single_run_step_matches(run_blocks: list[list[str]], step_name: str) -> bool:
    return len(run_blocks) == 1 and step_name_matches(run_blocks[0], step_name)


def append_cache_persistence_audit_contract_errors(errors: list[str], jobs: dict[str, list[str]]) -> None:
    audit_lines = jobs.get("cache-persistence-audit")
    if audit_lines is None:
        if "test-archive" in jobs:
            errors.append("cache-persistence-audit job is required")
        return

    audit_text = uncommented_text(audit_lines)
    audit_needs = extract_needs(audit_lines)
    audit_permissions = mapping_child_block(audit_lines, "permissions")
    audit_step_blocks = step_blocks(audit_lines)
    audit_probe_block = named_step_block(audit_lines, CACHE_PERSISTENCE_AUDIT_PROBE_STEP)

    append_failed_contracts(
        errors,
        (
            *((f"cache-persistence-audit needs {need}", need in audit_needs) for need in CACHE_PERSISTENCE_AUDIT_NEEDS),
            *(
                (
                    f"cache-persistence-audit permissions must include {permission_name}: read",
                    block_has_scalar(audit_permissions, permission_name, "read"),
                )
                for permission_name in ("contents", "actions")
            ),
            ("cache-persistence-audit must use always()", job_if_uses_always(audit_lines)),
            ("cache-persistence-audit must gate on full_ci_required", job_gates_on_full_ci_required(audit_lines)),
            (
                "cache-persistence-audit must require test-archive success",
                "needs.test-archive.result == 'success'" in audit_text,
            ),
            (
                "cache-persistence-audit must skip on validated nextest fingerprint reuse",
                NEXTEST_REUSE_MISS_EXPR in audit_text,
            ),
            ("cache-persistence-audit must not add extra steps", len(audit_step_blocks) == 2),
            ("cache-persistence-audit must checkout the repository before probing", first_step_uses_checkout(audit_step_blocks)),
            (
                f"cache-persistence-audit must include {CACHE_PERSISTENCE_AUDIT_PROBE_STEP} step",
                audit_probe_block is not None,
            ),
        ),
    )
    if audit_probe_block is None:
        return

    audit_run_blocks = [block for block in audit_step_blocks if step_declares_run(block)]
    audit_probe_run_lines = block_run_body_lines(audit_probe_block)
    append_failed_contracts(
        errors,
        (
            *(
                (message, block_has_scalar(audit_probe_block, name, value))
                for message, name, value in CACHE_PERSISTENCE_AUDIT_PROBE_SCALAR_REQUIREMENTS
            ),
            (
                "cache-persistence-audit must not add extra run steps",
                single_run_step_matches(audit_run_blocks, CACHE_PERSISTENCE_AUDIT_PROBE_STEP),
            ),
            (
                "cache-persistence-audit probe must be non-blocking",
                block_has_scalar(audit_probe_block, "continue-on-error", "true"),
            ),
        ),
    )
    append_missing_cache_persistence_probe_structure(errors, audit_probe_run_lines)


def normalize_script_text(text: str) -> str:
    text = re.sub(r"\\\s*\n\s*", " ", text)
    lines = [line.rstrip() for line in text.strip("\n").splitlines()]
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    indents = [len(line) - len(line.lstrip(" ")) for line in lines if line.strip()]
    margin = min(indents) if indents else 0
    normalized_lines = [line[margin:] if line.strip() else "" for line in lines]
    return "\n".join(re.sub(r"(?<=\S) {2,}(?=\S)", " ", line) for line in normalized_lines)


def block_run_body(block: list[str]) -> str:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        scalar = match.group(2).strip()
        if not scalar.startswith(("|", ">")):
            return unquote_yaml_scalar(scalar)
        run_indent = len(match.group(1))
        body_lines: list[str] = []
        for nested in block[index + 1 :]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                body_lines.append("")
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= run_indent:
                break
            body_lines.append(nested_clean)
        return normalize_script_text("\n".join(body_lines))
    return ""


def block_run_body_matches(block: list[str], expected: str) -> bool:
    return normalize_script_text(block_run_body(block)) == normalize_script_text(expected)


def repo_text_or_empty(relative_path: str) -> str:
    path = REPO_ROOT / relative_path
    try:
        return path.read_text(encoding="utf-8")
    except OSError:
        return ""


def block_step_property_indent(block: list[str]) -> int | None:
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        match = re.match(
            rf"^(\s*)-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?{YAML_KEY_PATTERN}\s*:\s*.*$",
            clean,
        )
        if match is None:
            return None
        return len(match.group(1)) + 2
    return None


def block_top_level_items(block: list[str]) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    step_item_indent = property_indent - 2
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        step_match = re.match(
            rf"^(\s*)-\s*(?:{YAML_ANCHOR_PATTERN}\s+)?({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$",
            clean,
        )
        if step_match is not None:
            if len(step_match.group(1)) != step_item_indent:
                continue
            key = unquote_yaml_scalar(step_match.group(2))
            value = step_match.group(3)
        else:
            indent = len(clean) - len(clean.lstrip(" "))
            if indent != property_indent:
                continue
            item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
            if item_match is None:
                return None
            key = unquote_yaml_scalar(item_match.group(1))
            value = item_match.group(2)
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(value)
    return items


def block_nested_mapping_items(block: list[str], parent_key: str) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    parent_indent: int | None = None
    item_indent: int | None = None
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if parent_indent is None:
            parent_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
            if (
                parent_match is not None
                and indent == property_indent
                and unquote_yaml_scalar(parent_match.group(1)) == parent_key
                and unquote_yaml_scalar(parent_match.group(2)) == ""
            ):
                parent_indent = indent
            continue
        if indent <= parent_indent:
            break
        if item_indent is None:
            item_indent = indent
        if indent != item_indent:
            continue
        item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def top_level_mapping_items(workflow_text: str, top_key: str) -> dict[str, str] | None:
    lines = workflow_text.splitlines()
    top_index: int | None = None
    for index, line in enumerate(lines):
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent != 0:
            continue
        top_match = re.match(rf"^({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if top_match is None:
            continue
        if unquote_yaml_scalar(top_match.group(1)) != top_key:
            continue
        if top_index is not None or unquote_yaml_scalar(top_match.group(2)) != "":
            return None
        top_index = index
    if top_index is None:
        return None

    item_indent: int | None = None
    items: dict[str, str] = {}
    for line in lines[top_index + 1 :]:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent == 0:
            break
        if item_indent is None:
            item_indent = indent
        if indent != item_indent:
            return None
        item_match = re.match(rf"^\s*({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def block_has_canonical_step_envelope(
    block: list[str],
    allowed_keys: frozenset[str],
    required_scalars: dict[str, str],
    nested_mappings: dict[str, dict[str, str]] | None = None,
) -> bool:
    items = block_top_level_items(block)
    if items is None:
        return False
    actual_keys = set(items)
    if actual_keys - set(allowed_keys):
        return False
    if not set(required_scalars).issubset(actual_keys):
        return False
    for key, expected in required_scalars.items():
        if items.get(key) != expected:
            return False
    for parent_key, expected_items in (nested_mappings or {}).items():
        actual_items = block_nested_mapping_items(block, parent_key)
        if actual_items != expected_items:
            return False
    return True


def block_has_raw_top_level_scalar(block: list[str], name: str, value: str) -> bool:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return False
    expected = f"{' ' * property_indent}{name}: {value}"
    return any(strip_comment(line).rstrip() == expected for line in block)


def job_top_level_items(job_lines: list[str]) -> dict[str, str] | None:
    items: dict[str, str] = {}
    for line in job_lines:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(" "))
        if indent != 4:
            continue
        item_match = re.match(rf"^\s{{4}}({YAML_KEY_PATTERN})\s*:\s*(.*?)\s*$", clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items


def has_line_matching(lines: list[str], pattern: re.Pattern[str]) -> bool:
    return any(pattern.match(strip_comment(line)) for line in lines)


def job_if_value(job_lines: list[str]) -> str:
    for index, line in enumerate(job_lines):
        clean = strip_comment(line).rstrip()
        match = re.match(r"^    if:\s*(?P<value>.*?)\s*$", clean)
        if match is not None:
            value = match.group("value")
            # Strip YAML block/folding scalar indicators; they are syntax, not part
            # of the evaluated expression.
            if value in {">-", ">+", ">", "|-", "|+", "|"}:
                value = ""
            child_values: list[str] = []
            for child in job_lines[index + 1 :]:
                child_clean = strip_comment(child).rstrip()
                if not child_clean.strip():
                    continue
                indent = len(child_clean) - len(child_clean.lstrip(" "))
                if indent <= 4:
                    break
                child_values.append(child_clean.strip())
            if child_values:
                return "\n".join([value, *child_values])
            return value
    return ""


def step_has_id(block: list[str], step_id: str) -> bool:
    return any(re.match(rf"^\s+id:\s*{re.escape(step_id)}\s*$", strip_comment(line)) for line in block)


def unique_step_with_id(job_lines: list[str], step_id: str) -> list[str] | None:
    matches = [block for block in step_blocks(job_lines) if step_has_id(block, step_id)]
    return matches[0] if len(matches) == 1 else None


def has_run_command(lines: list[str], command: str) -> bool:
    expected = {f"run: {command}", f"- run: {command}"}
    for line in lines:
        clean = strip_comment(line)
        if clean.strip() in expected:
            return True
        match = YAML_RUN_LINE_RE.match(clean)
        if match is not None and unquote_yaml_scalar(match.group(2)) == command:
            return True
    return False


def job_has_explicit_cache_key(job_lines: list[str]) -> bool:
    return any(CACHE_KEY_RE.match(strip_comment(line)) for line in job_lines)


def shared_registry_cache_errors(job: str, job_lines: list[str]) -> list[str]:
    blocks = rust_cache_blocks(job_lines)
    shared_blocks = [
        block for block in blocks if block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY)
    ]
    if not shared_blocks:
        return [f"{job} must use shared Cargo registry/git cache key"]

    errors: list[str] = []
    for block in blocks:
        if not block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY):
            errors.append(f"{job} must use only shared Cargo registry/git rust-cache blocks")
        if not block_has_input(block, "cache-targets", "false"):
            errors.append(f"{job} shared Cargo registry/git cache must disable target caching")
        if not block_has_input(block, "cache-bin", "false"):
            errors.append(f"{job} shared Cargo registry/git cache must disable cargo bin caching")
        if not block_has_input(block, "save-if", SHARED_REGISTRY_SAVE_IF):
            errors.append(f"{job} shared Cargo registry/git cache save must be main-only")
        if block_has_input(block, "cache-directories"):
            errors.append(f"{job} shared Cargo registry/git cache must not include target directories")
    return errors


def block_is_shared_registry_cache(block: list[str]) -> bool:
    return (
        block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY)
        and block_has_input(block, "cache-targets", "false")
        and block_has_input(block, "cache-bin", "false")
        and not block_has_input(block, "cache-directories")
    )


def block_uses_managed_target_cache(block: list[str]) -> bool:
    return any("actions/cache" in strip_comment(line) for line in block) and block_has_input(
        block, "path", "${{ steps.setup.outputs.managed_target_dir }}"
    )


def block_key_value_has_prefix(block: list[str], prefix: str) -> bool:
    for name, value in block_input_items(block):
        if name == "key" and prefix in value:
            return True
    return False


def nextest_fingerprint_errors(fingerprint_lines: list[str], archive_lines: list[str]) -> list[str]:
    blocks = step_blocks(fingerprint_lines)
    job_text = uncommented_text(fingerprint_lines)
    cache_blocks = [
        block
        for block in (
            action_blocks(archive_lines, "actions/cache/restore@")
            + action_blocks(archive_lines, "actions/cache/save@")
        )
        if block_has_input(block, "path", "${{ env.NEXTEST_ARCHIVE_PATH }}")
    ]
    run_block_indices = [
        index
        for index, block in enumerate(blocks)
        if TEST_ARCHIVE_FINGERPRINT_SCRIPT in uncommented_text(block)
    ]
    run_blocks = [blocks[index] for index in run_block_indices]
    upload_block_indices = [
        index
        for index, block in enumerate(blocks)
        if "actions/upload-artifact@" in uncommented_text(block)
        and block_has_input(block, "path", TEST_ARCHIVE_FINGERPRINT_PATH)
    ]
    upload_blocks = [
        blocks[index]
        for index in upload_block_indices
    ]

    if not run_blocks or not upload_blocks:
        return ["nextest-fingerprint must publish nextest archive fingerprint"]

    run_text = "\n".join(uncommented_text(block) for block in run_blocks)
    if any(output not in job_text for output in TEST_ARCHIVE_FINGERPRINT_REQUIRED_JOB_OUTPUTS):
        return ["nextest-fingerprint must expose secure nextest fingerprint output"]
    if TEST_ARCHIVE_FINGERPRINT_STEP_ID not in run_text:
        return ["nextest-fingerprint must expose secure nextest fingerprint output"]
    if any(argument not in run_text for argument in TEST_ARCHIVE_FINGERPRINT_SCRIPT_ARGS):
        return ["nextest-fingerprint must run the canonical producer script"]
    if TEST_ARCHIVE_FINGERPRINT_PATH not in run_text:
        return ["nextest-fingerprint must publish nextest archive fingerprint"]
    if any("hashFiles(" in uncommented_text(block) for block in run_blocks + upload_blocks):
        return ["nextest-fingerprint must not inline nextest hashFiles"]
    if not any(
        block_has_input(block, "name", "${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}")
        for block in upload_blocks
    ):
        return ["nextest-fingerprint artifact name must come from producer output"]
    repo_controlled_indices = [
        index
        for index, block in enumerate(blocks)
        if "./.github/actions/setup-environment" in uncommented_text(block)
        or 'just test-archive "$NEXTEST_ARCHIVE_PATH"' in uncommented_text(block)
    ]
    if repo_controlled_indices and (
        min(run_block_indices) >= min(repo_controlled_indices)
        or min(upload_block_indices) >= min(repo_controlled_indices)
    ):
        return ["nextest-fingerprint must publish nextest fingerprint before repo-controlled steps"]
    cache_key_step = named_step_block(archive_lines, TEST_ARCHIVE_CACHE_AUDIT_STEP)
    cache_key_step_text = uncommented_text(cache_key_step) if cache_key_step is not None else ""
    if any("hashFiles(" in (block_input_value(block, "key") or "") for block in cache_blocks):
        return ["nextest archive cache key must use nextest fingerprint output"]
    if TEST_ARCHIVE_CACHE_KEY not in cache_key_step_text:
        return ["nextest archive cache key must use nextest fingerprint output"]
    return []


def block_declares_restore_keys_prefix(block: list[str], prefix: str) -> bool:
    # Locate the `with:` line to determine the input indent. The marker for
    # `restore-keys:` is anchored at that exact indent so earlier lines whose
    # values happen to contain the substring `restore-keys:` (e.g., a quoted
    # step-level `name:`) cannot impersonate the input.
    input_indent: int | None = None
    for line in block:
        match = re.match(r"^(\s*)with:\s*$", strip_comment(line).rstrip())
        if match is not None:
            input_indent = len(match.group(1)) + 2
            break
    if input_indent is None:
        return False
    marker_re = re.compile(rf"^\s{{{input_indent}}}restore-keys:\s*(.*)$")
    for marker_idx, line in enumerate(block):
        match = marker_re.match(strip_comment(line))
        if not match:
            continue
        value = match.group(1).strip()
        # Inline-scalar form: `restore-keys: managed-target-v1-...-clippy-host-`.
        # Anything not starting with a block-scalar indicator is treated as an
        # inline value and matched directly.
        if not value.startswith(("|", ">")):
            return prefix in value
        # Block-scalar form: `restore-keys: |` (plus YAML 1.2 chomping or
        # explicit-indentation indicators like `|2`, `>+1`, `|-3`). Body lines
        # are indented strictly more than the marker line; the scan stops at
        # the first line whose indent is equal-or-lesser.
        for child in block[marker_idx + 1:]:
            child_text = strip_comment(child)
            if not child_text.strip():
                continue
            child_indent = len(child) - len(child.lstrip(" "))
            if child_indent <= input_indent:
                break
            if prefix in child_text:
                return True
        return False
    return False


def managed_target_cache_errors(job: str, job_lines: list[str]) -> list[str]:
    expected_key = MANAGED_TARGET_CACHE_KEYS[job]
    combined_blocks = [
        block
        for block in action_blocks(job_lines, "actions/cache@")
        if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
    ]
    restore_blocks = [
        block
        for block in action_blocks(job_lines, "actions/cache/restore@")
        if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
    ]
    save_blocks = [
        block
        for block in action_blocks(job_lines, "actions/cache/save@")
        if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
    ]
    target_blocks = restore_blocks + save_blocks
    if combined_blocks:
        return ["managed target cache saves must be push-to-main only"]
    if not restore_blocks:
        return [f"{job} must use isolated managed target cache"]
    if not save_blocks:
        return ["managed target cache saves must be push-to-main only"]

    expected_prefix = (
        f"managed-target-v1-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-{expected_key}-"
    )
    # The exact key source must carry the job-specific prefix. Checking the
    # whole cache block's text would also match a prefix that only appears in
    # `restore-keys:`, masking key/restore-keys drift.
    key_sources = [
        block_input_value(block, "key") or ""
        for block in target_blocks
    ]
    if job == "test-archive" and all(
        block_has_input(block, "key", TEST_ARCHIVE_TARGET_CACHE_KEY_OUTPUT)
        for block in target_blocks
    ):
        cache_key_step = named_step_block(job_lines, TEST_ARCHIVE_CACHE_AUDIT_STEP)
        if cache_key_step is None or expected_prefix not in uncommented_text(cache_key_step):
            return [f"{job} managed target cache key must isolate {expected_key}"]
    elif not key_sources or any(expected_prefix not in key_source for key_source in key_sources):
        return [f"{job} managed target cache key must isolate {expected_key}"]
    if not all(TEST_ARCHIVE_S3_MAIN_SAVE_GUARD in uncommented_text(block) for block in save_blocks):
        return ["managed target cache saves must be push-to-main only"]

    # #400: each managed-target cache MUST declare a restore-keys prefix fallback
    # matching the job's key prefix. Without it, any change to CI orchestration
    # files included in hashFiles (justfile, ci/rust-verification.toml,
    # scripts/rust_verification.py) misses the exact key and pays the full
    # ~22m aarch64 release cross-compile instead of an incremental rebuild.
    if not any(
        block_declares_restore_keys_prefix(block, expected_prefix) for block in restore_blocks
    ):
        return [
            f"{job} managed target cache must declare restore-keys prefix {expected_prefix}"
        ]
    return []


def job_just_lanes(job_lines: list[str]) -> set[str]:
    return {match.group(2) for match in JUST_LANE_RE.finditer(uncommented_text(job_lines))}


def block_uses_pinned_install_action(block: list[str]) -> bool:
    return any(TAIKI_INSTALL_ACTION_RE.match(strip_comment(line)) for line in block)


def install_action_tool_step(job_lines: list[str], tool: str, output: str) -> tuple[int, list[str]] | None:
    expected_tool = f"{tool}@${{{{ {output} }}}}"
    for index, block in enumerate(step_blocks(job_lines)):
        if block_uses_pinned_install_action(block) and block_has_input(block, "tool", expected_tool):
            return index, block
    return None


def named_step_block(lines: list[str], step_name: str) -> list[str] | None:
    name_re = re.compile(rf"^\s*(?:-\s*)?name:\s*{re.escape(step_name)}\s*$")
    for block in step_blocks(lines):
        if any(name_re.match(strip_comment(line)) for line in block):
            return block
    return None


def step_index(lines: list[str], step_name: str) -> int | None:
    for index, block in enumerate(step_blocks(lines)):
        if step_name_matches(block, step_name):
            return index
    return None


def step_occurs_after(lines: list[str], later_step: str, earlier_step: str) -> bool:
    later_index = step_index(lines, later_step)
    earlier_index = step_index(lines, earlier_step)
    return later_index is not None and earlier_index is not None and later_index > earlier_index


def step_name_matches(block: list[str], step_name: str) -> bool:
    name_re = re.compile(rf"^\s*(?:-\s*)?name:\s*{re.escape(step_name)}\s*$")
    return any(name_re.match(strip_comment(line)) for line in block)


def step_declares_run(block: list[str]) -> bool:
    return any(YAML_RUN_LINE_RE.match(strip_comment(line).rstrip()) is not None for line in block)


def first_step_running_command(job_lines: list[str], command: str) -> int | None:
    for index, block in enumerate(step_blocks(job_lines)):
        if block_runs_command(block, command):
            return index
    return None






def tokens_are_rust_version_probe(tokens: list[str]) -> bool:
    if not tokens:
        return False
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return tokens_are_rust_version_probe(tokens[assignment_index:])
    executable = pathlib.Path(tokens[0]).name
    if executable == "cargo":
        command_index = consume_cargo_global_options(tokens, 1)
        probe_commands = {"--version", "-V", "version", "--help", "-h", "help"}
        return command_index < len(tokens) and tokens[command_index] in probe_commands
    if raw_rust_tool_token(executable):
        return any(token in {"--version", "-V", "--help", "-h"} for token in tokens[1:])
    return False


def tokens_have_repo_automation_raw_cargo(
    tokens: list[str],
    *,
    variables: dict[str, str] | None = None,
) -> bool:
    if not tokens:
        return False
    variables = variables or {}
    for payload in shell_command_substitution_payloads(tokens):
        if tokens_have_raw_cargo_launch(payload, variables=variables):
            return True
    array_assignments, array_assignment_index = shell_array_assignment_values_from_tokens(tokens)
    if array_assignments and array_assignment_index == len(tokens):
        return array_assignment_values_have_cargo_executable(array_assignments)
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        segment_variables = dict(variables)
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    array_assignments, array_assignment_index = shell_array_assignment_values_from_tokens(segment)
                    if (
                        array_assignments
                        and array_assignment_index == len(segment)
                        and array_assignment_values_have_cargo_executable(array_assignments)
                    ):
                        return True
                    segment_variables.update(assignments)
                    segment = []
                    continue
                if tokens_have_repo_automation_raw_cargo(segment, variables=segment_variables):
                    return True
                segment = []
                continue
            segment.append(token)
        return tokens_have_repo_automation_raw_cargo(segment, variables=segment_variables)
    if tokens_are_rust_version_probe(tokens):
        return False
    return tokens_have_raw_cargo_launch(tokens, variables=variables)


def tokens_are_shell_array_assignment(tokens: list[str]) -> bool:
    assignments, assignment_index = shell_array_assignment_values_from_tokens(tokens)
    return bool(assignments) and assignment_index == len(tokens)


def array_assignment_values_have_cargo_executable(assignments: dict[str, str]) -> bool:
    return any(tokens_have_cargo_executable_launch(command_tokens(value)) for value in assignments.values())


def tokens_have_cargo_executable_launch(tokens: list[str], *, depth: int = 0) -> bool:
    if depth > 6:
        return True
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return False
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if tokens_have_cargo_executable_launch(segment, depth=depth + 1):
                    return True
                segment = []
                continue
            segment.append(token)
        return tokens_have_cargo_executable_launch(segment, depth=depth + 1)
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return assignment_index < len(tokens) and tokens_have_cargo_executable_launch(
            tokens[assignment_index:],
            depth=depth + 1,
        )
    executable = pathlib.Path(tokens[0]).name
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_cargo_executable_launch(inner, depth=depth + 1)
    if executable == "env":
        inner = env_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_cargo_executable_launch(inner, depth=depth + 1)
    return executable == "cargo"


def is_managed_just_recipe_guard(recipe: str, stripped_line: str) -> bool:
    expected = (
        f'if [ "${{BOLT_MANAGED_JUST:-}}" != "1" ]; then echo "ERROR: {recipe} '
        'must run through scripts/rust_verification.py run"; exit 2; fi'
    )
    return stripped_line == expected


def is_allowed_managed_just_recipe_command(recipe: str, stripped_line: str) -> bool:
    allowed_commands = {
        "managed-build": "cargo zigbuild --release --target {{target}} --locked",
        "managed-clippy": "cargo clippy --locked -- -D warnings",
    }
    return stripped_line == allowed_commands.get(recipe)


def repo_automation_raw_cargo_errors(file_name: str, text: str) -> list[str]:
    errors: list[str] = []
    managed_just_recipe = False
    current_just_recipe = ""
    shell_variables: dict[str, str] = {}
    is_justfile = file_name == "justfile" or file_name.startswith("justfile.")
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        if is_justfile and not line[:1].isspace():
            if stripped.startswith("["):
                continue
            if ":" in stripped and ":=" not in stripped:
                recipe = stripped.split(":", 1)[0].strip()
                current_just_recipe = recipe.split()[0] if recipe else ""
                managed_just_recipe = False
                continue
        if (
            is_justfile
            and current_just_recipe in {"managed-build", "managed-clippy"}
            and is_managed_just_recipe_guard(current_just_recipe, stripped)
        ):
            managed_just_recipe = True
            continue
        if is_justfile and managed_just_recipe:
            if is_allowed_managed_just_recipe_command(current_just_recipe, stripped):
                continue
        tokens = command_tokens(stripped)
        if tokens_have_repo_automation_raw_cargo(tokens, variables=shell_variables):
            errors.append("repo automation raw Cargo must use managed rust_verification wrapper")
            break
        assignments, is_persistent_assignment = persistent_shell_assignment_values(tokens)
        if is_persistent_assignment:
            shell_variables.update(assignments)
            continue
        tokens = expand_known_shell_assignment_names(tokens, shell_variables)
        tokens = expand_known_shell_command_variables(tokens, shell_variables)
        if tokens_have_repo_automation_raw_cargo(tokens, variables=shell_variables):
            errors.append("repo automation raw Cargo must use managed rust_verification wrapper")
            break
    return errors




def text_raw_cargo_storage_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    aliases: set[str] = set()
    variables: dict[str, str] = {}
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        tokens = command_tokens(stripped)
        messages.update(raw_cargo_storage_override_messages_from_tokens(tokens, aliases=aliases, variables=variables))
        shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(tokens)
        if is_persistent_assignment:
            variables.update(shell_assignments)
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    aliases.update(simple_cargo_aliases(segment, aliases))
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            aliases.update(simple_cargo_aliases(segment, aliases))
    return messages


def strip_yaml_anchor(value: str) -> tuple[str | None, str]:
    match = re.match(r"&([A-Za-z0-9_.-]+)(?:\s+|$)(.*)", value)
    if match is None:
        return None, value
    return match.group(1), match.group(2).strip()


def resolve_no_mistakes_scalar(value: str, anchors: dict[str, str]) -> tuple[str, str | None]:
    value = value.strip()
    alias = re.fullmatch(r"\*([A-Za-z0-9_.-]+)", value)
    if alias is not None:
        return anchors.get(alias.group(1), value), None
    anchor, value = strip_yaml_anchor(value)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        value = value[1:-1]
    return value, anchor


def record_no_mistakes_anchor_from_scalar(value: str, anchors: dict[str, str]) -> None:
    value = value.strip()
    if value.startswith("-"):
        value = value[1:].strip()
    value, anchor = resolve_no_mistakes_scalar(value, anchors)
    if anchor is not None:
        anchors[anchor] = value


def no_mistakes_anchor_candidate(value: str) -> tuple[str | None, str]:
    value = value.strip()
    if value.startswith("-"):
        value = value[1:].strip()
    return strip_yaml_anchor(value)


def no_mistakes_commands(config_text: str) -> dict[str, str]:
    commands: dict[str, str] = {}
    anchors: dict[str, str] = {}
    in_commands = False
    lines = config_text.splitlines()
    index = 0
    while index < len(lines):
        raw_line = lines[index]
        line = strip_comment(raw_line).rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            index += 1
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent == 0:
            name, separator, value = stripped.partition(":")
            in_commands = bool(separator) and name.strip() == "commands" and (
                not value.strip() or value.strip().startswith("#")
            )
            if separator:
                record_no_mistakes_anchor_from_scalar(value, anchors)
            index += 1
            continue
        if not in_commands:
            _, separator, value = stripped.partition(":")
            candidate_value = value if separator else stripped
            anchor, stripped_value = no_mistakes_anchor_candidate(candidate_value)
            if anchor is not None and (stripped_value in ("|", ">") or stripped_value.startswith(("|", ">"))):
                block_lines: list[str] = []
                index += 1
                while index < len(lines):
                    candidate = lines[index].rstrip()
                    candidate_stripped = candidate.strip()
                    if not candidate_stripped or candidate_stripped.startswith("#"):
                        index += 1
                        continue
                    candidate_indent = len(candidate) - len(candidate.lstrip(" "))
                    if candidate_indent <= indent:
                        break
                    block_lines.append(candidate_stripped)
                    index += 1
                anchors[anchor] = "\n".join(block_lines).strip()
                continue
            record_no_mistakes_anchor_from_scalar(candidate_value, anchors)
            index += 1
            continue
        if indent <= 2 and ":" in stripped:
            name, _, value = stripped.partition(":")
            value = value.strip()
            anchor, stripped_value = strip_yaml_anchor(value)
            if anchor is not None:
                value = stripped_value
            if value in ("|", ">") or value.startswith(("|", ">")):
                block_lines: list[str] = []
                index += 1
                while index < len(lines):
                    candidate = lines[index].rstrip()
                    candidate_stripped = candidate.strip()
                    if not candidate_stripped or candidate_stripped.startswith("#"):
                        index += 1
                        continue
                    candidate_indent = len(candidate) - len(candidate.lstrip(" "))
                    if candidate_indent <= indent:
                        break
                    block_lines.append(candidate_stripped)
                    index += 1
                command = "\n".join(block_lines).strip()
                commands[name.strip()] = command
                if anchor is not None:
                    anchors[anchor] = command
                continue
            scalar_parts = [value]
            index += 1
            while index < len(lines):
                candidate = lines[index].rstrip()
                candidate_stripped = candidate.strip()
                if not candidate_stripped or candidate_stripped.startswith("#"):
                    index += 1
                    continue
                candidate_indent = len(candidate) - len(candidate.lstrip(" "))
                if candidate_indent <= indent:
                    break
                scalar_parts.append(candidate_stripped)
                index += 1
            value = " ".join(part for part in scalar_parts if part).strip()
            value, scalar_anchor = resolve_no_mistakes_scalar(value if anchor is None else f"&{anchor} {value}", anchors)
            if scalar_anchor is not None:
                anchors[scalar_anchor] = value
            commands[name.strip()] = value
            continue
        index += 1
    return commands


def no_mistakes_command_section_errors(config_text: str, config_name: str) -> list[str]:
    errors: list[str] = []
    for raw_line in config_text.splitlines():
        line = raw_line.rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent != 0:
            continue
        name, separator, value = stripped.partition(":")
        if not separator or name.strip() != "commands":
            continue
        value = value.strip()
        if value and not value.startswith("#"):
            errors.append(f"{config_name} commands section must use block mapping")
    return errors


def command_has_managed_compile_heavy_invocation(command: str) -> bool:
    for raw_line in command.splitlines() or [command]:
        tokens = command_tokens(raw_line)
        normalized_tokens = managed_rust_verification_command_tokens(tokens)
        if normalized_tokens is None:
            continue
        managed_args = managed_rust_verification_cargo_args(tokens)
        if not managed_args:
            continue
        subcommand = cargo_subcommand(managed_args)
        if normalized_tokens[2] == "run" and subcommand in LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS:
            return True
        if normalized_tokens[2] == "cargo" and subcommand in LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS:
            return True
    return False


def verify_no_mistakes_config(config_text: str, config_name: str = ".no-mistakes.yaml") -> list[str]:
    errors: list[str] = no_mistakes_command_section_errors(config_text, config_name)
    for command_name, command in no_mistakes_commands(config_text).items():
        command_segments = [command, *command.splitlines()]
        storage_errors = raw_rust_storage_errors(command)
        if any(command_has_raw_cargo(segment) for segment in command_segments if segment.strip()) or any(
            "BOLT_MANAGED_JUST private just recipe bypass" in error for error in storage_errors
        ):
            errors.append(f"{config_name} commands.{command_name} raw Cargo drift must be classified")
        if command_has_managed_compile_heavy_invocation(command):
            errors.append(f"{config_name} commands.{command_name} wrapper-routed local compile-heavy Rust must be remote-first")
        for storage_error in storage_errors:
            if storage_error == "BOLT_MANAGED_JUST private just recipe bypass must be classified":
                continue
            errors.append(f"{config_name} commands.{command_name} {storage_error}")
    return errors






def just_recipe_blocks(justfile_text: str) -> dict[str, tuple[list[str], list[str]]]:
    recipes: dict[str, tuple[list[str], list[str]]] = {}
    lines = justfile_text.splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or line[0].isspace() or ":=" in line:
            index += 1
            continue
        header, separator, tail = stripped.partition(":")
        if not separator:
            index += 1
            continue
        name = header.split()[0]
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_-]*", name):
            index += 1
            continue
        dependencies = [token for token in tail.split() if token and not token.startswith("#")]
        body: list[str] = []
        index += 1
        while index < len(lines):
            candidate = lines[index]
            if candidate.strip() and not candidate[0].isspace():
                break
            if candidate.strip():
                body.append(candidate.strip())
            index += 1
        recipes[name] = (dependencies, body)
    return recipes


def active_recipe_lines(recipes: dict[str, tuple[list[str], list[str]]], name: str) -> list[str]:
    return [line for line in (strip_comment(raw_line).strip() for raw_line in recipes[name][1]) if line]


LOCAL_VERIFICATION_GATE_RECIPES = (
    "fmt-check",
    "source-fence-static",
    "ci-lint-workflow",
)
ACTIONLINT_WORKFLOW_REQUIRED_COMMANDS = (
    "python3 scripts/test_ci_storage_audit.py",
)
CI_LINT_WORKFLOW_RUNNER_COMMAND = "python3 scripts/run_ci_lint_suites.py"
CI_LINT_WORKFLOW_RUNNER_LINE = 'if ! python3 scripts/run_ci_lint_suites.py "$@"; then'
SOURCE_FENCE_STATIC_INNER_REQUIRED_COMMANDS = (
    "python3 scripts/run_fences.py",
)


def local_verification_inner_errors(
    recipes: dict[str, tuple[list[str], list[str]]],
    inner_name: str,
) -> list[str]:
    if inner_name not in recipes:
        return []
    errors: list[str] = []
    dependencies, _body = recipes[inner_name]
    if any(dependency in LOCAL_VERIFICATION_GATE_RECIPES for dependency in dependencies):
        errors.append(f"justfile {inner_name} must not depend on local verification gate recipes")
    for line in active_recipe_lines(recipes, inner_name):
        if "scripts/local_verification_gate.py" in line:
            errors.append(f"justfile {inner_name} must not invoke local verification gate recipes")
            continue
        for recipe_name in LOCAL_VERIFICATION_GATE_RECIPES:
            if re.search(rf"\bjust\s+{re.escape(recipe_name)}\b", line):
                errors.append(f"justfile {inner_name} must not invoke local verification gate recipes")
                break
    return errors


def gated_inner_recipe_name(
    recipes: dict[str, tuple[list[str], list[str]]],
    public_name: str,
    inner_name: str,
    errors: list[str],
) -> str:
    if public_name not in recipes:
        errors.append(f"justfile {public_name} recipe is required")
        return public_name
    public_lines = active_recipe_lines(recipes, public_name)
    gate_command = f"python3 scripts/local_verification_gate.py {public_name} -- just {inner_name}"
    if gate_command not in public_lines:
        errors.append(f"justfile {public_name} must run through scripts/local_verification_gate.py")
        return public_name
    if public_lines != [gate_command]:
        errors.append(f"justfile {public_name} must contain only the local verification gate command")
    if inner_name not in recipes:
        errors.append(f"justfile {inner_name} recipe is required")
        return public_name
    inner_dependencies, _inner_body = recipes[inner_name]
    if "require-local-verification-gate" not in inner_dependencies:
        errors.append(f"justfile {inner_name} must require the local verification gate")
    errors.extend(local_verification_inner_errors(recipes, inner_name))
    return inner_name


def ci_lint_suite_commands(errors: list[str]) -> tuple[str, ...]:
    try:
        import run_ci_lint_suites

        return tuple(" ".join(suite.command) for suite in run_ci_lint_suites.CI_LINT_SUITES)
    except (AttributeError, ImportError, SyntaxError, TypeError) as exc:
        errors.append(f"ci-lint workflow runner suite table must be importable: {type(exc).__name__}: {exc}")
        return ()


def verify_local_verification_gate_recipes(justfile_text: str) -> list[str]:
    recipes = just_recipe_blocks(justfile_text)
    errors: list[str] = []
    for public_name, inner_name in (
        ("fmt-check", "fmt-check-inner"),
        ("ci-lint-workflow", "ci-lint-workflow-inner"),
    ):
        gated_inner_recipe_name(recipes, public_name, inner_name, errors)
    if "ci-lint-workflow-inner" in recipes:
        ci_lint_inner_lines = active_recipe_lines(recipes, "ci-lint-workflow-inner")
        runner_count = sum(1 for line in ci_lint_inner_lines if line == CI_LINT_WORKFLOW_RUNNER_LINE)
        if runner_count == 0:
            errors.append(f"justfile ci-lint-workflow-inner must run {CI_LINT_WORKFLOW_RUNNER_COMMAND}")
        elif runner_count > 1:
            errors.append(f"justfile ci-lint-workflow-inner must run {CI_LINT_WORKFLOW_RUNNER_COMMAND} exactly once")

        for line in ci_lint_inner_lines:
            if "run_ci_lint_suites.py" in line and line != CI_LINT_WORKFLOW_RUNNER_LINE:
                errors.append("justfile ci-lint-workflow-inner must not invoke the runner outside the pinned line")

        suite_commands = ci_lint_suite_commands(errors)
        for line in ci_lint_inner_lines:
            if any(command in line for command in suite_commands):
                errors.append(
                    "justfile ci-lint-workflow-inner must not run CI lint suite commands outside "
                    "scripts/run_ci_lint_suites.py"
                )
                break
    return errors


def verify_source_fence_static_recipe(justfile_text: str) -> list[str]:
    recipes = just_recipe_blocks(justfile_text)
    errors: list[str] = []
    if "source-fence-static" not in recipes:
        errors.append("justfile source-fence-static recipe is required")
        return errors
    if "source-fence" not in recipes:
        errors.append("justfile source-fence recipe is required")
        return errors
    source_fence_dependencies, source_fence_body = recipes["source-fence"]
    if "source-fence-static" not in source_fence_dependencies:
        errors.append("justfile source-fence must depend on source-fence-static")
    static_recipe_name = gated_inner_recipe_name(
        recipes,
        "source-fence-static",
        "source-fence-static-inner",
        errors,
    )
    static_lines = active_recipe_lines(recipes, static_recipe_name)
    static_body = "\n".join(static_lines)
    if command_has_managed_compile_heavy_invocation(static_body) or re.search(r"\brust_verification\.py\b[^\n]*\bcargo\b", static_body):
        errors.append("justfile source-fence-static must not invoke wrapper-routed Cargo")
    if "cargo fetch" in static_body or re.search(r"\bscripts/verify_runtime_capture_yaml\.py\b", static_body):
        errors.append("justfile source-fence-static must stop before cargo fetch and runtime capture verification")
    for command in SOURCE_FENCE_STATIC_INNER_REQUIRED_COMMANDS:
        if command not in static_lines:
            errors.append(f"justfile source-fence-static must run {command}")
    if static_lines != list(SOURCE_FENCE_STATIC_INNER_REQUIRED_COMMANDS):
        errors.append("justfile source-fence-static-inner must contain only python3 scripts/run_fences.py")
    full_body = "\n".join(source_fence_body)
    if "verify_runtime_capture_yaml.py" not in full_body:
        errors.append("justfile source-fence must keep runtime capture verification in the full recipe")
    return errors


def string_set(table: dict[str, object], key: str) -> set[str] | None:
    value = table.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return None
    return set(value)


def local_compile_policy_errors(data: dict[str, object], display_name: str) -> list[str]:
    policy = data.get("local_compile_policy")
    if not isinstance(policy, dict):
        return [f"{display_name} must define [local_compile_policy]"]
    errors: list[str] = []
    if policy.get("enabled") is not True:
        errors.append(f"{display_name} local_compile_policy.enabled must be true")
    if policy.get("allowed_ci_env") != "GITHUB_ACTIONS":
        errors.append(f"{display_name} local_compile_policy.allowed_ci_env must be GITHUB_ACTIONS")
    if policy.get("break_glass_env") != "BOLT_ALLOW_LOCAL_RUST":
        errors.append(f"{display_name} local_compile_policy.break_glass_env must be BOLT_ALLOW_LOCAL_RUST")
    if string_set(policy, "refused_managed_commands") != LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS:
        errors.append(f"{display_name} local_compile_policy.refused_managed_commands must be build/clippy/test")
    if string_set(policy, "refused_cargo_subcommands") != LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS:
        errors.append(f"{display_name} local_compile_policy.refused_cargo_subcommands must match disk preflight and aliases")
    return errors


def remote_verification_policy_errors(data: dict[str, object], display_name: str, *, required: bool) -> list[str]:
    policy = data.get("remote_verification")
    if policy is None:
        return [f"{display_name} must define [remote_verification]"] if required else []
    if not required:
        return [f"{display_name} must not define [remote_verification]"]
    if not isinstance(policy, dict):
        return [f"{display_name} remote_verification must be a table"]
    expected = {
        "poll_interval_seconds": 15,
        "checks_appear_timeout_seconds": 300,
        "overall_timeout_seconds": 3600,
        "diagnostic_log_max_lines": 160,
        "diagnostic_log_max_bytes": 20000,
        "diagnostic_unavailable_notice_interval_polls": 4,
    }
    errors: list[str] = []
    for key, value in expected.items():
        if policy.get(key) != value:
            errors.append(f"{display_name} remote_verification.{key} must be {value}")
    return errors


def load_rust_verification_policy_toml(path: pathlib.Path, display_name: str) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        raise
    except tomllib.TOMLDecodeError as exc:
        raise PolicyError(f"{display_name} is invalid TOML: {exc}") from exc
    except OSError as exc:
        raise PolicyError(f"{display_name} could not be read: {exc}") from exc


def verify_rust_verification_policy(path: pathlib.Path, *, require_remote: bool) -> list[str]:
    display_name = path.relative_to(REPO_ROOT).as_posix()
    try:
        data = load_rust_verification_policy_toml(path, display_name)
    except FileNotFoundError:
        return [f"{display_name} is required"]
    except PolicyError as exc:
        return [str(exc)]
    errors: list[str] = []
    if data.get("schema_version") != 2:
        errors.append(f"{display_name} schema_version must be 2")
    errors.extend(local_compile_policy_errors(data, display_name))
    errors.extend(remote_verification_policy_errors(data, display_name, required=require_remote))
    return errors


def verify_rust_verification_policies() -> list[str]:
    errors: list[str] = []
    errors.extend(verify_rust_verification_policy(DEFAULT_RUST_VERIFICATION_POLICY, require_remote=True))
    errors.extend(verify_rust_verification_policy(DEFAULT_BVS_RUST_VERIFICATION_POLICY, require_remote=False))
    return errors


def exact_head_governance_cache_errors(workflow_text: str) -> list[str]:
    for line in workflow_text.splitlines():
        clean = strip_comment(line)
        if "hashFiles(" not in clean:
            continue
        if "managed-target-v1-" not in clean and "nextest-archive-v1-" not in clean:
            continue
        if any(cache_input in clean for cache_input in FORBIDDEN_MANAGED_TARGET_CACHE_INPUTS):
            return ["managed target cache keys must use Rust-relevant inputs only"]
    return []


















def alias_payload_storage_messages(text: str, *, depth: int = 0) -> set[str]:
    if depth > 4:
        return set()
    messages: set[str] = set()
    segment: list[str] = []
    for token in command_tokens(text) + [";"]:
        if token in SHELL_COMMAND_BOUNDARIES:
            if segment and pathlib.Path(segment[0]).name == "alias":
                for payload in shell_alias_payloads(segment).values():
                    messages.update(raw_rust_storage_errors(payload, alias_depth=depth + 1))
            segment = []
            continue
        segment.append(token)
    return messages


def tokens_define_cargo_alias(tokens: list[str]) -> bool:
    segment: list[str] = []
    for token in tokens:
        if token in SHELL_COMMAND_BOUNDARIES:
            if segment and segment[0] == "alias" and simple_cargo_aliases(segment):
                return True
            segment = []
            continue
        segment.append(token)
    return bool(segment and segment[0] == "alias" and simple_cargo_aliases(segment))


def text_has_alias_cargo_target_routing_override(text: str) -> bool:
    cargo_aliases: set[str] = set()
    for line in text.splitlines():
        if not line.strip():
            continue
        tokens = command_tokens(line)
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                elif any(token in cargo_aliases for token in segment):
                    expanded = expand_cargo_aliases(segment, cargo_aliases)
                    if tokens_have_target_routing_override(expanded) and tokens_have_raw_cargo_launch(expanded):
                        return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
            continue
        if not any(token in cargo_aliases for token in segment):
            continue
        expanded = expand_cargo_aliases(segment, cargo_aliases)
        if tokens_have_target_routing_override(expanded) and tokens_have_raw_cargo_launch(expanded):
            return True
    return False


def folded_yaml_run_commands(text: str) -> list[str]:
    lines = text.splitlines()
    commands: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        match = YAML_FOLDED_RUN_LINE_RE.match(line)
        if match is None:
            index += 1
            continue
        base_indent = len(match.group(1))
        block: list[str] = []
        index += 1
        while index < len(lines):
            candidate = lines[index]
            if not candidate.strip():
                index += 1
                continue
            indent = len(candidate) - len(candidate.lstrip(" "))
            if indent <= base_indent:
                break
            block.append(candidate.strip())
            index += 1
        if block:
            commands.append(" ".join(block))
    return commands


def step_run_command(block: list[str]) -> str | None:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        value = match.group(2).strip()
        if not value:
            return ""
        if value[0] not in {"|", ">"}:
            return unquote_yaml_scalar(value)
        folded = value[0] == ">"
        base_indent = len(match.group(1))
        raw_command_lines: list[str] = []
        for nested in block[index + 1 :]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= base_indent:
                break
            raw_command_lines.append(nested_clean)
        command_indent = min(
            (len(command) - len(command.lstrip(" ")) for command in raw_command_lines),
            default=base_indent + 1,
        )
        command_lines = [
            command[command_indent:] if command.startswith(" " * command_indent) else command.lstrip(" ")
            for command in raw_command_lines
        ]
        if folded:
            return " ".join(command.strip() for command in command_lines)
        return "\n".join(command_lines)
    return None


def workflow_run_commands(workflow_text: str) -> list[str]:
    commands: list[str] = []
    for job_lines in parse_jobs(workflow_text).values():
        for block in step_blocks(job_lines):
            command = step_run_command(block)
            if command is not None:
                commands.append(command)
    return commands


def yaml_run_shell_texts(yaml_text: str) -> list[str]:
    lines = yaml_text.splitlines()
    texts: list[str] = []
    index = 0
    while index < len(lines):
        clean = strip_comment(lines[index]).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            index += 1
            continue
        value = match.group(2).strip()
        if not value:
            texts.append("")
            index += 1
            continue
        if value[0] not in {"|", ">"}:
            texts.append(unquote_yaml_scalar(value))
            index += 1
            continue

        folded = value[0] == ">"
        base_indent = len(match.group(1))
        command_lines: list[str] = []
        index += 1
        while index < len(lines):
            nested_clean = strip_comment(lines[index]).rstrip()
            if not nested_clean.strip():
                index += 1
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(" "))
            if indent <= base_indent:
                break
            command_lines.append(nested_clean.strip())
            index += 1
        texts.append(" ".join(command_lines) if folded else "\n".join(command_lines))
    return texts




def workflow_run_shell_texts(workflow_text: str) -> list[str]:
    texts: list[str] = []
    step_scopes = list(parse_jobs(workflow_text).values())
    runs_block = top_level_block(workflow_text, "runs")
    if any(
        (match := re.match(r"^\s*using:\s*(.*?)\s*$", strip_comment(line)))
        and unquote_yaml_scalar(match.group(1).strip()) == "composite"
        for line in runs_block
    ):
        step_scopes.append(runs_block)
    for job_lines in step_scopes:
        persisted_env: dict[str, str] = {}
        for block in step_blocks(job_lines):
            command = step_run_command(block)
            if command is None:
                continue
            parts = [f"{name}={value}" for name, value in persisted_env.items()]
            if command.strip():
                parts.append(command)
            texts.append("\n".join(parts))
            for assignment in github_env_assignment_lines(command):
                name, separator, value = assignment.partition("=")
                if separator and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
                    persisted_env[name] = value
    return texts


def add_unique_errors(errors: list[str], messages: Iterable[str]) -> None:
    for message in messages:
        if message not in errors:
            errors.append(message)


def raw_rust_storage_errors(workflow_text: str, *, alias_depth: int = 0) -> list[str]:
    uncommented = uncommented_text(workflow_text.splitlines())
    folded_command_texts = folded_yaml_run_commands(uncommented)
    yaml_command_texts = yaml_run_shell_texts(uncommented)
    folded_commands = "\n".join(folded_command_texts)
    text = re.sub(r"\\\s*\n\s*", " ", "\n".join(part for part in (uncommented, folded_commands) if part))
    shell_texts = workflow_run_shell_texts(uncommented)
    if not shell_texts:
        shell_texts = [uncommented]
    shell_texts.extend(folded_command_texts)
    shell_texts.extend(yaml_command_texts)
    shell_texts = [re.sub(r"\\\s*\n\s*", " ", shell_text) for shell_text in shell_texts]
    checks: tuple[tuple[str, str], ...] = (
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_TARGET_DIR[\"']?\s*(?:=|:)", "CARGO_TARGET_DIR raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_BUILD_TARGET_DIR[\"']?\s*(?:=|:)", "CARGO_BUILD_TARGET_DIR raw target override must be classified"),
        (r"(?:target-dir|build\.target-dir)[^\n]*>\s*\.cargo/config\.toml|\.cargo/config\.toml[^\n]*(?:target-dir|build\.target-dir)", ".cargo/config.toml build.target-dir raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_TARGET_TMPDIR[\"']?\s*(?:=|:)", "CARGO_TARGET_TMPDIR raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_INCREMENTAL[\"']?\s*(?:=|:)", "CARGO_INCREMENTAL raw cache override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_BUILD_RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "CARGO_BUILD_RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_ENCODED_RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "CARGO_ENCODED_RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_INSTALL_ROOT[\"']?\s*(?:=|:)", "CARGO_INSTALL_ROOT install output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_HOME[\"']?\s*(?:=|:)", "CARGO_HOME raw cache override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTUP_HOME[\"']?\s*(?:=|:)", "RUSTUP_HOME raw toolchain override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTC_WRAPPER[\"']?\s*(?:=|:)", "RUSTC_WRAPPER raw compiler wrapper must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTC_WORKSPACE_WRAPPER[\"']?\s*(?:=|:)", "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified"),
        (r"(^|[^A-Za-z0-9_$\{])[\"']?BOLT_ALLOW_LOCAL_RUST[\"']?\s*(?:=|:|<<)", "BOLT_ALLOW_LOCAL_RUST local Rust break-glass must not be checked in"),
        (r"(^|[^A-Za-z0-9_$\{])[\"']?BOLT_MANAGED_JUST[\"']?\s*(?:=|:|<<)", "BOLT_MANAGED_JUST private just recipe bypass must be classified"),
        (r"(^|[^A-Za-z0-9_$\{])[\"']?GITHUB_ACTIONS[\"']?\s*(?:=|:|<<)", "GITHUB_ACTIONS local CI spoof must not be checked in"),
        (r"\bno-mistakes\b[^\n]*\bcargo\b", "no-mistakes raw Cargo drift must be classified"),
        (r"\bno-mistakes\b[^\n]*--worktree[^\n]*(?:--target-dir\s+target|\btarget\b)", "no-mistakes worktree-local target path evidence must be reported"),
        (r"\bcargo\b[^\n|]*\$@[^|]*\|\s*bash\b[^\n;&|]*\s-s\b[^\n;&|]*\s--target-dir\b", "cargo --target-dir raw target override must be classified"),
    )
    errors: list[str] = []
    for pattern, message in checks:
        if re.search(pattern, text):
            errors.append(message)
    for shell_text in shell_texts:
        add_unique_errors(errors, sorted(text_raw_cargo_storage_override_messages(shell_text)))
        add_unique_errors(errors, sorted(dynamic_env_target_override_messages(shell_text)))
        add_unique_errors(errors, sorted(alias_payload_storage_messages(shell_text, depth=alias_depth)))
    config_file_message = "cargo --config file raw target override must be classified"
    if (
        text_has_path_style_cargo_config(text)
        or any(text_has_path_style_cargo_config(shell_text) for shell_text in shell_texts)
    ):
        add_unique_errors(errors, [config_file_message])
    target_override_message = "cargo --target-dir raw target override must be classified"
    if any(text_has_alias_cargo_target_routing_override(shell_text) for shell_text in shell_texts):
        add_unique_errors(errors, [target_override_message])
    for shell_text in shell_texts:
        add_unique_errors(errors, storage_transfer_policy_errors(shell_text))
    return errors




def test_has_shard_reproduction_command(job_lines: list[str]) -> bool:
    return job_runs_command(job_lines, TEST_REPRODUCTION_ECHO)


def test_has_inline_shard_reproduction_command(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        for line in block:
            clean = strip_comment(line).strip()
            if clean.startswith(("run:", "- run:")) and "reproduce" in clean.lower() and TEST_REPRODUCTION_COMMAND in clean:
                return True
    return False


def job_skips_tag_reuse(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return (
        has_line_matching(job_lines, TAG_SKIP_IF_RE)
        or has_line_matching(job_lines, TAG_SKIP_ALWAYS_IF_RE)
        or FULL_CI_REQUIRED_EXPR in text
    )


def job_if_uses_always(job_lines: list[str]) -> bool:
    return has_line_matching(job_lines, GATE_IF_RE) or "always()" in uncommented_text(job_lines)


def job_gates_on_full_ci_required(job_lines: list[str]) -> bool:
    return FULL_CI_REQUIRED_EXPR in uncommented_text(job_lines)


def source_fence_runs_on_full_ci_or_docs(job_lines: list[str]) -> bool:
    return job_if_value(job_lines) == SOURCE_FENCE_JOB_IF_VALUE


def source_fence_uses_policy_switch(job_lines: list[str]) -> bool:
    return any(block_run_body_matches(block, SOURCE_FENCE_POLICY_SWITCH) for block in step_blocks(job_lines))


def source_fence_checkout_uses_docs_head_ref(job_lines: list[str]) -> bool:
    checkout_blocks = action_blocks(job_lines, "actions/checkout@")
    return len(checkout_blocks) == 1 and block_has_input(checkout_blocks[0], "ref", SOURCE_FENCE_CHECKOUT_REF)


def check_aarch64_runs_on_full_or_tag_reuse(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return FULL_CI_REQUIRED_EXPR in text and TAG_REUSE_POLICY_EXPR in text


def same_sha_job_has_outputs(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "source_run_id: ${{ steps.evidence.outputs.source_run_id }}",
        "check_suite_id: ${{ steps.evidence.outputs.check_suite_id }}",
        "artifact_id: ${{ steps.evidence.outputs.artifact_id }}",
        "source_sha: ${{ steps.evidence.outputs.source_sha }}",
    )
    return all(item in text for item in required)


def same_sha_job_runs_resolver(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "id: evidence" in text and "python3 scripts/find_same_sha_main_evidence.py" in text


def fingerprint_reuse_job_has_outputs(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "reuse_found: ${{ steps.reuse.outputs.reuse_found }}",
        "source_run_id: ${{ steps.reuse.outputs.source_run_id }}",
        "source_sha: ${{ steps.reuse.outputs.source_sha }}",
        "source_artifact_id: ${{ steps.reuse.outputs.source_artifact_id }}",
        "root_run_id: ${{ steps.reuse.outputs.root_run_id }}",
        "root_head_sha: ${{ steps.reuse.outputs.root_head_sha }}",
        "root_fingerprint_digest: ${{ steps.reuse.outputs.root_fingerprint_digest }}",
        "reason: ${{ steps.reuse.outputs.reason }}",
    )
    return all(item in text for item in required)


def fingerprint_reuse_job_uses_secure_current_fingerprint(job_lines: list[str]) -> bool:
    reuse_step = unique_step_with_id(job_lines, "reuse")
    if reuse_step is None:
        return False
    downloads_current_fingerprint = any(
        block_has_input(block, "pattern", "nextest-archive-fingerprint-*")
        for block in action_blocks(job_lines, "actions/download-artifact@")
    )
    return (
        block_run_body_matches(reuse_step, NEXTEST_FINGERPRINT_REUSE_RESOLVER_RUN)
        and not downloads_current_fingerprint
    )


def fingerprint_reuse_base_step_is_canonical(job_lines: list[str]) -> bool:
    base_step = unique_step_with_id(job_lines, "reuse_provenance_base")
    return (
        base_step is not None
        and block_has_canonical_step_envelope(
            base_step,
            NEXTEST_FINGERPRINT_REUSE_BASE_STEP_ALLOWED_KEYS,
            NEXTEST_FINGERPRINT_REUSE_BASE_STEP_SCALARS,
            {"env": NEXTEST_FINGERPRINT_REUSE_BASE_ENV},
        )
        and block_run_body_matches(base_step, NEXTEST_FINGERPRINT_REUSE_BASE_RUN)
    )


def ci_provenance_base_step_is_canonical(job_lines: list[str]) -> bool:
    base_step = unique_step_with_id(job_lines, "provenance_base")
    return (
        base_step is not None
        and block_has_canonical_step_envelope(
            base_step,
            TRUSTED_BASE_STEP_ALLOWED_KEYS,
            CI_PROVENANCE_BASE_STEP_SCALARS,
            {"env": TRUSTED_BASE_ENV},
        )
        and block_run_body_matches(base_step, CI_PROVENANCE_BASE_RUN)
    )


def gate_verdict_base_step_is_canonical(job_lines: list[str]) -> bool:
    base_step = unique_step_with_id(job_lines, "verdict_base")
    return (
        base_step is not None
        and block_has_canonical_step_envelope(
            base_step,
            TRUSTED_BASE_STEP_ALLOWED_KEYS,
            VERDICT_BASE_STEP_SCALARS,
            {"env": TRUSTED_BASE_ENV},
        )
        and block_run_body_matches(base_step, VERDICT_BASE_RUN)
    )


def fingerprint_reuse_job_runs_resolver(job_lines: list[str]) -> bool:
    reuse_step = unique_step_with_id(job_lines, "reuse")
    if reuse_step is None:
        return False
    return block_run_body_matches(reuse_step, NEXTEST_FINGERPRINT_REUSE_RESOLVER_RUN)


def fingerprint_reuse_resolver_is_canonical(job_lines: list[str]) -> bool:
    reuse_step = unique_step_with_id(job_lines, "reuse")
    return reuse_step is not None and block_run_body_matches(
        reuse_step,
        NEXTEST_FINGERPRINT_REUSE_RESOLVER_RUN,
    )


def fingerprint_reuse_resolver_envelope_is_canonical(job_lines: list[str]) -> bool:
    reuse_step = unique_step_with_id(job_lines, "reuse")
    return reuse_step is not None and block_has_canonical_step_envelope(
        reuse_step,
        NEXTEST_FINGERPRINT_REUSE_RESOLVER_STEP_ALLOWED_KEYS,
        NEXTEST_FINGERPRINT_REUSE_RESOLVER_STEP_SCALARS,
        {"env": NEXTEST_FINGERPRINT_REUSE_RESOLVER_ENV},
    )


def fingerprint_reuse_resolver_uses_bash(job_lines: list[str]) -> bool:
    reuse_step = unique_step_with_id(job_lines, "reuse")
    if reuse_step is None:
        return False
    text = uncommented_text(reuse_step)
    return "id: reuse" in text and "shell: bash" in text


def fingerprint_reuse_uses_canonical_job_if(job_lines: list[str]) -> bool:
    return job_if_value(job_lines) == FINGERPRINT_REUSE_JOB_IF_VALUE


def fingerprint_reuse_skips_main_branch(job_lines: list[str]) -> bool:
    return MAIN_BRANCH_SKIP_EXPR in job_if_value(job_lines)


def fingerprint_reuse_gates_on_detector_allowed(job_lines: list[str]) -> bool:
    return FINGERPRINT_REUSE_ALLOWED_EXPR in job_if_value(job_lines)


def fingerprint_reuse_gates_on_consumer_events(job_lines: list[str]) -> bool:
    return FINGERPRINT_REUSE_CONSUMER_EVENTS_EXPR in job_if_value(job_lines)


def classified_top_level_env_errors(workflow_text: str, workflow_name: str) -> list[str]:
    errors = []
    scoped_keys = set(REUSE_RELEVANT_WORKFLOW_ENV_KEYS)
    overlap = sorted(scoped_keys & REUSE_NEUTRAL_TOP_LEVEL_ENV_KEYS)
    if overlap:
        keys = ", ".join(overlap)
        errors.append(f"top-level env keys cannot be both reuse-scoped and build-neutral: {keys}")

    try:
        env_lines = top_level_block_lines(workflow_text, "env")
    except ProvenanceError as exc:
        return errors + [f"top-level env reuse scope could not parse {workflow_name}: {exc}"]

    entry_lines = [
        structural_line
        for line in env_lines[1:]
        if (structural_line := workflow_yaml_structural_line(line))
    ]
    if entry_lines:
        minimum_indent = min(len(line) - len(line.lstrip(" \t")) for line in entry_lines)
        for line in entry_lines:
            indent = len(line) - len(line.lstrip(" \t"))
            if indent != minimum_indent:
                errors.append(f"top-level env entry must use canonical indentation: {line!r}")

    seen_keys = set()
    for line in top_level_env_immediate_entry_lines(workflow_text):
        entry = top_level_env_entry_key_value(line)
        if entry is None:
            errors.append(f"top-level env entry is unparsable for reuse classification: {line!r}")
            continue
        key, value = entry
        seen_keys.add(key)
        if key in scoped_keys and not reuse_scoped_env_value_uses_single_line_scalar(value):
            errors.append(
                f"top-level env.{key} must use a same-line scalar value; "
                f"top-level env.{key} must use a single-line scalar value without YAML anchors "
                "or aliases or YAML tags for nextest reuse scope"
            )
        if key not in scoped_keys and key not in REUSE_NEUTRAL_TOP_LEVEL_ENV_KEYS:
            errors.append(
                f"top-level env.{key} must be classified as reuse-scoped or build-neutral; "
                "add it to exactly one of ci_provenance.REUSE_RELEVANT_WORKFLOW_ENV_KEYS "
                "or REUSE_NEUTRAL_TOP_LEVEL_ENV_KEYS"
            )

    for key in sorted(scoped_keys - seen_keys):
        errors.append(f"top-level env.{key} is reuse-scoped but missing from {workflow_name}")

    return errors

def workflow_structural_line_has_yaml_anchor_or_alias(line: str) -> bool:
    quote: str | None = None
    index = 0
    while index < len(line):
        char = line[index]
        if quote == '"':
            if char == "\\":
                index += 2
                continue
            if char == '"':
                quote = None
        elif quote == "'":
            if char == "'":
                if index + 1 < len(line) and line[index + 1] == "'":
                    index += 2
                    continue
                quote = None
        else:
            if char in ("'", '"'):
                quote = char
            elif char == "#" and (index == 0 or line[index - 1].isspace()):
                break
            elif char in ("&", "*"):
                previous = line[index - 1] if index > 0 else ""
                next_char = line[index + 1] if index + 1 < len(line) else ""
                if (
                    (index == 0 or previous.isspace() or previous in "[{,:-")
                    and next_char
                    and not next_char.isspace()
                    and next_char not in "&*[]{}:,#"
                ):
                    return True
        index += 1
    return False


def workflow_yaml_anchor_alias_errors(workflow_text: str) -> list[str]:
    block_scalar_parent_indent: int | None = None
    for line in workflow_text.splitlines():
        if block_scalar_parent_indent is not None:
            indent = len(line) - len(line.lstrip(" \t"))
            if not line.strip() or indent > block_scalar_parent_indent:
                continue
            block_scalar_parent_indent = None

        structural_line = workflow_yaml_structural_line(line)
        if not structural_line.strip():
            continue
        if workflow_structural_line_has_yaml_anchor_or_alias(structural_line):
            return ["YAML anchors and aliases must not be used in ci.yml while nextest reuse is enabled"]
        if workflow_line_starts_block_scalar(structural_line):
            block_scalar_parent_indent = len(line) - len(line.lstrip(" \t"))
    return []


UNSUPPORTED_YAML_REUSE_FEATURE_ERROR = (
    "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml "
    "while nextest reuse is enabled"
)


def workflow_structural_line_has_yaml_tag(line: str) -> bool:
    stripped = workflow_yaml_structural_line(line).lstrip()
    if stripped.startswith("!"):
        return True

    mapping_value = workflow_structural_mapping_value(line)
    if mapping_value is not None and mapping_value.startswith("!"):
        return True

    sequence_value = workflow_structural_sequence_value(line)
    return sequence_value is not None and sequence_value.startswith("!")


def workflow_structural_line_has_explicit_key(line: str) -> bool:
    sequence_value = workflow_structural_sequence_value(line)
    stripped = (
        sequence_value
        if sequence_value is not None
        else workflow_yaml_structural_line(line).lstrip()
    )
    return stripped == "?" or stripped.startswith("? ")


def workflow_yaml_unsupported_feature_errors(workflow_text: str) -> list[str]:
    block_scalar_parent_indent: int | None = None
    for line in workflow_text.splitlines():
        if block_scalar_parent_indent is not None:
            indent = len(line) - len(line.lstrip(" \t"))
            if not line.strip() or indent > block_scalar_parent_indent:
                continue
            block_scalar_parent_indent = None

        structural_line = workflow_yaml_structural_line(line)
        stripped = structural_line.strip()
        if not stripped:
            continue

        if not line.startswith((" ", "\t")) and (
            stripped.startswith(("%", "---", "..."))
        ):
            return [UNSUPPORTED_YAML_REUSE_FEATURE_ERROR]
        if workflow_structural_line_has_yaml_tag(structural_line):
            return [UNSUPPORTED_YAML_REUSE_FEATURE_ERROR]
        if workflow_structural_line_has_explicit_key(structural_line):
            return [UNSUPPORTED_YAML_REUSE_FEATURE_ERROR]
        if workflow_line_starts_block_scalar(structural_line):
            block_scalar_parent_indent = len(line) - len(line.lstrip(" \t"))

    return []


def test_shards_skip_on_fingerprint_reuse(job_lines: list[str]) -> bool:
    return NEXTEST_REUSE_MISS_EXPR in uncommented_text(job_lines)


def test_accepts_fingerprint_reuse(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        'reuse_found="${{ needs.nextest-fingerprint-reuse.outputs.reuse_found }}"',
        'if [[ "$reuse_found" == "true" ]]; then',
        '"${{ needs.nextest-fingerprint-reuse.result }}" != "success"',
        "nextest fingerprint reuse did not expose source_run_id",
        "nextest fingerprint reuse did not expose source_sha",
        "nextest fingerprint reuse did not expose source_artifact_id",
        "nextest fingerprint reuse did not expose root_run_id",
        "nextest fingerprint reuse did not expose root_head_sha",
        "nextest fingerprint reuse did not expose root_fingerprint_digest",
        "nextest archive reused from run",
    )
    return all(item in text for item in required)


def ci_provenance_emit_runs_emitter(job_lines: list[str]) -> bool:
    if not ci_provenance_base_step_is_canonical(job_lines):
        return False
    text = uncommented_text(job_lines)
    required = (
        "steps.provenance_base.outputs.script",
        "steps.provenance_base.outputs.config",
        "steps.provenance_base.outputs.workflow",
        'ci_policy_path="${{ needs.ci-policy.outputs.ci_policy_path }}"',
        "policy_args=()",
        'python3 "$provenance_script" emit-full-ci --help | grep -q -- "--ci-policy-path"',
        'policy_args+=(--ci-policy-path "$ci_policy_path")',
        "trusted base provenance emitter does not support ci_policy_path=$ci_policy_path",
        "workflow_args=()",
        'python3 "$provenance_script" emit-full-ci --help | grep -q -- "--workflow-file"',
        'workflow_args+=(--workflow-file "$provenance_workflow")',
        'reuse_found="${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || \'false\' }}"',
        'if [[ "$reuse_found" == "true" ]]; then',
        'python3 "$provenance_script" emit-inherited-ci --help',
        "trusted base provenance emitter does not support inherited CI records",
        'python3 "$provenance_script" emit-inherited-ci',
        "--root-run-id \"${{ needs.nextest-fingerprint-reuse.outputs.root_run_id }}\"",
        "--root-head-sha \"${{ needs.nextest-fingerprint-reuse.outputs.root_head_sha }}\"",
        "--root-fingerprint-digest \"${{ needs.nextest-fingerprint-reuse.outputs.root_fingerprint_digest }}\"",
        'python3 "$provenance_script" emit-full-ci',
        '--config "$provenance_config"',
        '"${policy_args[@]}"',
        '"${workflow_args[@]}"',
        "--output ci-provenance.json",
    )
    return all(item in text for item in required)


def ci_provenance_emit_checks_needs(job_lines: list[str], needs: tuple[str, ...]) -> list[str]:
    text = uncommented_text(job_lines)
    errors = []
    for need in needs:
        if need == "build":
            expected = "--conditional-job build.result=${{ needs.build.result }}"
            if text.count(expected) < 2:
                errors.append("ci-provenance-emit must pass build.result from needs.build.result")
            continue
        expected = f"--required-job {need}=${{{{ needs.{need}.result }}}}"
        if text.count(expected) < 2:
            errors.append(f"ci-provenance-emit must pass {need} result from needs.{need}.result")
    if text.count("--conditional-job build.required=${{ needs.detector.outputs.build_required }}") < 2:
        errors.append("ci-provenance-emit must pass build.required from needs.detector.outputs.build_required")
    if (
        text.count(f'--nextest-fingerprint "{TEST_ARCHIVE_FINGERPRINT_OUTPUT}"') < 2
        or "--nextest-fingerprint-path" in text
    ):
        errors.append("ci-provenance-emit must use secure nextest fingerprint output")
    return errors


def ci_provenance_emit_upload_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    upload_blocks = [
        block
        for block in action_blocks(job_lines, "actions/upload-artifact@")
        if block_step_id(block) == "upload-ci-provenance"
    ]
    if not upload_blocks:
        errors.append("ci-provenance-emit must upload ci-provenance artifact from upload-ci-provenance")
        return errors
    if not any(block_has_input(block, "path", "ci-provenance.json") for block in upload_blocks):
        errors.append("ci-provenance-emit must upload ci-provenance.json")
    return errors


def capture_artifact_metadata_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    text = uncommented_text(job_lines)
    if "ci_provenance.py artifact-metadata" not in text:
        errors.append("capture must derive artifact metadata from ci_provenance.py artifact-metadata")
    if '--config "$CAPTURE_PROVENANCE_CONFIG"' not in text:
        errors.append("capture artifact metadata must use CAPTURE_PROVENANCE_CONFIG")
    if '--run-attempt "${{ github.run_attempt }}"' not in text:
        errors.append("capture artifact metadata must use github.run_attempt")

    upload_blocks = [
        block
        for block in action_blocks(job_lines, "actions/upload-artifact@")
        if block_has_input(block, "path", "${{ env.CAPTURE_OUTPUT_DIR }}")
    ]
    if not upload_blocks:
        errors.append("capture must upload CAPTURE_OUTPUT_DIR")
        return errors
    return errors


def ci_provenance_emit_records_secure_fingerprint(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return (
        text.count(f'--nextest-fingerprint "{TEST_ARCHIVE_FINGERPRINT_OUTPUT}"') >= 2
        and "--nextest-fingerprint-path" not in text
    )


def clippy_installs_aarch64_toolchain(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "gcc-aarch64-linux-gnu" in text or "libc6-dev-arm64-cross" in text


def check_aarch64_installs_cross_compiler_packages(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "gcc-aarch64-linux-gnu" in text and "libc6-dev-arm64-cross" in text


def check_aarch64_has_coverage_owner_step(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        text = uncommented_text(block)
        if "Resolve aarch64 coverage owner" not in text:
            continue
        return (
            "needs.detector.outputs.build_required" in text
            and "aarch64 coverage is provided by build" in text
            and "running standalone aarch64 check" in text
        )
    return False


def check_aarch64_standalone_guard_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    def has_build_required_guard(block: list[str]) -> bool:
        for line in block:
            text = strip_comment(line)
            if CHECK_AARCH64_STANDALONE_IF_RE.match(text):
                return True
            normalized = text.replace('"true"', "'true'")
            if (
                re.match(r"^\s+(?:-\s*)?if:\s*", normalized)
                and "needs.detector.outputs.build_required != 'true'" in normalized
                and "||" not in normalized
            ):
                return True
        return False

    checks = (
        (
            "check-aarch64 setup must run only when build_required is not true",
            lambda block: any("./.github/actions/setup-environment" in line for line in block),
        ),
        (
            "check-aarch64 compiler install must run only when build_required is not true",
            lambda block: "gcc-aarch64-linux-gnu" in uncommented_text(block)
            or "libc6-dev-arm64-cross" in uncommented_text(block),
        ),
        (
            "check-aarch64 cache must run only when build_required is not true",
            lambda block: any("Swatinem/rust-cache" in line for line in block),
        ),
        (
            "check-aarch64 managed target cache must run only when build_required is not true",
            block_uses_managed_target_cache,
        ),
        (
            "check-aarch64 command must run only when build_required is not true",
            lambda block: block_runs_command(block, "just check-aarch64"),
        ),
    )
    blocks = step_blocks(job_lines)
    for message, matches in checks:
        for block in blocks:
            if matches(block) and not has_build_required_guard(block):
                errors.append(message)
                break
    return errors


GATE_TAG_REUSE_CONDITION = '"$policy_path" == "tag_reuse"'
GATE_FULL_CONDITION = '"$policy_path" == "full"'
GATE_ITERATION_CONDITION = '"$policy_path" == "iteration"'
GATE_NOOP_CONDITION = '"$policy_path" == "noop"'
GATE_DEFER_CONDITION = '"$policy_path" == "defer" || "$full_ci_deferred" == "true"'
GATE_EXPECTED_EVENT_CLASS_ASSIGNMENT = 'expected_event_class="${{ needs.ci-policy.outputs.expected_event_class }}"'
GATE_DEFER_CONTEXT_FAILURE_CONDITION = '"$expected_event_class" != "defer"'
GATE_NOOP_CONTEXT_FAILURE_CONDITION = '"$expected_event_class" != "noop"'
GATE_ITERATION_CONTEXT_FAILURE_CONDITION = '"$expected_event_class" != "iteration"'


def gate_checks_lane_success(gate_text: str, job: str) -> bool:
    condition = f'"${{{{ needs.{job}.result }}}}" != "success"'
    return branch_exits_reachable(gate_text, "if", condition)


def top_level_if_body_and_remainder(gate_text: str, condition: str) -> tuple[str, str] | None:
    lines = gate_text.splitlines()
    for start, line in enumerate(lines):
        match = IF_OR_ELIF_RE.match(line)
        if not match or match.group(1) != "if" or match.group("condition") != condition:
            continue
        depth = 0
        for index in range(start + 1, len(lines)):
            nested_match = IF_OR_ELIF_RE.match(lines[index])
            if nested_match and nested_match.group(1) == "if":
                depth += 1
                continue
            if not FI_RE.match(lines[index]):
                continue
            if depth == 0:
                return "\n".join(lines[start + 1 : index]), "\n".join(lines[index + 1 :])
            depth -= 1
    return None


def gate_tag_reuse_body(gate_text: str) -> str:
    sections = top_level_if_body_and_remainder(gate_text, GATE_TAG_REUSE_CONDITION)
    return sections[0] if sections is not None else ""


def gate_standard_body(gate_text: str) -> str:
    sections = top_level_if_body_and_remainder(gate_text, GATE_TAG_REUSE_CONDITION)
    return sections[1] if sections is not None else ""


def gate_checks_standard_lane_success(gate_text: str, job: str) -> bool:
    return gate_checks_lane_success(gate_standard_body(gate_text), job)


def gate_checks_build_result(gate_text: str) -> bool:
    # These literals intentionally lock the current gate shell contract.
    # Any gate refactor must update this verifier and its self-tests together.
    required_condition = '"$build_required" == "true"'
    true_result_condition = '"$build_result" != "success"'
    optional_result_condition = '"$build_result" != "success" && "$build_result" != "skipped"'
    chain = if_chain_bodies(gate_text, required_condition)
    if chain is None:
        return False
    return (
        'build_required="${{ needs.detector.outputs.build_required }}"' in gate_text
        and 'build_result="${{ needs.build.result }}"' in gate_text
        and branch_exits_reachable(chain.get(("if", required_condition), ""), "if", true_result_condition)
        and body_exits(chain.get(("elif", optional_result_condition), ""))
    )




def deploy_downloads_same_sha_artifact(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "actions/download-artifact",
        "artifact-ids: ${{ needs.same-sha-main-evidence.outputs.artifact_id }}",
        "github-token: ${{ github.token }}",
        "repository: ${{ github.repository }}",
        "run-id: ${{ needs.same-sha-main-evidence.outputs.source_run_id }}",
    )
    return all(item in text for item in required)


def deploy_logs_reused_evidence(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "needs.same-sha-main-evidence.outputs.source_run_id",
        "needs.same-sha-main-evidence.outputs.check_suite_id",
        "needs.same-sha-main-evidence.outputs.artifact_id",
        "needs.same-sha-main-evidence.outputs.source_sha",
    )
    return all(item in text for item in required)


def detector_forces_build_on_workflow_dispatch(job_lines: list[str]) -> bool:
    # The push and workflow_dispatch cases are unified in a single `if` arm:
    #   if [[ "..." == "push" || "..." == "workflow_dispatch" ]]; then
    # Verify that this combined arm exists and unconditionally emits value=true.
    text = uncommented_text(job_lines)
    branch = branch_body(
        text,
        "if",
        '"${{ github.event_name }}" == "push" || "${{ github.event_name }}" == "workflow_dispatch"',
    )
    return branch is not None and 'echo "value=true" >> "$GITHUB_OUTPUT"' in branch


def detector_forces_build_on_merge_group(job_lines: list[str]) -> bool:
    # merge_group is a dedicated elif arm so the existing push/workflow_dispatch
    # arm string stays intact:
    #   elif [[ "${{ github.event_name }}" == "merge_group" ]]; then
    # A skipped required check counts as passing in GitHub, so the required-capable
    # build job must run on the merge commit; verify the arm emits value=true.
    text = uncommented_text(job_lines)
    branch = branch_body(
        text,
        "elif",
        '"${{ github.event_name }}" == "merge_group"',
    )
    return branch is not None and 'echo "value=true" >> "$GITHUB_OUTPUT"' in branch


def backtester_detect_forces_bvs_changed_on_merge_group(job_lines: list[str]) -> bool:
    # Backtester CI has its own change detector output. On merge_group it must
    # force the proof lanes to run; otherwise backtester-gate would no-op green.
    text = uncommented_text(job_lines)
    branch = branch_body(
        text,
        "elif",
        '"${{ github.event_name }}" == "merge_group"',
    )
    return (
        branch is not None
        and 'echo "bvs_changed=true" >> "$GITHUB_OUTPUT"' in branch
        and body_exits_zero(branch)
    )


def backtester_detect_forced_events_use_exact_head_namespace(job_lines: list[str]) -> bool:
    # Events that bypass the PR diff detector must not trust the head-controlled
    # cache input helper/config for opaque archive cache keys. They run the proof
    # lanes and use the exact-head bootstrap namespace instead.
    text = uncommented_text(job_lines)
    for branch_type, condition in (
        ("if", '"${{ github.event_name }}" == "push" || "${{ github.event_name }}" == "workflow_dispatch"'),
        ("elif", '"${{ github.event_name }}" == "merge_group"'),
    ):
        branch = branch_body(text, branch_type, condition)
        if branch is None:
            return False
        if 'echo "bvs_changed=true" >> "$GITHUB_OUTPUT"' not in branch:
            return False
        if 'echo "bvs_bootstrap_changed=true" >> "$GITHUB_OUTPUT"' not in branch:
            return False
        if not body_exits_zero(branch):
            return False
    return True


def git_diff_pathspecs(block_text: str) -> tuple[str, ...] | None:
    normalized = re.sub(r"\\\s*\n\s*", " ", block_text)
    matches = [
        tuple(token for token in command_tokens(match.group("paths")) if token)
        for match in re.finditer(
            r"git\s+diff\s+--name-only\b.*?\s--\s(?P<paths>.*?)\)",
            normalized,
            re.DOTALL,
        )
    ]
    if len(matches) != 1:
        return None
    return matches[0]


def detector_maps_changed_to_any_changed(block_text: str) -> bool:
    chain = if_chain_bodies(block_text, '-n "$changed"')
    if chain is None:
        return False
    true_write = 'echo "any_changed=true" >> "$GITHUB_OUTPUT"'
    false_write = 'echo "any_changed=false" >> "$GITHUB_OUTPUT"'
    return (
        true_write in chain.get(("if", '-n "$changed"'), "")
        and false_write in chain.get(("else", ""), "")
        and block_text.count(true_write) == 1
        and block_text.count(false_write) == 1
    )




def detector_self_authorizing_governance_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    step_block = unique_step_with_id(job_lines, "self_authorizing_governance")
    step_text = uncommented_text(step_block or [])
    if step_block is None or not block_has_canonical_step_envelope(
        step_block,
        SELF_AUTHORIZING_GOVERNANCE_STEP_ALLOWED_KEYS,
        SELF_AUTHORIZING_GOVERNANCE_STEP_SCALARS,
    ):
        errors.append("detector self-authorizing governance step must match canonical envelope")
    if step_block is None or not block_run_body_matches(
        step_block,
        SELF_AUTHORIZING_GOVERNANCE_RUN,
    ):
        errors.append("detector must hard-block self-authorizing governance edits")
    pathspecs = git_diff_pathspecs(step_text) if step_text else None
    if pathspecs != SELF_AUTHORIZING_GOVERNANCE_PATHS:
        errors.append("detector must inspect self-authorizing governance rule-files")
    return errors


def detector_fingerprint_reuse_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    text = uncommented_text(job_lines)
    fingerprint_inputs_text = ""
    allowance_text = ""
    detector_refs_block = unique_step_with_id(job_lines, "pr_refs")
    fingerprint_inputs_block = unique_step_with_id(job_lines, "fingerprint_reuse_inputs_changed")
    allowance_block = unique_step_with_id(job_lines, "fingerprint_reuse_allowed")
    for block in step_blocks(job_lines):
        block_text = uncommented_text(block)
        if step_has_id(block, "fingerprint_reuse_inputs_changed"):
            fingerprint_inputs_text = block_text
        if step_has_id(block, "fingerprint_reuse_allowed"):
            allowance_text = block_text
    if FINGERPRINT_REUSE_ALLOWED_OUTPUT not in text:
        errors.append("detector must expose fingerprint_reuse_allowed")
    if FINGERPRINT_REUSE_REASON_OUTPUT not in text:
        errors.append("detector must expose fingerprint_reuse_reason")
    if detector_refs_block is None or not block_has_canonical_step_envelope(
        detector_refs_block,
        DETECTOR_REFS_STEP_ALLOWED_KEYS,
        DETECTOR_REFS_STEP_SCALARS,
        {"env": DETECTOR_REFS_STEP_ENV},
    ):
        errors.append("detector base/head refs step must match canonical envelope")
    if detector_refs_block is None or not block_run_body_matches(
        detector_refs_block,
        DETECTOR_REFS_RUN,
    ):
        errors.append("detector base/head refs step must match canonical script")
    if fingerprint_inputs_block is None or not block_has_canonical_step_envelope(
        fingerprint_inputs_block,
        FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_ALLOWED_KEYS,
        FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_SCALARS,
    ):
        errors.append("detector fingerprint-reuse governance step must match canonical envelope")
    if fingerprint_inputs_block is None or not block_run_body_matches(
        fingerprint_inputs_block,
        FINGERPRINT_REUSE_INPUTS_CHANGED_RUN,
    ):
        errors.append("detector fingerprint-reuse governance step must match canonical script")
    pathspecs = git_diff_pathspecs(fingerprint_inputs_text) if fingerprint_inputs_text else None
    if pathspecs != FINGERPRINT_REUSE_GOVERNANCE_PATHS:
        errors.append("detector must detect fingerprint-reuse governance changes")
    if fingerprint_inputs_text and not detector_maps_changed_to_any_changed(fingerprint_inputs_text):
        errors.append("detector must map fingerprint-reuse governance changes to any_changed=true")
    if allowance_block is None or not block_has_canonical_step_envelope(
        allowance_block,
        FINGERPRINT_REUSE_ALLOWED_STEP_ALLOWED_KEYS,
        FINGERPRINT_REUSE_ALLOWED_STEP_SCALARS,
    ):
        errors.append("detector fingerprint-reuse allowance step must match canonical envelope")
    if allowance_block is None or not block_run_body_matches(
        allowance_block,
        FINGERPRINT_REUSE_ALLOWED_RUN,
    ):
        errors.append("detector fingerprint-reuse allowance step must match canonical script")
    allowance_chain = if_chain_bodies(
        allowance_text,
        '"${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true"',
    )
    if allowance_chain is None:
        errors.append("detector must determine fingerprint_reuse_allowed")
    elif (
        'echo "value=false" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(
            (
                "if",
                '"${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true"',
            ),
            "",
        )
        or 'echo "reason=governance-changed" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(
            (
                "if",
                '"${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true"',
            ),
            "",
        )
        or 'echo "value=true" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(
            (
                "elif",
                '"${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group"',
            ),
            "",
        )
        or 'echo "reason=consumer-event" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(
            (
                "elif",
                '"${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group"',
            ),
            "",
        )
        or 'echo "value=false" >> "$GITHUB_OUTPUT"' not in allowance_chain.get(("else", ""), "")
        or 'echo "reason=non-consumer-event" >> "$GITHUB_OUTPUT"' not in allowance_chain.get(("else", ""), "")
        or allowance_text.count('echo "value=false" >> "$GITHUB_OUTPUT"') != 2
        or allowance_text.count('echo "value=true" >> "$GITHUB_OUTPUT"') != 1
    ):
        errors.append("detector must determine fingerprint_reuse_allowed")
    if allowance_text.count('echo "reason=') != 3:
        errors.append("detector must explain fingerprint_reuse_allowed decisions")
    return errors


def detector_docs_only_archive_errors(job_lines: list[str]) -> list[str]:
    docs_only_block = unique_step_with_id(job_lines, "docs_only")
    if docs_only_block is None:
        return []
    text = uncommented_text(docs_only_block)
    errors: list[str] = []
    for required in (
        'git archive "$base_ref"',
        "ci/rust-verification.toml",
        "ci/github-actions-runners.toml",
        ".github/workflows/ci.yml",
        'python3 "$base_tree/scripts/verify_ci_path_filters.py"',
    ):
        if required not in text:
            errors.append(f"detector docs-only classifier base archive must include {required}")
    return errors


def base_ref_git_archive_commands(workflow_text: str) -> list[list[str]]:
    commands: list[list[str]] = []
    for job_lines in parse_jobs(workflow_text).values():
        for block in step_blocks(job_lines):
            for logical_line in shell_logical_lines(block_run_body(block)):
                tokens = command_tokens_with_line_boundaries(logical_line)
                index = 0
                while index + 2 < len(tokens):
                    if tokens[index : index + 3] != ["git", "archive", "$base_ref"]:
                        index += 1
                        continue
                    end = index + 3
                    while end < len(tokens) and tokens[end] not in {"|", "&&", "||", ";", "\n"}:
                        end += 1
                    commands.append(tokens[index:end])
                    index = end
    return commands


def base_ref_archive_scripts_directory_errors(workflow_text: str) -> list[str]:
    errors: list[str] = []
    for command in base_ref_git_archive_commands(workflow_text):
        archive_args = command[3:]
        script_args = [arg for arg in archive_args if arg.startswith("scripts/")]
        if "scripts/" in archive_args and all(arg == "scripts/" for arg in script_args):
            continue
        rendered = " ".join(command)
        errors.append(
            "base_ref git archive must archive scripts/ wholesale and must not list "
            f"individual scripts: {rendered}"
        )
    return errors


def deploy_verifies_downloaded_artifact_checksum(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "cd artifact" in text and "sha256sum -c bolt-v2.sha256" in text


def job_permission_has(job_lines: list[str], permission: str, value: str) -> bool:
    return any(re.match(rf"^\s+{re.escape(permission)}:\s*{re.escape(value)}\s*$", strip_comment(line)) for line in job_lines)


def workflow_permissions_have_actions_read(workflow_text: str) -> bool:
    return re.search(r"(?m)^permissions:\n(?:^\s+[A-Za-z0-9_-]+:\s+\w+\n)*^\s+actions:\s+read\s*$", workflow_text) is not None


def workflow_permissions_have_issues_read(workflow_text: str) -> bool:
    return re.search(r"(?m)^permissions:\n(?:^\s+[A-Za-z0-9_-]+:\s+\w+\n)*^\s+issues:\s+read\s*$", workflow_text) is not None


def configured_ci_provenance_dispatch_names() -> tuple[dict[str, str] | None, list[str]]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return None, config_errors
    assert config is not None
    ci_provenance = config.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        return None, ["ci/github-actions-runners.toml must define [ci_provenance]"]
    dispatch = ci_provenance.get("dispatch")
    if not isinstance(dispatch, dict):
        return None, ["ci_provenance.dispatch must be a table"]
    required_keys = ("run_name_default", "run_name_iteration")
    missing = sorted(
        key for key in required_keys
        if not isinstance(dispatch.get(key), str) or not cast(str, dispatch.get(key)).strip()
    )
    if missing:
        return None, [f"ci_provenance.dispatch must define non-empty string keys: {missing}"]
    return {key: cast(str, dispatch[key]) for key in required_keys}, []


def top_level_key_block_text(workflow_text: str, key: str) -> str:
    lines = workflow_text.splitlines()
    key_re = re.compile(rf"^{re.escape(key)}:\s*.*$")
    for index, line in enumerate(lines):
        clean = strip_comment(line)
        if not key_re.match(clean):
            continue
        block = [clean]
        for child_line in lines[index + 1 :]:
            child_clean = strip_comment(child_line)
            if child_clean and not child_clean.startswith((" ", "\t")):
                break
            block.append(child_clean)
        return "\n".join(block)
    return ""


def workflow_run_name_errors(workflow_text: str) -> list[str]:
    names, errors = configured_ci_provenance_dispatch_names()
    if names is None:
        return errors
    run_name_text = top_level_key_block_text(workflow_text, "run-name")
    if "run-name: >-" not in run_name_text:
        errors.append("workflow must define run-name for dispatch class markers")
    if "github.event.inputs.full_ci" in run_name_text or "dispatch:full" in run_name_text:
        errors.append("workflow run-name must not publish a dispatch full marker")
    if f"&& '{names['run_name_iteration']}'" not in run_name_text:
        errors.append("workflow run-name must publish configured dispatch iteration marker")
    if f"|| '{names['run_name_default']}' " + "}}" not in run_name_text:
        errors.append("workflow run-name must preserve configured non-dispatch name")
    return errors


def branch_body(gate_text: str, keyword: str, condition: str) -> str | None:
    pattern = re.compile(
        rf"^\s*{keyword}\s+\[\[\s*{re.escape(condition)}\s*\]\];\s*then\s*$\n(?P<body>.*?)(?=^\s*(?:elif|else|fi)\b)",
        re.MULTILINE | re.DOTALL,
    )
    match = pattern.search(gate_text)
    if match is None:
        return None
    return match.group("body")


def branch_exists(gate_text: str, keyword: str, condition: str) -> bool:
    return branch_body(gate_text, keyword, condition) is not None


def branch_exits(gate_text: str, keyword: str, condition: str) -> bool:
    body = branch_body(gate_text, keyword, condition)
    if body is None:
        return False
    return body_exits(body)


def shell_line_exit_codes(line: str) -> list[str | None]:
    codes: list[str | None] = []
    tokens = command_tokens(line)
    cursor = 0
    at_command_start = True
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            at_command_start = True
            cursor += 1
            continue
        token_name = pathlib.Path(token).name
        if at_command_start and token_name == "exit":
            code = tokens[cursor + 1] if cursor + 1 < len(tokens) and re.fullmatch(r"[0-9]+", tokens[cursor + 1]) else None
            codes.append(code)
        elif (
            at_command_start
            and token_name in {"command", "eval"}
            and cursor + 1 < len(tokens)
            and pathlib.Path(tokens[cursor + 1]).name == "exit"
        ):
            code_index = cursor + 2
            code = tokens[code_index] if code_index < len(tokens) and re.fullmatch(r"[0-9]+", tokens[code_index]) else None
            codes.append(code)
        at_command_start = False
        cursor += 1
    return codes


def shell_line_has_exit_command(line: str) -> bool:
    tokens = command_tokens(line)
    at_command_start = True
    for index, token in enumerate(tokens):
        if token in SHELL_COMMAND_BOUNDARIES:
            at_command_start = True
            continue
        token_name = pathlib.Path(token).name
        if at_command_start and token_name == "exit":
            return True
        if (
            at_command_start
            and token_name in {"command", "eval"}
            and index + 1 < len(tokens)
            and pathlib.Path(tokens[index + 1]).name == "exit"
        ):
            return True
        at_command_start = False
    return False


def shell_line_is_simple_exit(line: str) -> bool:
    tokens = command_tokens(line)
    if not tokens or pathlib.Path(tokens[0]).name != "exit":
        return False
    if len(tokens) == 1:
        return True
    return len(tokens) == 2 and re.fullmatch(r"[0-9]+", tokens[1]) is not None


def branch_is_reachable_before_top_level_exit(gate_text: str, keyword: str, condition: str) -> bool:
    depth = 0
    for line in shell_logical_lines(gate_text):
        clean = strip_comment(line).strip()
        if not clean:
            continue
        if FI_RE.match(line):
            depth = max(0, depth - 1)
            continue
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            if (
                depth == 0
                and branch_match.group(1) == keyword
                and branch_match.group("condition") == condition
            ):
                return True
            if branch_match.group(1) == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            continue
        if depth == 0 and shell_line_exit_codes(clean):
            return False
    return False


def branch_exits_reachable(gate_text: str, keyword: str, condition: str) -> bool:
    if not branch_is_reachable_before_top_level_exit(gate_text, keyword, condition):
        return False
    return branch_exits(gate_text, keyword, condition)


def body_exits(body: str) -> bool:
    return body_exits_with_code(body, "1")


def body_exits_zero(body: str) -> bool:
    return body_exits_with_code(body, "0")


def body_exits_with_code(body: str, code: str) -> bool:
    exit_codes: list[str | None] = []
    depth = 0
    for line in shell_logical_lines(body):
        clean = strip_comment(line).strip()
        if not clean:
            continue
        if FI_RE.match(line):
            depth = max(0, depth - 1)
            continue
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            if branch_match.group(1) == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            continue
        line_exit_codes = shell_line_exit_codes(clean)
        if depth != 0:
            continue
        if shell_line_has_exit_command(clean):
            if not shell_line_is_simple_exit(clean):
                return False
            exit_codes.extend(line_exit_codes)
            continue
        if clean.startswith("echo "):
            continue
        return False
    return exit_codes == [code]


def extract_action_input_block(action_text: str, input_name: str) -> list[str]:
    lines = action_text.splitlines()
    input_re = re.compile(rf"^  {re.escape(input_name)}:\s*$")
    next_input_re = re.compile(r"^  [A-Za-z0-9_.-]+:\s*$")
    for start, line in enumerate(lines):
        if not input_re.match(strip_comment(line)):
            continue
        end = len(lines)
        for index in range(start + 1, len(lines)):
            clean = strip_comment(lines[index])
            if clean and not clean.startswith((" ", "\t")):
                end = index
                break
            if next_input_re.match(clean):
                end = index
                break
        return lines[start:end]
    return []


def input_block_has_default_false(input_block: list[str]) -> bool:
    return any(re.match(r"^\s+default:\s*(['\"]?)false\1\s*$", strip_comment(line)) for line in input_block)


def input_block_has_default_empty(input_block: list[str]) -> bool:
    return any(re.match(r"^\s+default:\s*(?:\"\"|'')\s*$", strip_comment(line)) for line in input_block)


def action_step_line(action_text: str, step_name: str) -> int | None:
    pattern = re.compile(rf"^\s+-\s+name:\s*{re.escape(step_name)}\s*$")
    for line_number, line in enumerate(action_text.splitlines(), start=1):
        if pattern.match(strip_comment(line)):
            return line_number
    return None


def extract_action_output_block(action_text: str, output_name: str) -> list[str]:
    lines = action_text.splitlines()
    output_re = re.compile(rf"^  {re.escape(output_name)}:\s*$")
    next_output_re = re.compile(r"^  [A-Za-z0-9_.-]+:\s*$")
    for start, line in enumerate(lines):
        if not output_re.match(strip_comment(line)):
            continue
        end = len(lines)
        for index in range(start + 1, len(lines)):
            clean = strip_comment(lines[index])
            if clean and not clean.startswith((" ", "\t")):
                end = index
                break
            if next_output_re.match(clean):
                end = index
                break
        return lines[start:end]
    return []


def verify_workflow(workflow_text: str) -> list[str]:
    errors: list[str] = job_header_indent_errors(workflow_text)
    if SPIKE_PROBE_MARKER_RE.search(uncommented_text(workflow_text.splitlines())):
        errors.append("ci workflow must not contain BOLT_SPIKE probe instrumentation")
    errors.extend(workflow_steps_alias_errors(workflow_text))
    jobs = parse_jobs(workflow_text)
    triggers = workflow_trigger_keys(workflow_text)
    is_ci_topology = "pull_request" in triggers and "push" in triggers
    errors.extend(raw_rust_storage_errors(workflow_text))
    errors.extend(exact_head_governance_cache_errors(workflow_text))
    errors.extend(base_ref_archive_scripts_directory_errors(workflow_text))
    errors.extend(partition_workflow_boundary_errors(workflow_text, "ci.yml"))
    for job_lines in jobs.values():
        errors.extend(upload_artifact_pin_errors(job_lines))

    actual_pr_paths_ignore = extract_paths_ignore_for_trigger(workflow_text, "pull_request")
    if actual_pr_paths_ignore is not None:
        errors.append(
            "on.pull_request must have no paths-ignore so host-health and gate proof run on docs PRs; "
            f"got {actual_pr_paths_ignore!r}"
        )
    actual_push_paths_ignore = extract_paths_ignore_for_trigger(workflow_text, "push")
    if actual_push_paths_ignore is not None:
        errors.append(
            "on.push must have no paths-ignore (push to main/tags must always run full CI); "
            f"got {actual_push_paths_ignore!r}"
        )
    if is_ci_topology:
        errors.extend(workflow_pull_request_type_errors(workflow_text))
        if "workflow_dispatch" not in triggers:
            errors.append("workflow must define workflow_dispatch")
        if "full_ci:" in "\n".join(workflow_trigger_block(workflow_text, "workflow_dispatch")):
            errors.append("workflow_dispatch must not define a full_ci input")
        errors.extend(workflow_run_name_errors(workflow_text))
        if "merge_group" not in triggers:
            # The merge queue dispatches merge_group/checks_requested; required
            # checks that do not declare it never report and block the merge.
            errors.append("on must define merge_group for merge queue full CI")

    errors.extend(verify_pr_concurrency(workflow_text))

    if not workflow_permissions_have_actions_read(workflow_text):
        errors.append("workflow permissions must include actions: read")
    if not workflow_permissions_have_issues_read(workflow_text):
        errors.append("workflow permissions must include issues: read")

    for job in REQUIRED_JOBS:
        if job not in jobs:
            errors.append(f"missing required job {job}")

    if "detector" in jobs and not detector_forces_build_on_workflow_dispatch(jobs["detector"]):
        errors.append("detector must force build_required=true for workflow_dispatch runs")
    if "detector" in jobs and not detector_forces_build_on_merge_group(jobs["detector"]):
        errors.append("detector must force build_required=true for merge_group full CI")
    if "detector" in jobs:
        errors.extend(detector_self_authorizing_governance_errors(jobs["detector"]))
        errors.extend(detector_fingerprint_reuse_errors(jobs["detector"]))
        errors.extend(detector_docs_only_archive_errors(jobs["detector"]))

    if "ci-policy" in jobs:
        errors.extend(ci_policy_job_errors(jobs["ci-policy"]))

    if "capture" in jobs:
        errors.extend(capture_artifact_metadata_errors(jobs["capture"]))

    for job in TAG_SKIP_REQUIRED_JOBS:
        if job in jobs and not job_skips_tag_reuse(jobs[job]):
            errors.append(f"{job} must skip on tag reuse")

    if "source-fence" in jobs and "detector" not in extract_needs(jobs["source-fence"]):
        # FR-005: #342 owns the early-fail source-fence lane, so it remains detector-gated.
        errors.append("source-fence needs detector")
    if "source-fence" in jobs:
        source_fence_needs = extract_needs(jobs["source-fence"])
        if "ci-policy" not in source_fence_needs:
            errors.append("source-fence needs ci-policy")
        if not source_fence_runs_on_full_ci_or_docs(jobs["source-fence"]):
            errors.append("source-fence must run for full_ci_required or docs policy")
        if not source_fence_uses_policy_switch(jobs["source-fence"]):
            errors.append("source-fence must branch to just source-fence for full CI and just source-fence-static for docs policy")
        if not source_fence_checkout_uses_docs_head_ref(jobs["source-fence"]):
            errors.append("source-fence checkout must use pull_request head SHA for docs policy and github.sha otherwise")

    for job_name, recipe in JOB_REQUIRED_JUST_RECIPE.items():
        if job_name in jobs and not job_runs_command(jobs[job_name], f"just {recipe}"):
            errors.append(f"{job_name} must run just {recipe}")

    for job_name in sorted(CI_RUST_FAST_LINKER_JOBS):
        if job_name in jobs and not job_has_setup_input(jobs[job_name], "install-rust-linker", "true"):
            errors.append(f"ci.yml {job_name} must install configured Rust linker")

    if "deny" in jobs:
        deny_needs = extract_needs(jobs["deny"])
        if "detector" not in deny_needs:
            errors.append("deny needs detector")
        if "ci-policy" not in deny_needs:
            errors.append("deny needs ci-policy")
        if not job_gates_on_full_ci_required(jobs["deny"]):
            errors.append("deny must gate on full_ci_required")

    if "test-archive" in jobs:
        test_archive_needs = extract_needs(jobs["test-archive"])
        if "detector" not in test_archive_needs:
            errors.append("test-archive needs detector")
        # #400: source-fence and test-archive run in parallel. The aggregate
        # `gate` job is the sole merge enforcer for both lanes; reintroducing a
        # serial dep would re-create the fail-fast cost #400 eliminated.
        if "source-fence" in test_archive_needs:
            errors.append("test-archive must not need source-fence")
    if "test-shards" in jobs:
        errors.append("test-shards job must not reintroduce nextest archive artifact fan-out")

    for job_name, job_lines in jobs.items():
        if job_name != "test-archive" and "BOLT_RUST_VERIFICATION_SCCACHE" in uncommented_text(job_lines):
            errors.append("BOLT_RUST_VERIFICATION_SCCACHE opt-in must stay scoped to the test-archive job")

    if "clippy" in jobs:
        clippy_needs = extract_needs(jobs["clippy"])
        if "detector" not in clippy_needs:
            errors.append("clippy needs detector")
        if "ci-policy" not in clippy_needs:
            errors.append("clippy needs ci-policy")
        if not job_gates_on_full_ci_required(jobs["clippy"]):
            errors.append("clippy must gate on full_ci_required")
        clippy_text = uncommented_text(jobs["clippy"])
        if not job_runs_command(jobs["clippy"], "just fmt-check"):
            errors.append("clippy must run just fmt-check")
        if not job_has_setup_input(jobs["clippy"], "lint-workflow-contract", '"true"'):
            errors.append(".github/workflows/ci.yml clippy must enable workflow contract lint")
        if not job_has_toolchain_component(jobs["clippy"], "rustfmt"):
            errors.append(".github/workflows/ci.yml clippy must install rustfmt component")
        if "just check-aarch64" in clippy_text:
            errors.append("clippy must not run check-aarch64")
        if clippy_installs_aarch64_toolchain(jobs["clippy"]):
            errors.append("clippy must not install aarch64 cross compiler")

    if "check-aarch64" in jobs:
        check_aarch64_needs = extract_needs(jobs["check-aarch64"])
        if "detector" not in check_aarch64_needs:
            errors.append("check-aarch64 needs detector")
        if "ci-policy" not in check_aarch64_needs:
            errors.append("check-aarch64 needs ci-policy")
        if not check_aarch64_runs_on_full_or_tag_reuse(jobs["check-aarch64"]):
            errors.append("check-aarch64 must run on full CI or tag reuse")
        if not check_aarch64_has_coverage_owner_step(jobs["check-aarch64"]):
            errors.append("check-aarch64 must document build-lane aarch64 coverage delegation")
        if not check_aarch64_installs_cross_compiler_packages(jobs["check-aarch64"]):
            errors.append("check-aarch64 must install aarch64 cross compiler packages")
        errors.extend(check_aarch64_standalone_guard_errors(jobs["check-aarch64"]))

    if "nextest-fingerprint" in jobs:
        fingerprint_needs = extract_needs(jobs["nextest-fingerprint"])
        if "ci-policy" not in fingerprint_needs:
            errors.append("nextest-fingerprint needs ci-policy")
        if "detector" not in fingerprint_needs:
            errors.append("nextest-fingerprint needs detector")
        if not job_gates_on_full_ci_required(jobs["nextest-fingerprint"]):
            errors.append("nextest-fingerprint must gate on full_ci_required")
        if "test-archive" in jobs:
            errors.extend(nextest_fingerprint_errors(jobs["nextest-fingerprint"], jobs["test-archive"]))

    errors.extend(
        partition_job_body_digest_errors(
            label="test-archive",
            job_lines=jobs.get("test-archive"),
            expected_sha256=ROOT_TEST_ARCHIVE_JOB_SHA256,
            constant_name="ROOT_TEST_ARCHIVE_JOB_SHA256",
        )
    )
    if "test-archive" in jobs:
        test_archive_needs = extract_needs(jobs["test-archive"])
        if "ci-policy" not in test_archive_needs:
            errors.append("test-archive needs ci-policy")
        if "detector" not in test_archive_needs:
            errors.append("test-archive needs detector")
        if "nextest-fingerprint" not in test_archive_needs:
            errors.append("test-archive needs nextest-fingerprint")
        if "nextest-fingerprint-reuse" not in test_archive_needs:
            errors.append("test-archive needs nextest-fingerprint-reuse")
        if not job_if_uses_always(jobs["test-archive"]):
            errors.append("test-archive must use always()")
        if not job_gates_on_full_ci_required(jobs["test-archive"]):
            errors.append("test-archive must gate on full_ci_required")
        if NEXTEST_REUSE_MISS_EXPR not in uncommented_text(jobs["test-archive"]):
            errors.append("test-archive must skip on validated nextest fingerprint reuse")
        if "needs.nextest-fingerprint.result == 'success'" not in uncommented_text(jobs["test-archive"]):
            errors.append("test-archive must require nextest-fingerprint success")
        if "needs.detector.result == 'success'" not in uncommented_text(jobs["test-archive"]):
            errors.append("test-archive must require detector success")
        archive_lines = jobs["test-archive"]
        archive_text = uncommented_text(archive_lines)
        if named_step_block(archive_lines, "Run nextest archive partitions") is None:
            errors.append("test-archive must define Run nextest archive partitions step")
        archive_restore_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/restore@")
            if block_has_input(block, "path", "${{ env.NEXTEST_ARCHIVE_PATH }}")
        ]
        archive_save_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/save@")
            if block_has_input(block, "path", "${{ env.NEXTEST_ARCHIVE_PATH }}")
        ]
        archive_cache_blocks = archive_restore_blocks + archive_save_blocks
        sidecar_restore_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/restore@")
            if block_has_input(block, "path", "${{ env.ROOT_BIN_SIDECARS_PATH }}")
        ]
        sidecar_save_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/save@")
            if block_has_input(block, "path", "${{ env.ROOT_BIN_SIDECARS_PATH }}")
        ]
        sidecar_cache_blocks = sidecar_restore_blocks + sidecar_save_blocks
        archive_upload_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/upload-artifact@")
            if block_has_input(block, "name", "nextest-archive")
            and block_has_input(block, "path", "${{ env.NEXTEST_ARCHIVE_PATH }}")
        ]
        target_restore_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/restore@")
            if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
        ]
        target_save_blocks = [
            block
            for block in action_blocks(archive_lines, "actions/cache/save@")
            if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
        ]
        cache_key_step = named_step_block(archive_lines, TEST_ARCHIVE_CACHE_AUDIT_STEP)
        cache_key_step_text = uncommented_text(cache_key_step) if cache_key_step is not None else ""
        archive_s3_restore_block = named_step_block(archive_lines, "Restore nextest archive from S3")
        archive_s3_save_block = named_step_block(archive_lines, "Save nextest archive to S3")
        sidecar_s3_restore_block = named_step_block(archive_lines, "Restore root binary sidecars from S3")
        sidecar_s3_save_block = named_step_block(archive_lines, "Save root binary sidecars to S3")
        s3_eligibility_block = named_step_block(archive_lines, TEST_ARCHIVE_S3_ELIGIBILITY_STEP)
        s3_aws_block = named_step_block(archive_lines, TEST_ARCHIVE_S3_AWS_CONFIG_STEP)
        s3_summary_block = named_step_block(archive_lines, TEST_ARCHIVE_S3_SUMMARY_STEP)
        s3_summary_text = uncommented_text(s3_summary_block) if s3_summary_block is not None else ""
        target_cache_keys = [
            block_input_value(block, "key") or ""
            for block in target_restore_blocks + target_save_blocks
        ]
        if cache_key_step_text:
            target_cache_keys.append(cache_key_step_text)
        if TEST_ARCHIVE_PATH not in archive_text:
            errors.append("test-archive must declare nextest archive path")
        if TEST_ARCHIVE_SIDECAR_PATH not in archive_text:
            errors.append("test-archive must declare root binary sidecar path")
        if archive_cache_blocks or sidecar_cache_blocks:
            errors.append("test-archive payloads must use S3 artifact cache, not GitHub Actions cache")
        if any("hashFiles(" in (block_input_value(block, "key") or "") for block in archive_cache_blocks):
            errors.append("nextest archive cache key must use nextest fingerprint output")
        if TEST_ARCHIVE_CACHE_KEY not in cache_key_step_text:
            errors.append("nextest archive cache key must use nextest fingerprint output")
        if any("hashFiles(" in (block_input_value(block, "key") or "") for block in sidecar_cache_blocks):
            errors.append("root binary sidecar cache key must use nextest fingerprint output")
        if TEST_ARCHIVE_SIDECAR_CACHE_KEY not in cache_key_step_text:
            errors.append("root binary sidecar cache key must use nextest fingerprint output")
        for required in (
            TEST_ARCHIVE_S3_ENABLED_ENV,
            TEST_ARCHIVE_S3_BUCKET_ENV,
            TEST_ARCHIVE_S3_REGION_ENV,
            TEST_ARCHIVE_S3_PREFIX_ENV,
        ):
            if required not in archive_text:
                errors.append("test-archive payloads must use S3 artifact cache, not GitHub Actions cache")
                break
        if s3_eligibility_block is None or "continue-on-error: true" not in uncommented_text(s3_eligibility_block):
            errors.append("test-archive S3 artifact cache eligibility must be fail-open")
        if s3_aws_block is None or "continue-on-error: true" not in uncommented_text(s3_aws_block):
            errors.append("test-archive S3 artifact cache AWS credential setup must be fail-open")
        for block, label, key_output, path_var, object_fragment in (
            (archive_s3_restore_block, "nextest archive", TEST_ARCHIVE_CACHE_KEY_OUTPUT, "$NEXTEST_ARCHIVE_PATH", "/nextest-archive/${CACHE_KEY}.tar.zst"),
            (sidecar_s3_restore_block, "root binary sidecar", TEST_ARCHIVE_SIDECAR_CACHE_KEY_OUTPUT, "$ROOT_BIN_SIDECARS_PATH", "/root-bin-sidecars/${CACHE_KEY}.tar.gz"),
        ):
            text = uncommented_text(block) if block is not None else ""
            if block is None or TEST_ARCHIVE_S3_RESTORE_GUARD not in text:
                errors.append(f"test-archive must restore {label} from S3 fail-open")
            if key_output not in text or path_var not in text or object_fragment not in text or "aws s3 cp" not in text:
                errors.append(f"test-archive must restore {label} from S3 fail-open")
            if "cache-hit=false" not in text or "exit 0" not in text:
                errors.append(f"test-archive must restore {label} from S3 fail-open")
            if (
                "aws s3api head-object" not in text
                or 'Metadata."nextest-digest"' not in text
                or '"$metadata_digest" != "$DIGEST"' not in text
                or "exit 1" not in text
            ):
                errors.append(f"test-archive must fail closed on {label} S3 digest mismatch")
            if "Delete the object or repopulate it from a main push." not in text:
                errors.append(f"test-archive must explain recovery for {label} S3 digest mismatch")
            if TEST_ARCHIVE_RESTORE_RESULT_OUTPUT not in text or TEST_ARCHIVE_RESTORE_REASON_OUTPUT not in text:
                errors.append("test-archive must emit restore result and reason outputs")
        if s3_summary_block is None or "if: always()" not in s3_summary_text or TEST_ARCHIVE_S3_SUMMARY_AWS_ENV not in s3_summary_text:
            errors.append("test-archive must summarize nextest archive S3 restore state")
            errors.append("test-archive must summarize root binary sidecars S3 restore state")
        else:
            if (
                TEST_ARCHIVE_S3_SUMMARY_RESTORE_STATE not in s3_summary_text
                or TEST_ARCHIVE_S3_SUMMARY_NEXT_LINE not in s3_summary_text
            ):
                errors.append("test-archive must summarize nextest archive S3 restore state")
            if (
                TEST_ARCHIVE_S3_SUMMARY_RESTORE_STATE not in s3_summary_text
                or TEST_ARCHIVE_S3_SUMMARY_SIDECAR_LINE not in s3_summary_text
            ):
                errors.append("test-archive must summarize root binary sidecars S3 restore state")
        for block, label, key_output, path_var, object_fragment in (
            (archive_s3_save_block, "nextest archive", TEST_ARCHIVE_CACHE_KEY_OUTPUT, "$NEXTEST_ARCHIVE_PATH", "/nextest-archive/${CACHE_KEY}.tar.zst"),
            (sidecar_s3_save_block, "root binary sidecar", TEST_ARCHIVE_SIDECAR_CACHE_KEY_OUTPUT, "$ROOT_BIN_SIDECARS_PATH", "/root-bin-sidecars/${CACHE_KEY}.tar.gz"),
        ):
            text = uncommented_text(block) if block is not None else ""
            if block is None or TEST_ARCHIVE_S3_MAIN_SAVE_GUARD not in text or "continue-on-error: true" not in text:
                errors.append(f"test-archive must save {label} to S3 only from push-to-main")
            if (
                block is None
                or "steps.nextest-artifact-cache.outputs.cache_mode == 'read_write'" not in text
                or "steps.nextest-artifact-cache-aws.outcome == 'success'" not in text
            ):
                errors.append(f"test-archive must save {label} to S3 only from push-to-main with write credentials")
            if key_output not in text or path_var not in text or object_fragment not in text or "aws s3 cp" not in text:
                errors.append(f"test-archive must save {label} to S3 only from push-to-main")
            if (
                'save-status=skipped' not in text
                or 'save-status=success' not in text
                or 'save-status=failed' not in text
                or "exit 1" not in text
            ):
                errors.append(f"test-archive must emit explicit {label} S3 save status")
        if not job_has_setup_input(archive_lines, "include-managed-target-dir", '"true"'):
            errors.append("test-archive must opt into managed target dir")
        if not target_restore_blocks:
            errors.append("test-archive must restore archive build target cache")
        if any(
            TEST_ARCHIVE_TARGET_CACHE_RESTORE_GUARD not in uncommented_text(block)
            for block in target_restore_blocks
        ):
            errors.append("test-archive must restore target cache only while producing archive or sidecars")
        if not target_save_blocks:
            errors.append("test-archive must save archive build target cache")
        if any(
            TEST_ARCHIVE_TARGET_CACHE_SAVE_GUARD not in uncommented_text(block)
            for block in target_save_blocks
        ):
            errors.append("test-archive must save target cache only on target cache miss")
        if not target_restore_blocks or not target_save_blocks or not all(
            block_has_input(block, "key", TEST_ARCHIVE_TARGET_CACHE_KEY_OUTPUT)
            for block in target_restore_blocks + target_save_blocks
        ):
            errors.append("test-archive managed target cache key must use root nextest cache key output")
        if TEST_ARCHIVE_TARGET_CACHE_KEY not in cache_key_step_text:
            errors.append("test-archive cache persistence keys must come from single-source cache key outputs")
        if "nextest-archive-build-v1" in archive_text:
            errors.append("test-archive must not save a second archive-build cache")
        if archive_upload_blocks:
            errors.append("test-archive must not upload nextest archive artifact")
        if any(block_has_input(block, "restore-keys") for block in archive_cache_blocks):
            errors.append("test-archive cache must not use restore-keys")
        if any(block_has_input(block, "restore-keys") for block in sidecar_cache_blocks):
            errors.append("root binary sidecar cache must not use restore-keys")
        archive_build_block = named_step_block(archive_lines, "Build nextest archive")
        if archive_build_block is None or TEST_ARCHIVE_CACHE_HIT_GUARD not in uncommented_text(archive_build_block):
            errors.append("test-archive build must be skipped on archive cache hit")
        if archive_build_block is None or TEST_ARCHIVE_TEST_PROFILE_ENV not in uncommented_text(archive_build_block):
            errors.append("test-archive build must use test profile debug knob")
        if archive_build_block is None or TEST_ARCHIVE_SIDECAR_PROFILE_ENV not in uncommented_text(archive_build_block):
            errors.append("test-archive build must use dev profile debug knob for sidecars")
        if (
            TEST_ARCHIVE_SIDECAR_CACHE_HIT_GUARD not in archive_text
            or 'tar -xzf "$ROOT_BIN_SIDECARS_PATH" -C "${{ steps.setup.outputs.managed_target_dir }}"' not in archive_text
        ):
            errors.append("test-archive must extract cached root binary sidecars")
        sidecar_pack_block = named_step_block(archive_lines, "Pack root binary sidecars from archive build")
        if sidecar_pack_block is None or TEST_ARCHIVE_SIDECAR_PACK_GUARD not in uncommented_text(sidecar_pack_block):
            errors.append("test-archive must pack root binary sidecars from archive builds on archive-cache miss")
        if sidecar_pack_block is None or TEST_ARCHIVE_SIDECAR_PACK_COMMAND not in uncommented_text(sidecar_pack_block):
            errors.append("test-archive archive-miss sidecar pack must use tracked root binary sidecar helper")
        if TEST_ARCHIVE_SIDECAR_BUILD_COMMAND not in archive_text:
            errors.append("test-archive must build CARGO_BIN_EXE sidecars on sidecar cache miss")
        sidecar_block = named_step_block(archive_lines, "Build root binary sidecars")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_BUILD_GUARD not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar cargo build must run only on archive-cache hit and sidecar-cache miss")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_PROFILE_ENV not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar build must use dev profile debug knob")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_PACK_COMMAND not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar build must use tracked root binary sidecar helper")
        if 'just test-archive "$NEXTEST_ARCHIVE_PATH"' not in archive_text:
            errors.append("test-archive must build through just test-archive")
        for output in TEST_ARCHIVE_CACHE_AUDIT_OUTPUTS:
            if output not in archive_text:
                errors.append("test-archive must expose cache persistence audit outputs")
                break
        if "archive_build_target_cache_hit: ${{ steps.test-target-cache.outputs.cache-hit }}" in archive_text:
            errors.append("test-archive archive build target cache hit output must be explicit when restore is skipped")
        if "archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || steps.test-target-cache.outputs.cache-hit }}" in archive_text:
            errors.append("test-archive archive build target cache hit output must default cache misses to false")
        for output in TEST_ARCHIVE_CACHE_AUDIT_SAVE_OUTCOME_OUTPUTS:
            if output not in archive_text:
                errors.append("test-archive must expose cache persistence save outcomes")
                break
        for label, step_id in TEST_ARCHIVE_CACHE_SAVE_STEP_IDS:
            block = named_step_block(archive_lines, label)
            if block is None or step_id not in uncommented_text(block):
                errors.append("test-archive cache save steps must have stable ids for persistence evidence")
                break
        for label, step_id in TEST_ARCHIVE_CACHE_RESTORE_STEP_IDS:
            block = named_step_block(archive_lines, label)
            if block is None or step_id not in uncommented_text(block):
                errors.append("test-archive cache restore steps must have stable ids for persistence evidence")
                break
        if "id: cache-audit-keys" in archive_text or "Emit cache persistence audit keys" in archive_text:
            errors.append("test-archive cache persistence keys must come from single-source cache key outputs")
        if cache_key_step is None or TEST_ARCHIVE_CACHE_AUDIT_STEP_ID not in cache_key_step_text:
            errors.append("test-archive must resolve root nextest cache keys")
        elif (
            TEST_ARCHIVE_CACHE_KEY not in cache_key_step_text
            or TEST_ARCHIVE_SIDECAR_CACHE_KEY not in cache_key_step_text
            or TEST_ARCHIVE_TARGET_CACHE_KEY not in cache_key_step_text
            or not all(output in cache_key_step_text for output in TEST_ARCHIVE_CACHE_AUDIT_KEY_OUTPUTS)
        ):
            errors.append("test-archive cache persistence keys must come from single-source cache key outputs")
        # Fail-open contract for the S3 sccache compile cache (#1011): when the
        # opt-in is wired, the cache must never be able to fail the required build,
        # and cache use must be gated to trusted refs (the IAM trust scope is the
        # real poison boundary, but keep the workflow itself honest too).
        if "BOLT_RUST_VERIFICATION_SCCACHE" in archive_text:
            if TEST_ARCHIVE_SCCACHE_OPT_IN not in archive_text:
                errors.append("test-archive sccache opt-in must stay conditional on the resolver, never hardcoded")
            sccache_setup_block = named_step_block(archive_lines, "Setup governed sccache")
            sccache_setup_text = uncommented_text(sccache_setup_block) if sccache_setup_block is not None else ""
            if sccache_setup_block is None or f"uses: {SCCACHE_SETUP_ACTION_PATH}" not in sccache_setup_text:
                errors.append("test-archive sccache setup must route through the shared sccache action")
            for fragment in (
                "id: sccache",
                TEST_ARCHIVE_SCCACHE_ACTIVE_INPUT,
                SCCACHE_READONLY_ROLE_INPUT,
                TEST_ARCHIVE_SCCACHE_WRITE_ROLE_INPUT,
            ):
                if fragment not in sccache_setup_text:
                    errors.append(f"test-archive sccache setup must include {fragment!r}")
            # Value, not mere presence: the flag must be "1" so a future edit cannot
            # silently flip it to "0" and make S3/server I/O errors fatal.
            sccache_eligibility_text = repo_text_or_empty("scripts/sccache_eligibility.py")
            if TEST_ARCHIVE_SCCACHE_IGNORE_IO not in sccache_eligibility_text:
                errors.append('test-archive sccache must set SCCACHE_IGNORE_SERVER_IO_ERROR: "1" (degrade S3 errors to local compile)')
            # Even a mid-build sccache server crash (which SCCACHE_IGNORE_SERVER_IO_ERROR
            # does not cover) must not fail the build: rust_verification.py owns the
            # retry. Keep the workflow to one managed invocation.
            build_block = named_step_block(archive_lines, "Build nextest archive")
            build_text = uncommented_text(build_block) if build_block is not None else ""
            if build_block is None or TEST_ARCHIVE_OWNER_COMMAND not in build_text:
                errors.append("test-archive sccache build must route through the Rust verification owner")
            if "sccache-fail-open.sh" in build_text:
                errors.append("test-archive sccache retry must be owned by rust_verification.py, not workflow shell")
            if "AWS_CI_CACHE_ROLE_ARN" in sccache_setup_text.replace(TEST_ARCHIVE_SCCACHE_WRITE_ROLE_INPUT, ""):
                errors.append("test-archive sccache write role must only be passed to the shared sccache action")
            stats_block = named_step_block(archive_lines, "Print sccache stats")
            stats_text = uncommented_text(stats_block) if stats_block is not None else ""
            if stats_block is None or f"uses: {SCCACHE_STATS_ACTION_PATH}" not in stats_text:
                errors.append("test-archive sccache must print stats after the compile step")
            elif not step_block_has_field(stats_block, "if", "always()") or not step_occurs_after(archive_lines, "Print sccache stats", "Build nextest archive"):
                errors.append("test-archive sccache must print stats after the compile step")
        if TEST_ARCHIVE_DOWNLOAD_ACTION in archive_text:
            errors.append("test-archive must not download nextest archive artifact")
        if TEST_ARCHIVE_SHARDS_ASSIGNMENT not in archive_text or TEST_ARCHIVE_SHARDS_ASSERT not in archive_text:
            errors.append("test-archive must fail closed on invalid nextest shard count")
        if "for shard in 1 2 3 4" in archive_text or "count:${shard}/4" in archive_text or "{1..$shards}" in archive_text:
            errors.append("test-archive partition count must come from nextest fingerprint output")
        if TEST_ARCHIVE_PARTITION_LOOP not in archive_text:
            errors.append("test-archive must run all nextest archive partitions")
        if TEST_ARCHIVE_PARTITION_GROUP not in archive_text or TEST_REPRODUCTION_ECHO not in archive_text:
            errors.append("test-archive must log partition diagnostics")
        if TEST_PARTITION_COMMAND not in archive_text:
            errors.append("test-archive must run partitioned nextest from local archive")
        if TEST_ARCHIVE_EXTRACT_ROOT_INIT not in archive_text:
            errors.append("test-archive must create nextest archive extract root")
        if TEST_ARCHIVE_PARTITION_LOG_ASSIGN not in archive_text or TEST_ARCHIVE_PARTITION_TEE not in archive_text or TEST_ARCHIVE_PARTITION_LOG_TAIL not in archive_text:
            errors.append("test-archive must log partition diagnostics")
        if TEST_ARCHIVE_PARTITION_RC_CAPTURE not in archive_text:
            errors.append("test-archive partition failures must preserve shard exit codes")
        if TEST_ARCHIVE_PARTITION_ERROR_ANNOTATION not in archive_text:
            errors.append("test-archive partition failures must emit shard error annotations")
        for fragment in (
            TEST_ARCHIVE_PARTITION_STATUS_INIT,
            TEST_ARCHIVE_PARTITION_STATUS_MARK,
            TEST_ARCHIVE_PARTITION_STATUS_EXIT,
            TEST_ARCHIVE_PARTITION_FAILURE_WRAPPER,
        ):
            if fragment not in archive_text:
                errors.append("test-archive must aggregate partition failures")
                break

    append_cache_persistence_audit_contract_errors(errors, jobs)

    if "nextest-fingerprint-reuse" in jobs:
        errors.extend(workflow_yaml_anchor_alias_errors(workflow_text))
        errors.extend(workflow_yaml_unsupported_feature_errors(workflow_text))
        reuse_lines = jobs["nextest-fingerprint-reuse"]
        reuse_needs = extract_needs(reuse_lines)
        if "ci-policy" not in reuse_needs:
            errors.append("nextest-fingerprint-reuse needs ci-policy")
        if "detector" not in reuse_needs:
            errors.append("nextest-fingerprint-reuse needs detector")
        if "nextest-fingerprint" not in reuse_needs:
            errors.append("nextest-fingerprint-reuse needs nextest-fingerprint")
        if not job_if_uses_always(reuse_lines):
            errors.append("nextest-fingerprint-reuse must use always()")
        if not job_gates_on_full_ci_required(reuse_lines):
            errors.append("nextest-fingerprint-reuse must gate on full_ci_required")
        if not fingerprint_reuse_uses_canonical_job_if(reuse_lines):
            errors.append("nextest-fingerprint-reuse must use the canonical job if")
        if not fingerprint_reuse_gates_on_consumer_events(reuse_lines):
            errors.append("nextest-fingerprint-reuse must admit PR, workflow_dispatch, and merge_group consumers")
        if not fingerprint_reuse_skips_main_branch(reuse_lines):
            errors.append("nextest-fingerprint-reuse must skip main branch")
        if not fingerprint_reuse_gates_on_detector_allowed(reuse_lines):
            errors.append("nextest-fingerprint-reuse must gate on fingerprint_reuse_allowed")
        if not fingerprint_reuse_job_has_outputs(reuse_lines):
            errors.append("nextest-fingerprint-reuse must expose reuse provenance outputs")
        if not fingerprint_reuse_base_step_is_canonical(reuse_lines):
            errors.append("nextest-fingerprint-reuse must probe trusted base inherited emitter support")
        if not fingerprint_reuse_resolver_envelope_is_canonical(reuse_lines):
            errors.append("nextest-fingerprint-reuse resolver step must match canonical envelope")
        if not fingerprint_reuse_resolver_is_canonical(reuse_lines):
            errors.append("nextest-fingerprint-reuse resolver step must match canonical script")
        if not fingerprint_reuse_job_uses_secure_current_fingerprint(reuse_lines):
            errors.append("nextest-fingerprint-reuse must use secure current nextest fingerprint output")
        if not fingerprint_reuse_job_runs_resolver(reuse_lines):
            errors.append("nextest-fingerprint-reuse must run ci_provenance.py resolve-fingerprint")
        if not fingerprint_reuse_resolver_uses_bash(reuse_lines):
            errors.append("nextest-fingerprint-reuse resolver must use bash")

    if "test" in jobs:
        test_needs = extract_needs(jobs["test"])
        test_text = uncommented_text(jobs["test"])
        if "ci-policy" not in test_needs:
            errors.append("test needs ci-policy")
        if "detector" not in test_needs:
            errors.append("test needs detector")
        if not job_gates_on_full_ci_required(jobs["test"]):
            errors.append("test must gate on full_ci_required")
        if "nextest-fingerprint" not in test_needs:
            errors.append("test needs nextest-fingerprint")
        if "test-archive" not in test_needs:
            errors.append("test needs test-archive")
        if "nextest-fingerprint-reuse" not in test_needs:
            errors.append("test needs nextest-fingerprint-reuse")
        if not gate_checks_lane_success(test_text, "nextest-fingerprint"):
            errors.append("test must check needs.nextest-fingerprint.result")
        if not gate_checks_lane_success(test_text, "test-archive"):
            errors.append("test must check needs.test-archive.result")
        if not test_accepts_fingerprint_reuse(jobs["test"]):
            errors.append("test must accept validated nextest fingerprint reuse")
        if not job_if_uses_always(jobs["test"]):
            errors.append("test must use always()")
        if NEXTEST_REUSE_SUMMARY_LINE not in test_text:
            errors.append("test must summarize nextest fingerprint reuse decision")
        if any(fragment not in test_text for fragment in NEXTEST_REUSE_SUMMARY_ENV_LINES):
            errors.append("test must pass nextest reuse summary inputs through env")
        if any(fragment not in test_text for fragment in NEXTEST_REUSE_SUMMARY_ASSIGNMENTS):
            errors.append("test must read nextest reuse summary inputs from env")

    if "build" in jobs:
        build_needs = extract_needs(jobs["build"])
        if "detector" not in build_needs:
            errors.append("build needs detector")
        if "ci-policy" not in build_needs:
            errors.append("build needs ci-policy")
        if not job_gates_on_full_ci_required(jobs["build"]):
            errors.append("build must gate on full_ci_required")
        if not has_line_matching(jobs["build"], BUILD_IF_RE):
            errors.append("build must gate on needs.detector.outputs.build_required and skip tag reuse")

    if "ci-provenance-emit" in jobs:
        emit_lines = jobs["ci-provenance-emit"]
        emit_needs = extract_needs(emit_lines)
        if "ci-policy" not in emit_needs:
            errors.append("ci-provenance-emit needs ci-policy")
        for job in (*CI_PROVENANCE_REQUIRED_JOBS, "build"):
            if job not in emit_needs:
                errors.append(f"ci-provenance-emit needs {job}")
        if "nextest-fingerprint-reuse" not in emit_needs:
            errors.append("ci-provenance-emit needs nextest-fingerprint-reuse")
        if "gate" in emit_needs:
            errors.append("ci-provenance-emit must not need gate")
        if not job_if_uses_always(emit_lines):
            errors.append("ci-provenance-emit must use always()")
        if not job_skips_tag_reuse(emit_lines):
            errors.append("ci-provenance-emit must skip tag reuse")
        if not job_gates_on_full_ci_required(emit_lines):
            errors.append("ci-provenance-emit must gate on full_ci_required")
        if NEXTEST_REUSE_MISS_EXPR in uncommented_text(emit_lines):
            errors.append("ci-provenance-emit must emit inherited provenance during nextest fingerprint reuse")
        if not ci_provenance_emit_runs_emitter(emit_lines):
            errors.append("ci-provenance-emit must run provenance emitter")
        errors.extend(ci_provenance_emit_checks_needs(emit_lines, (*CI_PROVENANCE_REQUIRED_JOBS, "build")))
        errors.extend(ci_provenance_emit_upload_errors(emit_lines))
        if not ci_provenance_emit_records_secure_fingerprint(emit_lines):
            errors.append("ci-provenance-emit must record nextest fingerprint when present")

    if "same-sha-main-evidence" in jobs:
        if "detector" not in extract_needs(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence needs detector")
        if not has_line_matching(jobs["same-sha-main-evidence"], SAME_SHA_IF_RE):
            errors.append("same-sha-main-evidence must be tag-gated")
        if not same_sha_job_has_outputs(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence must expose source run, check suite, artifact, and SHA outputs")
        if not same_sha_job_runs_resolver(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence must run resolver script")

    if "gate" in jobs:
        gate_needs = extract_needs(jobs["gate"])
        gate_text = uncommented_text(jobs["gate"])
        if "ci-policy" not in gate_needs:
            errors.append("gate needs ci-policy")
        for job in GATE_REQUIRED:
            if job not in gate_needs:
                errors.append(f"gate needs {job}")
            required_arg = f"--job {job}=${{{{ needs.{job}.result }}}}"
            if required_arg not in gate_text:
                errors.append(f"gate shared verdict call must include {required_arg}")
        if "same-sha-main-evidence" not in gate_needs:
            errors.append("gate needs same-sha-main-evidence")
        for job in ("nextest-fingerprint", "test-archive"):
            if job not in gate_needs:
                errors.append(f"gate needs {job}")
        if "nextest-fingerprint-reuse" not in gate_needs:
            errors.append("gate needs nextest-fingerprint-reuse")
        if not gate_verdict_base_step_is_canonical(jobs["gate"]):
            errors.append("gate must use pinned trusted base-tree ci_provenance.py verdict")
        errors.extend(gate_policy_truth_table_errors(gate_text))
        errors.extend(gate_checks_same_sha_reuse(gate_text))
        errors.extend(gate_checks_nextest_fingerprint_reuse(gate_text))
        if not has_line_matching(jobs["gate"], GATE_IF_RE):
            errors.append("gate must use always()")
        if "nextest_fingerprint" in gate_text:
            errors.append("gate must not read nextest_fingerprint")

    if "deploy" in jobs:
        deploy_needs = extract_needs(jobs["deploy"])
        for job in DEPLOY_REQUIRED_NEEDS:
            if job not in deploy_needs:
                errors.append(f"deploy needs {job}")
        if not has_line_matching(jobs["deploy"], DEPLOY_IF_RE):
            errors.append("deploy must be tag-gated")
        if not job_permission_has(jobs["deploy"], "actions", "read"):
            errors.append("deploy permissions must include actions: read")
        if not deploy_downloads_same_sha_artifact(jobs["deploy"]):
            errors.append("deploy must download same-SHA main artifact by artifact ID")
        if not deploy_logs_reused_evidence(jobs["deploy"]):
            errors.append("deploy must log reused source run, check suite, artifact, and SHA")
        if not deploy_verifies_downloaded_artifact_checksum(jobs["deploy"]):
            errors.append("deploy must verify downloaded artifact checksum")

    for job, lines in jobs.items():
        uses_target_dir = job_uses_managed_target_dir(lines)
        opts_in = job_opts_into_managed_target_dir(lines)
        if uses_target_dir and not opts_in:
            errors.append(f"{job} uses managed target dir but setup does not opt in")
        if opts_in and not uses_target_dir:
            errors.append(f"{job} opts into managed target dir but does not use it")

    for job in TARGET_DIR_JOBS:
        if job in jobs and not job_uses_managed_target_dir(jobs[job]):
            errors.append(f"{job} must use setup.outputs.managed_target_dir or managed_target_dir_relative")

    for job in CACHE_KEY_JOBS:
        if job in jobs and not job_has_explicit_cache_key(jobs[job]):
            errors.append(f"{job} must declare explicit rust-cache key or shared-key")

    for job in REGISTRY_CACHE_JOBS:
        if job in jobs:
            errors.extend(shared_registry_cache_errors(job, jobs[job]))

    for job in MANAGED_TARGET_CACHE_KEYS:
        if job in jobs:
            errors.extend(managed_target_cache_errors(job, jobs[job]))

    return errors


def verify_managed_workflow(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []
    jobs = parse_jobs(workflow_text)

    for job, lines in jobs.items():
        lanes = job_just_lanes(lines)
        if not lanes:
            continue
        if not setup_action_blocks(lines):
            errors.append(f"{workflow_name} {job} must use setup-environment")
            continue
        if not job_has_setup_input(lines, "just-version", "${{ env.JUST_VERSION }}"):
            errors.append(f"{workflow_name} {job} setup just-version must come from env.JUST_VERSION")
        if "clippy" in lanes and not job_has_toolchain_component(lines, "clippy"):
            errors.append(f"{workflow_name} {job} must install clippy component")
        if lanes.intersection({"deny", "deny-advisories"}):
            if not job_has_setup_input(lines, "include-deny-version", '"true"'):
                errors.append(f"{workflow_name} {job} must include deny version")
            if "steps.setup.outputs.deny_version" not in uncommented_text(lines):
                errors.append(f"{workflow_name} {job} must use setup.outputs.deny_version")
        if lanes.intersection({"test", "test-archive", "test-archive-run"}):
            if not job_has_setup_input(lines, "include-nextest-version", '"true"'):
                errors.append(f"{workflow_name} {job} must include nextest version")
            if "steps.setup.outputs.nextest_version" not in uncommented_text(lines):
                errors.append(f"{workflow_name} {job} must use setup.outputs.nextest_version")
        if "check-aarch64" in lanes:
            if not job_has_setup_input(lines, "include-build-values", '"true"'):
                errors.append(f"{workflow_name} {job} must include build values")
            if not job_has_setup_input(lines, "use-default-target", '"true"'):
                errors.append(f"{workflow_name} {job} must use default target")
        if "build" in lanes:
            if not job_has_setup_input(lines, "include-build-values", '"true"'):
                errors.append(f"{workflow_name} {job} must include build values")
            if not job_has_setup_input(lines, "use-default-target", '"true"'):
                errors.append(f"{workflow_name} {job} must use default target")
            text = uncommented_text(lines)
            if "steps.setup.outputs.zig_version" not in text:
                errors.append(f"{workflow_name} {job} must use setup.outputs.zig_version")
            if "steps.setup.outputs.zigbuild_version" not in text:
                errors.append(f"{workflow_name} {job} must use setup.outputs.zigbuild_version")

    return errors


def verify_build_artifacts(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []
    if REPO_LOCAL_ARTIFACT_RE.search(uncommented_text(workflow_text.splitlines())):
        errors.append(f"{workflow_name} must not reference repo-local target release artifacts")

    jobs = parse_jobs(workflow_text)
    build = jobs.get("build")
    if build is None:
        return errors
    build_text = uncommented_text(build)
    if BINARY_PATH_COMMAND not in build_text:
        errors.append(f"{workflow_name} build must resolve artifact through rust_verification_owner binary-path")
    if 'cp "$binary_path" "$stage_dir/bolt-v2"' not in build_text:
        errors.append(f"{workflow_name} build must copy the managed binary into a staged artifact directory")
    if "steps.managed_artifact.outputs.stage_dir" not in build_text:
        errors.append(f"{workflow_name} build upload must use the staged artifact directory")
    binary_upload_blocks = [
        block
        for block in action_blocks(build, "actions/upload-artifact@")
        if block_step_id(block) == "upload-bolt-v2-binary"
    ]
    if not binary_upload_blocks:
        errors.append(f"{workflow_name} build must upload the staged binary from upload-bolt-v2-binary")
    return errors


def verify_prebuilt_tool_installs(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []

    jobs = parse_jobs(workflow_text)
    for job, job_lines in jobs.items():
        for tool in sorted(cargo_install_source_build_tools_in_text(uncommented_text(job_lines))):
            errors.append(f"{workflow_name} {job} must not compile {tool} from source")

    for job, (tool, output) in CI_INSTALL_ACTION_TOOLS.items():
        job_lines = jobs.get(job)
        if job_lines is None:
            continue
        step = install_action_tool_step(job_lines, tool, output)
        if step is None:
            errors.append(f"{workflow_name} {job} must install {tool} with pinned taiki-e/install-action")
            continue
        install_index, block = step
        if not block_has_input(block, "fallback", "none"):
            errors.append(f"{workflow_name} {job} install-action fallback must be none")
        command = CI_INSTALL_ACTION_COMMANDS[job]
        command_index = first_step_running_command(job_lines, command)
        if command_index is not None and install_index >= command_index:
            errors.append(f"{workflow_name} {job} must install {tool} before {command}")

    return errors


def verify_setup_action(action_text: str) -> list[str]:
    errors: list[str] = []
    uncommented_lines = [strip_comment(line) for line in action_text.splitlines()]
    uncommented = "\n".join(uncommented_lines)
    install_just_step = named_step_block(action_text.splitlines(), "Install just")
    if (
        install_just_step is None
        or not block_uses_pinned_install_action(install_just_step)
        or not block_has_input(install_just_step, "tool", SETUP_JUST_TOOL)
    ):
        errors.append("setup action must install just with pinned taiki-e/install-action")
    elif not block_has_input(install_just_step, "fallback", "none"):
        errors.append("setup action just install-action fallback must be none")
    step_lines = [action_step_line(action_text, step) for step in SETUP_ACTION_ORDERED_STEPS]
    if any(line is None for line in step_lines):
        errors.append("setup action missing required ordered steps")
    elif any(left >= right for left, right in zip(step_lines, step_lines[1:]) if left is not None and right is not None):
        errors.append("setup action step order drifted")
    for literal in SETUP_ACTION_REQUIRED_LITERALS:
        if literal not in uncommented:
            errors.append(f"setup action missing expected literal {literal!r}")
    if SETUP_FAST_LINKER_FAIL_OPEN_WARNING not in uncommented:
        errors.append("setup action Rust linker install failures must fail open")
    for output_name, output_mapping in SETUP_ACTION_OUTPUT_MAPPINGS.items():
        output_block = extract_action_output_block(action_text, output_name)
        if not output_block:
            errors.append(f"setup action missing exported output {output_name!r}")
        elif output_mapping not in uncommented_text(output_block):
            errors.append(f"setup action missing output mapping for {output_name!r}")
    target_dir_input = extract_action_input_block(action_text, "include-managed-target-dir")
    if not target_dir_input:
        errors.append("setup action missing include-managed-target-dir input")
    elif not input_block_has_default_false(target_dir_input):
        errors.append("setup action include-managed-target-dir default must be false")
    cargo_build_jobs_input = extract_action_input_block(action_text, "build-jobs-key")
    if not cargo_build_jobs_input:
        errors.append("setup action missing build-jobs-key input")
    elif not input_block_has_default_empty(cargo_build_jobs_input):
        errors.append("setup action build-jobs-key default must be empty")
    rust_linker_input = extract_action_input_block(action_text, "install-rust-linker")
    if not rust_linker_input:
        errors.append("setup action missing install-rust-linker input")
    elif not input_block_has_default_false(rust_linker_input):
        errors.append("setup action install-rust-linker default must be false")
    if not any(SETUP_TARGET_DIR_EXPORT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action must export managed_target_dir from target_dir step")
    if not any(SETUP_TARGET_DIR_RELATIVE_EXPORT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action must export managed_target_dir_relative from target_dir step")
    if not any(line.strip() == SETUP_TARGET_DIR_RELATIVE_COMPUTE for line in uncommented_lines):
        errors.append("setup action target_dir step must compute managed_target_dir_relative from workspace to target dir")
    if not any(SETUP_TARGET_DIR_RELATIVE_OUTPUT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action target_dir step must write managed_target_dir_relative")
    if not any(SETUP_TARGET_DIR_IF_RE.match(line) for line in uncommented_lines):
        errors.append("setup action target dir step must be conditional")
    if not any(SETUP_CARGO_BUILD_JOBS_ENV_OUTPUT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action must export configured CARGO_BUILD_JOBS to GITHUB_ENV")
    return errors


def rust_text_has_test_attr(masked_text: str) -> bool:
    return RUST_TEST_ATTR_RE.search(masked_text) is not None


def rust_inner_attr_is_banned(attr_name: str) -> bool:
    return attr_name in BANNED_RUST_INNER_ATTRS or attr_name.startswith("crate_")


def format_banned_inner_attr(attr_name: str) -> str:
    return f"#![{attr_name}(...)]"


def test_manifest_referenced_by(manifest: CiTestManifest) -> dict[str, list[str]]:
    referenced_by: dict[str, list[str]] = {}
    for harness, members in manifest.harness_to_members.items():
        for member in members:
            if member == harness:
                continue
            referenced_by.setdefault(member, []).append(harness)
    return referenced_by


def verify_test_harness_manifest(
    *,
    cargo_manifest_path: pathlib.Path | str | None = None,
    tests_root: pathlib.Path | str | None = None,
    workflow_path: pathlib.Path | str | None = None,
    justfile_path: pathlib.Path | str | None = None,
) -> list[str]:
    cargo_manifest = pathlib.Path(cargo_manifest_path) if cargo_manifest_path is not None else REPO_ROOT / "Cargo.toml"
    root = pathlib.Path(tests_root) if tests_root is not None else REPO_ROOT / "tests"
    workflow = pathlib.Path(workflow_path) if workflow_path is not None else DEFAULT_WORKFLOW
    justfile = pathlib.Path(justfile_path) if justfile_path is not None else REPO_ROOT / "justfile"
    errors: list[str] = []

    try:
        with cargo_manifest.open("rb") as handle:
            cargo_config = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return [f"{cargo_manifest.name} could not be parsed for explicit test harness governance: {exc}"]

    package = cargo_config.get("package")
    if not isinstance(package, dict) or package.get("autotests") is not False:
        errors.append(f"{cargo_manifest.name} [package].autotests must be false for explicit test harnesses")

    try:
        manifest = build_test_manifest(cargo_manifest, root)
    except Exception as exc:
        errors.append(f"{cargo_manifest.name} explicit test harness manifest could not be built: {exc}")
        return errors

    harness_roots = set(manifest.harness_to_members)
    if len(harness_roots) != EXPECTED_HARNESS_COUNT:
        errors.append(
            f"{cargo_manifest.name} explicit test harness count must be {EXPECTED_HARNESS_COUNT}, got {len(harness_roots)}"
        )

    referenced_by = test_manifest_referenced_by(manifest)
    for harness, members in manifest.harness_to_members.items():
        for member in members:
            if member in harness_roots and member != harness:
                errors.append(f"tests/{member}.rs is a harness root and must not be mod-ed by harness {harness}")

    for stem, harnesses in sorted(referenced_by.items()):
        if len(harnesses) <= 1:
            continue
        unique_harnesses = sorted(set(harnesses))
        test_path = root / f"{stem}.rs"
        if len(unique_harnesses) == 1:
            errors.append(f"{test_path.relative_to(root.parent).as_posix()} is registered multiple times by harness {unique_harnesses[0]}")
        else:
            errors.append(
                f"{test_path.relative_to(root.parent).as_posix()} is registered by multiple harnesses: {', '.join(unique_harnesses)}"
            )

    for test_path in sorted(root.glob("*.rs")):
        stem = test_path.stem
        try:
            masked_text = _mask_rust_non_code(test_path.read_text(encoding="utf-8"))
        except OSError as exc:
            errors.append(f"{test_path.relative_to(root.parent).as_posix()} could not be read: {exc}")
            continue
        rel_path = test_path.relative_to(root.parent).as_posix()
        has_test_attr = rust_text_has_test_attr(masked_text)
        if stem not in harness_roots:
            for attr_name in RUST_INNER_ATTR_RE.findall(masked_text):
                if rust_inner_attr_is_banned(attr_name):
                    errors.append(
                        f"{rel_path} uses banned module-level inner attribute {format_banned_inner_attr(attr_name)}"
                    )
        if stem in harness_roots:
            continue
        if stem in DECLARED_TOP_LEVEL_TEST_HELPERS:
            if has_test_attr:
                errors.append(f"{rel_path} is declared as a test helper but contains #[test]")
            continue
        harnesses = referenced_by.get(stem, [])
        if has_test_attr:
            if not harnesses:
                errors.append(f"{rel_path} has #[test] but is not registered in any explicit test harness")
            elif len(harnesses) == 1 and manifest.member_to_harness.get(stem) == harnesses[0]:
                continue
            else:
                errors.append(f"{rel_path} has #[test] but is not registered by exactly one explicit test harness")
            continue
        errors.append(f"{rel_path} is neither a harness root, a #[test]-bearing registered member, nor a declared test helper")

    for file_name, path in ((".github/workflows/ci.yml", workflow), ("justfile", justfile)):
        if not path.exists():
            continue
        errors.extend(verify_test_harness_test_args(file_name, path.read_text(encoding="utf-8"), manifest))

    return errors


def verify_test_harness_test_args(file_name: str, text: str, manifest: CiTestManifest) -> list[str]:
    errors: list[str] = []
    harness_roots = set(manifest.harness_to_members)
    for match in re.finditer(r"['\"]?--test['\"]?(?:=|\s+)(?P<quote>[\"']?)(?P<name>[A-Za-z0-9_-]+)(?P=quote)", text):
        test_name = match.group("name")
        if test_name in harness_roots:
            continue
        harness = manifest.member_to_harness.get(test_name)
        if harness is not None and test_name not in harness_roots:
            errors.append(
                f"{file_name} references retired integration-test member {test_name!r} with --test; use harness {harness!r}"
            )
        else:
            expected = ", ".join(sorted(harness_roots))
            errors.append(f"{file_name} references unknown integration-test binary {test_name!r} with --test; expected one of: {expected}")
    # Source-fence recipes select tests as `--test <harness> -- <member>:: ...`.
    # The harness token is checked above; validate each positional <member>:: filter
    # resolves to a real member of THAT harness (a typo/stale member silently matches
    # zero tests while the required check reports green).
    for line in text.splitlines():
        head = re.search(r"['\"]?--test['\"]?(?:=|\s+)[\"']?(?P<harness>[A-Za-z0-9_-]+)[\"']?", line)
        if head is None or " -- " not in line:
            continue
        harness = head.group("harness")
        for pm in re.finditer(r"\b(?P<member>[A-Za-z0-9_]+)::", line.split(" -- ", 1)[1]):
            member = pm.group("member")
            owner = manifest.member_to_harness.get(member)
            if owner != harness:
                filt = member + "::"
                errors.append(
                    f"{file_name} source-fence test filter {filt!r} does not belong to "
                    f"--test harness {harness!r} (member maps to {owner!r}); typo or stale member"
                )
    return errors


def live_node_nextest_expected_clause(member: str, manifest: CiTestManifest) -> str:
    harness = manifest.member_to_harness.get(member, member)
    if harness == member:
        return f"binary(={member})"
    return f"(binary(={harness}) & test(/^{member}::/))"


def live_node_nextest_filter_matches(member: str, manifest: CiTestManifest, filter_expr: object) -> bool:
    if not isinstance(filter_expr, str):
        return False
    harness = manifest.member_to_harness.get(member, member)
    if harness == member:
        return f"binary(={member})" in filter_expr
    return f"binary(={harness})" in filter_expr and f"test(/^{member}::/)" in filter_expr


def nextest_override_sensitive_keys(override: dict[object, object]) -> set[str]:
    keys: set[str] = set()
    for key in override:
        if not isinstance(key, str):
            continue
        if key in NEXTEST_SENSITIVE_OVERRIDE_KEYS or key.endswith("timeout"):
            keys.add(key)
    return keys


def nextest_filter_binaries(filter_expr: object) -> set[str]:
    if not isinstance(filter_expr, str):
        return set()
    return set(NEXTEST_BINARY_FILTER_RE.findall(filter_expr))


def nextest_filter_has_unparseable_binary(filter_expr: object) -> bool:
    """True if any binary(...) target is not the audited equality form binary(=name).

    Regex-form binary(/.../) and other shapes parse to an empty binary set, which
    would silently slip past the subset/skip checks; treat them as unauditable so
    the override fails closed.
    """
    if not isinstance(filter_expr, str):
        return False
    for match in re.finditer(r"\bbinary\(", filter_expr):
        if not NEXTEST_BINARY_EQ_TAIL_RE.match(filter_expr, match.end()):
            return True
    return False


def nextest_filter_test_prefixes(filter_expr: object) -> set[str]:
    """The set of harness-scoped member prefixes test(/^member::/) in the filter."""
    if not isinstance(filter_expr, str):
        return set()
    return set(NEXTEST_TEST_PREFIX_RE.findall(filter_expr))


def nextest_override_is_known_root_unit(override: dict[object, object]) -> bool:
    filter_expr = override.get("filter")
    if override.get("test-group") != LIVE_NODE_TEST_GROUP or not isinstance(filter_expr, str):
        return False
    if set(override) != {"filter", "test-group"}:
        return False
    if nextest_filter_has_unparseable_binary(filter_expr):
        return False
    return nextest_filter_binaries(filter_expr) <= {"bolt_v2"} and all(
        fragment in filter_expr for fragment in LIVE_NODE_UNIT_TEST_FILTERS
    )


def nextest_override_is_known_live_node(override: dict[object, object], manifest: CiTestManifest) -> bool:
    filter_expr = override.get("filter")
    if override.get("test-group") != LIVE_NODE_TEST_GROUP or not isinstance(filter_expr, str):
        return False
    if set(override) != {"filter", "test-group"}:
        return False
    if nextest_filter_has_unparseable_binary(filter_expr):
        return False
    expected_clauses = [live_node_nextest_expected_clause(member, manifest) for member in LIVE_NODE_NEXTEST_BINARIES]
    expected_binaries = {
        manifest.member_to_harness.get(member, member)
        for member in LIVE_NODE_NEXTEST_BINARIES
    }
    consolidated_members = {
        member
        for member in LIVE_NODE_NEXTEST_BINARIES
        if manifest.member_to_harness.get(member, member) != member
    }
    return (
        nextest_filter_binaries(filter_expr) <= expected_binaries
        and nextest_filter_test_prefixes(filter_expr) == consolidated_members
        and all(clause in filter_expr for clause in expected_clauses)
    )


def nextest_unregistered_override_errors(overrides: list[object], manifest: CiTestManifest) -> list[str]:
    errors: list[str] = []
    for index, override in enumerate(overrides, start=1):
        if not isinstance(override, dict):
            continue
        sensitive_keys = nextest_override_sensitive_keys(override)
        filter_expr = override.get("filter")
        if (
            not sensitive_keys
            and not nextest_filter_binaries(filter_expr)
            and not nextest_filter_has_unparseable_binary(filter_expr)
        ):
            continue
        if nextest_override_is_known_root_unit(override) or nextest_override_is_known_live_node(override, manifest):
            continue
        errors.append(
            "nextest config has unregistered per-binary override "
            f"#{index}: keys {', '.join(sorted(sensitive_keys)) or '<none>'}, filter {filter_expr!r}"
        )
    return errors


def verify_nextest_config(config_text: str, *, manifest: CiTestManifest | None = None) -> list[str]:
    errors: list[str] = []
    try:
        config = tomllib.loads(config_text)
    except tomllib.TOMLDecodeError as exc:
        return [f"nextest config invalid TOML: {exc}"]
    if manifest is None:
        manifest = build_test_manifest(REPO_ROOT / "Cargo.toml", REPO_ROOT / "tests")

    groups = config.get("test-groups", {})
    if not isinstance(groups, dict):
        groups = {}
    live_node_group = groups.get(LIVE_NODE_TEST_GROUP)
    if not isinstance(live_node_group, dict):
        errors.append("nextest config missing live-node test group")
    elif live_node_group.get("max-threads") != 1:
        errors.append("nextest live-node test group max-threads must be 1")

    profile = config.get("profile", {})
    default_profile = profile.get("default", {}) if isinstance(profile, dict) else {}
    overrides = default_profile.get("overrides", []) if isinstance(default_profile, dict) else []
    if not isinstance(overrides, list):
        overrides = []
    live_node_filters = [
        override.get("filter")
        for override in overrides
        if isinstance(override, dict) and override.get("test-group") == LIVE_NODE_TEST_GROUP
    ]
    missing_live_node_filters = [
        live_node_nextest_expected_clause(member, manifest)
        for member in LIVE_NODE_NEXTEST_BINARIES
        if not any(live_node_nextest_filter_matches(member, manifest, filter_expr) for filter_expr in live_node_filters)
    ]
    missing_unit_filters = [
        fragment
        for fragment in LIVE_NODE_UNIT_TEST_FILTERS
        if not any(isinstance(filter_expr, str) and fragment in filter_expr for filter_expr in live_node_filters)
    ]
    if missing_live_node_filters or missing_unit_filters:
        missing = ", ".join(
            missing_live_node_filters + missing_unit_filters
        )
        errors.append(f"nextest config must assign LiveNode test paths to live-node group: missing {missing}")
    errors.extend(nextest_unregistered_override_errors(overrides, manifest))
    return errors


def verify_text(workflow_text: str, action_text: str, nextest_config_text: str) -> list[str]:
    return verify_workflows({"ci.yml": workflow_text}, action_text, nextest_config_text)


def repo_automation_source_build_errors(text: str) -> list[str]:
    return [
        f"repo automation must not compile {tool} from source"
        for tool in sorted(cargo_install_source_build_tools_in_text(text))
    ]


def backtester_managed_target_cache_errors(file_name: str, text: str) -> list[str]:
    if not any(prefix in text for prefix in ("managed-target-bvs-v", "bvs-nextest-archive-v", "bvs-bin-sidecars-v")):
        return []
    errors: list[str] = []
    for _job_id, job_lines in parse_jobs(text).items():
        job_text = "\n".join(job_lines)
        cache_key_seen = False
        for line in job_text.splitlines():
            if not any(prefix in line for prefix in ("managed-target-bvs-v", "bvs-nextest-archive-v", "bvs-bin-sidecars-v")):
                continue
            if "key:" not in line:
                continue
            cache_key_seen = True
            if "hashFiles(" in line:
                errors.append("backtester cache key must use ci_input_sets digest, not inline hashFiles")
            if "${{ steps.bvs_cache_inputs.outputs.digest }}" not in line:
                errors.append("backtester cache key must include steps.bvs_cache_inputs.outputs.digest")
        if not cache_key_seen:
            continue
        if "python3 scripts/ci_input_sets.py hash backtester_cache" not in job_text:
            errors.append("backtester cache key digest must come from ci_input_sets backtester_cache")
        if file_name.endswith("backtester-ci.yml") and (
            'if [[ "${{ needs.detect.outputs.bvs_bootstrap_changed }}" == "true" ]]; then' not in job_text
            or 'echo "digest=bootstrap-${GITHUB_SHA}" >> "$GITHUB_OUTPUT"' not in job_text
        ):
            errors.append("backtester cache key digest must use exact-head namespace when CI input-set bootstrap changes")
        for block in action_blocks(job_lines, "actions/cache@"):
            block_text = uncommented_text(block)
            if "managed-target-bvs-v" in block_text:
                errors.append("backtester managed target cache saves must be push-to-main only")
        for block in action_blocks(job_lines, "actions/cache/save@"):
            block_text = uncommented_text(block)
            if "managed-target-bvs-v" not in block_text:
                continue
            if "github.event_name == 'push'" not in block_text or "github.ref == 'refs/heads/main'" not in block_text:
                errors.append("backtester managed target cache saves must be push-to-main only")
    return errors


def inline_integer_matrix_values(job_text: str, matrix_key: str) -> tuple[int, ...] | None:
    match = re.search(rf"(?m)^        {re.escape(matrix_key)}: \[([0-9, ]+)\]\s*$", job_text)
    if match is None:
        return None
    parts = [part.strip() for part in match.group(1).split(",")]
    if not parts or any(not re.fullmatch(r"[1-9][0-9]*", part) for part in parts):
        return None
    return tuple(int(part) for part in parts)




def named_step_run_block(job_text: str, step_name: str) -> str | None:
    lines = job_text.splitlines()
    for line_number, line in enumerate(lines):
        if line.strip() != f"- name: {step_name}":
            continue
        for run_line_number in range(line_number + 1, len(lines)):
            run_line = lines[run_line_number]
            if run_line.strip().startswith("- name: "):
                break
            if run_line.strip() != "run: |":
                continue
            run_indent = len(run_line) - len(run_line.lstrip())
            block_lines: list[str] = []
            for body_line in lines[run_line_number + 1 :]:
                if body_line.strip():
                    body_indent = len(body_line) - len(body_line.lstrip())
                    if body_indent <= run_indent:
                        break
                block_lines.append(body_line)
            return "\n".join(block_lines)
        return None
    return None


def job_body_sha256(job_lines: list[str]) -> str:
    return hashlib.sha256("\n".join(job_lines).encode("utf-8")).hexdigest()


def partition_job_body_digest_errors(
    *,
    label: str,
    job_lines: list[str] | None,
    expected_sha256: str,
    constant_name: str,
) -> list[str]:
    if job_lines is None:
        return [f"{label}: job body digest pin target job not found; update {constant_name} for legitimate edits"]
    actual_sha256 = job_body_sha256(job_lines)
    if actual_sha256 == expected_sha256:
        return []
    return [
        f"{label} job body digest changed: expected {expected_sha256}, got {actual_sha256}; "
        f"update {constant_name} with reviewed workflow job body changes"
    ]


PARTITION_WORKFLOW_TOP_LEVEL_KEYS = frozenset(
    {"concurrency", "env", "jobs", "name", "on", "permissions", "run-name"}
)


def partition_workflow_top_level_key_errors(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []
    allowed_keys = ", ".join(sorted(PARTITION_WORKFLOW_TOP_LEVEL_KEYS))
    seen_keys: set[str] = set()
    for line in workflow_text.splitlines():
        structural = workflow_yaml_structural_line(line).rstrip()
        if not structural.strip() or structural.startswith((" ", "\t")):
            continue
        match = re.fullmatch(rf"({YAML_KEY_PATTERN})\s*:.*", structural)
        entry = structural
        if match is not None:
            entry = unquote_yaml_scalar(match.group(1))
            if entry in seen_keys:
                errors.append(f"{workflow_name} duplicate top-level key {entry!r} is not allowed")
            else:
                seen_keys.add(entry)
        if entry not in PARTITION_WORKFLOW_TOP_LEVEL_KEYS:
            errors.append(
                f"{workflow_name} top-level entry {entry!r} is not allowed; "
                f"allowed keys: {allowed_keys}; offending line: {structural!r}"
            )
    return errors


def partition_workflow_boundary_errors(workflow_text: str, workflow_name: str) -> list[str]:
    errors = classified_top_level_env_errors(workflow_text, workflow_name)
    errors.extend(partition_workflow_top_level_key_errors(workflow_text, workflow_name))
    return errors


class WorkflowJobStep(NamedTuple):
    name: str | None
    run_text: str | None
    uses: str | None


def workflow_job_steps(job_text: str) -> tuple[WorkflowJobStep, ...]:
    lines = job_text.splitlines()
    steps_index = next((index for index, line in enumerate(lines) if line.strip() == "steps:"), None)
    if steps_index is None:
        return ()

    steps_indent = len(lines[steps_index]) - len(lines[steps_index].lstrip())
    step_indent: int | None = None
    steps: list[WorkflowJobStep] = []
    index = steps_index + 1
    while index < len(lines):
        line = lines[index]
        if line.strip():
            indent = len(line) - len(line.lstrip())
            if indent <= steps_indent:
                break
            if line.lstrip().startswith("- "):
                if step_indent is None:
                    step_indent = indent
                if indent == step_indent:
                    step_end = len(lines)
                    for candidate_end in range(index + 1, len(lines)):
                        candidate_line = lines[candidate_end]
                        if not candidate_line.strip():
                            continue
                        candidate_indent = len(candidate_line) - len(candidate_line.lstrip())
                        if candidate_indent <= steps_indent:
                            step_end = candidate_end
                            break
                        if candidate_indent == step_indent and candidate_line.lstrip().startswith("- "):
                            step_end = candidate_end
                            break
                    steps.append(workflow_job_step(lines, index, step_end, step_indent))
                    index = step_end
                    continue
        index += 1
    return tuple(steps)


def workflow_job_step(lines: list[str], step_start: int, step_end: int, step_indent: int) -> WorkflowJobStep:
    name: str | None = None
    run_text: str | None = None
    uses: str | None = None

    def capture_run_block(body_start: int, run_indent: int) -> tuple[str, int]:
        block_lines: list[str] = []
        body_index = body_start
        while body_index < step_end:
            body_line = lines[body_index]
            if body_line.strip():
                body_indent = len(body_line) - len(body_line.lstrip())
                if body_indent <= run_indent:
                    break
            block_lines.append(body_line)
            body_index += 1
        return "\n".join(block_lines), body_index

    first_content = lines[step_start].lstrip().removeprefix("- ").strip()
    if first_content:
        if first_content.startswith("name: "):
            name = first_content.removeprefix("name: ").strip()
        elif first_content.startswith("uses: "):
            uses = first_content.removeprefix("uses: ").strip()
        elif first_content == "run: |":
            run_text, _body_end = capture_run_block(step_start + 1, step_indent)
        elif first_content.startswith("run: "):
            run_text = first_content.removeprefix("run: ").strip()

    index = step_start + 1
    while index < step_end:
        line = lines[index]
        if not line.strip():
            index += 1
            continue
        indent = len(line) - len(line.lstrip())
        if indent != step_indent + 2:
            index += 1
            continue

        content = line.strip()
        if content.startswith("name: "):
            name = content.removeprefix("name: ").strip()
        elif content.startswith("uses: "):
            uses = content.removeprefix("uses: ").strip()
        elif content == "run: |":
            run_text, index = capture_run_block(index + 1, indent)
            continue
        elif content.startswith("run: "):
            run_text = content.removeprefix("run: ").strip()
        index += 1

    return WorkflowJobStep(name=name, run_text=run_text, uses=uses)




def step_block_has_field(block: list[str] | None, key: str, value: str) -> bool:
    if block is None:
        return False
    step_indent: int | None = None
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        stripped = clean.lstrip()
        if stripped.startswith("- "):
            step_indent = len(clean) - len(stripped)
            first_field = stripped.removeprefix("- ").strip()
            if first_field == f"{key}: {value}":
                return True
            continue
        if step_indent is None:
            continue
        if len(clean) - len(stripped) == step_indent + 2 and stripped == f"{key}: {value}":
            return True
    return False


def step_block_has_key(block: list[str] | None, key: str) -> bool:
    if block is None:
        return False
    step_indent: int | None = None
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        stripped = clean.lstrip()
        if stripped.startswith("- "):
            step_indent = len(clean) - len(stripped)
            first_field = stripped.removeprefix("- ").strip()
            if first_field.startswith(f"{key}:"):
                return True
            continue
        if step_indent is None:
            continue
        if len(clean) - len(stripped) == step_indent + 2 and stripped.startswith(f"{key}:"):
            return True
    return False


def just_invocation_count(run_lines: tuple[str, ...], recipe: str) -> int:
    return sum(len(re.findall(rf"\bjust\s+{re.escape(recipe)}\b", line)) for line in run_lines)


def bte_test_invocation_count(run_block: str) -> int:
    return sum(
        len(re.findall(r"\bjust\s+bte-test\b", line))
        for line in simple_shell_lines(run_block)
    )


def missing_nextest_junit_stage_lines(report_path: str) -> tuple[str, ...]:
    return (
        f'report="{report_path}"',
        'staged="junit-unit-${{ matrix.run_number }}.xml"',
        'if [[ -f "$report" ]]; then',
        'cp "$report" "$staged"',
        "else",
        'python3 - > "$staged" <<\'PY\'',
        "import os",
        "import xml.sax.saxutils as sax",
        'rc = sax.escape(os.environ.get("MERGIFY_TEST_EXIT_CODE", "unknown"))',
        'print(\'<?xml version="1.0" encoding="UTF-8"?>\')',
        'print(\'<testsuite name="nextest-preflight" tests="1" failures="1">\')',
        'print(\'<testcase classname="ci" name="missing-nextest-junit">\')',
        'print(f\'<failure message="nextest JUnit report was not produced">MERGIFY_TEST_EXIT_CODE={rc}; see the Run tests log.</failure>\')',
        "print('</testcase></testsuite>')",
        "PY",
        "fi",
    )


BVS_BACKTESTER_ALLOWED_SIBLING_RUN_STEPS = {
    "Resolve crate managed target dir": (
        'dir="$(python3 "${{ steps.setup.outputs.rust_verification_owner }}" target-dir --repo crates/backtesting-vertical-slice)"',
        'echo "dir=$dir" >> "$GITHUB_OUTPUT"',
    ),
    "Compute BVS cache input hash": (
        'echo "digest=$(python3 scripts/ci_input_sets.py hash backtester_cache)" >> "$GITHUB_OUTPUT"',
    ),
    "Configure nextest JUnit output": (
        "printf '%s\\n' \\",
        "'[profile.default.junit]' \\",
        '\'path = "junit-unit-${{ matrix.run_number }}.xml"\' \\',
        "'store-success-output = false' \\",
        "'store-failure-output = true' \\",
        '> "$RUNNER_TEMP/nextest-junit.toml"',
    ),
    "Stage JUnit report": missing_nextest_junit_stage_lines(
        "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
    ),
}
BVS_BACKTESTER_ALLOWED_USES_STEPS = frozenset(
    (
        (None, "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"),
        ("Setup environment", "./.github/actions/setup-environment"),
        ("Setup read-only sccache", SCCACHE_SETUP_ACTION_PATH),
        ("Print sccache stats", SCCACHE_STATS_ACTION_PATH),
        (None, "Swatinem/rust-cache@c19371144df3bb44fab255c43d04cbc2ab54d1c4"),
        ("Restore test target cache", "actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae"),
        ("Install cargo-nextest", "taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538"),
        ("Upload test results to Mergify", "mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3"),
    )
)
# This exact shell/action allowlist is scoped to the partitioned BVS backtester lane.
# Root and issue-789 lanes are governed by their trigger, matrix, and required-fragment contracts.
PARTITIONED_BVS_BACKTESTER_POLICY_LABELS = frozenset({"backtester full job", "backtester smoke job"})


def bvs_backtester_job_steps_are_allowlisted(job_text: str) -> bool:
    seen_run_step_names: set[str] = set()
    seen_uses_steps: set[tuple[str | None, str]] = set()
    for step in workflow_job_steps(job_text):
        if step.run_text is not None:
            if step.name is None or step.name in seen_run_step_names:
                return False
            if step.name == "Run tests":
                seen_run_step_names.add(step.name)
                continue
            expected_lines = BVS_BACKTESTER_ALLOWED_SIBLING_RUN_STEPS.get(step.name)
            if expected_lines is None or simple_shell_lines(step.run_text) != expected_lines:
                return False
            seen_run_step_names.add(step.name)
            continue

        if step.uses is not None:
            uses_step = (step.name, step.uses)
            if uses_step not in BVS_BACKTESTER_ALLOWED_USES_STEPS or uses_step in seen_uses_steps:
                return False
            seen_uses_steps.add(uses_step)
            continue

        return False
    return {"Run tests", "Stage JUnit report"} <= seen_run_step_names




FLAKY_TEST_DETECTION_SHARED_FORBIDDEN_FRAGMENTS = (
    ("must not use dynamic matrix expressions", "fromJSON("),
    ("must not inspect event names for smoke/full selection", "github.event_name"),
    ("must not use mode inputs for smoke/full selection", "inputs.mode"),
    (
        "root JUnit staging must not copy from the cargo target dir",
        "${{ steps.setup.outputs.managed_target_dir }}/nextest/default/junit-unit-",
    ),
    (
        "backtester JUnit staging must not copy from the cargo target dir",
        "${{ steps.crate_target.outputs.dir }}/nextest/default/junit-unit-",
    ),
)

FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS = {
    ".github/workflows/flaky-test-detection.yml": {
        "workflow_triggers": frozenset({"workflow_dispatch"}),
        "required_workflow_fragments": (),
        "forbidden_workflow_fragments": (),
        "jobs": (
            (
                "flaky-detection-rust-root",
                "root full job",
                (
                    "set +e",
                    "rc=$?",
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    'target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "missing-nextest-junit",
                    "if: success() || failure()",
                ),
            ),
            (
                "flaky-detection-rust-backtester",
                "backtester full job",
                (
                    "set +e",
                    "rc=$?",
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    "missing-nextest-junit",
                    'crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "if: success() || failure()",
                ),
            ),
            (
                "flaky-detection-rust-backtester-issue-789",
                "issue-789 full job",
                (
                    "set +e",
                    "rc=$?",
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    "missing-nextest-junit",
                    'crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "if: success() || failure()",
                ),
            ),
        ),
    },
    ".github/workflows/flaky-test-smoke.yml": {
        "workflow_triggers": frozenset({"schedule", "workflow_dispatch"}),
        "required_workflow_fragments": (),
        "forbidden_workflow_fragments": (),
        "jobs": (
            (
                "flaky-smoke-rust-root",
                "root smoke job",
                (
                    "run_number: [1]",
                    "set +e",
                    'rc="${PIPESTATUS[0]}"',
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    'target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "missing-nextest-junit",
                    "if: success() || failure()",
                ),
            ),
            (
                "flaky-smoke-rust-backtester",
                "backtester smoke job",
                (
                    "run_number: [1]",
                    "shard: [1]",
                    "set +e",
                    'rc="${PIPESTATUS[0]}"',
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    'crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "missing-nextest-junit",
                    "if: success() || failure()",
                ),
            ),
            (
                "flaky-smoke-rust-backtester-issue-789",
                "issue-789 smoke job",
                (
                    "run_number: [1]",
                    "set +e",
                    'rc="${PIPESTATUS[0]}"',
                    "set -e",
                    "MERGIFY_TEST_EXIT_CODE=%s\\n",
                    'exit "$rc"',
                    'crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml',
                    "missing-nextest-junit",
                    "if: success() || failure()",
                ),
            ),
        ),
    },
}
FLAKY_TEST_DETECTION_REQUIRED_WORKFLOW_FILES = frozenset(FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS)


def flaky_test_detection_workflow_errors(text: str, contract: dict[str, object]) -> list[str]:
    workflow_text = uncommented_text(text.splitlines())
    expected_triggers = contract["workflow_triggers"]
    jobs = parse_jobs(text)
    expected_jobs = contract["jobs"]
    expected_ids = {job_id for job_id, _label, _fragments in expected_jobs}
    job_texts = {job_id: uncommented_text(job_lines) for job_id, job_lines in jobs.items()}
    errors: list[str] = []
    errors.extend(
        f"flaky-test-detection workflow triggers must be {sorted(expected_triggers)}"
        for actual_triggers in (workflow_trigger_keys(text),)
        if actual_triggers != expected_triggers
    )
    errors.extend(
        f"flaky-test-detection {message}"
        for message, fragment in (
            *FLAKY_TEST_DETECTION_SHARED_FORBIDDEN_FRAGMENTS,
            *contract["forbidden_workflow_fragments"],
        )
        if fragment in workflow_text
    )
    errors.extend(
        f"flaky-test-detection {message}"
        for message, fragment in contract["required_workflow_fragments"]
        if fragment not in workflow_text
    )
    if any(re.match(r"^    if:", line) for job_lines in jobs.values() for line in job_lines):
        errors.append("flaky-test-detection workflows must not use job-level if gates")
    if set(jobs) != expected_ids:
        errors.append(f"flaky-test-detection workflow jobs must be {sorted(expected_ids)}")
    for job_id, label, fragments in expected_jobs:
        if job_id not in job_texts:
            errors.append(f"flaky-test-detection missing {label} {job_id}")
            continue
        job_text = job_texts[job_id]
        run_block = named_step_run_block(job_text, "Run tests")
        run_lines = simple_shell_lines(run_block or "")
        if 'printf \'MERGIFY_TEST_EXIT_CODE=%s\\n\' "$rc" >> "$GITHUB_ENV"' not in run_lines:
            errors.append(f"flaky-test-detection {label} missing MERGIFY_TEST_EXIT_CODE export")
        if not run_lines or run_lines[-1] != 'exit "$rc"':
            errors.append(f"flaky-test-detection {label} missing 'exit \"$rc\"'")
        stage_block = named_step_block(jobs[job_id], "Stage JUnit report")
        stage_text = uncommented_text(stage_block) if stage_block is not None else ""
        if not step_block_has_field(stage_block, "if", "success() || failure()"):
            errors.append(f"flaky-test-detection {label} missing 'if: success() || failure()'")
        stage_lines = simple_shell_lines(named_step_run_block(job_text, "Stage JUnit report") or "")
        report_path = (
            "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
            if label in {"root full job", "root smoke job"}
            else "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
        )
        if stage_lines != missing_nextest_junit_stage_lines(report_path):
            errors.append(f"flaky-test-detection {label} JUnit staging must synthesize a missing-report failure")
        errors.extend(
            f"flaky-test-detection {label} missing {fragment!r}"
            for fragment in fragments
            if fragment not in job_text
        )
        if label in PARTITIONED_BVS_BACKTESTER_POLICY_LABELS:
            bte_run_block = named_step_run_block(job_text, "Run tests")
            if bte_run_block is None:
                errors.append(f"flaky-test-detection {label} must have a Run tests run block")
                bte_run_block = ""
            invocation_count = bte_test_invocation_count(job_text)
            if invocation_count != 1:
                errors.append(f"flaky-test-detection {label} must have exactly one just bte-test invocation")
            if not bvs_backtester_job_steps_are_allowlisted(job_text):
                errors.append(f"flaky-test-detection {label} must keep BVS job steps unchanged")
            denominators = simple_bte_run_block_partition_denominators(bte_run_block)
            if len(denominators) != 1:
                errors.append(f"flaky-test-detection {label} must keep just bte-test in a simple Run tests block")
                errors.append(f"flaky-test-detection {label} must have one matrix.shard partition argument")
        if label in {"root full job", "backtester full job", "issue-789 full job"}:
            run_numbers = inline_integer_matrix_values(job_text, "run_number")
            if run_numbers is None:
                errors.append(f"flaky-test-detection {label} run_number matrix must be an inline integer list")
            elif not one_indexed_sequence(run_numbers):
                errors.append(f"flaky-test-detection {label} run_number matrix must be one-indexed and contiguous")
        if label == "backtester full job":
            shards = inline_integer_matrix_values(job_text, "shard")
            if shards is None:
                errors.append("flaky-test-detection backtester full job shard matrix must be an inline integer list")
            elif not one_indexed_sequence(shards):
                errors.append("flaky-test-detection backtester full job shard matrix must be one-indexed and contiguous")
            if len(denominators) == 1 and shards is not None and denominators[0] != len(shards):
                errors.append("flaky-test-detection backtester full job partition denominator must match shard matrix length")
        if label == "backtester smoke job":
            shards = inline_integer_matrix_values(job_text, "shard")
            if len(denominators) == 1 and shards is not None and denominators[0] <= len(shards):
                errors.append("flaky-test-detection backtester smoke job partition denominator must exceed scheduled shard count")
    return errors


def verify_flaky_test_detection_workflows(texts: dict[str, str]) -> list[str]:
    missing_errors = [
        f"{file_name}: flaky-test-detection required workflow is missing"
        for file_name in sorted(FLAKY_TEST_DETECTION_REQUIRED_WORKFLOW_FILES - texts.keys())
    ]
    contract_errors = [
        f"{file_name}: {error}"
        for file_name in sorted(FLAKY_TEST_DETECTION_REQUIRED_WORKFLOW_FILES & texts.keys())
        for error in flaky_test_detection_workflow_errors(
            texts[file_name],
            FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS[file_name],
        )
    ]
    return missing_errors + contract_errors


DEBUG_LANE_SCCACHE_OPT_IN = "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}"
DEBUG_TEST_SCCACHE_ACTIVE_INPUT = (
    "active: ${{ (steps.debug-archive-ready.outputs.value != 'true' || inputs.package != '') && 'true' || 'false' }}"
)
DEBUG_LANE_TEST_PROFILE_ENV = 'CARGO_PROFILE_TEST_DEBUG: "0"'
DEBUG_LANE_DEV_PROFILE_ENV = 'CARGO_PROFILE_DEV_DEBUG: "0"'


def debug_lane_sccache_workflow_env_errors(workflow_name: str, workflow_text: str) -> list[str]:
    workflow_clean = uncommented_text(workflow_text.splitlines())
    forbidden_fragments = (
        "SCCACHE_BUCKET: ${{ vars.CI_SCCACHE_BUCKET }}",
        "SCCACHE_REGION: ${{ vars.CI_SCCACHE_REGION }}",
        "SCCACHE_S3_KEY_PREFIX: ${{ vars.CI_SCCACHE_S3_KEY_PREFIX }}",
        'SCCACHE_S3_SERVER_SIDE_ENCRYPTION: "true"',
        'SCCACHE_IGNORE_SERVER_IO_ERROR: "1"',
    )
    return [
        f"{workflow_name} debug-lane sccache env must be owned by the shared sccache action, not workflow env {fragment}"
        for fragment in forbidden_fragments
        if fragment in workflow_clean
    ]


def sccache_setup_action_contract_errors(action_text: str, config_text: str) -> list[str]:
    errors: list[str] = []
    if not action_text:
        return [f"{SCCACHE_SETUP_ACTION_FILE} must exist as the single sccache setup owner"]
    action_lines = action_text.splitlines()
    eligibility_block = named_step_block(action_lines, "Resolve sccache eligibility")
    eligibility_text = uncommented_text(eligibility_block) if eligibility_block is not None else ""
    aws_block = named_step_block(action_lines, "Configure AWS credentials for sccache")
    aws_text = uncommented_text(aws_block) if aws_block is not None else ""
    install_block = named_step_block(action_lines, "Install sccache")
    install_text = uncommented_text(install_block) if install_block is not None else ""
    enable_block = named_step_block(action_lines, "Resolve sccache enablement")
    enable_text = uncommented_text(enable_block) if enable_block is not None else ""
    summary_block = named_step_block(action_lines, "Summarize sccache state")
    summary_text = uncommented_text(summary_block) if summary_block is not None else ""

    for block, text, step_name in (
        (eligibility_block, eligibility_text, "Resolve sccache eligibility"),
        (aws_block, aws_text, "Configure AWS credentials for sccache"),
        (install_block, install_text, "Install sccache"),
        (enable_block, enable_text, "Resolve sccache enablement"),
    ):
        if block is None:
            errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must include step {step_name!r}")
        elif not step_block_has_field(block, "continue-on-error", "true"):
            errors.append(f"{step_name} must be continue-on-error")

    action_clean = uncommented_text(action_lines)
    if SCCACHE_LOCATION_CONFIG_DEFAULT not in action_clean:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must default to {SCCACHE_LOCATION_CONFIG_PATH}")
    for fragment in (
        "SCCACHE_ACTIVE: ${{ inputs.active }}",
        "READ_ROLE_ARN: ${{ inputs.role-arn }}",
        "WRITE_ROLE_ARN: ${{ inputs.write-role-arn }}",
        "CONFIG_PATH: ${{ inputs.config-path }}",
        "python3.12 scripts/sccache_eligibility.py",
    ):
        if fragment not in eligibility_text:
            errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must include {fragment!r}")
    for fragment in (
        'event_name == "push"',
        'event_name == "workflow_dispatch"',
        "read_allowed =",
        "SCCACHE_IGNORE_SERVER_IO_ERROR=1",
    ):
        if fragment not in eligibility_text:
            continue
        errors.append("sccache setup action must delegate trust and env resolution to scripts/sccache_eligibility.py")
    if "role-to-assume: ${{ steps.eligibility.outputs.role_arn }}" not in aws_text:
        errors.append("Configure AWS credentials for sccache must assume the action-selected role")
    if "aws-region: ${{ steps.eligibility.outputs.region }}" not in aws_text:
        errors.append("Configure AWS credentials for sccache must use the TOML-selected region")
    if "uses: aws-actions/configure-aws-credentials@e7f100cf4c008499ea8adda475de1042d6975c7b" not in aws_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must install pinned aws credentials action")
    if "uses: mozilla-actions/sccache-action@9e7fa8a12102821edf02ca5dbea1acd0f89a2696" not in install_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must install pinned sccache action")
    if 'version: "v0.10.0"' not in install_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must pin sccache v0.10.0")
    if 'disable_annotations: "true"' not in install_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must disable vendor sccache stats annotations")
    for fragment in ('"$SCCACHE_PATH" --start-server', '"$SCCACHE_PATH" --zero-stats || true'):
        if fragment not in enable_text:
            errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must include {fragment!r}")
    if not step_block_has_field(enable_block, "if", "always()"):
        errors.append("Resolve sccache enablement must run under always()")
    if (
        not step_block_has_field(summary_block, "if", "always()")
        or "sccache cache:" not in summary_text
        or "$GITHUB_STEP_SUMMARY" not in summary_text
        or "SCCACHE_CACHE_MODE: ${{ steps.eligibility.outputs.cache_mode || 'none' }}" not in summary_text
    ):
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must summarize sccache state under always()")
    if '"$SCCACHE_PATH" --show-stats || true' in action_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must not print stats before the compile step")
    if "write-enabled:" in action_text or "WRITE_ENABLED" in action_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must not expose a caller-owned write-enabled path")
    if "bolt-v2-ci-cache-675819144420-us-east-2" in action_text or "sccache/bolt-v2/arm64/root-nextest/" in action_text:
        errors.append(f"{SCCACHE_SETUP_ACTION_FILE} must read the sccache location from {SCCACHE_LOCATION_CONFIG_PATH}")
    try:
        config = tomllib.loads(config_text)
    except tomllib.TOMLDecodeError as exc:
        errors.append(f"{SCCACHE_LOCATION_CONFIG_PATH} must be valid TOML: {exc}")
        return errors
    location = config.get("location") if isinstance(config, dict) else None
    if not isinstance(location, dict):
        errors.append(f"{SCCACHE_LOCATION_CONFIG_PATH} must define [location]")
        return errors
    for key in ("bucket", "region"):
        if not isinstance(location.get(key), str) or not location.get(key):
            errors.append(f"{SCCACHE_LOCATION_CONFIG_PATH} must set location.{key} to a non-empty string")
    key_prefix = location.get("key_prefix")
    if not isinstance(key_prefix, str) or not key_prefix or not key_prefix.endswith("/"):
        errors.append(f"{SCCACHE_LOCATION_CONFIG_PATH} must set location.key_prefix must be a non-empty string ending in '/'")
    return errors


def sccache_eligibility_script_contract_errors(script_text: str) -> list[str]:
    errors: list[str] = []
    if not script_text:
        return [f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} must own sccache trust and env resolution"]
    for fragment in (
        "def resolve_sccache_eligibility(",
        'event_name == "push" and github_ref == "refs/heads/main"',
        'event_name == "workflow_dispatch" and github_ref == "refs/heads/main"',
        'read_allowed = event_name in {"pull_request", "merge_group", "workflow_dispatch", "schedule"}',
        'cache_mode = "read_write"',
        'cache_mode = "read_only"',
        'cache_mode = "none"',
        'SCCACHE_BUCKET={eligibility.bucket}',
        'SCCACHE_REGION={eligibility.region}',
        'SCCACHE_S3_KEY_PREFIX={eligibility.key_prefix}',
        "SCCACHE_S3_SERVER_SIDE_ENCRYPTION=true",
        "SCCACHE_IGNORE_SERVER_IO_ERROR=1",
    ):
        if fragment not in script_text:
            errors.append(f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} must include {fragment!r}")
    try:
        tree = ast.parse(script_text, filename=SCCACHE_ELIGIBILITY_SCRIPT_FILE)
    except SyntaxError as exc:
        errors.append(f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} must parse as Python: {exc}")
        return errors
    resolver = next((node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "resolve_sccache_eligibility"), None)
    if resolver is None:
        errors.append(f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} must define resolve_sccache_eligibility")
        return errors

    def assigned_value(name: str) -> ast.AST | None:
        for node in ast.walk(resolver):
            if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == name for target in node.targets):
                return node.value
        return None

    trusted_write = assigned_value("trusted_write")
    expected_trusted_write = ast.parse(
        'write_requested and ((event_name == "push" and github_ref == "refs/heads/main") '
        'or (event_name == "workflow_dispatch" and github_ref == "refs/heads/main"))',
        mode="eval",
    ).body
    if trusted_write is None or ast.dump(trusted_write, include_attributes=False) != ast.dump(expected_trusted_write, include_attributes=False):
        errors.append(f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} trusted_write expression must restrict write access to main push/dispatch")

    read_allowed = assigned_value("read_allowed")
    expected_read_allowed = ast.parse('event_name in {"pull_request", "merge_group", "workflow_dispatch", "schedule"}', mode="eval").body
    if read_allowed is None or ast.dump(read_allowed, include_attributes=False) != ast.dump(expected_read_allowed, include_attributes=False):
        errors.append(f"{SCCACHE_ELIGIBILITY_SCRIPT_FILE} read_allowed expression must restrict reads to pull_request/merge_group/workflow_dispatch/schedule")
    return errors


def sccache_stats_action_contract_errors(action_text: str) -> list[str]:
    errors: list[str] = []
    if not action_text:
        return [f"{SCCACHE_STATS_ACTION_FILE} must exist as the single sccache stats owner"]
    action_clean = uncommented_text(action_text.splitlines())
    for fragment in ('if [[ "$SCCACHE_ENABLED" == "true" && -n "${SCCACHE_PATH:-}" ]]; then', '"$SCCACHE_PATH" --show-stats || true'):
        if fragment not in action_clean:
            errors.append(f"{SCCACHE_STATS_ACTION_FILE} must include {fragment!r}")
    if "SCCACHE_ENABLED: ${{ inputs.enabled }}" not in action_clean:
        errors.append(f"{SCCACHE_STATS_ACTION_FILE} must be gated by the caller's enabled input")
    return errors


def debug_lane_test_execution_lines(compile_lines: tuple[str, ...], command_fragments: tuple[str, ...]) -> list[str]:
    return [
        line
        for line in compile_lines
        if any(fragment in line for fragment in command_fragments)
        and "--no-run" not in line
    ]


def debug_lane_sccache_job_errors(
    workflow_name: str,
    job_name: str,
    job_lines: list[str],
    *,
    compile_step_name: str,
    require_managed_target_dir: bool,
    require_debug_archive_compile_condition: bool = False,
) -> list[str]:
    label = f"{workflow_name} {job_name}"
    job_text = uncommented_text(job_lines)
    errors: list[str] = []
    if not job_permission_has(job_lines, "id-token", "write"):
        errors.append(f"{label} must grant id-token: write for read-only sccache OIDC")
    if not job_has_setup_input(job_lines, "install-rust-linker", "true"):
        errors.append(f"{label} must install configured Rust linker")
    if require_managed_target_dir and not job_has_setup_input(job_lines, "include-managed-target-dir", "true"):
        errors.append(f"{label} must opt into the managed target dir")
    setup_block = named_step_block(job_lines, "Setup read-only sccache")
    setup_text = uncommented_text(setup_block) if setup_block is not None else ""
    for fragment in (
        "id: sccache",
        f"uses: {SCCACHE_SETUP_ACTION_PATH}",
        SCCACHE_READONLY_ROLE_INPUT,
    ):
        if fragment not in setup_text:
            errors.append(f"{label} must route read-only sccache through the shared sccache action with {fragment!r}")
    if (
        "AWS_CI_CACHE_ROLE_ARN" in setup_text
        or "write-role-arn:" in setup_text
        or "write-enabled:" in setup_text
        or "bucket:" in setup_text
        or "region:" in setup_text
        or "key-prefix:" in setup_text
    ):
        errors.append(f"{label} must use only the PR-readonly sccache role")
    if require_debug_archive_compile_condition and DEBUG_TEST_SCCACHE_ACTIVE_INPUT not in setup_text:
        errors.append(f"{label} sccache must also run for package debug-test compiles")

    compile_block = named_step_block(job_lines, compile_step_name)
    compile_text = uncommented_text(compile_block) if compile_block is not None else ""
    if step_block_has_key(compile_block, "continue-on-error"):
        errors.append(f"{label} compile/test run step must not use continue-on-error")
    if DEBUG_LANE_SCCACHE_OPT_IN not in compile_text:
        errors.append(f"{label} compile step must opt into managed sccache conditionally")
    for fragment in (DEBUG_LANE_TEST_PROFILE_ENV, DEBUG_LANE_DEV_PROFILE_ENV):
        if fragment not in compile_text:
            errors.append(f"{label} compile step must match the test-archive debug profile env")
    compile_lines = simple_shell_lines(compile_text)
    if "sccache-fail-open.sh" in compile_text or "RUST_PROBE_COMPILE_ONLY" in compile_text:
        errors.append(f"{label} retry and compile/test split must be owned by rust_verification.py")
    if workflow_name.endswith("flaky-test-smoke.yml"):
        expected_run_line = {
            "flaky-smoke-rust-root": 'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast 2>&1 | tee -a "$log"',
            "flaky-smoke-rust-backtester": 'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" --partition "count:${{ matrix.shard }}/4" -- --skip issue_789_first_real_free_data_taker_pl 2>&1 | tee -a "$log"',
            "flaky-smoke-rust-backtester-issue-789": 'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" issue_789_first_real_free_data_taker_pl 2>&1 | tee -a "$log"',
        }.get(job_name)
        if expected_run_line is None:
            errors.append(f"{label} has no governed flaky smoke command contract")
            test_execution_lines = []
        else:
            test_execution_lines = [line for line in compile_lines if line == expected_run_line]
            recipe = "test" if job_name == "flaky-smoke-rust-root" else "bte-test"
            if compile_lines.count(expected_run_line) != 1 or just_invocation_count(compile_lines, recipe) != 1:
                errors.append(f"{label} run step must execute tests through one managed just invocation")
        if not test_execution_lines:
            errors.append(f"{label} run step must execute tests through one managed just invocation")
        if any(
            line.startswith("BOLT_RUST_VERIFICATION_SCCACHE=0 ")
            and ("just test " in line or "just bte-test " in line)
            for line in compile_lines
        ):
            errors.append(f"{label} test execution must not force sccache off")
        if 'printf \'MERGIFY_TEST_EXIT_CODE=%s\\n\' "$rc" >> "$GITHUB_ENV"' not in compile_lines or not compile_lines or compile_lines[-1] != 'exit "$rc"':
            errors.append(f"{label} flaky smoke run step must exit with captured rc")
        stage_block = named_step_block(job_lines, "Stage JUnit report")
        stage_lines = simple_shell_lines(named_step_run_block(uncommented_text(job_lines), "Stage JUnit report") or "")
        report_path = (
            "target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
            if job_name == "flaky-smoke-rust-root"
            else "crates/backtesting-vertical-slice/target/nextest/default/junit-unit-${{ matrix.run_number }}.xml"
        )
        if stage_lines != missing_nextest_junit_stage_lines(report_path):
            errors.append(f"{label} JUnit staging must synthesize a missing-report failure")
    if workflow_name.endswith("debug-test.yml"):
        expected_run_line = 'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"'
        test_execution_lines = [line for line in compile_lines if line == expected_run_line]
        if compile_lines.count(expected_run_line) != 1 or just_invocation_count(compile_lines, "debug-test") != 1:
            errors.append(f"{label} run step must execute debug-test through one managed just invocation")
        if any(line.startswith("BOLT_RUST_VERIFICATION_SCCACHE=0 ") and "just debug-test" in line for line in compile_lines):
            errors.append(f"{label} test execution must not force sccache off")
    if workflow_name.endswith("rust-probe.yml"):
        if compile_lines.count("bash .github/scripts/run-rust-probe.sh") != 1:
            errors.append(f"{label} Rust Probe must route through one managed owner invocation")
    stats_block = named_step_block(job_lines, "Print sccache stats")
    stats_text = uncommented_text(stats_block) if stats_block is not None else ""
    if stats_block is None or f"uses: {SCCACHE_STATS_ACTION_PATH}" not in stats_text:
        errors.append(f"{label} must print sccache stats after compile")
    elif not step_block_has_field(stats_block, "if", "always()") or not step_occurs_after(job_lines, "Print sccache stats", compile_step_name):
        errors.append(f"{label} must print sccache stats after compile")
    if "RUSTC_WRAPPER:" in job_text:
        errors.append(f"{label} must not bypass managed_env with a direct RUSTC_WRAPPER env")
    return errors


def bvs_debug_lane_cache_policy_errors(policy_text: str) -> list[str]:
    try:
        policy = tomllib.loads(policy_text)
    except tomllib.TOMLDecodeError as exc:
        return [f"backtesting-vertical-slice rust policy must be valid TOML for flaky smoke cache parity: {exc}"]
    expected_tables: tuple[tuple[str, dict[str, object]], ...] = (
        (
            "remote_compile_cache",
            {
                "enabled": True,
                "enable_env": "BOLT_RUST_VERIFICATION_SCCACHE",
                "ci_env": "GITHUB_ACTIONS",
                "wrapper_env": "SCCACHE_PATH",
                "wrapper_program": "sccache",
            },
        ),
        (
            "remote_fast_linker",
            {
                "enabled": True,
                "ci_env": "GITHUB_ACTIONS",
                "linker_env": "BOLT_RUST_FAST_LINKER",
                "programs": ["mold", "lld"],
            },
        ),
    )
    errors: list[str] = []
    for table_name, expected_values in expected_tables:
        table = policy.get(table_name)
        if not isinstance(table, dict):
            errors.append(f"backtesting-vertical-slice rust policy must include [{table_name}] for flaky smoke cache parity")
            continue
        for key, expected in expected_values.items():
            if table.get(key) != expected:
                errors.append(
                    f"backtesting-vertical-slice rust policy must set {table_name}.{key}={expected!r} for flaky smoke cache parity"
                )
    return errors


def verify_debug_lane_compile_cache_parity(
    workflows: dict[str, str],
    bvs_policy_text: str,
) -> list[str]:
    workflow_specs = (
        (
            ".github/workflows/debug-test.yml",
            (
                ("debug-test", "Run debug test", True, True),
            ),
        ),
        (
            ".github/workflows/rust-probe.yml",
            (
                ("probe-heavy", "Run Rust Probe", True, False),
                ("probe-light", "Run Rust Probe", True, False),
            ),
        ),
        (
            ".github/workflows/flaky-test-smoke.yml",
            (
                ("flaky-smoke-rust-root", "Run tests", True, False),
                ("flaky-smoke-rust-backtester", "Run tests", False, False),
                ("flaky-smoke-rust-backtester-issue-789", "Run tests", False, False),
            ),
        ),
    )
    errors: list[str] = []
    errors.extend(
        sccache_setup_action_contract_errors(
            repo_text_or_empty(SCCACHE_SETUP_ACTION_FILE),
            repo_text_or_empty(SCCACHE_LOCATION_CONFIG_PATH),
        )
    )
    errors.extend(sccache_eligibility_script_contract_errors(repo_text_or_empty(SCCACHE_ELIGIBILITY_SCRIPT_FILE)))
    errors.extend(sccache_stats_action_contract_errors(repo_text_or_empty(SCCACHE_STATS_ACTION_FILE)))
    for workflow_name, job_specs in workflow_specs:
        workflow_text = workflows.get(workflow_name)
        if workflow_text is None:
            errors.append(f"{workflow_name} must exist for debug-lane sccache parity")
            continue
        errors.extend(debug_lane_sccache_workflow_env_errors(workflow_name, workflow_text))
        jobs = parse_jobs(workflow_text)
        for job_name, compile_step_name, require_managed_target_dir, require_debug_archive_condition in job_specs:
            job_lines = jobs.get(job_name)
            if job_lines is None:
                errors.append(f"{workflow_name} must define {job_name} for debug-lane sccache parity")
                continue
            errors.extend(
                debug_lane_sccache_job_errors(
                    workflow_name,
                    job_name,
                    job_lines,
                    compile_step_name=compile_step_name,
                    require_managed_target_dir=require_managed_target_dir,
                    require_debug_archive_compile_condition=require_debug_archive_condition,
                )
            )
    errors.extend(bvs_debug_lane_cache_policy_errors(bvs_policy_text))
    return errors


def backtester_gate_detect_result_errors(file_name: str, text: str) -> list[str]:
    if not file_name.endswith("backtester-ci.yml"):
        return []
    jobs = parse_jobs(text)
    gate = jobs.get("gate")
    if gate is None:
        return []
    gate_text = uncommented_text(gate)
    if "backtester-gate" not in gate_text:
        return []
    if "detect" not in extract_needs(gate):
        return ["backtester-gate must need detect"]
    if "--job detect=${{ needs.detect.result }}" in gate_text:
        return []
    return ["backtester-gate shared verdict call must include needs.detect.result"]


def backtester_test_shard_errors(file_name: str, text: str) -> list[str]:
    if not file_name.endswith("backtester-ci.yml"):
        return []
    jobs = parse_jobs(text)
    archive_job = jobs.get("test-archive")
    test_job = jobs.get("test")
    issue_job = jobs.get("issue_789")
    gate_job = jobs.get("gate")
    errors: list[str] = []
    errors.extend(partition_workflow_boundary_errors(text, "backtester-ci.yml"))
    errors.extend(
        partition_job_body_digest_errors(
            label="backtester bvs-test archive",
            job_lines=archive_job,
            expected_sha256=BVS_TEST_ARCHIVE_JOB_SHA256,
            constant_name="BVS_TEST_ARCHIVE_JOB_SHA256",
        )
    )
    if archive_job is None:
        errors.append("backtester bvs-test must define archive producer job")
    if test_job is not None:
        errors.append("backtester bvs-test must run partitions in the archive producer, not a matrix shard job")
    if issue_job is None:
        errors.append("backtester bvs-test must define manual issue-789 diagnostic job")
    if archive_job is None:
        return errors
    archive_text = uncommented_text(archive_job)
    job_text = uncommented_text(test_job) if test_job is not None else ""
    issue_text = uncommented_text(issue_job) if issue_job is not None else ""
    gate_text = uncommented_text(gate_job) if gate_job is not None else ""
    consumer_text = f"{job_text}\n{issue_text}"
    combined_text = f"{archive_text}\n{consumer_text}"
    if "just bte-test --partition" in combined_text:
        errors.append("backtester bvs-test must not run direct per-shard target builds")
    if "for shard in $(seq 1 \"$BVS_NEXTEST_SHARDS\")" not in archive_text:
        errors.append("backtester bvs-test archive producer must run every BVS partition")
    if (
        'partition_log="$RUNNER_TEMP/bvs-nextest-archive-partition-${shard}.log"' not in archive_text
        or '2>&1 | tee "$partition_log"' not in archive_text
        or 'tail -80 "$partition_log"' not in archive_text
    ):
        errors.append("backtester bvs-test archive must log partition diagnostics")
    if BVS_PARTITION_FAILURE_WRAPPER not in archive_text:
        errors.append("backtester bvs-test partition failures must use contiguous failure wrapper")
    if named_step_block(archive_job, "test") is None:
        errors.append("backtester bvs-test archive must define test partition step")
    if 'rc="${PIPESTATUS[0]}"' not in archive_text:
        errors.append("backtester bvs-test partition failures must preserve shard exit codes")
    if 'echo "::error title=BVS nextest archive partition failed::shard=${shard}/${BVS_NEXTEST_SHARDS} exit=${rc}"' not in archive_text:
        errors.append("backtester bvs-test partition failures must emit shard error annotations")
    if "build --locked --bins" in combined_text:
        errors.append("backtester bvs-test sidecars must not build every binary")
    if "find debug -maxdepth 1 -type f -perm -111" in combined_text:
        errors.append("backtester bvs-test sidecars must not blanket-pack target/debug executables")
    if "name: bvs-test-payload" in combined_text:
        errors.append("backtester bvs-test must not publish or consume the legacy fan-out payload")
    if action_blocks(archive_job, "actions/download-artifact@"):
        errors.append("backtester required bvs-test path must not download a test payload artifact")
    if "managed-target-bvs-v" in consumer_text or "test-target-cache" in consumer_text:
        errors.append("backtester bvs-test consumers must not restore the managed target cache")
    if 'just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH" --lib --test' in archive_text:
        errors.append("backtester bvs-test archive targets must be discovered, not hardcoded in workflow YAML")
    if gate_job is not None and (
        "issue_789" in extract_needs(gate_job) or "needs.issue_789.result" in gate_text
    ):
        errors.append("backtester diagnostic issue-789 lane must not gate merge proof")
    artifact_cache_blocks = [
        block
        for block in action_blocks(archive_job, "actions/cache/restore@")
        + action_blocks(archive_job, "actions/cache/save@")
        if any(
            fragment in uncommented_text(block)
            for fragment in (
                "BVS_NEXTEST_ARCHIVE_PATH",
                "BVS_BIN_SIDECARS_PATH",
                "bvs-nextest-archive-v",
                "bvs-bin-sidecars-v",
            )
        )
    ]
    if artifact_cache_blocks:
        errors.append("backtester bvs-test archive payloads must use S3 artifact cache, not GitHub Actions cache")
    bvs_s3_eligibility_block = named_step_block(archive_job, "Resolve BVS nextest artifact cache eligibility")
    bvs_s3_aws_block = named_step_block(archive_job, "Configure AWS credentials for BVS nextest artifact cache")
    bvs_archive_s3_restore_block = named_step_block(archive_job, "Restore BVS nextest archive from S3")
    bvs_sidecar_s3_restore_block = named_step_block(archive_job, "Restore BVS binary sidecars from S3")
    bvs_archive_s3_save_block = named_step_block(archive_job, "Save BVS nextest archive")
    bvs_sidecar_s3_save_block = named_step_block(archive_job, "Save BVS binary sidecars")
    bvs_s3_summary_block = named_step_block(archive_job, "Summarize BVS nextest archive S3 state")
    bvs_s3_eligibility_text = uncommented_text(bvs_s3_eligibility_block) if bvs_s3_eligibility_block else ""
    bvs_s3_aws_text = uncommented_text(bvs_s3_aws_block) if bvs_s3_aws_block else ""
    bvs_s3_summary_text = uncommented_text(bvs_s3_summary_block) if bvs_s3_summary_block else ""
    if bvs_s3_eligibility_block is None or "continue-on-error: true" not in bvs_s3_eligibility_text:
        errors.append("backtester bvs-test archive S3 artifact cache eligibility must be fail-open")
    if (
        'if [[ "$GITHUB_EVENT_NAME" == "push" && "$GITHUB_REF" == "refs/heads/main" ]]; then' not in bvs_s3_eligibility_text
        or 'cache_mode="read_write"' not in bvs_s3_eligibility_text
        or 'role_arn="$ROLE_ARN"' not in bvs_s3_eligibility_text
        or 'elif [[ "$GITHUB_EVENT_NAME" == "pull_request" || "$GITHUB_EVENT_NAME" == "merge_group" || "$GITHUB_EVENT_NAME" == "workflow_dispatch" ]]; then' not in bvs_s3_eligibility_text
        or 'cache_mode="read_only"' not in bvs_s3_eligibility_text
        or 'role_arn="$PR_READONLY_ROLE_ARN"' not in bvs_s3_eligibility_text
        or 'echo "role_arn=$role_arn" >> "$GITHUB_OUTPUT"' not in bvs_s3_eligibility_text
        or 'echo "cache_mode=$cache_mode" >> "$GITHUB_OUTPUT"' not in bvs_s3_eligibility_text
    ):
        errors.append("backtester bvs-test archive S3 role selection must split main writers from read-only consumers")
    if bvs_s3_aws_block is None or "continue-on-error: true" not in bvs_s3_aws_text:
        errors.append("backtester bvs-test archive S3 AWS credential setup must be fail-open")
    if "role-to-assume: ${{ steps.bvs-nextest-artifact-cache.outputs.role_arn }}" not in bvs_s3_aws_text:
        errors.append("backtester bvs-test archive S3 AWS credential setup must assume the resolved role")
    bvs_restore_guard = (
        "if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true' "
        "&& steps.bvs-nextest-artifact-cache-aws.outcome == 'success'"
    )
    for block in (bvs_archive_s3_restore_block, bvs_sidecar_s3_restore_block):
        block_text = uncommented_text(block) if block is not None else ""
        if block is None or bvs_restore_guard not in block_text:
            errors.append("backtester bvs-test archive must gate S3 restores on eligibility and AWS credential success")
        if TEST_ARCHIVE_RESTORE_RESULT_OUTPUT not in block_text or TEST_ARCHIVE_RESTORE_REASON_OUTPUT not in block_text:
            errors.append("backtester bvs-test archive must emit restore result and reason outputs")
    if (
        bvs_s3_summary_block is None
        or "if: always()" not in bvs_s3_summary_text
        or "S3_AWS_OUTCOME: ${{ steps.bvs-nextest-artifact-cache-aws.outcome }}" not in bvs_s3_summary_text
        or "restore_state()" not in bvs_s3_summary_text
        or 'echo "BVS nextest archive S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_nextest_restore} reason=${bvs_nextest_reason}"' not in bvs_s3_summary_text
    ):
        errors.append("backtester bvs-test archive must summarize BVS nextest archive S3 restore state")
    if (
        bvs_s3_summary_block is None
        or "if: always()" not in bvs_s3_summary_text
        or "S3_AWS_OUTCOME: ${{ steps.bvs-nextest-artifact-cache-aws.outcome }}" not in bvs_s3_summary_text
        or "restore_state()" not in bvs_s3_summary_text
        or 'echo "BVS binary sidecars S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_sidecar_restore} reason=${bvs_sidecar_reason}"' not in bvs_s3_summary_text
    ):
        errors.append("backtester bvs-test archive must summarize BVS binary sidecars S3 restore state")
    for block, label in (
        (bvs_archive_s3_save_block, "nextest archive"),
        (bvs_sidecar_s3_save_block, "binary sidecars"),
    ):
        block_text = uncommented_text(block) if block is not None else ""
        if block is None or "continue-on-error: true" not in block_text:
            errors.append(f"backtester bvs-test archive must save {label} to S3 fail-open")
        if (
            "github.event_name == 'push'" not in block_text
            or "github.ref == 'refs/heads/main'" not in block_text
            or "steps.bvs-nextest-artifact-cache.outputs.cache_mode == 'read_write'" not in block_text
            or "steps.bvs-nextest-artifact-cache-aws.outcome == 'success'" not in block_text
        ):
            errors.append(f"backtester bvs-test archive must save {label} to S3 only from push-to-main with write credentials")
        if (
            'save-status=skipped' not in block_text
            or 'save-status=success' not in block_text
            or 'save-status=failed' not in block_text
            or "exit 1" not in block_text
        ):
            errors.append(f"backtester bvs-test archive must emit explicit {label} S3 save status")

    archive_fragments = [
        ("backtester bvs-test archive must use archive job name", "name: bvs-test archive"),
        ("backtester bvs-test archive must declare archive path", "BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst"),
        ("backtester bvs-test archive must declare sidecar path", "BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz"),
        ("backtester bvs-test archive must declare four archive partitions", 'BVS_NEXTEST_SHARDS: "4"'),
        (
            "backtester bvs-test archive must expose nextest artifact S3 kill switch",
            "NEXTEST_ARTIFACT_CACHE_ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}",
        ),
        (
            "backtester bvs-test archive must expose nextest artifact S3 prefix",
            "NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}",
        ),
        (
            "backtester bvs-test archive must compute the shared BVS cache input digest",
            "python3 scripts/ci_input_sets.py hash backtester_cache",
        ),
        (
            "backtester bvs-test archive must restore nextest archive cache explicitly",
            "id: bvs-nextest-archive-cache",
        ),
        (
            "backtester bvs-test archive must resolve S3 cache eligibility",
            "id: bvs-nextest-artifact-cache",
        ),
        (
            "backtester bvs-test archive S3 restore must be fail-open",
            "cache-hit=false",
        ),
        (
            "backtester bvs-test archive must fail closed on S3 digest mismatch",
            "aws s3api head-object",
        ),
        (
            "backtester bvs-test archive must fail closed on S3 digest mismatch",
            'Metadata."nextest-digest"',
        ),
        (
            "backtester bvs-test archive must fail closed on S3 digest mismatch",
            '"$metadata_digest" != "$DIGEST"',
        ),
        (
            "backtester bvs-test archive must fail closed on nextest archive S3 digest mismatch",
            "BVS nextest archive S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push.",
        ),
        (
            "backtester bvs-test archive must fail closed on binary sidecar S3 digest mismatch",
            "BVS binary sidecar S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push.",
        ),
        (
            "backtester bvs-test archive must restore caches from S3",
            'aws s3 cp "$uri" "$BVS_NEXTEST_ARCHIVE_PATH" --only-show-errors',
        ),
        (
            "backtester bvs-test archive cache key must be exact and content-addressed",
            "CACHE_KEY: bvs-nextest-archive-v4-${{ runner.os }}-${{ runner.arch }}-test-profile-discovered-targets-shards-4-${{ steps.bvs_cache_inputs.outputs.digest }}",
        ),
        (
            "backtester bvs-test archive must restore binary sidecar cache",
            "id: bvs-bin-sidecars-cache",
        ),
        (
            "backtester bvs-test sidecar cache key must be exact and content-addressed",
            "CACHE_KEY: bvs-bin-sidecars-v4-${{ runner.os }}-${{ runner.arch }}-test-profile-discovered-cargo-bin-exe-${{ steps.bvs_cache_inputs.outputs.digest }}",
        ),
        (
            "backtester bvs-test sidecars must restore from S3",
            'aws s3 cp "$uri" "$BVS_BIN_SIDECARS_PATH" --only-show-errors',
        ),
        (
            "backtester bvs-test archive must resolve the crate managed target directory",
            "id: crate_target",
        ),
        (
            "backtester bvs-test archive must save shared registry cache from the archive producer only",
            "save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}",
        ),
        (
            "backtester bvs-test archive must build archive only on cache miss",
            "if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must derive archive targets from source",
            "python3 scripts/rust_test_targets.py archive-args --crate crates/backtesting-vertical-slice",
        ),
        (
            "backtester bvs-test archive must build a nextest archive from discovered targets",
            'just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH" "${archive_args[@]}"',
        ),
        (
            "backtester bvs-test archive must save nextest archive cache explicitly",
            "id: bvs-nextest-archive-cache-save",
        ),
        (
            "backtester bvs-test archive saves must be main-only",
            "github.event_name == 'push' && github.ref == 'refs/heads/main'",
        ),
        (
            "backtester bvs-test archive must save nextest archive to S3",
            'aws s3 cp "$BVS_NEXTEST_ARCHIVE_PATH" "$uri" --only-show-errors',
        ),
        (
            "backtester bvs-test archive must build sidecars only on sidecar cache miss",
            "if: steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must build binary sidecars",
            "Build BVS binary sidecars",
        ),
        (
            "backtester bvs-test archive sidecars must derive from CARGO_BIN_EXE references",
            "python3 scripts/rust_test_targets.py sidecars --crate crates/backtesting-vertical-slice",
        ),
        (
            "backtester bvs-test archive sidecars must use managed cargo",
            'python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo crates/backtesting-vertical-slice -- "${cargo_args[@]}"',
        ),
        (
            "backtester bvs-test archive must pack only required sidecars",
            'tar --null -czf "$GITHUB_WORKSPACE/$BVS_BIN_SIDECARS_PATH" --files-from -',
        ),
        (
            "backtester bvs-test archive must save binary sidecar cache",
            "id: bvs-bin-sidecars-cache-save",
        ),
        (
            "backtester bvs-test archive must save binary sidecars to S3",
            'aws s3 cp "$BVS_BIN_SIDECARS_PATH" "$uri" --only-show-errors',
        ),
        (
            "backtester bvs-test archive must expose BVS S3 save outcomes",
            "bvs_nextest_archive_cache_save_outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
        ),
        (
            "backtester bvs-test archive must expose BVS S3 save outcomes",
            "bvs_bin_sidecars_cache_save_outcome: ${{ steps.bvs-bin-sidecars-cache-save.outputs.save-status || (steps.bvs-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
        ),
        (
            "backtester bvs-test archive must summarize BVS S3 save outcomes",
            "BVS nextest archive S3 save outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
        ),
        (
            "backtester bvs-test archive must summarize BVS S3 save outcomes",
            "BVS binary sidecars S3 save outcome: ${{ steps.bvs-bin-sidecars-cache-save.outputs.save-status || (steps.bvs-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}",
        ),
        (
            "backtester bvs-test archive must restore target cache only while producing caches",
            "if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must save target cache only after archive/sidecar misses",
            "if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}",
        ),
        (
            "backtester bvs-test archive must fail closed on missing local payload",
            'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty"; exit 1; }',
        ),
        (
            "backtester bvs-test archive must fail closed on missing sidecars",
            'test -s "$BVS_BIN_SIDECARS_PATH" || { echo "BVS binary sidecars missing or empty"; exit 1; }',
        ),
        (
            "backtester bvs-test archive must log payload sizes",
            "stat -c 'bvs-payload-size %n %s'",
        ),
        (
            "backtester bvs-test archive must extract sidecars locally",
            'tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"',
        ),
        (
            "backtester bvs-test archive must list scoped archive tests",
            'nextest list --archive-file "$GITHUB_WORKSPACE/$BVS_NEXTEST_ARCHIVE_PATH"',
        ),
        (
            "backtester bvs-test archive must create archive extract root",
            'mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"',
        ),
        (
            "backtester bvs-test archive must exclude dedicated issue-789 lane",
            "-- --skip issue_789_first_real_free_data_taker_pl",
        ),
        (
            "backtester bvs-test archive must run partitioned tests from the local archive",
            'just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${shard}/${BVS_NEXTEST_SHARDS}" -- --skip issue_789_first_real_free_data_taker_pl',
        ),
    ]
    issue_fragments = [
        ("backtester bvs-test issue-789 must use dedicated job name", "name: bvs-test issue-789"),
        (
            "backtester bvs-test issue-789 must depend on backtester-gate",
            "needs: [ci-policy, detect, gate]",
        ),
        (
            "backtester bvs-test issue-789 must be manual workflow_dispatch only",
            "github.event_name == 'workflow_dispatch'",
        ),
        (
            "backtester bvs-test issue-789 must require explicit issue_789 input",
            "github.event.inputs.issue_789 == 'true'",
        ),
        (
            "backtester bvs-test issue-789 must run only on iteration policy",
            "needs.ci-policy.outputs.ci_policy_path == 'iteration'",
        ),
        (
            "backtester bvs-test issue-789 must only run after iteration gate succeeds",
            "needs.gate.result == 'success'",
        ),
        ("backtester bvs-test issue-789 must declare lib archive path", "BVS_ISSUE_789_ARCHIVE_PATH: .nextest-archive/bvs-issue-789-lib.tar.zst"),
        (
            "backtester bvs-test issue-789 must write the first-P/L artifact path",
            "BOLT_ISSUE_789_RESULT_PATH:",
        ),
        (
            "backtester bvs-test issue-789 must build a dedicated lib archive",
            'just bte-test-archive "$BVS_ISSUE_789_ARCHIVE_PATH" --lib',
        ),
        (
            "backtester bvs-test issue-789 must log lib archive size",
            "stat -c 'bvs-issue-789-archive-size %n %s'",
        ),
        (
            "backtester bvs-test issue-789 must create archive extract root",
            'mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"',
        ),
        (
            "backtester bvs-test issue-789 must run only the dedicated long test",
            'just bte-test-archive-run "$BVS_ISSUE_789_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" issue_789_first_real_free_data_taker_pl',
        ),
        (
            "backtester bvs-test issue-789 artifact name must be deterministic",
            "name: issue-789-first-pl-${{ github.run_id }}-${{ github.run_attempt }}",
        ),
        (
            "backtester bvs-test issue-789 artifact must fail closed if missing",
            "if-no-files-found: error",
        ),
    ]
    for message, fragment in archive_fragments:
        if fragment not in archive_text:
            errors.append(message)
    if issue_job is not None:
        for message, fragment in issue_fragments:
            if fragment not in issue_text:
                errors.append(message)
        if "needs.ci-policy.outputs.full_ci_required" in issue_text:
            errors.append("backtester bvs-test issue-789 must not depend on full CI dispatch")
    return errors


CACHE_SAME_RUN_TRANSPORT_FAIL_ON_MISS_MESSAGE = (
    "workflow must not use cache as a fail-closed same-run transport (fail-on-cache-miss: true); "
    "use upload/download-artifact for same-run cross-job handoff"
)
CACHE_SAME_RUN_TRANSPORT_GUARD_MESSAGE = (
    "workflow must not fail a job on a cache miss (cache-hit guard + exit 1); cache is "
    "best-effort and may be evicted before the consumer runs — use upload/download-artifact "
    "for same-run cross-job handoff"
)
CACHE_MISS_IF_RE = re.compile(r"\bcache-hit\b\s*(?:!=\s*[\"']?true[\"']?|==\s*[\"']?false[\"']?)")
# A cache-miss-guarded build step may contain nested validation failures (e.g.
# the producer's `if [ count -eq 0 ]; then ... exit 1; fi`), which are NOT the
# banned shape. The banned same-run transport shape is a fail-closed `exit 1`
# reached unconditionally: at top level, or chained after a command via `||`,
# `&&`, or `;` (covering `test -s x || exit 1` and `... || { ...; exit 1; }`).
# The producer's nested `exit 1` is indented and not operator-chained, so it is
# correctly excluded. A cache-miss guard expressed inside the run body (rather
# than the step `if:`) or delegated to a separate script is outside this line
# scanner's scope; see cache_same_run_transport_errors.
EXIT_ONE_RE = re.compile(
    r"(?m)(?:^exit\s+1\b|\|\|\s*exit\s+1\b|&&\s*exit\s+1\b|;\s*exit\s+1\b)"
)
# `fail-on-cache-miss: <truthy>` in same-line YAML forms, including flow-style
# (`with: { fail-on-cache-miss: true }`), optional `!!bool` tag, optional quotes,
# flexible spacing, case-insensitive truthy value. Folded/block scalars are
# handled by a continuation-line peek below. Not anchored to the whole line, so
# flow-style maps are caught; a negative lookbehind avoids matching a longer key
# such as `my-fail-on-cache-miss`. The caller strips comments before matching.
# `true`/`!!bool true` enable the directive; `yes`/`on` are rejected loudly by
# actions/cache's boolean input parser — either way it is not a silent same-run
# transport and must not ship.
FAIL_ON_CACHE_MISS_TRUE_RE = re.compile(
    r"(?<![\w-])fail-on-cache-miss:\s*(?:!!bool\s+)?[\"']?(?:true|yes|on)\b",
    re.IGNORECASE,
)
FAIL_ON_CACHE_MISS_BLOCK_SCALAR_RE = re.compile(
    r"(?<![\w-])fail-on-cache-miss:\s*(?:!!\S+\s+)?[>|][-+0-9]*\s*(?:#.*)?$",
    re.IGNORECASE,
)
FAIL_ON_CACHE_MISS_BLOCK_TRUTHY_RE = re.compile(r"^[\"']?(?:true|yes|on)[\"']?$", re.IGNORECASE)


def is_workflow_yaml(file_name: str) -> bool:
    normalized = file_name.replace("\\", "/")
    return normalized.startswith(".github/workflows/") and normalized.endswith((".yml", ".yaml"))


def step_has_cache_miss_guard(block: list[str]) -> bool:
    for line in block:
        clean = strip_comment(line).rstrip()
        if re.match(r"^\s*(?:-\s*)?if:\s*", clean) and CACHE_MISS_IF_RE.search(clean):
            return True
    return False


def has_fail_on_cache_miss_true(text: str) -> bool:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if FAIL_ON_CACHE_MISS_TRUE_RE.search(strip_comment(line)):
            return True
        if not FAIL_ON_CACHE_MISS_BLOCK_SCALAR_RE.search(strip_comment(line)):
            continue
        key_indent = len(line) - len(line.lstrip(" "))
        for continuation in lines[index + 1:]:
            continuation_indent = len(continuation) - len(continuation.lstrip(" "))
            if continuation_indent <= key_indent:
                break
            continuation_value = strip_comment(continuation).strip()
            if not continuation_value:
                continue
            if FAIL_ON_CACHE_MISS_BLOCK_TRUTHY_RE.fullmatch(continuation_value):
                return True
            break
    return False


def cache_same_run_transport_errors(file_name: str, text: str) -> list[str]:
    if not is_workflow_yaml(file_name):
        return []
    errors: list[str] = []
    if has_fail_on_cache_miss_true(text):
        errors.append(CACHE_SAME_RUN_TRANSPORT_FAIL_ON_MISS_MESSAGE)
    if any(
        step_has_cache_miss_guard(block) and EXIT_ONE_RE.search(block_run_body(block))
        for job_lines in parse_jobs(text).values()
        for block in step_blocks(job_lines)
    ):
        errors.append(CACHE_SAME_RUN_TRANSPORT_GUARD_MESSAGE)
    return errors


BACKTESTER_FULL_PROOF_IF = "needs.detect.outputs.bvs_changed == 'true'"
BACKTESTER_DEFER_CONDITION = '"$policy_path" == "defer" || "$full_ci_deferred" == "true"'
BACKTESTER_ITERATION_CONDITION = '"$policy_path" == "iteration"'
BACKTESTER_NOOP_CONDITION = '"$policy_path" == "noop"'
BACKTESTER_DEFER_ACTION_FILTER = """contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)"""
BACKTESTER_GATE_NAME_OUTPUT = "name: ${{ needs.ci-policy.outputs.backtester_gate_name }}"
BACKTESTER_REQUIRED_GATE_COMMENT = (
    "events publish only `backtester-gate-iteration`, which is feedback-only and must not be\n"
    "# marked required. Ready/non-draft proof paths publish `backtester-gate` after recomputing\n"
    "# proof lanes for crate-changing noop/defer paths. PRs that do not touch the crate still pass\n"
    "# through the explicit no-crate proof."
)
BACKTESTER_DEFER_ACTION_LIST_RE = re.compile(
    r"contains\(fromJSON\('(?P<actions>\[[^']+\])'\), github\.event\.action\)"
)
BACKTESTER_POLICY_DEFER_ACTIONS = {
    row.removeprefix("draft_pr_") if row.startswith("draft_pr_") else row
    for row in (
        "draft_pr_synchronize",
        "draft_pr_opened",
        "draft_pr_reopened",
        "draft_pr_edited",
        "converted_to_draft",
    )
}


def has_backtester_full_proof_guard(job_text: str) -> bool:
    return (
        "needs.ci-policy.outputs.full_ci_required == 'true'" in job_text
        and "needs.detect.outputs.bvs_changed == 'true'" in job_text
        and "needs.ci-policy.outputs.ci_policy_path == 'noop'" in job_text
        and "needs.ci-policy.outputs.full_ci_deferred == 'true'" in job_text
    )


def backtester_concurrency_group_text(text: str) -> str:
    block = top_level_block(text, "concurrency")
    group_lines: list[str] = []
    for line in block:
        if line.strip().startswith("cancel-in-progress:"):
            break
        group_lines.append(line)
    return " ".join(line.strip() for line in group_lines if line.strip())


def workflow_header_text(text: str) -> str:
    header_lines: list[str] = []
    for line in text.splitlines():
        if line.strip() == "on:":
            break
        header_lines.append(line)
    return "\n".join(header_lines)


def backtester_defer_action_lists(text: str) -> set[str]:
    return {match.group("actions") for match in BACKTESTER_DEFER_ACTION_LIST_RE.finditer(text)}


def backtester_defer_actions(text: str) -> set[str] | None:
    actions: set[str] = set()
    for raw_actions in backtester_defer_action_lists(text):
        try:
            parsed_actions = json.loads(raw_actions)
        except json.JSONDecodeError:
            return None
        if not isinstance(parsed_actions, list) or not all(isinstance(action, str) for action in parsed_actions):
            return None
        actions.update(parsed_actions)
    return actions


def backtester_draft_deferral_errors(file_name: str, text: str) -> list[str]:
    if not file_name.endswith("backtester-ci.yml"):
        return []
    jobs = parse_jobs(text)
    errors: list[str] = []
    if BACKTESTER_REQUIRED_GATE_COMMENT not in workflow_header_text(text):
        errors.append("backtester draft deferral must document that only backtester-gate should be required")
    policy = jobs.get("ci-policy")
    if policy is None:
        errors.append("backtester draft deferral must define ci-policy job")
    else:
        if "full_ci:" in "\n".join(workflow_trigger_block(text, "workflow_dispatch")):
            errors.append("backtester workflow_dispatch must not define a full_ci input")
        policy_text = uncommented_text(policy)
        for required in [
            "full_ci_required: ${{ steps.policy.outputs.full_ci_required }}",
            "full_ci_deferred: ${{ steps.policy.outputs.full_ci_deferred }}",
            "gate_name: ${{ steps.policy.outputs.gate_name }}",
            "backtester_gate_name: ${{ steps.policy.outputs.backtester_gate_name }}",
            "expected_event_class: ${{ steps.policy.outputs.expected_event_class }}",
            "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
            "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
            'git check-ref-format "refs/heads/$base_branch"',
            "git archive \"$base_ref\" scripts/ ci/github-actions-runners.toml",
            "steps.policy_base.outputs.script",
            'python3 "$policy_script" ci-policy',
            '--event-name "${{ github.event_name }}"',
            '--event-action "${{ github.event.action || \'\' }}"',
            '--pull-request-draft "${{ github.event.pull_request.draft || false }}"',
            "PR_HEAD_REF: ${{ github.event.pull_request.head.ref || '' }}",
            '--pull-request-head-ref "$PR_HEAD_REF"',
            "PR_AUTHOR_ID: ${{ github.event.pull_request.user.id || '' }}",
            "author_args=()",
            'python3 "$policy_script" ci-policy --help | grep -q -- "--pull-request-author-id"',
            'author_args=(--pull-request-author-id "$PR_AUTHOR_ID")',
            '"${author_args[@]}"',
            f'--pull-request-base-changed "${{{{ {PR_BASE_CHANGED_EXPR} }}}}"',
            "EVENT_SENDER_ID: ${{ github.event.sender.id }}",
            '--ref "${{ github.ref }}"',
        ]:
            if required not in policy_text:
                errors.append(f"backtester draft deferral ci-policy job must include {required}")
        errors.extend(ci_policy_event_sender_command_errors(policy))

    for heavy_job in ("clippy", "test-archive"):
        job = jobs.get(heavy_job)
        if job is None:
            continue
        needs = extract_needs(job)
        if "ci-policy" not in needs:
            errors.append(f"backtester draft deferral managed-heavy job {heavy_job} must need ci-policy")
        if not has_backtester_full_proof_guard(uncommented_text(job)):
            errors.append("backtester draft deferral managed-heavy jobs must require full CI policy")

    gate = jobs.get("gate")
    if gate is None:
        errors.append("backtester draft deferral must define backtester-gate")
    else:
        gate_text = uncommented_text(gate)
        if BACKTESTER_GATE_NAME_OUTPUT not in gate_text:
            errors.append("backtester draft deferral gate name must come from ci-policy backtester_gate_name output")
        if "ci-policy" not in extract_needs(gate):
            errors.append("backtester draft deferral gate must need ci-policy")
        for required in (
            "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
            "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
            'git check-ref-format "refs/heads/$base_branch"',
            "git archive \"$base_ref\" scripts/ ci/github-actions-runners.toml",
            "steps.verdict_base.outputs.script",
            'python3 "$verdict_script" check-backtester-gate',
        ):
            if required not in gate_text:
                errors.append(
                    f"backtester draft deferral gate must use trusted base-tree check-backtester-gate verdict ({required})"
                )
        for required in (
            "--policy-path \"${{ needs.ci-policy.outputs.ci_policy_path }}\"",
            "--expected-event-class \"${{ needs.ci-policy.outputs.expected_event_class }}\"",
            "--full-ci-deferred \"${{ needs.ci-policy.outputs.full_ci_deferred }}\"",
            "--bvs-changed \"${{ needs.detect.outputs.bvs_changed || 'false' }}\"",
            "--job ci-policy=${{ needs.ci-policy.result }}",
            "--job detect=${{ needs.detect.result }}",
            "--job fmt=${{ needs.fmt.result }}",
            "--job clippy=${{ needs.clippy.result }}",
            "--job test-archive=${{ needs.test-archive.result }}",
            "--job test=${{ needs.test-archive.result }}",
        ):
            if required not in gate_text:
                errors.append(f"backtester draft deferral shared gate call must include {required}")
        if "resolve-gate-carry-forward" in gate_text:
            errors.append("backtester draft deferral gate must recompute instead of carrying forward unavailable provenance")
        if gate_text and ("issue_789" in extract_needs(gate) or "needs.issue_789.result" in gate_text):
            errors.append("backtester diagnostic issue-789 lane must not gate merge proof")

    group_text = backtester_concurrency_group_text(text)
    if "format('bvs-pr-{0}-deferred', github.event.number)" not in group_text or "format('bvs-pr-{0}-full', github.event.number)" not in group_text:
        errors.append("backtester draft deferral concurrency must split deferred PR runs from full proof runs")
    if "format('bvs-pr-{0}-noop', github.event.number)" not in group_text:
        errors.append("backtester draft deferral concurrency must split noop PR runs from full proof runs")
    if READY_PR_NOOP_EXPR not in _normalize_concurrency_text(group_text):
        errors.append("backtester draft deferral concurrency must use the canonical ready PR noop predicate")
    if BACKTESTER_DEFER_ACTION_FILTER not in group_text:
        errors.append("backtester draft deferral concurrency must use the deferred draft action filter")
    if "dispatch-iteration" not in group_text:
        errors.append("backtester draft deferral concurrency must use the workflow_dispatch iteration group")
    if "github.event.inputs.full_ci" in group_text or "dispatch-full" in group_text:
        errors.append("backtester draft deferral concurrency must not define dispatch-full runs")
    defer_action_lists = backtester_defer_action_lists(group_text)
    if len(defer_action_lists) != 1:
        errors.append("backtester draft deferral must use one deferred draft action list across gate and concurrency")
    defer_actions = backtester_defer_actions(group_text)
    if defer_actions is None:
        errors.append("backtester draft deferral must use a valid deferred draft action list")
    elif defer_actions:
        missing_trigger_actions = sorted(defer_actions - workflow_pull_request_types(text))
        if missing_trigger_actions:
            errors.append(
                "backtester draft deferral pull_request types must include deferred actions: "
                + ", ".join(missing_trigger_actions)
            )
        if defer_actions != BACKTESTER_POLICY_DEFER_ACTIONS:
            errors.append("backtester draft deferral action list must match ci_provenance defer policy actions")
    return errors


def backtester_detect_path_errors(file_name: str, text: str) -> list[str]:
    if not file_name.endswith("backtester-ci.yml"):
        return []
    detect_job = parse_jobs(text).get("detect", [])
    errors: list[str] = []
    detect_text = "\n".join(detect_job)
    if "bvs_bootstrap_changed: ${{ steps.detect.outputs.bvs_bootstrap_changed }}" not in detect_text:
        errors.append("backtester detect must expose CI input-set bootstrap changes")
    validate_required = "python3 scripts/ci_input_sets.py validate backtester_cache backtester_detect"
    if validate_required not in detect_text:
        errors.append("backtester detect must validate CI input sets before skip decisions")
    bootstrap_required = 'git diff --name-only "${base_sha}...HEAD" -- scripts/ci_input_sets.py ci/rust-ci-inputs.toml > "$bootstrap_changed_path"'
    if bootstrap_required not in detect_text:
        errors.append("backtester detect must force-run on CI input-set bootstrap changes")
    bootstrap_branch = branch_body(detect_text, "if", '-s "$bootstrap_changed_path"')
    if (
        bootstrap_branch is None
        or 'echo "bvs_changed=true" >> "$GITHUB_OUTPUT"' not in bootstrap_branch
        or 'echo "bvs_bootstrap_changed=true" >> "$GITHUB_OUTPUT"' not in bootstrap_branch
        or "exit 0" not in bootstrap_branch
    ):
        errors.append("backtester detect must mark CI input-set bootstrap changes")
    required = 'python3 scripts/ci_input_sets.py changed backtester_detect --base "$base_sha" --head HEAD'
    if required not in detect_text:
        errors.append("backtester detect paths must come from ci_input_sets backtester_detect")
    non_bootstrap_detect_text = detect_text.replace(bootstrap_required, "")
    if 'git diff --name-only "${base_sha}...HEAD" --' in non_bootstrap_detect_text:
        errors.append("backtester detect paths must not be duplicated inline")
    return errors


def ci_input_set_config_errors(file_name: str, text: str) -> list[str]:
    if file_name != "ci/rust-ci-inputs.toml" and not file_name.endswith("/ci/rust-ci-inputs.toml"):
        return []
    try:
        config = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        return [f"CI input set config is invalid TOML: {exc}"]
    sets = config.get("sets")
    if not isinstance(sets, dict):
        return ["CI input set config must define [sets.<name>] tables"]

    def string_list(value: object, *, label: str) -> list[str]:
        if value is None:
            return []
        if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
            raise ValueError(f"{label} must be a list of non-empty strings")
        return value

    def resolve_set(name: str, stack: tuple[str, ...] = ()) -> list[str]:
        if name in stack:
            raise ValueError("input set cycle: " + " -> ".join((*stack, name)))
        table = sets.get(name)
        if not isinstance(table, dict):
            raise ValueError(f"unknown input set: {name}")
        paths: list[str] = []
        for parent in string_list(table.get("include_sets"), label=f"sets.{name}.include_sets"):
            paths.extend(resolve_set(parent, (*stack, name)))
        paths.extend(string_list(table.get("paths"), label=f"sets.{name}.paths"))
        deduped: list[str] = []
        seen: set[str] = set()
        for path in paths:
            if path not in seen:
                seen.add(path)
                deduped.append(path)
        return deduped

    try:
        cache = set(resolve_set("backtester_cache"))
        detect = set(resolve_set("backtester_detect"))
    except ValueError as exc:
        return [str(exc)]

    errors: list[str] = []
    for required in [
        "Cargo.lock",
        "Cargo.toml",
        ".gitignore",
        "build.rs",
        "gated_source_roots.manifest",
        "src/**",
        "tests/**",
        "specs/023-nt-research-analytics-platform/reference/**",
        "crates/backtesting-vertical-slice/Cargo.lock",
        "crates/backtesting-vertical-slice/Cargo.toml",
        "crates/backtesting-vertical-slice/src/**",
        "crates/backtesting-vertical-slice/tests/**",
        "scripts/rust_test_targets.py",
    ]:
        if required not in cache:
            errors.append(f"backtester_cache input set must include {required}")
    errors.extend(ci_input_sets.backtester_cache_pathspec_policy_errors(cache))
    for required in [
        "scripts/ci_input_sets.py",
        "ci/rust-ci-inputs.toml",
        ".github/actions/setup-environment/**",
        "scripts/ci_provenance.py",
        "ci/github-actions-runners.toml",
        ".github/workflows/backtester-ci.yml",
        "scripts/rust_test_targets.py",
    ]:
        if required not in detect:
            errors.append(f"backtester_detect input set must include {required}")
    return errors


def backtester_nextest_archive_recipe_errors(file_name: str, text: str) -> list[str]:
    if file_name != "justfile" and not file_name.endswith("/justfile"):
        return []
    start = text.find("bte-test-archive archive *args:")
    if start == -1:
        return []
    end = text.find("\nbte-build:", start)
    recipe_text = text[start:] if end == -1 else text[start:end]
    errors: list[str] = []
    for required in (
        'case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac',
        '--archive-file "$archive_path"',
    ):
        if required not in recipe_text:
            errors.append("backtester nextest archive recipes must absolutize archive paths from repo_root")
    for forbidden in ('--archive-file "{{archive}}"', "--archive-file '{{archive}}'"):
        if forbidden in recipe_text:
            errors.append("backtester nextest archive recipes must not pass crate-relative archive paths")
    return errors


def normalized_repo_file_name(file_name: str) -> str:
    normalized = file_name.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    repo_root = REPO_ROOT.as_posix()
    if normalized.startswith(repo_root):
        normalized = normalized[len(repo_root) :].lstrip("/")
    return normalized


def jules_advisory_workflow_contract_errors(file_name: str, text: str) -> list[str]:
    normalized = normalized_repo_file_name(file_name)
    workflow_path = (
        normalized
        if normalized.startswith(".github/workflows/")
        else f".github/workflows/{normalized}"
    )
    is_allowed_jules_workflow = workflow_path in JULES_ADVISORY_WORKFLOW_PATHS
    errors: list[str] = []
    if JULES_ADVISORY_SECRET in text and not is_allowed_jules_workflow:
        return ["JULES_API_KEY may only be used by Jules advisory workflows"]
    if not is_allowed_jules_workflow:
        return []

    required = (
        ("permissions: {}", "Jules advisory workflows must use empty permissions"),
        (
            f"{JULES_ADVISORY_SECRET}: ${{{{ secrets.{JULES_ADVISORY_SECRET} }}}}",
            "Jules advisory workflows must use only the JULES_API_KEY secret",
        ),
        (
            f"JULES_SESSIONS_ENDPOINT: ${{{{ vars.{JULES_ADVISORY_ENDPOINT_VARIABLE} }}}}",
            "Jules advisory workflows must use configured sessions endpoint variable",
        ),
        (
            f"timeout-minutes: ${{{{ fromJSON(vars.{JULES_ADVISORY_TIMEOUT_VARIABLE}) }}}}",
            "Jules advisory workflows must use configured session timeout variable",
        ),
        ('"$JULES_SESSIONS_ENDPOINT"', "Jules advisory workflows must use configured sessions endpoint variable"),
        ('automationMode: "AUTO_CREATE_PR"', "Jules advisory workflows must use Jules PR automation mode"),
        ("requirePlanApproval: true", "Jules advisory workflows must require plan approval"),
        ("continue-on-error: true", "Jules advisory workflows must remain non-blocking"),
        ("Create a draft pull request only", "Jules advisory workflows must constrain Jules to draft PRs"),
        ("Label any pull request with agent:jules", "Jules advisory workflows must label Jules PRs"),
    )
    for needle, message in required:
        if needle not in text:
            errors.append(message)

    if "https://jules.googleapis.com" in text:
        errors.append("Jules advisory workflows must use configured sessions endpoint variable")
    if "timeout-minutes: 10" in text:
        errors.append("Jules advisory workflows must use configured session timeout variable")
    if "requirePlanApproval: false" in text:
        errors.append("Jules advisory workflows must require plan approval")
    if "Verified Jules session evidence" in text:
        errors.append("Jules advisory workflows must not claim verified session evidence on unavailable results")

    secret_refs = set(GITHUB_SECRET_REF_RE.findall(text))
    extra_secrets = secret_refs - {JULES_ADVISORY_SECRET}
    if extra_secrets:
        errors.append(
            "Jules advisory workflows must not reference non-Jules secrets: "
            + ", ".join(sorted(extra_secrets))
        )
    for forbidden in ("github.token", "GITHUB_TOKEN", "role-to-assume:", "aws-actions/"):
        if forbidden in text:
            errors.append("Jules advisory workflows must not use GitHub token or AWS credentials")
            break

    shell_text = "\n".join(yaml_run_shell_texts(uncommented_text(text.splitlines())))
    if JULES_AWS_COMMAND_RE.search(shell_text) is not None or "AWS_" in shell_text:
        errors.append("Jules advisory workflows must not use AWS commands")

    success_if = "if: ${{ steps.invoke-jules.outcome == 'success' }}"
    success_notice = "::notice::Jules advisory session started and returned a session id"
    if success_if not in text or success_notice not in text:
        errors.append("Jules advisory workflows must emit verified session notice only on invoke success")

    unavailable_if = "if: ${{ steps.invoke-jules.outcome != 'success' }}"
    unavailable_warning = "::warning::Jules advisory session did not start"
    if unavailable_if not in text or unavailable_warning not in text:
        errors.append("Jules advisory workflows must warn when invocation is unavailable")

    return errors


def verify_repo_automation_texts(texts: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for file_name, text in texts.items():
        if file_name == "ci/rust-ci-inputs.toml" or file_name.endswith("/ci/rust-ci-inputs.toml"):
            add_unique_errors(
                errors,
                (f"{file_name}: {error}" for error in ci_input_set_config_errors(file_name, text)),
            )
            continue
        errors.extend(f"{file_name}: {error}" for error in raw_rust_storage_errors(text))
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_nextest_archive_recipe_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_managed_target_cache_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_gate_detect_result_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_test_shard_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_detect_path_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in backtester_draft_deferral_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in cache_same_run_transport_errors(file_name, text)),
        )
        add_unique_errors(
            errors,
            (f"{file_name}: {error}" for error in jules_advisory_workflow_contract_errors(file_name, text)),
        )
        if file_name == "actionlint.yml" or file_name.endswith("/actionlint.yml"):
            for required_command in ACTIONLINT_WORKFLOW_REQUIRED_COMMANDS:
                command_count = workflow_run_command_count(text, required_command)
                if command_count == 0:
                    errors.append(f"{file_name}: actionlint workflow must run {required_command}")
                elif command_count > 1:
                    errors.append(f"{file_name}: actionlint workflow must run {required_command} exactly once")
            add_unique_errors(
                errors,
                (
                    f"{file_name}: {error}"
                    for error in workflow_pull_request_type_errors(
                        text,
                        required_types=("ready_for_review", "edited"),
                    )
                ),
            )
            if "merge_group" not in workflow_trigger_keys(text):
                # actionlint is a required check; it must report on merge_group
                # or the merge queue is blocked.
                errors.append(f"{file_name}: on must define merge_group for merge queue")
            # actionlint uses a simpler concurrency shape than ci.yml (no PR
            # draft-deferral split), so it cannot reuse verify_pr_concurrency.
            # Hold its merge_group concurrency arm to the same fail-closed
            # isolation: keyed on github.ref, never cancelled.
            add_unique_errors(
                errors,
                (
                    f"{file_name}: {error}"
                    for error in verify_merge_group_concurrency(text)
                ),
            )
        if file_name == "backtester-ci.yml" or file_name.endswith("/backtester-ci.yml"):
            jobs = parse_jobs(text)
            add_unique_errors(
                errors,
                (
                    f"{file_name}: {error}"
                    for error in workflow_pull_request_type_errors(
                        text,
                        required_types=("ready_for_review", "edited", "converted_to_draft"),
                    )
                ),
            )
            if "merge_group" not in workflow_trigger_keys(text):
                errors.append(f"{file_name}: on must define merge_group for merge queue")
            if "detect" in jobs and not backtester_detect_forces_bvs_changed_on_merge_group(jobs["detect"]):
                errors.append(
                    f"{file_name}: backtester detect must force bvs_changed=true for merge_group"
                )
            if "detect" in jobs and not backtester_detect_forced_events_use_exact_head_namespace(jobs["detect"]):
                errors.append(
                    f"{file_name}: backtester forced detect events must use exact-head cache namespace"
                )
            add_unique_errors(
                errors,
                (
                    f"{file_name}: {error}"
                    for error in verify_merge_group_concurrency(text)
                ),
            )
        automation_texts = [text, *yaml_run_shell_texts(uncommented_text(text.splitlines()))]
        for automation_text in automation_texts:
            add_unique_errors(
                errors,
                (f"{file_name}: {error}" for error in repo_automation_raw_cargo_errors(file_name, automation_text)),
            )
            add_unique_errors(
                errors,
                (f"{file_name}: {error}" for error in repo_automation_source_build_errors(automation_text)),
            )
    return errors


def verify_workflows(workflows: dict[str, str], action_text: str, nextest_config_text: str) -> list[str]:
    errors: list[str] = []
    for workflow_name, workflow_text in workflows.items():
        is_managed_workflow = workflow_name in {
            "ci.yml",
            ".github/workflows/ci.yml",
            "advisory.yml",
            ".github/workflows/advisory.yml",
        }
        if workflow_name == "ci.yml" or workflow_name.endswith("/ci.yml"):
            errors.extend(verify_workflow(workflow_text))
        else:
            errors.extend(verify_repo_automation_texts({workflow_name: workflow_text}))
        if is_managed_workflow:
            errors.extend(verify_managed_workflow(workflow_text, workflow_name))
            errors.extend(verify_build_artifacts(workflow_text, workflow_name))
            errors.extend(verify_prebuilt_tool_installs(workflow_text, workflow_name))
    errors.extend(raw_rust_storage_errors(action_text))
    errors.extend(verify_setup_action(action_text))
    errors.extend(verify_nextest_config(nextest_config_text))
    install_action_pin_sources = dict(workflows)
    install_action_pin_sources[".github/actions/setup-environment/action.yml"] = action_text
    errors.extend(verify_install_action_pin_consistency(install_action_pin_sources))
    return errors


def verify_install_action_pin_consistency(sources: dict[str, str]) -> list[str]:
    # Dependabot groups action bumps so all taiki-e/install-action pins move
    # together; this guards against half-bumps in human-authored PRs that
    # leave workflow/action files referencing inconsistent SHAs. Scan line-by-line
    # after stripping comments so commentary containing the action ref does
    # not produce false positives.
    #
    # The broad detector (TAIKI_INSTALL_ACTION_MENTION_RE) finds every line
    # that mentions the action ref at all — including YAML multi-line scalar
    # form where `uses:` sits on a preceding line. Any such line that does
    # not match the strict single-line pinned form is reported with a precise
    # file:line so mutable tags (e.g. @v2), multi-line scalars, mismatched
    # quotes, and other malformed pins fail loudly instead of being silently
    # skipped. SHAs are lowercased before bucketing so the consistency check
    # treats uppercase and lowercase hex as the same pin. Lines that fail
    # the strict form do NOT contribute to the bucket map — a malformed
    # reference must not phantom-bucket and mask a real drift.
    errors: list[str] = []
    sha_to_files: dict[str, list[str]] = {}
    for source_name, source_text in sources.items():
        previous_line_was_bare_uses_key = False
        for line_index, line in enumerate(source_text.splitlines(), start=1):
            clean = strip_comment(line)
            mentions_install_action = bool(TAIKI_INSTALL_ACTION_MENTION_RE.search(clean))
            scoped_to_uses_value = (
                bool(TAIKI_INSTALL_ACTION_USES_LINE_RE.match(clean))
                or previous_line_was_bare_uses_key
            )
            previous_line_was_bare_uses_key = bool(TAIKI_INSTALL_ACTION_BARE_USES_KEY_RE.match(clean))
            if not mentions_install_action or not scoped_to_uses_value:
                continue
            match = TAIKI_INSTALL_ACTION_RE.match(clean)
            if match is None:
                errors.append(
                    f"{source_name}:{line_index}: taiki-e/install-action must be referenced as "
                    f"'uses: taiki-e/install-action@<40-hex-SHA>' on a single line, got: {clean.strip()}"
                )
                continue
            sha = match.group(2).lower()
            sha_to_files.setdefault(sha, []).append(source_name)
    if len(sha_to_files) > 1:
        parts = sorted(
            f"{sha} in {','.join(sorted(set(files)))}"
            for sha, files in sha_to_files.items()
        )
        errors.append("taiki-e/install-action pin drift: " + "; ".join(parts))
    return errors


def require_config_table(parent: dict[str, object], key: str, prefix: str) -> dict[str, object]:
    value = parent.get(key)
    if not isinstance(value, dict):
        raise ValueError(f"{prefix}.{key} must be a table")
    return value


def require_config_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_config_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"{prefix}.{key} must be a positive integer")
    return value


def require_config_string_list(parent: dict[str, object], key: str, prefix: str) -> list[str]:
    value = parent.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) and item.strip() for item in value):
        raise ValueError(f"{prefix}.{key} must be a non-empty string list")
    return value


def require_config_string_map(parent: dict[str, object], key: str, prefix: str) -> dict[str, str]:
    value = parent.get(key)
    if not isinstance(value, dict) or not value:
        raise ValueError(f"{prefix}.{key} must be a non-empty string table")
    if not all(isinstance(item_key, str) and item_key.strip() for item_key in value):
        raise ValueError(f"{prefix}.{key} keys must be non-empty strings")
    if not all(isinstance(item_value, str) and item_value.strip() for item_value in value.values()):
        raise ValueError(f"{prefix}.{key} values must be non-empty strings")
    return dict(value)


def require_config_bool(parent: dict[str, object], key: str, prefix: str) -> bool:
    value = parent.get(key)
    if not isinstance(value, bool):
        raise ValueError(f"{prefix}.{key} must be a boolean")
    return value


def require_config_only_keys(parent: dict[str, object], allowed_keys: set[str], prefix: str) -> None:
    unexpected_keys = sorted(set(parent) - allowed_keys)
    if unexpected_keys:
        raise ValueError(f"{prefix} has unexpected keys: {unexpected_keys!r}")


def resolve_config_positive_int_ref(data: dict[str, object], ref: str, prefix: str) -> int:
    keys = ref.split(".")
    if not keys or any(not key for key in keys):
        raise ValueError(f"{prefix} must be a dotted TOML key reference")
    current: object = data
    for key in keys:
        if not isinstance(current, dict) or key not in current:
            raise ValueError(f"{prefix} references missing TOML key {ref!r}")
        current = current[key]
    if isinstance(current, bool) or not isinstance(current, int) or current <= 0:
        raise ValueError(f"{prefix} must reference a positive integer")
    return current


def resolve_config_string_ref(data: dict[str, object], ref: str, prefix: str) -> str:
    keys = ref.split(".")
    if not keys or any(not key for key in keys):
        raise ValueError(f"{prefix} must be a dotted TOML key reference")
    current: object = data
    for key in keys:
        if not isinstance(current, dict) or key not in current:
            raise ValueError(f"{prefix} references missing TOML key {ref!r}")
        current = current[key]
    if not isinstance(current, str) or not current.strip():
        raise ValueError(f"{prefix} must reference a non-empty string")
    return current


def resolve_config_string_map_ref(data: dict[str, object], ref: str, prefix: str) -> dict[str, str]:
    keys = ref.split(".")
    if not keys or any(not key for key in keys):
        raise ValueError(f"{prefix} must be a dotted TOML key reference")
    current: object = data
    for key in keys:
        if not isinstance(current, dict) or key not in current:
            raise ValueError(f"{prefix} references missing TOML key {ref!r}")
        current = current[key]
    if not isinstance(current, dict) or not current:
        raise ValueError(f"{prefix} must reference a non-empty string table")
    if not all(isinstance(item_key, str) and item_key.strip() for item_key in current):
        raise ValueError(f"{prefix} must reference a table with non-empty string keys")
    if not all(isinstance(item_value, str) and item_value.strip() for item_value in current.values()):
        raise ValueError(f"{prefix} must reference a table with non-empty string values")
    return dict(current)


def render_config_string_template(template: str, template_vars: dict[str, str], prefix: str) -> str:
    placeholders = set(CONFIG_TEMPLATE_PLACEHOLDER_RE.findall(template))
    if not placeholders:
        raise ValueError(f"{prefix} must include at least one template placeholder")
    missing_vars = sorted(placeholders - set(template_vars))
    if missing_vars:
        raise ValueError(f"{prefix} missing template vars: {missing_vars!r}")
    unused_vars = sorted(set(template_vars) - placeholders)
    if unused_vars:
        raise ValueError(f"{prefix} has unused template vars: {unused_vars!r}")
    rendered = template
    for name in sorted(placeholders):
        rendered = rendered.replace(f"{{{name}}}", template_vars[name])
    return rendered


def artifact_retention_select_source_mode(
    raw: dict[str, object],
    modes: tuple[ArtifactRetentionSourceMode, ...],
    prefix: str,
    field: str,
) -> ArtifactRetentionSourceResolver:
    complete_modes: list[ArtifactRetentionSourceMode] = []
    partial_modes: list[ArtifactRetentionSourceMode] = []
    for mode in modes:
        present_keys = [key for key in mode.keys if key in raw]
        if len(present_keys) == len(mode.keys):
            complete_modes.append(mode)
        elif present_keys:
            partial_modes.append(mode)
    if len(complete_modes) > 1 or (complete_modes and partial_modes):
        raise ValueError(f"{prefix} must define exactly one {field} source")
    if partial_modes:
        missing_keys = [
            key
            for mode in partial_modes
            for key in mode.keys
            if key not in raw
        ]
        raise ValueError(
            f"{prefix} has partial {field} source; missing {sorted(missing_keys)!r}"
        )
    if not complete_modes:
        raise ValueError(f"{prefix} must define exactly one {field} source")
    return complete_modes[0].resolver


def resolve_repo_toml_config_file(config_path: pathlib.Path, file_ref: str, prefix: str) -> dict[str, object]:
    if not isinstance(file_ref, str) or not file_ref.strip():
        raise ValueError(f"{prefix} must be a non-empty string")
    if "\\" in file_ref:
        raise ValueError(f"{prefix} must be a POSIX repo-relative TOML path")
    repo_path = pathlib.PurePosixPath(file_ref)
    if repo_path.is_absolute() or any(part in {"", ".", ".."} for part in repo_path.parts):
        raise ValueError(f"{prefix} must be a repo-relative TOML path")
    if repo_path.suffix != ".toml":
        raise ValueError(f"{prefix} must reference a TOML file")
    repo_root = config_path.parent.parent.resolve()
    file_path = repo_root.joinpath(*repo_path.parts)
    if not file_path.is_file():
        raise ValueError(f"{prefix} references missing TOML file {file_ref!r}")
    if not file_path.resolve().is_relative_to(repo_root):
        raise ValueError(f"{prefix} must resolve inside the repository")
    return tomllib.loads(file_path.read_text(encoding="utf-8"))


def resolve_artifact_retention_config_ref(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    file_key: str,
    ref_key: str,
    prefix: str,
) -> tuple[dict[str, object], str, str]:
    file_ref = require_config_string(raw, file_key, prefix)
    ref = require_config_string(raw, ref_key, prefix)
    target_config = resolve_repo_toml_config_file(config_path, file_ref, f"{prefix}.{file_key}")
    return target_config, file_ref, ref


def artifact_retention_literal_name_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> str:
    del data, config_path
    return require_config_string(raw, "artifact_name", prefix)


def artifact_retention_config_ref_name_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> str:
    target_config, _file_ref, ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "artifact_name_config_file",
        "artifact_name_config_ref",
        prefix,
    )
    return resolve_config_string_ref(target_config, ref, f"{prefix}.artifact_name_config_ref")


def artifact_retention_template_name_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> str:
    template_config, _template_file_ref, template_ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "artifact_name_template_config_file",
        "artifact_name_template_config_ref",
        prefix,
    )
    vars_config, _vars_file_ref, vars_ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "artifact_name_template_vars_config_file",
        "artifact_name_template_vars_config_ref",
        prefix,
    )
    artifact_name_template = resolve_config_string_ref(
        template_config,
        template_ref,
        f"{prefix}.artifact_name_template_config_ref",
    )
    artifact_name_template_vars = resolve_config_string_map_ref(
        vars_config,
        vars_ref,
        f"{prefix}.artifact_name_template_vars_config_ref",
    )
    return render_config_string_template(
        artifact_name_template,
        artifact_name_template_vars,
        f"{prefix}.artifact_name_template_config_ref",
    )


def artifact_retention_literal_days_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> ArtifactRetentionResolvedInt:
    del data, config_path
    return ArtifactRetentionResolvedInt(
        value=require_config_positive_int(raw, "retention_days", prefix),
        config_file=None,
        config_ref=None,
    )


def artifact_retention_config_ref_days_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> ArtifactRetentionResolvedInt:
    target_config, file_ref, ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "retention_days_config_file",
        "retention_days_config_ref",
        prefix,
    )
    return ArtifactRetentionResolvedInt(
        value=resolve_config_positive_int_ref(target_config, ref, f"{prefix}.retention_days_config_ref"),
        config_file=file_ref,
        config_ref=ref,
    )


def artifact_retention_optional_required_if(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> str | None:
    keys = ("required_if_config_file", "required_if_config_ref")
    present_keys = [key for key in keys if key in raw]
    if not present_keys:
        return None
    if len(present_keys) != len(keys):
        missing_keys = sorted(set(keys) - set(present_keys))
        raise ValueError(f"{prefix} has partial required if source; missing {missing_keys!r}")
    target_config, _file_ref, ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "required_if_config_file",
        "required_if_config_ref",
        prefix,
    )
    return resolve_config_string_ref(target_config, ref, f"{prefix}.required_if_config_ref")


def artifact_retention_literal_class_ceiling_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> ArtifactRetentionResolvedInt:
    del data, config_path
    return ArtifactRetentionResolvedInt(
        value=require_config_positive_int(raw, "max_retention_days", prefix),
        config_file=None,
        config_ref=None,
    )


def artifact_retention_config_ref_class_ceiling_source(
    data: dict[str, object],
    config_path: pathlib.Path,
    raw: dict[str, object],
    prefix: str,
) -> ArtifactRetentionResolvedInt:
    target_config, file_ref, ref = resolve_artifact_retention_config_ref(
        data,
        config_path,
        raw,
        "max_retention_days_config_file",
        "max_retention_days_config_ref",
        prefix,
    )
    return ArtifactRetentionResolvedInt(
        value=resolve_config_positive_int_ref(target_config, ref, f"{prefix}.max_retention_days_config_ref"),
        config_file=file_ref,
        config_ref=ref,
    )


ARTIFACT_RETENTION_NAME_SOURCE_MODES = (
    ArtifactRetentionSourceMode("literal", ("artifact_name",), artifact_retention_literal_name_source),
    ArtifactRetentionSourceMode(
        "config_ref",
        ("artifact_name_config_file", "artifact_name_config_ref"),
        artifact_retention_config_ref_name_source,
    ),
    ArtifactRetentionSourceMode(
        "template",
        (
            "artifact_name_template_config_file",
            "artifact_name_template_config_ref",
            "artifact_name_template_vars_config_file",
            "artifact_name_template_vars_config_ref",
        ),
        artifact_retention_template_name_source,
    ),
)
ARTIFACT_RETENTION_RETENTION_SOURCE_MODES = (
    ArtifactRetentionSourceMode("literal", ("retention_days",), artifact_retention_literal_days_source),
    ArtifactRetentionSourceMode(
        "config_ref",
        ("retention_days_config_file", "retention_days_config_ref"),
        artifact_retention_config_ref_days_source,
    ),
)
ARTIFACT_RETENTION_CLASS_CEILING_SOURCE_MODES = (
    ArtifactRetentionSourceMode("literal", ("max_retention_days",), artifact_retention_literal_class_ceiling_source),
    ArtifactRetentionSourceMode(
        "config_ref",
        ("max_retention_days_config_file", "max_retention_days_config_ref"),
        artifact_retention_config_ref_class_ceiling_source,
    ),
)


def validate_artifact_retention_config(data: dict[str, object], config_path: pathlib.Path) -> ArtifactRetentionPolicy:
    artifact_retention = data.get("artifact_retention")
    if not isinstance(artifact_retention, dict):
        raise ValueError("ci/github-actions-runners.toml must define [artifact_retention]")
    require_config_only_keys(artifact_retention, {"classes", "uploads", "lookback_bindings"}, "artifact_retention")

    raw_classes = require_config_table(artifact_retention, "classes", "artifact_retention")
    classes: dict[str, ArtifactRetentionClass] = {}
    for class_name, raw_class in sorted(raw_classes.items()):
        if not isinstance(class_name, str) or not class_name:
            raise ValueError("artifact_retention.classes keys must be non-empty strings")
        if not isinstance(raw_class, dict):
            raise ValueError(f"artifact_retention.classes.{class_name} must be a table")
        prefix = f"artifact_retention.classes.{class_name}"
        require_config_only_keys(
            raw_class,
            {"max_retention_days", "max_retention_days_config_file", "max_retention_days_config_ref"},
            prefix,
        )
        max_retention_resolver = artifact_retention_select_source_mode(
            raw_class,
            ARTIFACT_RETENTION_CLASS_CEILING_SOURCE_MODES,
            prefix,
            "max retention",
        )
        max_retention = max_retention_resolver(data, config_path, raw_class, prefix)
        if not isinstance(max_retention, ArtifactRetentionResolvedInt):
            raise ValueError(f"{prefix} max retention source resolved invalid type")
        classes[class_name] = ArtifactRetentionClass(
            max_retention_days=max_retention.value,
        )

    raw_uploads = require_config_table(artifact_retention, "uploads", "artifact_retention")
    uploads: dict[str, ArtifactRetentionUploadSite] = {}
    for upload_key, raw_upload in sorted(raw_uploads.items()):
        if not isinstance(upload_key, str) or not upload_key:
            raise ValueError("artifact_retention.uploads keys must be non-empty strings")
        key_parts = upload_key.split("::")
        if len(key_parts) != 3 or any(not part for part in key_parts):
            raise ValueError("artifact_retention.uploads keys must be source::job_id::step_id")
        if not artifact_retention_source_is_canonical(key_parts[0]):
            raise ValueError(
                "artifact_retention.uploads source must use canonical repo path "
                "under .github/workflows/ or .github/actions/"
            )
        if not isinstance(raw_upload, dict):
            raise ValueError(f"artifact_retention.uploads.{upload_key} must be a table")
        prefix = f"artifact_retention.uploads.{upload_key}"
        require_config_only_keys(
            raw_upload,
            {
                "artifact_name",
                "artifact_name_config_file",
                "artifact_name_config_ref",
                "artifact_name_template_config_file",
                "artifact_name_template_config_ref",
                "artifact_name_template_vars_config_file",
                "artifact_name_template_vars_config_ref",
                "artifact_class",
                "retention_days",
                "retention_days_config_file",
                "retention_days_config_ref",
                "required_if_config_file",
                "required_if_config_ref",
            },
            prefix,
        )
        artifact_name_resolver = artifact_retention_select_source_mode(
            raw_upload,
            ARTIFACT_RETENTION_NAME_SOURCE_MODES,
            prefix,
            "artifact name",
        )
        artifact_name = artifact_name_resolver(data, config_path, raw_upload, prefix)
        if not isinstance(artifact_name, str):
            raise ValueError(f"{prefix} artifact name source resolved invalid type")
        artifact_class = require_config_string(raw_upload, "artifact_class", prefix)
        if artifact_class not in classes:
            raise ValueError(f"{prefix}.artifact_class must reference a configured class")
        retention_resolver = artifact_retention_select_source_mode(
            raw_upload,
            ARTIFACT_RETENTION_RETENTION_SOURCE_MODES,
            prefix,
            "retention-days",
        )
        retention = retention_resolver(data, config_path, raw_upload, prefix)
        if not isinstance(retention, ArtifactRetentionResolvedInt):
            raise ValueError(f"{prefix} retention-days source resolved invalid type")
        required_if = artifact_retention_optional_required_if(data, config_path, raw_upload, prefix)
        if artifact_class == DEPLOYABLE_ARTIFACT_CLASS and required_if is None:
            raise ValueError(f"{prefix} deployable uploads must define required_if")
        if upload_key == DEPLOY_ARTIFACT_UPLOAD_KEY:
            if artifact_class != DEPLOYABLE_ARTIFACT_CLASS:
                raise ValueError(f"{prefix}.artifact_class must be {DEPLOYABLE_ARTIFACT_CLASS}")
            if (
                raw_upload.get("artifact_name_config_file") != RUNNERS_CONFIG_REF
                or raw_upload.get("artifact_name_config_ref") != DEPLOY_ARTIFACT_NAME_REF
            ):
                raise ValueError(f"{prefix}.artifact_name_config_ref must be {DEPLOY_ARTIFACT_NAME_REF}")
            if retention.config_file != RUNNERS_CONFIG_REF or retention.config_ref != DEPLOY_ARTIFACT_RETENTION_REF:
                raise ValueError(f"{prefix}.retention_days_config_ref must be {DEPLOY_ARTIFACT_RETENTION_REF}")
            if (
                raw_upload.get("required_if_config_file") != RUNNERS_CONFIG_REF
                or raw_upload.get("required_if_config_ref") != DEPLOY_ARTIFACT_REQUIRED_IF_REF
            ):
                raise ValueError(f"{prefix}.required_if_config_ref must be {DEPLOY_ARTIFACT_REQUIRED_IF_REF}")
        uploads[upload_key] = ArtifactRetentionUploadSite(
            artifact_name=artifact_name,
            artifact_class=artifact_class,
            retention_days=retention.value,
            retention_config_file=retention.config_file,
            retention_config_ref=retention.config_ref,
            required_if=required_if,
        )

    used_classes = {site.artifact_class for site in uploads.values()}
    unused_classes = sorted(set(classes) - used_classes)
    if unused_classes:
        raise ValueError(f"artifact_retention.classes has unused classes: {unused_classes!r}")

    raw_bindings = require_config_table(artifact_retention, "lookback_bindings", "artifact_retention")
    lookback_bindings: dict[str, ArtifactRetentionLookbackBinding] = {}
    for binding_name, raw_binding in sorted(raw_bindings.items()):
        if not isinstance(binding_name, str) or not binding_name.strip():
            raise ValueError("artifact_retention.lookback_bindings keys must be non-empty strings")
        if not isinstance(raw_binding, dict):
            raise ValueError(f"artifact_retention.lookback_bindings.{binding_name} must be a table")
        prefix = f"artifact_retention.lookback_bindings.{binding_name}"
        require_config_only_keys(raw_binding, {"upload", "config_file", "retention_ref", "lookback_ref"}, prefix)
        upload = require_config_string(raw_binding, "upload", prefix)
        config_file = require_config_string(raw_binding, "config_file", prefix)
        retention_ref = require_config_string(raw_binding, "retention_ref", prefix)
        lookback_ref = require_config_string(raw_binding, "lookback_ref", prefix)
        if upload not in uploads:
            raise ValueError(f"{prefix}.upload must reference a configured upload")
        site = uploads[upload]
        if site.retention_config_file != config_file or site.retention_config_ref != retention_ref:
            raise ValueError(f"{prefix} must match the upload retention source")
        if upload == DEPLOY_ARTIFACT_UPLOAD_KEY and lookback_ref != DEPLOY_ARTIFACT_LOOKBACK_REF:
            raise ValueError(f"{prefix}.lookback_ref must be {DEPLOY_ARTIFACT_LOOKBACK_REF}")
        binding_config = resolve_repo_toml_config_file(config_path, config_file, f"{prefix}.config_file")
        retention_days = resolve_config_positive_int_ref(binding_config, retention_ref, f"{prefix}.retention_ref")
        max_lookback_age_seconds = resolve_config_positive_int_ref(binding_config, lookback_ref, f"{prefix}.lookback_ref")
        try:
            check_lookback_le_retention(retention_days, max_lookback_age_seconds)
        except ProvenanceError as exc:
            raise ValueError(f"{prefix}: {exc}") from exc
        lookback_bindings[binding_name] = ArtifactRetentionLookbackBinding(
            upload=upload,
            config_file=config_file,
            retention_ref=retention_ref,
            lookback_ref=lookback_ref,
        )

    required_binding_uploads = sorted(
        upload_key
        for upload_key, site in uploads.items()
        if site.retention_config_file is not None and site.retention_config_ref is not None
    )
    declared_binding_uploads = sorted(binding.upload for binding in lookback_bindings.values())
    if declared_binding_uploads != required_binding_uploads:
        raise ValueError(
            "artifact_retention.lookback_bindings must exactly cover config-ref retention uploads: "
            f"expected {required_binding_uploads!r}, got {declared_binding_uploads!r}"
        )

    return ArtifactRetentionPolicy(classes=classes, uploads=uploads, lookback_bindings=lookback_bindings)


def validate_ci_provenance_config(data: dict[str, object]) -> dict[str, object]:
    ci_provenance = data.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        raise ValueError("ci/github-actions-runners.toml must define [ci_provenance]")

    duplicated_fingerprint_keys = {
        "fingerprint_artifact_prefix",
        "fingerprint_workflow",
    } & set(ci_provenance)
    if duplicated_fingerprint_keys:
        raise ValueError(
            "[ci_provenance] must reference [meter] fingerprint keys instead of duplicating "
            + ", ".join(sorted(duplicated_fingerprint_keys))
        )

    if ci_provenance.get("schema_version") != 1:
        raise ValueError("ci_provenance.schema_version must be 1")
    artifact_name_template = require_config_string(
        ci_provenance, "artifact_name_template", "ci_provenance"
    )
    if "{run_attempt}" not in artifact_name_template:
        raise ValueError("ci_provenance.artifact_name_template must include {run_attempt}")
    artifact_name_template_vars = require_config_string_map(
        ci_provenance,
        "artifact_name_template_vars",
        "ci_provenance",
    )
    render_config_string_template(
        artifact_name_template,
        artifact_name_template_vars,
        "ci_provenance.artifact_name_template",
    )
    if require_config_string(ci_provenance, "workflow_key", "ci_provenance") != "ci":
        raise ValueError("ci_provenance.workflow_key must be ci")
    require_config_string(ci_provenance, "workflow_name", "ci_provenance")
    require_config_string(ci_provenance, "workflow_path", "ci_provenance")
    if require_config_string(ci_provenance, "fingerprint_source", "ci_provenance") != "meter":
        raise ValueError("ci_provenance.fingerprint_source must be meter")

    meter = data.get("meter")
    if not isinstance(meter, dict):
        raise ValueError("ci/github-actions-runners.toml must define [meter]")
    require_config_string(meter, "fingerprint_artifact_prefix", "meter")
    require_config_string(meter, "fingerprint_workflow", "meter")

    full_ci = require_config_table(ci_provenance, "full_ci", "ci_provenance")
    required_jobs = require_config_string_list(full_ci, "required_jobs", "ci_provenance.full_ci")
    if tuple(required_jobs) != CI_PROVENANCE_REQUIRED_JOBS:
        raise ValueError(
            "ci_provenance.full_ci.required_jobs must match the current full-CI logical jobs"
        )
    conditional_jobs = require_config_string_list(
        full_ci, "conditional_jobs", "ci_provenance.full_ci"
    )
    if conditional_jobs != ["build"]:
        raise ValueError("ci_provenance.full_ci.conditional_jobs must be ['build']")
    conditional_outputs = full_ci.get("conditional_job_outputs")
    if (
        not isinstance(conditional_outputs, dict)
        or conditional_outputs.get("build") != "detector.build_required"
    ):
        raise ValueError(
            "ci_provenance.full_ci.conditional_job_outputs.build must be detector.build_required"
        )
    jobs = require_config_table(full_ci, "jobs", "ci_provenance.full_ci")
    for job in (*CI_PROVENANCE_REQUIRED_JOBS, "build"):
        if job not in jobs:
            raise ValueError(f"ci_provenance.full_ci.jobs.{job} missing")
        job_table = jobs[job]
        if not isinstance(job_table, dict):
            raise ValueError(f"ci_provenance.full_ci.jobs.{job} must be a table")
        require_config_string(job_table, "check_name", f"ci_provenance.full_ci.jobs.{job}")
        if job == "build" and job_table.get("conditional") != "detector.build_required":
            raise ValueError(
                "ci_provenance.full_ci.jobs.build.conditional must be detector.build_required"
            )

    deploy = require_config_table(ci_provenance, "deploy", "ci_provenance")
    require_config_string(deploy, "artifact_name", "ci_provenance.deploy")
    artifact_upload_if = require_config_string(deploy, "artifact_upload_if", "ci_provenance.deploy")
    deploy_retention_days = require_config_positive_int(
        deploy, "artifact_retention_days", "ci_provenance.deploy"
    )
    deploy_lookback_age_seconds = require_config_positive_int(
        deploy, "artifact_lookback_age_seconds", "ci_provenance.deploy"
    )
    try:
        check_lookback_le_retention(deploy_retention_days, deploy_lookback_age_seconds)
    except ProvenanceError as exc:
        raise ValueError(
            "ci_provenance.deploy.artifact_lookback_age_seconds must not exceed artifact retention"
        ) from exc
    source_event = deploy.get("require_source_event")
    source_branch = deploy.get("require_source_branch")
    if source_event != "push":
        raise ValueError("ci_provenance.deploy.require_source_event must be push")
    if source_branch != "main":
        raise ValueError("ci_provenance.deploy.require_source_branch must be main")
    expected_upload_if = (
        f"${{{{ github.event_name == '{source_event}' && "
        f"github.ref == 'refs/heads/{source_branch}' }}}}"
    )
    if artifact_upload_if != expected_upload_if:
        raise ValueError(
            "ci_provenance.deploy.artifact_upload_if must match push to main deploy source policy"
        )
    if deploy.get("require_gate_check") is not True:
        raise ValueError("ci_provenance.deploy.require_gate_check must be true")

    dispatch = require_config_table(ci_provenance, "dispatch", "ci_provenance")
    run_name_default = require_config_string(dispatch, "run_name_default", "ci_provenance.dispatch")
    run_name_iteration = require_config_string(dispatch, "run_name_iteration", "ci_provenance.dispatch")
    proof_gate_job = require_config_string(dispatch, "proof_gate_job", "ci_provenance.dispatch")
    workflow_name = require_config_string(ci_provenance, "workflow_name", "ci_provenance")
    if run_name_default != workflow_name:
        raise ValueError("ci_provenance.dispatch.run_name_default must match workflow_name")

    gate_names = require_config_table(ci_provenance, "gate_names", "ci_provenance")
    for key in CI_PROVENANCE_GATE_NAME_KEYS:
        gate_name = require_config_string(gate_names, key, "ci_provenance.gate_names")
        if not github_actions_output_safe_check_name(gate_name):
            raise ValueError(
                f"ci_provenance.gate_names.{key} must be a GitHub Actions output-safe check name"
            )
    if proof_gate_job != gate_names["gate_required"]:
        raise ValueError("ci_provenance.dispatch.proof_gate_job must match required gate name")
    gate_name_errors = gate_name_collision_errors(gate_names)
    if gate_name_errors:
        raise ValueError("; ".join(gate_name_errors))

    docs = require_config_table(ci_provenance, "docs", "ci_provenance")
    docs_safe_paths = tuple(require_config_string_list(docs, "safe_paths", "ci_provenance.docs"))
    docs_path_errors = docs_safe_path_contract_errors(docs_safe_paths)
    if docs_path_errors:
        raise ValueError("; ".join(docs_path_errors))
    forbidden_paths = require_config_string_list(
        docs,
        "forbidden_ignored_build_paths",
        "ci_provenance.docs",
    )
    if ".claude/rust-verification.toml" not in forbidden_paths:
        raise ValueError("ci_provenance.docs.forbidden_ignored_build_paths must preserve .claude/rust-verification.toml")
    non_heavy_jobs = require_config_string_list(
        docs,
        "non_heavy_required_jobs",
        "ci_provenance.docs",
    )
    if non_heavy_jobs != ["detector", "source-fence"]:
        raise ValueError("ci_provenance.docs.non_heavy_required_jobs must be ['detector', 'source-fence']")

    api_limits = require_config_table(ci_provenance, "api_limits", "ci_provenance")
    for key in (
        "workflow_runs_per_page",
        "run_jobs_per_page",
        "run_artifacts_per_page",
        "max_lookback_pages",
        "max_lookback_age_seconds",
    ):
        require_config_positive_int(api_limits, key, "ci_provenance.api_limits")

    artifacts = require_config_table(ci_provenance, "artifacts", "ci_provenance")
    retention_days = require_config_positive_int(
        artifacts, "retention_days", "ci_provenance.artifacts"
    )
    try:
        check_lookback_le_retention(
            retention_days,
            require_config_positive_int(
                api_limits,
                "max_lookback_age_seconds",
                "ci_provenance.api_limits",
            ),
        )
    except ProvenanceError as exc:
        raise ValueError(
            "ci_provenance.api_limits.max_lookback_age_seconds must not exceed artifact retention"
        ) from exc

    policy = require_config_table(ci_provenance, "policy", "ci_provenance")
    unexpected_policy_keys = set(policy) - set(CI_PROVENANCE_POLICY_ROWS) - {"override"}
    if unexpected_policy_keys:
        raise ValueError(
            f"ci_provenance.policy has unexpected keys: {sorted(unexpected_policy_keys)!r}"
        )
    for row in CI_PROVENANCE_POLICY_ROWS:
        value = policy.get(row)
        if value not in CI_PROVENANCE_POLICY_VALUES:
            raise ValueError(
                f"ci_provenance.policy.{row} must be one of {sorted(CI_PROVENANCE_POLICY_VALUES)!r}"
            )
    proof_errors = policy_proof_invariant_errors(policy)
    contract_errors = policy_contract_errors(policy)
    if proof_errors or contract_errors:
        raise ValueError("; ".join([*proof_errors, *contract_errors]))
    override = require_config_table(policy, "override", "ci_provenance.policy")
    if override.get("force_full_ci") is not False:
        raise ValueError("ci_provenance.policy.override.force_full_ci must default to false")
    if override.get("ignore_emit_failure") is not False:
        raise ValueError(
            "ci_provenance.policy.override.ignore_emit_failure must default to false"
        )

    mergify = require_config_table(ci_provenance, "mergify", "ci_provenance")
    if require_config_string(mergify, "temp_pr_head_ref_prefix", "ci_provenance.mergify") != "mergify/merge-queue/":
        raise ValueError("ci_provenance.mergify.temp_pr_head_ref_prefix must be mergify/merge-queue/")

    return ci_provenance


def validate_dispatch_cancel_config(data: dict[str, object]) -> dict[str, object]:
    section = data.get("dispatch_cancel")
    if not isinstance(section, dict):
        raise ValueError("ci/github-actions-runners.toml must define [dispatch_cancel]")
    event = section.get("workflow_event")
    if event != "workflow_dispatch":
        raise ValueError("dispatch_cancel.workflow_event must be workflow_dispatch")
    active_statuses = section.get("active_statuses")
    required_statuses = {"queued", "requested", "waiting", "pending", "in_progress"}
    if not isinstance(active_statuses, list) or set(active_statuses) != required_statuses:
        raise ValueError(
            "dispatch_cancel.active_statuses must cover queued, requested, waiting, pending, and in_progress"
        )
    for key in ("workflow_runs_per_page", "max_pages"):
        require_config_positive_int(section, key, "dispatch_cancel")
    return section


def validate_jules_advisory_config(data: dict[str, object]) -> dict[str, object]:
    section = data.get("jules_advisory")
    if not isinstance(section, dict):
        raise ValueError("ci/github-actions-runners.toml must define [jules_advisory]")
    workflow_paths = require_config_string_list(
        section, "workflow_paths", "jules_advisory"
    )
    if set(workflow_paths) != JULES_ADVISORY_WORKFLOW_PATHS:
        raise ValueError("jules_advisory.workflow_paths must match Jules advisory workflows")
    secret = require_config_string(section, "secret", "jules_advisory")
    if secret != JULES_ADVISORY_SECRET:
        raise ValueError("jules_advisory.secret must be JULES_API_KEY")
    sessions_endpoint_variable = require_config_string(
        section, "sessions_endpoint_variable", "jules_advisory"
    )
    if sessions_endpoint_variable != JULES_ADVISORY_ENDPOINT_VARIABLE:
        raise ValueError("jules_advisory.sessions_endpoint_variable must be JULES_SESSIONS_ENDPOINT")
    timeout_variable = require_config_string(
        section, "session_timeout_minutes_variable", "jules_advisory"
    )
    if timeout_variable != JULES_ADVISORY_TIMEOUT_VARIABLE:
        raise ValueError("jules_advisory.session_timeout_minutes_variable must be JULES_SESSION_TIMEOUT_MINUTES")
    sessions_endpoint = require_config_string(
        section, "sessions_endpoint", "jules_advisory"
    )
    timeout_minutes = require_config_positive_int(
        section, "session_timeout_minutes", "jules_advisory"
    )
    if section.get("require_plan_approval") is not True:
        raise ValueError("jules_advisory.require_plan_approval must be true")
    return {
        "workflow_paths": sorted(workflow_paths),
        "secret": secret,
        "sessions_endpoint_variable": sessions_endpoint_variable,
        "session_timeout_minutes_variable": timeout_variable,
        "repository_variables": {
            sessions_endpoint_variable: sessions_endpoint,
            timeout_variable: str(timeout_minutes),
        },
        "require_plan_approval": True,
    }


def github_actions_runners_config_floor_errors() -> list[str]:
    findings: list[str] = []
    if not DEFAULT_RUNNERS_CONFIG.exists():
        require_nonempty((), RUNNERS_CONFIG_LABEL, findings)
        return findings
    try:
        text = DEFAULT_RUNNERS_CONFIG.read_text(encoding="utf-8")
    except OSError as exc:
        return [f"github-actions runner config invalid: {exc}"]
    require_nonempty(text.strip(), RUNNERS_CONFIG_LABEL, findings)
    return findings


def load_required_github_actions_runners_config() -> tuple[dict[str, object] | None, list[str]]:
    floor_errors = github_actions_runners_config_floor_errors()
    if floor_errors:
        return None, floor_errors
    try:
        return load_github_actions_runners_config(), []
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        return None, [f"github-actions runner config invalid: {exc}"]


def load_github_actions_runners_config(
    path: pathlib.Path | None = None,
) -> dict[str, object]:
    if path is None:
        path = DEFAULT_RUNNERS_CONFIG
    if not path.exists():
        raise FileNotFoundError(f"managed runner config missing: {path}")
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    runners = data.get("runners")
    workflows = data.get("workflows")
    meter = data.get("meter")
    if not isinstance(runners, dict) or not isinstance(workflows, dict):
        raise ValueError("ci/github-actions-runners.toml must define [runners] and [workflows]")
    if not isinstance(meter, dict):
        raise ValueError("ci/github-actions-runners.toml must define [meter]")
    ci_provenance = validate_ci_provenance_config(data)
    artifact_retention = validate_artifact_retention_config(data, path)
    dispatch_cancel = validate_dispatch_cancel_config(data)
    jules_advisory = validate_jules_advisory_config(data)
    cargo_build_jobs = validate_cargo_build_jobs_config(data)
    meter_workflows = meter.get("included_workflows")
    if not isinstance(meter_workflows, list) or not all(
        isinstance(workflow, str) and workflow for workflow in meter_workflows
    ):
        raise ValueError("meter.included_workflows must be a non-empty string list")
    meter_api_limits = meter.get("api_limits")
    if not isinstance(meter_api_limits, dict):
        raise ValueError("meter.api_limits must be a table")
    for key in (
        "workflow_runs_per_page",
        "run_jobs_per_page",
        "run_artifacts_per_page",
        "branch_pull_requests_per_page",
        "draft_timeline_items",
    ):
        value = meter_api_limits.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"meter.api_limits.{key} must be a positive integer")
    tier_to_var: dict[str, str] = {}
    managed_labels: list[str] = []
    for tier, entry in runners.items():
        if not isinstance(entry, dict):
            raise ValueError(f"runners.{tier} must be a table")
        variable = entry.get("variable")
        label = entry.get("label")
        if not isinstance(variable, str) or not variable:
            raise ValueError(f"runners.{tier}.variable must be a non-empty string")
        if not isinstance(label, str) or not label:
            raise ValueError(f"runners.{tier}.label must be a non-empty string")
        tier_to_var[tier] = variable
        if tier != "github_hosted":
            managed_labels.append(label)
    for workflow_key, job_table in workflows.items():
        if not isinstance(job_table, dict):
            raise ValueError(f"workflows.{workflow_key} must be a table")
        for job, tier in job_table.items():
            if not isinstance(tier, str) or not tier:
                raise ValueError(f"workflows.{workflow_key}.{job} must name a runner tier")
    return {
        "tier_to_var": tier_to_var,
        "managed_labels": sorted(set(managed_labels)),
        "meter_included_workflows": sorted(set(meter_workflows)),
        "variables": sorted(
            set(tier_to_var.values()) | set(jules_advisory["repository_variables"])
        ),
        "workflows": workflows,
        "ci_provenance": ci_provenance,
        "artifact_retention": artifact_retention,
        "dispatch_cancel": dispatch_cancel,
        "jules_advisory": jules_advisory,
        "cargo_build_jobs": cargo_build_jobs,
    }


def extract_job_runs_on_var(job_lines: list[str]) -> str | None:
    for line in job_lines:
        match = JOB_RUNS_ON_VAR_RE.match(line)
        if match is not None:
            return match.group(1)
    return None


def workflow_trigger_keys(workflow_text: str) -> set[str]:
    lines = [strip_comment(line).rstrip() for line in workflow_text.splitlines()]
    for index, line in enumerate(lines):
        if line == "on:":
            triggers: set[str] = set()
            for child in lines[index + 1 :]:
                if child and not child.startswith((" ", "\t")):
                    break
                match = re.match(r"^  ([^ \t:#][^:#]*):", child)
                if match:
                    triggers.add(match.group(1).strip().strip("'\""))
            return triggers
        if line.startswith("on:"):
            inline = line[len("on:") :].strip()
            if inline.startswith("[") and inline.endswith("]"):
                return {
                    item.strip().strip("'\"")
                    for item in inline[1:-1].split(",")
                    if item.strip()
                }
            if inline:
                return {inline.strip().strip("'\"")}
    return set()


def load_ci_runner_debug_config(path: pathlib.Path | None = None) -> dict[str, str]:
    if path is None:
        path = DEFAULT_RUNNERS_CONFIG
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    section = data.get("ci_runner_debug")
    if not isinstance(section, dict):
        raise ValueError("ci/github-actions-runners.toml must define [ci_runner_debug]")
    required = ("ssh_wait_minutes_variable", "ssh_public_key_secret", "ssh_runner_action")
    config: dict[str, str] = {}
    for key in required:
        value = section.get(key)
        if not isinstance(value, str) or not value:
            raise ValueError(f"ci_runner_debug.{key} must be a non-empty string")
        config[key] = value
    if not SSH_RUNNER_ACTION_RE.fullmatch(config["ssh_runner_action"]):
        raise ValueError(
            "ci_runner_debug.ssh_runner_action must pin ubicloud/ssh-runner to a 40-character SHA"
        )
    return config


def validate_cargo_build_jobs_config(data: dict[str, object]) -> dict[str, dict[str, int]]:
    section = data.get("cargo_build_jobs")
    if not isinstance(section, dict):
        raise ValueError("ci/github-actions-runners.toml must define [cargo_build_jobs]")
    config: dict[str, dict[str, int]] = {}
    for workflow_key, job_table in section.items():
        if not isinstance(workflow_key, str) or not workflow_key:
            raise ValueError("cargo_build_jobs workflow keys must be non-empty strings")
        if not isinstance(job_table, dict):
            raise ValueError(f"cargo_build_jobs.{workflow_key} must be a table")
        config[workflow_key] = {}
        for job, value in job_table.items():
            if not isinstance(job, str) or not job:
                raise ValueError(f"cargo_build_jobs.{workflow_key} job keys must be non-empty strings")
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                raise ValueError(f"cargo_build_jobs.{workflow_key}.{job} must be a positive integer")
            config[workflow_key][job] = value
    return config


def verify_ci_runner_debug_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/ci-runner-debug.yml"
    if workflow_name not in workflows:
        return []
    floor_errors = github_actions_runners_config_floor_errors()
    if floor_errors:
        return floor_errors
    try:
        debug_config = load_ci_runner_debug_config()
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        return [f"ci runner debug config invalid: {exc}"]

    workflow_text = workflows[workflow_name]
    errors: list[str] = []
    expected_action = f"uses: {debug_config['ssh_runner_action']}"
    expected_secret = f"secrets.{debug_config['ssh_public_key_secret']}"
    expected_wait = f"vars.{debug_config['ssh_wait_minutes_variable']}"
    triggers = workflow_trigger_keys(workflow_text)
    if triggers != {"workflow_dispatch"}:
        errors.append(
            f"{workflow_name} must be manual-only with only workflow_dispatch, got {sorted(triggers)!r}"
        )
    jobs = parse_jobs(workflow_text)
    for job in ("debug-heavy", "debug-light"):
        job_lines = jobs.get(job)
        if job_lines is None:
            continue
        if not any(expected_action in line for line in job_lines):
            errors.append(f"{workflow_name} {job} must reference {expected_action!r}")
        if not any(expected_secret in line for line in job_lines):
            errors.append(f"{workflow_name} {job} must reference {expected_secret!r}")
        if not any(expected_wait in line for line in job_lines):
            errors.append(f"{workflow_name} {job} must reference {expected_wait!r}")
    return errors


def verify_debug_test_workflow(
    workflows: dict[str, str],
    justfile_text: str,
    mergify_text: str = "",
) -> list[str]:
    workflow_name = ".github/workflows/debug-test.yml"
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist for the dispatch-only fast debug lane"]
    errors: list[str] = []
    workflow_clean = uncommented_text(workflow_text.splitlines())
    triggers = workflow_trigger_keys(workflow_text)
    if triggers != {"workflow_dispatch"}:
        errors.append(
            f"{workflow_name} debug-test workflow must be workflow_dispatch-only, got {sorted(triggers)!r}"
        )
    trigger_text = "\n".join(workflow_trigger_block(workflow_text, "workflow_dispatch"))
    for input_name in ("ref", "filter", "package"):
        if f"      {input_name}:" not in trigger_text:
            errors.append(f"{workflow_name} debug-test workflow must declare {input_name} input")
    if '        required: true' not in trigger_text or '        default: ""' not in trigger_text:
        errors.append(f"{workflow_name} debug-test workflow must pin required ref/filter and optional package inputs")

    expected_scoped_permissions = {
        ("permissions", "contents", "read"),
        ("jobs.debug-test", "contents", "read"),
        ("jobs.debug-test", "id-token", "write"),
    }
    scoped_permissions = yaml_permissions_scoped_grants(workflow_text)
    if scoped_permissions != expected_scoped_permissions:
        errors.append(f"{workflow_name} debug-test workflow permissions must match scoped allowlist")
    if top_level_block(workflow_text, "concurrency"):
        errors.append(f"{workflow_name} debug-test workflow must not declare concurrency")
    if "AWS_CI_CACHE_ROLE_ARN" in workflow_clean or "AWS_CI_CACHE_PR_READONLY_ROLE_ARN" not in workflow_clean:
        errors.append(f"{workflow_name} debug-test workflow must use the PR-readonly cache role only")
    if "if this lane ever becomes gate-relevant, it migrates to digest pins" not in workflow_text:
        errors.append(f"{workflow_name} debug-test workflow must document the digest-pin graduation contract")
    for forbidden in (
        "actions:",
        "checks:",
        "pull-requests:",
        "check-runs",
    ):
        if forbidden in workflow_clean:
            errors.append(f"{workflow_name} debug-test workflow must not reference {forbidden}")
    if re.search(r"(^|\n)\s*(?:gate|needs):|ci-provenance|provenance|check-ci-gate|check-backtester-gate", workflow_clean):
        errors.append(f"{workflow_name} debug-test workflow must not reference provenance or gate jobs")
    if re.search(r"(^|[\s;&|()])cargo\s+(?:nextest|test|build|check|clippy)\b", workflow_clean):
        errors.append(f"{workflow_name} debug-test workflow must not run raw cargo")

    jobs = parse_jobs(workflow_text)
    job = jobs.get("debug-test")
    if job is None:
        errors.append(f"{workflow_name} debug-test workflow must define debug-test job")
    else:
        job_text = uncommented_text(job)
        if "runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}" not in job_text:
            errors.append(f"{workflow_name} debug-test workflow must run on vars.CI_RUNNER_MANAGED_HEAVY")
        if "timeout-minutes: 30" not in job_text:
            errors.append(f"{workflow_name} debug-test workflow timeout must be 30 minutes")
        for fragment in (
            "build-jobs-key: debug_test.debug-test",
            "include-nextest-version: \"true\"",
            "include-managed-target-dir: \"true\"",
            "just debug-test \"$DEBUG_TEST_FILTER\" \"$DEBUG_TEST_PACKAGE\"",
            "BOLT_RUST_VERIFICATION_SCCACHE:",
            "DEBUG_TEST_FILTER: ${{ inputs.filter }}",
            "DEBUG_TEST_PACKAGE: ${{ inputs.package || '' }}",
            'echo "Checked-out SHA: $(git rev-parse HEAD)" >> "$GITHUB_STEP_SUMMARY"',
            "tail -120 \"$log\"",
            "grep -E",
        ):
            if fragment not in job_text:
                errors.append(f"{workflow_name} debug-test workflow must call managed just debug-test recipe")
                break
        for step_name in (
            "Resolve debug nextest fingerprint",
            "Restore debug nextest archive from S3",
            "Restore debug root binary sidecars from S3",
            "Resolve debug archive reuse",
            "Run debug test",
        ):
            if named_step_block(job, step_name) is None:
                errors.append(f"{workflow_name} debug-test workflow must include step {step_name!r}")
        step_name = "Resolve debug archive cache eligibility"
        step_block = named_step_block(job, step_name)
        step_text = uncommented_text(step_block) if step_block is not None else ""
        if "PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}" not in step_text:
            errors.append(f"{workflow_name} {step_name}' must bind PR_READONLY_ROLE_ARN to the PR-readonly role var")
        if 'echo "role_arn=$PR_READONLY_ROLE_ARN" >> "$GITHUB_OUTPUT"' not in step_text:
            errors.append(f"{workflow_name} {step_name}' must output PR_READONLY_ROLE_ARN as role_arn")
        step_name = "Configure AWS credentials for debug archive cache"
        step_block = named_step_block(job, step_name)
        step_text = uncommented_text(step_block) if step_block is not None else ""
        if "role-to-assume: ${{ steps.debug-archive-cache.outputs.role_arn }}" not in step_text:
            errors.append(f"{workflow_name} {step_name}' must assume the resolved debug archive role")
        sccache_setup_block = named_step_block(job, "Setup read-only sccache")
        sccache_setup_text = uncommented_text(sccache_setup_block) if sccache_setup_block is not None else ""
        for fragment in (
            f"uses: {SCCACHE_SETUP_ACTION_PATH}",
            SCCACHE_READONLY_ROLE_INPUT,
            DEBUG_TEST_SCCACHE_ACTIVE_INPUT,
        ):
            if fragment not in sccache_setup_text:
                errors.append(f"{workflow_name} debug-test workflow must route sccache through the shared read-only setup action")
        run_step = named_step_block(job, "Run debug test")
        run_text = uncommented_text(run_step) if run_step is not None else ""
        preserves_status = 'rc="${PIPESTATUS[0]}"' in run_text
        if "shell: bash" not in run_text or not preserves_status:
            errors.append(f"{workflow_name} debug-test workflow must preserve nextest exit status under bash")

    if "debug-test" in mergify_text:
        errors.append("debug-test workflow must not be referenced by .mergify.yml")

    if justfile_text:
        if (
            'debug-test filter package="": check-workspace require-rust-verification-owner' not in justfile_text
            and 'debug-test filter package="" *extra_args: check-workspace require-rust-verification-owner'
            not in justfile_text
        ):
            errors.append("justfile must define debug-test filter package")
        for fragment in (
            'python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- nextest run --locked',
            '--archive-file "$NEXTEST_ARCHIVE_PATH"',
            '-E "$filter"',
        ):
            if fragment not in justfile_text:
                errors.append("justfile debug-test recipe must route nextest through managed cargo")
                break
        for fragment in (
            'filter="${DEBUG_TEST_FILTER:-}"',
            'package="${DEBUG_TEST_PACKAGE:-}"',
            'if [[ -z "$filter" ]]; then filter={{quote(filter)}}; fi',
            'if [[ -z "$package" ]]; then package={{quote(package)}}; fi',
        ):
            if fragment not in justfile_text:
                errors.append("justfile debug-test recipe must shell-quote direct filter/package arguments")
                break
        if 'if [[ -z "$filter" ]]; then echo "ERROR: debug-test filter must be non-empty" >&2; exit 2; fi' not in justfile_text:
            errors.append("justfile debug-test recipe must fail closed on an empty filter")
    return errors


def verify_dispatch_ci_cancel_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/dispatch-ci-cancel.yml"
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist to cancel stale branch workflow_dispatch CI runs"]
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None

    ci_provenance = config["ci_provenance"]
    workflow_event = config["dispatch_cancel"]["workflow_event"]
    expected_ci_name = str(ci_provenance["workflow_name"])
    jobs = parse_jobs(workflow_text)
    job = jobs.get("cancel-obsolete-dispatch")
    errors: list[str] = []
    if workflow_trigger_keys(workflow_text) != {"workflow_run"}:
        errors.append(f"{workflow_name} must trigger only on workflow_run")
    trigger = "\n".join(workflow_trigger_block(workflow_text, "workflow_run"))
    if f'workflows: ["{expected_ci_name}"]' not in trigger and f"workflows: ['{expected_ci_name}']" not in trigger:
        errors.append(f"{workflow_name} workflow_run trigger must watch {expected_ci_name!r}")
    if "types: [requested]" not in trigger:
        errors.append(f"{workflow_name} workflow_run trigger must use requested only")
    permissions = "\n".join(top_level_block(workflow_text, "permissions"))
    if "  actions: write" not in permissions:
        errors.append(f"{workflow_name} permissions must include actions: write")
    if "  contents: read" not in permissions:
        errors.append(f"{workflow_name} permissions must include contents: read")
    if job is None:
        errors.append(f"{workflow_name} must define cancel-obsolete-dispatch job")
        return errors
    job_if = job_if_value(job)
    job_text = "\n".join(job)
    event_guard = f"github.event.workflow_run.event == '{workflow_event}'"
    path_guard = f"github.event.workflow_run.path == '{ci_provenance['workflow_path']}'"
    if event_guard not in job_if:
        errors.append(f"{workflow_name} job must filter workflow_dispatch runs")
    if "workflow_run.name" in job_if:
        errors.append(f"{workflow_name} job must not filter the configured CI workflow by mutable name")
    if path_guard not in job_if:
        errors.append(f"{workflow_name} job must filter the configured CI workflow by path")
    if re.search(rf"{re.escape(event_guard)}\s*&&\s*{re.escape(path_guard)}", job_if) is None:
        errors.append(f"{workflow_name} job must join workflow_dispatch and CI filters with &&")
    if "python3 scripts/cancel_obsolete_dispatch_runs.py" not in job_text:
        errors.append(f"{workflow_name} job must run scripts/cancel_obsolete_dispatch_runs.py")
    if "GITHUB_TOKEN: ${{ github.token }}" not in job_text:
        errors.append(f"{workflow_name} job must pass github.token without exposing secrets")
    if "GITHUB_EVENT_PATH: ${{ github.event_path }}" not in job_text:
        errors.append(f"{workflow_name} job must pass github.event_path")
    if "GITHUB_REPOSITORY: ${{ github.repository }}" not in job_text:
        errors.append(f"{workflow_name} job must pass github.repository")
    return errors


def verify_merge_readiness_ci_job(workflow_text: str) -> list[str]:
    workflow_name = ".github/workflows/ci.yml"
    jobs = parse_jobs(workflow_text)
    job = jobs.get("merge-readiness-progress")
    errors: list[str] = []
    if job is None:
        return [f"{workflow_name} must define merge-readiness-progress job"]
    job_text = "\n".join(job)
    job_if = job_if_value(job)
    if _normalize_concurrency_text(job_if) != EXPECTED_MERGE_READINESS_PROGRESS_IF:
        errors.append(
            "merge-readiness-progress job if-condition must run only on non-draft "
            "Mergify proof PRs while skipping metadata-only proof PR edits"
        )
    for required in (
        "      contents: read",
        "      checks: read",
        "      pull-requests: write",
    ):
        if required not in job_text:
            errors.append(
                f"merge-readiness-progress permissions must include {required.strip()}"
            )
    if "      issues:" in job_text:
        errors.append("merge-readiness-progress must not request issues: write")
    if "      actions:" in job_text:
        errors.append("merge-readiness-progress must not request actions:")
    for required in (
        "uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd",
        "          ref: ${{ github.event.pull_request.base.sha }}",
        "          persist-credentials: false",
    ):
        if required not in job_text:
            errors.append("merge-readiness-progress must check out the PR base SHA only")
            break
    for forbidden in ("refs/pull", "ref: ${{ github.event.pull_request.head"):
        if forbidden in job_text:
            errors.append("merge-readiness-progress must not check out PR head code")
    if "python3 scripts/merge_readiness.py comment" not in job_text:
        errors.append("merge-readiness-progress must run merge_readiness.py comment")
    if "--watch" not in job_text:
        errors.append("merge-readiness-progress must watch until terminal status or timeout")
    for required in (
        "GITHUB_TOKEN: ${{ github.token }}",
        "GITHUB_REPOSITORY: ${{ github.repository }}",
        "PR_NUMBER: ${{ github.event.pull_request.number }}",
        "PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}",
    ):
        if required not in job_text:
            errors.append(f"merge-readiness-progress must pass {required.split(':', 1)[0]}")
    return errors


def verify_merge_readiness_finalizer_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/merge-readiness-finalizer.yml"
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist to mark stale merge-readiness comments stalled"]
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None

    ci_provenance = config["ci_provenance"]
    expected_ci_name = str(ci_provenance["workflow_name"])
    errors: list[str] = []
    if workflow_trigger_keys(workflow_text) != {"workflow_run"}:
        errors.append(f"{workflow_name} must trigger only on workflow_run")
    trigger = "\n".join(workflow_trigger_block(workflow_text, "workflow_run"))
    if f'workflows: ["{expected_ci_name}"]' not in trigger and f"workflows: ['{expected_ci_name}']" not in trigger:
        errors.append(f"{workflow_name} workflow_run trigger must watch {expected_ci_name!r}")
    if "types: [completed]" not in trigger:
        errors.append(f"{workflow_name} workflow_run trigger must use completed only")
    permissions = "\n".join(top_level_block(workflow_text, "permissions"))
    for required in (
        "  contents: read",
        "  checks: read",
        "  actions: read",
        "  pull-requests: write",
    ):
        if required not in permissions:
            errors.append(f"{workflow_name} permissions must include {required.strip()}")
    for forbidden in ("  actions: write", "  issues:"):
        if forbidden in permissions:
            errors.append(f"{workflow_name} permissions must not include {forbidden.strip()}")

    jobs = parse_jobs(workflow_text)
    job = jobs.get("mark-stalled")
    if job is None:
        errors.append(f"{workflow_name} must define mark-stalled job")
        return errors
    job_if = job_if_value(job)
    job_text = "\n".join(job)
    event_guard = "github.event.workflow_run.event == 'pull_request'"
    path_guard = f"github.event.workflow_run.path == '{ci_provenance['workflow_path']}'"
    if event_guard not in job_if:
        errors.append(f"{workflow_name} job must filter pull_request runs")
    if "workflow_run.name" in job_if:
        errors.append(f"{workflow_name} job must not filter the configured CI workflow by mutable name")
    if path_guard not in job_if:
        errors.append(f"{workflow_name} job must filter the configured CI workflow by path")
    if re.search(rf"{re.escape(event_guard)}\s*&&\s*{re.escape(path_guard)}", job_if) is None:
        errors.append(f"{workflow_name} job must join pull_request and CI filters with &&")
    if "github.event.workflow_run.head" in job_text or "refs/pull" in job_text:
        errors.append(f"{workflow_name} job must not check out PR head code")
    if "          persist-credentials: false" not in job_text:
        errors.append(f"{workflow_name} checkout must not persist credentials")
    if "python3 scripts/merge_readiness.py finalize-stalled" not in job_text:
        errors.append(f"{workflow_name} job must run scripts/merge_readiness.py finalize-stalled")
    for required in (
        "GITHUB_TOKEN: ${{ github.token }}",
        "GITHUB_EVENT_PATH: ${{ github.event_path }}",
        "GITHUB_REPOSITORY: ${{ github.repository }}",
    ):
        if required not in job_text:
            errors.append(f"{workflow_name} job must pass {required.split(':', 1)[0]}")
    return errors


def verify_coverage_enforcer_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/coverage-enforcer.yml"
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist as its own workflow"]

    errors: list[str] = []
    for other_name, other_text in workflows.items():
        if other_name == workflow_name:
            continue
        if "coverage-enforcer" in parse_jobs(other_text):
            errors.append("coverage-enforcer must not be defined inside another workflow")

    if workflow_trigger_keys(workflow_text) != {"pull_request", "merge_group"}:
        errors.append(f"{workflow_name} must trigger only on pull_request and merge_group")
    errors.extend(workflow_pull_request_type_errors(workflow_text))
    pull_request_trigger = "\n".join(workflow_trigger_block(workflow_text, "pull_request"))
    if "paths:" in pull_request_trigger or "paths-ignore:" in pull_request_trigger:
        errors.append(f"{workflow_name} on.pull_request must not define paths filters")
    merge_group_trigger = "\n".join(workflow_trigger_block(workflow_text, "merge_group"))
    if "types: [checks_requested]" not in merge_group_trigger:
        errors.append(f"{workflow_name} merge_group trigger must use checks_requested")
    errors.extend(f"{workflow_name} {error}" for error in verify_merge_group_concurrency(workflow_text))

    if top_level_mapping_items(workflow_text, "permissions") != EXPECTED_COVERAGE_ENFORCER_PERMISSIONS:
        errors.append(f"{workflow_name} permissions must match the exact read-only map")

    jobs = parse_jobs(workflow_text)
    job = jobs.get("coverage-enforcer")
    if job is None:
        errors.append(f"{workflow_name} must define coverage-enforcer job")
        return errors
    job_text = "\n".join(job)
    job_items = job_top_level_items(job)
    expected_job_keys = {"name", "runs-on", "steps"}
    if job_items is None or set(job_items) != expected_job_keys:
        errors.append(f"{workflow_name} coverage-enforcer job must use only the pinned job-level keys")
    if job_items is not None and "if" in job_items:
        errors.append(
            f"{workflow_name} coverage-enforcer job must not define a job-level "
            "if-condition; required checks must report success or failure, never skipped"
        )
    if job_items is not None and "continue-on-error" in job_items:
        errors.append(f"{workflow_name} coverage-enforcer job must not define job-level continue-on-error")
    if job_items is not None and "permissions" in job_items:
        errors.append(f"{workflow_name} coverage-enforcer job must not define job-level permissions")
    job_if = job_if_value(job)
    if "if" not in (job_items or {}) and _normalize_concurrency_text(job_if) != EXPECTED_COVERAGE_ENFORCER_IF:
        errors.append(
            f"{workflow_name} coverage-enforcer job must not define a job-level "
            "if-condition; required checks must report success or failure, never skipped"
        )
    trusted_base_ref = (
        "          ref: ${{ github.event.pull_request.base.sha || "
        "github.event.merge_group.base_sha }}"
    )
    for required in (
        "uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd",
        trusted_base_ref,
        "          persist-credentials: false",
    ):
        if required not in job_text:
            errors.append(f"{workflow_name} must check out only the trusted base tree")
            break
    steps_text = "\n".join(line for block in step_blocks(job) for line in block)
    for forbidden in (
        "github.event.pull_request.head",
        "github.head_ref",
        "refs/pull",
    ):
        if forbidden in steps_text:
            errors.append(f"{workflow_name} must not check out PR head code")
            break
    if "          persist-credentials: false" not in job_text:
        errors.append(f"{workflow_name} checkout must not persist credentials")
    steps = step_blocks(job)
    enforce_steps = [
        block for block in steps if step_name_matches(block, "Enforce coverage map")
    ]
    run_steps = [block for block in steps if step_declares_run(block)]
    if len(steps) != 3 or not (
        block_has_canonical_step_envelope(
            steps[0],
            frozenset({"uses", "with"}),
            {"uses": "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"},
            {"with": EXPECTED_COVERAGE_ENFORCER_CHECKOUT_WITH},
        )
        and block_has_canonical_step_envelope(
            steps[1],
            frozenset({"uses", "with"}),
            {"uses": "actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405"},
            {"with": EXPECTED_COVERAGE_ENFORCER_SETUP_PYTHON_WITH},
        )
        and step_name_matches(steps[2], "Enforce coverage map")
    ):
        errors.append(f"{workflow_name} coverage-enforcer job steps must match the pinned trusted-base topology")
    elif len(enforce_steps) != 1:
        errors.append(f"{workflow_name} job must run scripts/coverage_enforcer.py")
    elif len(run_steps) != 1 or run_steps[0] != enforce_steps[0]:
        errors.append(
            f"{workflow_name} coverage-enforcer job must run scripts/coverage_enforcer.py "
            "only through the pinned Enforce coverage map step"
        )
    else:
        enforce_step = enforce_steps[0]
        if not block_has_canonical_step_envelope(
            enforce_step,
            frozenset({"name", "env", "run"}),
            {"name": "Enforce coverage map", "run": "|"},
            {"env": EXPECTED_COVERAGE_ENFORCER_ENV},
        ):
            errors.append(f"{workflow_name} coverage-enforcer Enforce coverage map step must be canonical")
        elif not block_has_raw_top_level_scalar(enforce_step, "run", "|"):
            errors.append(f"{workflow_name} coverage-enforcer Enforce coverage map step must be canonical")
        elif tuple(block_run_body_lines(enforce_step)) != EXPECTED_COVERAGE_ENFORCER_RUN_BODY:
            errors.append(f"{workflow_name} job must guard first-run trusted-base bootstrap")
    if "python3 scripts/coverage_enforcer.py" not in job_text:
        errors.append(f"{workflow_name} job must run scripts/coverage_enforcer.py")
    for required in (
        "GITHUB_TOKEN: ${{ github.token }}",
        "GITHUB_EVENT_PATH: ${{ github.event_path }}",
        "GITHUB_REPOSITORY: ${{ github.repository }}",
    ):
        if required not in job_text:
            errors.append(f"{workflow_name} job must pass {required.split(':', 1)[0]}")
    return errors


def workflow_schedule_crons(workflow_text: str) -> tuple[list[str], list[str]]:
    crons: list[str] = []
    extras: list[str] = []
    for line in workflow_trigger_block(workflow_text, "schedule"):
        clean = strip_comment(line).strip()
        if not clean:
            continue
        match = re.fullmatch(r"-\s*cron:\s*(.+)", clean)
        if match is None:
            extras.append(clean)
            continue
        crons.append(yaml_scalar(match.group(1)))
    return crons, extras


INVALID_STORAGE_TRIPWIRE_KEY = "<invalid-storage-tripwire-key>"


class StorageCleanupAlertWorkflowContract(NamedTuple):
    workflow_path: str
    job_id: str
    top_level_keys: tuple[str, ...]
    job_keys: tuple[str, ...]
    runner_var: str
    job_timeout_minutes: int
    schedule_cron: str
    concurrency_group: str
    cancel_in_progress: bool
    run_command: str
    json_artifact_action: str
    json_artifact_step_id: str
    json_artifact_upload_if: str
    json_artifact_name: str
    json_artifact_path: str
    json_artifact_if_no_files_found: str
    json_artifact_retention_days: int
    triggers: tuple[str, ...]
    permissions: Mapping[str, str]
    required_fragments: tuple[str, ...]
    forbidden_fragments: tuple[str, ...]


def load_storage_cleanup_alert_workflow_contract(config_text: str) -> StorageCleanupAlertWorkflowContract:
    document = tomllib.loads(config_text)
    storage_audit = require_config_table(document, "storage_audit", "ci/github-actions-runners.toml")
    alert_table = require_config_table(
        storage_audit,
        "cleanup_feasibility_alert",
        "storage_audit",
    )
    schema_version = require_config_positive_int(
        alert_table,
        "schema_version",
        "storage_audit.cleanup_feasibility_alert",
    )
    if schema_version != 1:
        raise ValueError("storage_audit.cleanup_feasibility_alert.schema_version must be 1")
    workflow = require_config_table(
        alert_table,
        "workflow",
        "storage_audit.cleanup_feasibility_alert",
    )
    prefix = "storage_audit.cleanup_feasibility_alert.workflow"
    return StorageCleanupAlertWorkflowContract(
        workflow_path=require_config_string(workflow, "path", prefix),
        job_id=require_config_string(workflow, "job_id", prefix),
        top_level_keys=tuple(require_config_string_list(workflow, "top_level_keys", prefix)),
        job_keys=tuple(require_config_string_list(workflow, "job_keys", prefix)),
        runner_var=require_config_string(workflow, "runner_var", prefix),
        job_timeout_minutes=require_config_positive_int(workflow, "job_timeout_minutes", prefix),
        schedule_cron=require_config_string(workflow, "schedule_cron", prefix),
        concurrency_group=require_config_string(workflow, "concurrency_group", prefix),
        cancel_in_progress=require_config_bool(workflow, "cancel_in_progress", prefix),
        run_command=require_config_string(workflow, "run_command", prefix),
        json_artifact_action=require_config_string(workflow, "json_artifact_action", prefix),
        json_artifact_step_id=require_config_string(workflow, "json_artifact_step_id", prefix),
        json_artifact_upload_if=require_config_string(workflow, "json_artifact_upload_if", prefix),
        json_artifact_name=require_config_string(workflow, "json_artifact_name", prefix),
        json_artifact_path=require_config_string(workflow, "json_artifact_path", prefix),
        json_artifact_if_no_files_found=require_config_string(workflow, "json_artifact_if_no_files_found", prefix),
        json_artifact_retention_days=require_config_positive_int(workflow, "json_artifact_retention_days", prefix),
        triggers=tuple(require_config_string_list(workflow, "triggers", prefix)),
        permissions=require_config_string_map(workflow, "permissions", prefix),
        required_fragments=tuple(require_config_string_list(workflow, "required_fragments", prefix)),
        forbidden_fragments=tuple(require_config_string_list(workflow, "forbidden_fragments", prefix)),
    )


def verify_storage_cleanup_alert_workflow(workflows: dict[str, str], runners_config_text: str) -> list[str]:
    try:
        workflow_contract = load_storage_cleanup_alert_workflow_contract(runners_config_text)
    except (tomllib.TOMLDecodeError, ValueError) as exc:
        return [f"storage cleanup alert workflow policy invalid: {exc}"]

    workflow_name = workflow_contract.workflow_path
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist"]

    errors: list[str] = []
    workflow_keys = workflow_top_level_keys(workflow_text)
    allowed_workflow_keys = set(workflow_contract.top_level_keys)
    if set(workflow_keys) != allowed_workflow_keys or len(workflow_keys) != len(set(workflow_keys)):
        errors.append(f"{workflow_name} top-level keys must match storage_audit.cleanup_feasibility_alert.workflow")
    if workflow_trigger_keys(workflow_text) != set(workflow_contract.triggers):
        errors.append(f"{workflow_name} triggers must match storage_audit.cleanup_feasibility_alert.workflow.triggers")
    schedule_crons, schedule_extras = workflow_schedule_crons(workflow_text)
    if schedule_crons != [workflow_contract.schedule_cron] or schedule_extras:
        errors.append(f"{workflow_name} schedule cron must match storage_audit.cleanup_feasibility_alert.workflow.schedule_cron")

    permissions_block = top_level_block(workflow_text, "permissions")
    actual_permissions = scalar_mapping(permissions_block)
    if actual_permissions != dict(workflow_contract.permissions):
        errors.append(f"{workflow_name} permissions must match storage_audit.cleanup_feasibility_alert.workflow.permissions")

    expected_concurrency = {
        "group": workflow_contract.concurrency_group,
        "cancel-in-progress": str(workflow_contract.cancel_in_progress).lower(),
    }
    concurrency_block = top_level_block(workflow_text, "concurrency")
    actual_concurrency = scalar_mapping(concurrency_block)
    if actual_concurrency != expected_concurrency:
        errors.append(f"{workflow_name} concurrency must match storage_audit.cleanup_feasibility_alert.workflow")

    for forbidden in workflow_contract.forbidden_fragments:
        if forbidden in workflow_text:
            errors.append(
                f"{workflow_name} must not contain forbidden workflow fragment from storage_audit.cleanup_feasibility_alert.workflow.forbidden_fragments"
            )

    jobs = parse_jobs(workflow_text)
    if set(jobs) != {workflow_contract.job_id}:
        errors.append(f"{workflow_name} must define only the configured storage cleanup alert job")
    job = jobs.get(workflow_contract.job_id)
    if job is None:
        errors.append(f"{workflow_name} must define configured storage cleanup alert job")
        return errors
    job_text = "\n".join(job)
    job_keys = storage_tripwire_job_top_level_keys(job)
    allowed_job_keys = set(workflow_contract.job_keys)
    if set(job_keys) != allowed_job_keys or len(job_keys) != len(set(job_keys)):
        errors.append(f"{workflow_name} storage cleanup alert job keys must match the workflow contract")
    actual_var = extract_job_runs_on_var(job)
    if actual_var != workflow_contract.runner_var:
        errors.append(f"{workflow_name} storage cleanup alert runs-on must match storage_audit.cleanup_feasibility_alert.workflow.runner_var")
    actual_timeout = storage_tripwire_job_scalar_value(job, "timeout-minutes")
    if actual_timeout != str(workflow_contract.job_timeout_minutes):
        errors.append(
            f"{workflow_name} storage cleanup alert timeout-minutes must match storage_audit.cleanup_feasibility_alert.workflow.job_timeout_minutes"
        )

    if any(storage_tripwire_key_at_indent(line, 4) == "permissions" for line in job):
        errors.append(f"{workflow_name} storage cleanup alert job must not define job-level permissions")
    if any(storage_tripwire_key_at_any_indent(line) == "continue-on-error" for line in job):
        errors.append(f"{workflow_name} storage cleanup alert job must not use continue-on-error")

    steps = step_blocks(job)
    if len(steps) != 3:
        errors.append(f"{workflow_name} storage cleanup alert job must contain exactly checkout, run, and upload steps")
    else:
        checkout_action = storage_tripwire_expected_checkout_action(workflow_contract.required_fragments)
        persist_credentials = storage_tripwire_expected_persist_credentials(workflow_contract.required_fragments)
        expected_env = storage_tripwire_expected_env(workflow_contract.required_fragments)
        checkout_items = block_top_level_items(steps[0])
        if (
            checkout_action is None
            or persist_credentials is None
            or checkout_items is None
            or set(checkout_items) != {"uses", "with"}
            or checkout_items.get("uses") != checkout_action
            or block_nested_mapping_items(steps[0], "with") != {"persist-credentials": persist_credentials}
        ):
            errors.append(f"{workflow_name} checkout step must match storage_audit.cleanup_feasibility_alert.workflow.required_fragments")
        run_items = block_top_level_items(steps[1])
        if (
            not expected_env
            or run_items is None
            or set(run_items) != {"name", "env", "run"}
            or not run_items.get("name")
            or block_nested_mapping_items(steps[1], "env") != expected_env
            or step_run_command(steps[1]) != workflow_contract.run_command
        ):
            errors.append(f"{workflow_name} run step must match storage_audit.cleanup_feasibility_alert.workflow contract")
        upload_items = block_top_level_items(steps[2])
        expected_upload_with = {
            "name": workflow_contract.json_artifact_name,
            "path": workflow_contract.json_artifact_path,
            "if-no-files-found": workflow_contract.json_artifact_if_no_files_found,
            "retention-days": str(workflow_contract.json_artifact_retention_days),
        }
        if (
            upload_items is None
            or set(upload_items) != {"name", "id", "if", "uses", "with"}
            or upload_items.get("name") != "Upload cleanup feasibility JSON"
            or upload_items.get("id") != workflow_contract.json_artifact_step_id
            or upload_items.get("if") != workflow_contract.json_artifact_upload_if
            or upload_items.get("uses") != workflow_contract.json_artifact_action
            or block_nested_mapping_items(steps[2], "with") != expected_upload_with
        ):
            errors.append(f"{workflow_name} upload step must match storage_audit.cleanup_feasibility_alert.workflow contract")

    for required in workflow_contract.required_fragments:
        if required not in job_text:
            errors.append(f"{workflow_name} job must contain storage_audit.cleanup_feasibility_alert.workflow.required_fragments")
    return errors


def storage_tripwire_key_at_indent(line: str, indent: int) -> str | None:
    clean = strip_comment(line).rstrip()
    if not clean:
        return None
    actual_indent = len(clean) - len(clean.lstrip(" "))
    if actual_indent != indent:
        return None
    match = re.fullmatch(rf"\s{{{indent}}}({YAML_KEY_PATTERN})\s*:\s*.*", clean)
    if match is None:
        return INVALID_STORAGE_TRIPWIRE_KEY
    return unquote_yaml_scalar(match.group(1))


def storage_tripwire_key_at_any_indent(line: str) -> str | None:
    clean = strip_comment(line).rstrip()
    if not clean:
        return None
    match = re.fullmatch(rf"\s*({YAML_KEY_PATTERN})\s*:\s*.*", clean)
    if match is None:
        return None
    return unquote_yaml_scalar(match.group(1))


def workflow_top_level_keys(workflow_text: str) -> list[str]:
    keys: list[str] = []
    for line in workflow_text.splitlines():
        key = storage_tripwire_key_at_indent(line, 0)
        if key is not None:
            keys.append(key)
    return keys


def storage_tripwire_job_top_level_keys(job_lines: list[str]) -> list[str]:
    keys: list[str] = []
    for line in job_lines:
        key = storage_tripwire_key_at_indent(line, 4)
        if key is not None:
            keys.append(key)
    return keys


def storage_tripwire_job_scalar_value(job_lines: list[str], key: str) -> str | None:
    values: list[str] = []
    for line in job_lines:
        clean = strip_comment(line).rstrip()
        match = re.fullmatch(rf"\s{{4}}{re.escape(key)}\s*:\s*(.+)", clean)
        if match is not None:
            values.append(yaml_scalar(match.group(1)))
    return values[0] if len(values) == 1 else None


def storage_tripwire_expected_checkout_action(required_fragments: tuple[str, ...]) -> str | None:
    actions = [
        fragment.removeprefix("uses: ").strip()
        for fragment in required_fragments
        if fragment.startswith("uses: ")
    ]
    return actions[0] if len(actions) == 1 else None


def storage_tripwire_expected_persist_credentials(required_fragments: tuple[str, ...]) -> str | None:
    values = [
        fragment.split(":", 1)[1].strip()
        for fragment in required_fragments
        if fragment.startswith("persist-credentials:")
    ]
    return values[0] if len(values) == 1 else None


def storage_tripwire_expected_env(required_fragments: tuple[str, ...]) -> dict[str, str]:
    env: dict[str, str] = {}
    for fragment in required_fragments:
        match = re.fullmatch(r"([A-Z][A-Z0-9_]*):\s*(.+)", fragment)
        if match is not None:
            env[match.group(1)] = match.group(2)
    return env


def verify_storage_tripwire_workflow(workflows: dict[str, str], policy_text: str) -> list[str]:
    try:
        policy = ci_storage_tripwire.load_policy_text(
            policy_text,
            source="storage tripwire policy",
        )
    except ci_storage_tripwire.TripwireError as exc:
        return [f"storage tripwire policy invalid: {exc}"]

    workflow_contract = policy.workflow
    workflow_name = workflow_contract.workflow_path
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist"]

    errors: list[str] = []
    workflow_keys = workflow_top_level_keys(workflow_text)
    allowed_workflow_keys = set(workflow_contract.top_level_keys)
    if set(workflow_keys) != allowed_workflow_keys or len(workflow_keys) != len(set(workflow_keys)):
        errors.append(f"{workflow_name} top-level keys must match the storage tripwire workflow contract")
    if workflow_trigger_keys(workflow_text) != set(workflow_contract.triggers):
        errors.append(f"{workflow_name} triggers must match storage_tripwire.workflow.triggers")
    schedule_crons, schedule_extras = workflow_schedule_crons(workflow_text)
    if schedule_crons != [workflow_contract.schedule_cron] or schedule_extras:
        errors.append(f"{workflow_name} schedule cron must match storage_tripwire.workflow.schedule_cron")

    actual_permissions = scalar_mapping(top_level_block(workflow_text, "permissions"))
    if actual_permissions != dict(workflow_contract.permissions):
        errors.append(f"{workflow_name} permissions must match storage_tripwire.workflow.permissions")

    expected_concurrency = {
        "group": workflow_contract.concurrency_group,
        "cancel-in-progress": str(workflow_contract.cancel_in_progress).lower(),
    }
    actual_concurrency = scalar_mapping(top_level_block(workflow_text, "concurrency"))
    if actual_concurrency != expected_concurrency:
        errors.append(f"{workflow_name} concurrency must match storage_tripwire.workflow concurrency settings")

    for forbidden in workflow_contract.forbidden_fragments:
        if forbidden in workflow_text:
            errors.append(
                f"{workflow_name} must not contain forbidden workflow fragment from storage_tripwire.workflow.forbidden_fragments"
            )

    jobs = parse_jobs(workflow_text)
    if set(jobs) != {workflow_contract.job_id}:
        errors.append(f"{workflow_name} must define only the configured storage tripwire job")
    job = jobs.get(workflow_contract.job_id)
    if job is None:
        errors.append(f"{workflow_name} must define configured storage tripwire job")
        return errors
    job_text = "\n".join(job)
    job_keys = storage_tripwire_job_top_level_keys(job)
    allowed_job_keys = set(workflow_contract.job_keys)
    if set(job_keys) != allowed_job_keys or len(job_keys) != len(set(job_keys)):
        errors.append(f"{workflow_name} storage tripwire job keys must match the workflow contract")
    if job_if_value(job) != workflow_contract.job_if:
        errors.append(f"{workflow_name} storage tripwire job if must match storage_tripwire.workflow.job_if")
    actual_var = extract_job_runs_on_var(job)
    if actual_var != workflow_contract.runner_var:
        errors.append(f"{workflow_name} storage tripwire runs-on must match storage_tripwire.workflow.runner_var")

    if any(storage_tripwire_key_at_indent(line, 4) == "permissions" for line in job):
        errors.append(f"{workflow_name} storage tripwire job must not define job-level permissions")
    if any(storage_tripwire_key_at_any_indent(line) == "continue-on-error" for line in job):
        errors.append(f"{workflow_name} storage tripwire job must not use continue-on-error")

    steps = step_blocks(job)
    if len(steps) != 2:
        errors.append(f"{workflow_name} storage tripwire job must contain exactly checkout and run steps")
    else:
        checkout_action = storage_tripwire_expected_checkout_action(workflow_contract.required_fragments)
        persist_credentials = storage_tripwire_expected_persist_credentials(workflow_contract.required_fragments)
        expected_env = storage_tripwire_expected_env(workflow_contract.required_fragments)
        checkout_items = block_top_level_items(steps[0])
        if (
            checkout_action is None
            or persist_credentials is None
            or checkout_items is None
            or set(checkout_items) != {"uses", "with"}
            or checkout_items.get("uses") != checkout_action
            or block_nested_mapping_items(steps[0], "with") != {"persist-credentials": persist_credentials}
        ):
            errors.append(f"{workflow_name} checkout step must match storage_tripwire.workflow.required_fragments")
        run_items = block_top_level_items(steps[1])
        if (
            not expected_env
            or run_items is None
            or set(run_items) != {"name", "env", "run"}
            or not run_items.get("name")
            or block_nested_mapping_items(steps[1], "env") != expected_env
            or step_run_command(steps[1]) != workflow_contract.run_command
        ):
            errors.append(f"{workflow_name} run step must match storage_tripwire.workflow contract")

    for required in workflow_contract.required_fragments:
        if required not in job_text:
            errors.append(f"{workflow_name} job must contain storage_tripwire.workflow.required_fragments")
    return errors


def has_storage_tripwire_workflow(workflows: Mapping[str, str]) -> bool:
    return any(
        workflow_path in workflows
        for workflow_path, config_key in WORKFLOW_RUNNER_CONFIG_KEYS.items()
        if config_key == STORAGE_TRIPWIRE_RUNNER_CONFIG_KEY
    )


def verify_github_actions_runner_contract(workflows: dict[str, str]) -> list[str]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None

    tier_to_var = config["tier_to_var"]
    meter_included_workflows = set(config["meter_included_workflows"])
    workflow_tables = config["workflows"]
    cargo_build_jobs = config["cargo_build_jobs"]
    errors: list[str] = []
    known_workflow_keys = set(WORKFLOW_RUNNER_CONFIG_KEYS.values())
    for workflow_key in sorted(workflow_tables):
        if workflow_key not in known_workflow_keys:
            errors.append(
                f"workflows.{workflow_key} in ci/github-actions-runners.toml has no workflow contract"
            )
    managed_workflows = {
        workflow_key
        for workflow_key, job_table in workflow_tables.items()
        if isinstance(job_table, dict)
        and any(isinstance(tier, str) and tier != "github_hosted" for tier in job_table.values())
    }
    if meter_included_workflows != managed_workflows:
        errors.append(
            "meter.included_workflows must match workflows with managed runner tiers: "
            f"expected {sorted(managed_workflows)!r}, got {sorted(meter_included_workflows)!r}"
        )
    for workflow_key, job_table in sorted(workflow_tables.items()):
        if not isinstance(job_table, dict):
            continue
        fingerprint_tier = job_table.get("nextest-fingerprint")
        archive_tier = job_table.get("test-archive")
        if (
            isinstance(fingerprint_tier, str)
            and isinstance(archive_tier, str)
            and fingerprint_tier != archive_tier
        ):
            errors.append(
                f"workflows.{workflow_key} nextest-fingerprint and test-archive must use the same runner tier"
            )
    if isinstance(cargo_build_jobs, dict):
        for workflow_key, job_table in sorted(cargo_build_jobs.items()):
            configured_workflow = workflow_tables.get(workflow_key)
            if not isinstance(configured_workflow, dict):
                errors.append(
                    f"cargo_build_jobs.{workflow_key} in ci/github-actions-runners.toml has no workflow contract"
                )
                continue
            if not isinstance(job_table, dict):
                continue
            for job in sorted(job_table):
                if job not in configured_workflow:
                    errors.append(
                        f"cargo_build_jobs.{workflow_key}.{job} must reference a configured workflow job"
                    )

    for workflow_name, workflow_text in sorted(workflows.items()):
        jobs = parse_jobs(workflow_text)
        if not jobs:
            continue
        workflow_key = WORKFLOW_RUNNER_CONFIG_KEYS.get(workflow_name)
        if workflow_key is None:
            errors.append(
                f"{workflow_name} must be mapped in ci/github-actions-runners.toml"
            )
            continue
        job_table = workflow_tables.get(workflow_key)
        if not isinstance(job_table, dict):
            errors.append(f"workflows.{workflow_key} missing in ci/github-actions-runners.toml")
            continue
        cargo_job_table = cargo_build_jobs.get(workflow_key) if isinstance(cargo_build_jobs, dict) else None
        if not isinstance(cargo_job_table, dict):
            cargo_job_table = {}
        workflow_env_text = uncommented_text(top_level_block(workflow_text, "env"))
        if INLINE_CARGO_BUILD_JOBS_RE.search(workflow_env_text):
            errors.append(
                f"{workflow_name} workflow-level CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml via setup-environment"
            )
        configured_jobs = set(job_table)
        actual_jobs = set(jobs)
        for job in sorted(configured_jobs - actual_jobs):
            errors.append(
                f"{workflow_name} configured runner job {job} missing from workflow"
            )
        for job in sorted(actual_jobs - configured_jobs):
            errors.append(
                f"{workflow_name} job {job} missing from ci/github-actions-runners.toml"
            )
        for job in sorted(configured_jobs & actual_jobs):
            tier = job_table[job]
            expected_var = tier_to_var.get(tier)
            if expected_var is None:
                errors.append(f"unknown runner tier {tier!r} for {workflow_name} {job}")
                continue
            actual_var = extract_job_runs_on_var(jobs[job])
            if actual_var is None:
                errors.append(
                    f"{workflow_name} {job} runs-on must reference vars.{expected_var} "
                    "(no hardcoded runner labels)"
                )
                continue
            if actual_var != expected_var:
                errors.append(
                    f"{workflow_name} {job} runs-on must use vars.{expected_var}, got vars.{actual_var}"
                )
            job_text = uncommented_text(jobs[job])
            if INLINE_CARGO_BUILD_JOBS_RE.search(job_text):
                errors.append(
                    f"{workflow_name} {job} CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml via setup-environment"
                )
            if job in cargo_job_table:
                expected_key = f"{workflow_key}.{job}"
                if not job_has_setup_input(jobs[job], "build-jobs-key", expected_key):
                    errors.append(
                        f"{workflow_name} {job} must resolve CARGO_BUILD_JOBS from cargo_build_jobs.{expected_key}"
                    )
                else:
                    for setup_error in cargo_build_jobs_setup_order_errors(jobs[job], expected_key):
                        errors.append(f"{workflow_name} {job} {setup_error}")
            elif "build-jobs-key:" in job_text:
                errors.append(
                    f"{workflow_name} {job} has build-jobs-key but is missing from cargo_build_jobs.{workflow_key} in ci/github-actions-runners.toml"
                )
            if workflow_name == "ci.yml" and job in CI_RUST_FAST_LINKER_JOBS:
                if not job_has_setup_input(jobs[job], "install-rust-linker", "true"):
                    errors.append(f"{workflow_name} {job} must install configured Rust linker")
    return errors


def actionlint_config_variables(actionlint_text: str) -> set[str]:
    variables: set[str] = set()
    in_section = False
    for line in actionlint_text.splitlines():
        clean = strip_comment(line).strip()
        if clean == "config-variables:":
            in_section = True
            continue
        if in_section:
            if clean and not clean.startswith("- "):
                break
            if clean.startswith("- "):
                variables.add(clean[2:].strip())
    return variables


def workflow_repository_variables(workflows: dict[str, str]) -> set[str]:
    variables: set[str] = set()
    for workflow_text in workflows.values():
        for match in re.finditer(r"vars\.([A-Z0-9_]+)", workflow_text):
            variables.add(match.group(1))
    return variables


def verify_actionlint_runner_contract(
    workflows: dict[str, str],
    actionlint_path: pathlib.Path = DEFAULT_ACTIONLINT_CONFIG,
) -> list[str]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None
    if not actionlint_path.exists():
        return [f"actionlint config missing: {actionlint_path}"]

    text = actionlint_path.read_text(encoding="utf-8")
    allowed_variables = actionlint_config_variables(text)
    errors: list[str] = []
    for label in config["managed_labels"]:
        if f"- {label}" not in text:
            errors.append(f".github/actionlint.yaml must list managed runner label {label!r}")
    for variable in config["variables"]:
        if variable not in allowed_variables:
            errors.append(f".github/actionlint.yaml must allow config variable {variable!r}")
    for variable in sorted(workflow_repository_variables(workflows)):
        if variable not in allowed_variables:
            errors.append(
                f".github/actionlint.yaml must allow repository variable {variable!r} "
                "referenced by workflow vars.* expressions"
            )
    expected_variables = set(config["variables"]) | workflow_repository_variables(workflows)
    for variable in sorted(allowed_variables - expected_variables):
        errors.append(
            f".github/actionlint.yaml allows stale config variable {variable!r} "
            "not referenced by workflows or ci/github-actions-runners.toml"
        )
    return errors


def repo_workflow_texts() -> dict[str, str]:
    if not DEFAULT_WORKFLOW_DIR.exists():
        return {}
    paths: set[pathlib.Path] = set()
    for pattern in DEFAULT_WORKFLOW_GLOBS:
        paths.update(DEFAULT_WORKFLOW_DIR.glob(pattern))
    return {path.relative_to(REPO_ROOT).as_posix(): path.read_text() for path in sorted(paths)}


def self_authorizing_governance_cli(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Block PRs that edit governance and introduce newly permitted capability in one diff."
    )
    parser.add_argument("--repo", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    args = parser.parse_args(argv)
    errors = self_authorizing_governance_diff_errors(
        args.repo.resolve(),
        args.base,
        args.head,
    )
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: no self-authorizing governance edit coupling detected.")
    return 0


def main() -> int:
    workflow_texts = repo_workflow_texts()
    action_text = DEFAULT_SETUP_ACTION.read_text()
    nextest_config_text = DEFAULT_NEXTEST_CONFIG.read_text()
    bvs_policy_text = (
        DEFAULT_BVS_RUST_VERIFICATION_POLICY.read_text()
        if DEFAULT_BVS_RUST_VERIFICATION_POLICY.exists()
        else ""
    )
    repo_automation_texts = {
        path.relative_to(REPO_ROOT).as_posix(): path.read_text()
        for path in DEFAULT_REPO_AUTOMATION_FILES
        if path.exists()
    }
    for directory, pattern in DEFAULT_REPO_AUTOMATION_GLOBS:
        if not directory.exists():
            continue
        for path in sorted(directory.glob(pattern)):
            repo_automation_texts[path.relative_to(REPO_ROOT).as_posix()] = path.read_text()
    composite_action_texts = {
        path: text
        for path, text in repo_automation_texts.items()
        if path.startswith(".github/actions/") and path.endswith(("/action.yml", "/action.yaml"))
    }
    errors = verify_workflows(workflow_texts, action_text, nextest_config_text)
    errors.extend(verify_artifact_retention_policy(workflow_texts, composite_action_texts))
    errors.extend(verify_github_actions_runner_contract(workflow_texts))
    errors.extend(verify_ci_runner_debug_workflow(workflow_texts))
    errors.extend(
        verify_debug_test_workflow(
            workflow_texts,
            repo_automation_texts.get("justfile", ""),
            DEFAULT_MERGIFY_CONFIG.read_text() if DEFAULT_MERGIFY_CONFIG.exists() else "",
        )
    )
    errors.extend(verify_debug_lane_compile_cache_parity(workflow_texts, bvs_policy_text))
    errors.extend(verify_dispatch_ci_cancel_workflow(workflow_texts))
    ci_workflow = workflow_texts.get(".github/workflows/ci.yml")
    if ci_workflow is not None:
        errors.extend(verify_merge_readiness_ci_job(ci_workflow))
    errors.extend(verify_merge_readiness_finalizer_workflow(workflow_texts))
    errors.extend(verify_coverage_enforcer_workflow(workflow_texts))
    try:
        storage_tripwire_policy = ci_storage_tripwire.discover_policy_path(REPO_ROOT)
        errors.extend(
            verify_storage_tripwire_workflow(
                workflow_texts,
                storage_tripwire_policy.read_text(encoding="utf-8"),
            )
        )
    except ci_storage_tripwire.NoTripwirePolicyError as exc:
        if has_storage_tripwire_workflow(workflow_texts):
            errors.append(f"storage tripwire policy discovery failed: {exc}")
    except ci_storage_tripwire.TripwirePolicyInventoryError as exc:
        if has_storage_tripwire_workflow(workflow_texts):
            errors.append(f"storage tripwire policy discovery failed: {exc}")
    except ci_storage_tripwire.TripwireError as exc:
        errors.append(f"storage tripwire policy discovery failed: {exc}")
    errors.extend(verify_actionlint_runner_contract(workflow_texts))
    errors.extend(verify_repo_automation_texts(repo_automation_texts))
    errors.extend(verify_flaky_test_detection_workflows(workflow_texts))
    errors.extend(verify_rust_verification_policies())
    errors.extend(verify_test_harness_manifest())
    if "justfile" in repo_automation_texts:
        errors.extend(verify_local_verification_gate_recipes(repo_automation_texts["justfile"]))
        errors.extend(verify_source_fence_static_recipe(repo_automation_texts["justfile"]))
    if DEFAULT_NO_MISTAKES_CONFIG.exists():
        errors.extend(verify_no_mistakes_config(DEFAULT_NO_MISTAKES_CONFIG.read_text()))
    if DEFAULT_MERGIFY_CONFIG.exists():
        errors.extend(verify_mergify_config(DEFAULT_MERGIFY_CONFIG.read_text()))
    else:
        errors.append(".mergify.yml is required for Mergify queue governance")
    runners_config_floor_errors = github_actions_runners_config_floor_errors()
    if runners_config_floor_errors:
        errors.extend(
            error for error in runners_config_floor_errors if error not in errors
        )
    else:
        runners_config_text = DEFAULT_RUNNERS_CONFIG.read_text(encoding="utf-8")
        errors.extend(verify_storage_cleanup_alert_workflow(workflow_texts, runners_config_text))
        errors.extend(mergify_proof_prefix_alignment_errors(load_config(DEFAULT_RUNNERS_CONFIG)))
    errors = list(dict.fromkeys(errors))
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: CI workflow hygiene verifier passed.")
    return 0


def cli(argv: list[str]) -> int:
    if argv and argv[0] == "self-authorizing-governance":
        return self_authorizing_governance_cli(argv[1:])
    if argv:
        print(f"ERROR: unknown verify_ci_workflow_hygiene mode: {argv[0]}", file=sys.stderr)
        return 2
    return main()


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(cli(sys.argv[1:]))
