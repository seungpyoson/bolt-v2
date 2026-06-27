#!/usr/bin/env python3
"""Verify CI workflow hygiene invariants for the current workflow topology."""

from __future__ import annotations

from collections.abc import Iterable
import functools
import json
import pathlib
import re
import shlex
import sys
import tomllib
from typing import NamedTuple

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from ci_provenance import (
    GATE_NAME_KEYS,
    MERGIFY_TEMP_PR_TRANSIENT_PREFIX,
    POLICY_ROWS,
    POLICY_VALUES,
    ProvenanceError,
    gate_name_collision_errors,
    github_actions_output_safe_check_name,
    policy_contract_errors,
    evaluate_ci_policy as provenance_evaluate_ci_policy,
    docs_safe_path_contract_errors,
    ProvenanceConfig,
    load_config,
    mergify_temp_pr_matches,
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
JOB_RUNS_ON_VAR_RE = re.compile(r"^    runs-on:\s*\$\{\{\s*vars\.([A-Z0-9_]+)\s*\}\}\s*$")
WORKFLOW_RUNNER_CONFIG_KEYS = {
    "ci.yml": "ci",
    ".github/workflows/ci.yml": "ci",
    "backtester-ci.yml": "backtester_ci",
    ".github/workflows/backtester-ci.yml": "backtester_ci",
    "dispatch-ci-cancel.yml": "dispatch_ci_cancel",
    ".github/workflows/dispatch-ci-cancel.yml": "dispatch_ci_cancel",
    "merge-readiness-finalizer.yml": "merge_readiness_finalizer",
    ".github/workflows/merge-readiness-finalizer.yml": "merge_readiness_finalizer",
    "coverage-enforcer.yml": "coverage_enforcer",
    ".github/workflows/coverage-enforcer.yml": "coverage_enforcer",
    "ci-runner-debug.yml": "ci_runner_debug",
    ".github/workflows/ci-runner-debug.yml": "ci_runner_debug",
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
    "advisory.yml": "advisory",
    ".github/workflows/advisory.yml": "advisory",
    "summary.yml": "summary",
    ".github/workflows/summary.yml": "summary",
    "stale.yml": "stale",
    ".github/workflows/stale.yml": "stale",
}
SSH_RUNNER_ACTION_RE = re.compile(r"^ubicloud/ssh-runner@[0-9a-f]{40}$")
DEFAULT_REPO_AUTOMATION_FILES = (REPO_ROOT / "justfile",)
DEFAULT_REPO_AUTOMATION_GLOBS = (
    (REPO_ROOT / "scripts", "*.sh"),
    (REPO_ROOT / "tests", "*.sh"),
    (REPO_ROOT / ".github" / "scripts", "*.sh"),
    (REPO_ROOT / ".github" / "actions", "*/action.yml"),
    (REPO_ROOT / ".github" / "actions", "*/action.yaml"),
)
S3_ACTIVE_TARGET_CACHE_MESSAGE = "S3 active mutable target cache must be rejected"
LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS = {"build", "clippy", "test"}
LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS = set(CARGO_DISK_PREFLIGHT_SUBCOMMANDS) | set(CARGO_ALIAS_SUBCOMMANDS)
YAML_ANCHOR_PATTERN = r"&[A-Za-z0-9_.-]+"
YAML_KEY_PATTERN = r"""(?:[A-Za-z0-9_.-]+|'[^']*(?:''[^']*)*'|"(?:[^"\\]|\\.)*")"""
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
    "ready_pr": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_target=True, queue_covered=True),
    "ready_pr_edited_no_base": PolicyRowSemantics(),
    "ready_pr_reopened": PolicyRowSemantics(),
    "ready_for_review": PolicyRowSemantics(changes_required_context=True, queue_covered=True),
    "docs": PolicyRowSemantics(),
    "workflow_dispatch": PolicyRowSemantics(changes_required_context=True, mergeable_without_queue=False),
    "workflow_dispatch_full_ci": PolicyRowSemantics(changes_required_context=True, mergeable_without_queue=False),
    "main_push": PolicyRowSemantics(changes_head_sha=True, changes_target=True),
    "merge_group": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_queue_origin=True),
    "mergify_temp_pr": PolicyRowSemantics(changes_head_sha=True, changes_queue_origin=True),
    "tag": PolicyRowSemantics(changes_target=True),
    "unknown_event": PolicyRowSemantics(changes_head_sha=True, changes_base=True, changes_target=True),
}
TAG_SKIPPED_JOBS = (
    "deny",
    "clippy",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "nextest-fingerprint-reuse",
    "test",
    "build",
    "ci-provenance-emit",
)
PR_BASE_CHANGED_EXPR = "github.event.changes.base.ref.from != ''"
READY_PR_NOOP_EXPR = (
    "github.event.pull_request.draft == false"
    " && (github.event.action == 'reopened'"
    " || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != '')))"
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
    "config_parsing",
    "lake_batch",
    "nt_runtime_capture",
    "venue_contract",
)
EXPECTED_HARNESS_COUNT = 9
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
TAG_REUSE_POLICY_EXPR = "needs.ci-policy.outputs.ci_policy_path == 'tag_reuse'"
NEXTEST_REUSE_MISS_EXPR = "needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true'"
MAIN_BRANCH_SKIP_EXPR = "github.ref != 'refs/heads/main'"
BUILD_REQUIRED_EXPR = "needs.detector.outputs.build_required == 'true'"
FINGERPRINT_REUSE_ALLOWED_EXPR = "needs.detector.outputs.fingerprint_reuse_allowed == 'true'"
FINGERPRINT_REUSE_PR_EVENT_EXPR = "github.event_name == 'pull_request'"
FINGERPRINT_REUSE_JOB_IF_VALUE = (
    "${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' "
    "&& github.event_name == 'pull_request' "
    "&& needs.detector.outputs.fingerprint_reuse_allowed == 'true' "
    "&& github.ref != 'refs/heads/main' }}"
)
FINGERPRINT_REUSE_ALLOWED_OUTPUT = (
    "fingerprint_reuse_allowed: ${{ steps.fingerprint_reuse_allowed.outputs.value }}"
)
FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_ALLOWED_KEYS = frozenset(
    ("name", "id", "if", "shell", "run")
)
FINGERPRINT_REUSE_INPUTS_CHANGED_STEP_SCALARS = {
    "id": "fingerprint_reuse_inputs_changed",
    "if": "github.event_name == 'pull_request'",
    "shell": "bash",
    "run": "|",
}
FINGERPRINT_REUSE_ALLOWED_STEP_ALLOWED_KEYS = frozenset(("name", "id", "shell", "run"))
FINGERPRINT_REUSE_ALLOWED_STEP_SCALARS = {
    "id": "fingerprint_reuse_allowed",
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
    "run": ">",
}
NEXTEST_FINGERPRINT_REUSE_RESOLVER_ENV = {"GITHUB_TOKEN": "${{ github.token }}"}
FINGERPRINT_REUSE_INPUTS_CHANGED_RUN = """base_ref="${{ steps.pr_refs.outputs.base_ref }}"
head_ref="${{ steps.pr_refs.outputs.head_ref }}"
changed="$(git diff --name-only "${base_ref}...${head_ref}" -- \\
  .github/workflows/ci.yml \\
  .github/actions/setup-environment/action.yml \\
  ci/nextest-fingerprint.toml \\
  ci/github-actions-runners.toml \\
  scripts/nextest_fingerprint.py \\
  scripts/test_nextest_fingerprint.py \\
  scripts/root_bin_sidecars.py \\
  scripts/test_root_bin_sidecars.py \\
  scripts/ci_provenance.py \\
  scripts/test_ci_provenance.py \\
  scripts/verify_ci_workflow_hygiene.py \\
  scripts/test_verify_ci_workflow_hygiene.py)"
if [[ -n "$changed" ]]; then
  echo "any_changed=true" >> "$GITHUB_OUTPUT"
else
  echo "any_changed=false" >> "$GITHUB_OUTPUT"
fi"""
FINGERPRINT_REUSE_ALLOWED_RUN = """if [[ "${{ github.event_name }}" != "pull_request" ]]; then
  echo "value=false" >> "$GITHUB_OUTPUT"
elif [[ "${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true" ]]; then
  echo "value=false" >> "$GITHUB_OUTPUT"
else
  echo "value=true" >> "$GITHUB_OUTPUT"
fi"""
NEXTEST_FINGERPRINT_REUSE_RESOLVER_RUN = """python3 scripts/ci_provenance.py resolve-fingerprint
--current-run-id "${{ github.run_id }}"
--current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"
| tee -a "$GITHUB_OUTPUT\""""
GATE_NEXTEST_FINGERPRINT_REUSE_BRANCH = """if [[ "${{ needs.nextest-fingerprint-reuse.result }}" != "success" ]]; then
  echo "nextest fingerprint reuse resolver did not succeed"
  exit 1
fi
if [[ "${{ needs.ci-provenance-emit.result }}" != "skipped" ]]; then
  echo "ci-provenance-emit unexpectedly ran during nextest fingerprint reuse"
  exit 1
fi
echo "nextest archive reused from run ${{ needs.nextest-fingerprint-reuse.outputs.source_run_id }} at ${{ needs.nextest-fingerprint-reuse.outputs.source_sha }}\""""
FINGERPRINT_REUSE_GOVERNANCE_PATHS = (
    ".github/workflows/ci.yml",
    ".github/actions/setup-environment/action.yml",
    "ci/nextest-fingerprint.toml",
    "ci/github-actions-runners.toml",
    "scripts/nextest_fingerprint.py",
    "scripts/test_nextest_fingerprint.py",
    "scripts/root_bin_sidecars.py",
    "scripts/test_root_bin_sidecars.py",
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
    "cancel-in-progress must apply to all pull_request and workflow_dispatch full CI runs only"
)
GATE_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?always\(\)\s*(?:\}\})?\s*$")
DEPLOY_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*always\(\)\s*&&\s*startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*"
    r"needs\.gate\.result\s*==\s*['\"]success['\"]\s*&&\s*"
    r"needs\.same-sha-main-evidence\.result\s*==\s*['\"]success['\"]\s*\}\}\s*$"
)
EXIT_RE = re.compile(r"^\s*exit(?:\s+([0-9]+))?\s*$", re.MULTILINE)
IF_OR_ELIF_RE = re.compile(r"^\s*(if|elif)\s+\[\[\s*(?P<condition>.*?)\s*\]\];\s*then\s*$")
ELSE_RE = re.compile(r"^\s*else\s*$")
FI_RE = re.compile(r"^\s*fi\s*$")
TARGET_DIR_OPT_IN_RE = re.compile(r"^\s+include-managed-target-dir:\s*(['\"])true\1\s*$")
SETUP_TARGET_DIR_EXPORT_RE = re.compile(r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir\s*\}\}\s*$")
SETUP_TARGET_DIR_RELATIVE_EXPORT_RE = re.compile(
    r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir_relative\s*\}\}\s*$"
)
SETUP_TARGET_DIR_RELATIVE_OUTPUT_RE = re.compile(
    r'^\s*echo\s+"managed_target_dir_relative=\$managed_target_dir_relative"\s*>>\s*"\$GITHUB_OUTPUT"\s*$'
)
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
    "just ci-lint-workflow",
    "awk -F'\\\"' '/^channel = / {print $2}' rust-toolchain.toml",
    "just --evaluate deny_version",
    "just --evaluate nextest_version",
    "just --evaluate target",
    "just --evaluate zig_version",
    "just --evaluate zigbuild_version",
    "just --evaluate rust_verification_owner",
    'target-dir --repo "$GITHUB_WORKSPACE"',
    "os.path.relpath",
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
}
SETUP_ACTION_ORDERED_STEPS = (
    "Lint workflow contract",
    "Read shared values",
    "Resolve managed target dir",
    "Setup Rust toolchain",
)
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
TEST_ARCHIVE_PARTITION_FAILURE_WRAPPER = (
    f"if ! {TEST_PARTITION_COMMAND}; then\n"
    "              status=1\n"
    "            fi"
)
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
EXACT_HEAD_GOVERNANCE_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
    "'scripts/command_understanding.py'",
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
# Value, not mere presence: the fail-open flag must be literally "1", and the build
# must retry without sccache on failure, so a future edit cannot silently disable
# either and make the cache able to fail the required build.
TEST_ARCHIVE_SCCACHE_IGNORE_IO = 'SCCACHE_IGNORE_SERVER_IO_ERROR: "1"'
TEST_ARCHIVE_SCCACHE_RETRY = "BOLT_RUST_VERIFICATION_SCCACHE=0 just test-archive"
TEST_ARCHIVE_SCCACHE_PREFIX_PRECONDITION = (
    '[[ -n "$role_arn" && -n "$BUCKET" && -n "$REGION" && -n "$PREFIX" ]]'
)
TEST_ARCHIVE_SCCACHE_LOCATION_PRECONDITION = (
    '[[ "$BUCKET" == "bolt-v2-ci-cache-675819144420-us-east-2" && "$REGION" == "us-east-2" && "$PREFIX" == "sccache/bolt-v2/arm64/root-nextest/" ]]'
)
TEST_ARCHIVE_SCCACHE_MAIN_DISPATCH_TRUST = (
    'if [[ "$GITHUB_EVENT_NAME" == "workflow_dispatch" && "$GITHUB_REF" == "refs/heads/main" ]]; then trusted=true; fi'
)
TEST_ARCHIVE_SCCACHE_MAIN_PUSH_TRUST = (
    'if [[ "$GITHUB_EVENT_NAME" == "push" && "$GITHUB_REF" == "refs/heads/main" ]]; then trusted=true; fi'
)
TEST_ARCHIVE_SCCACHE_TRUSTED_ASSIGNMENTS = (
    TEST_ARCHIVE_SCCACHE_MAIN_DISPATCH_TRUST,
    TEST_ARCHIVE_SCCACHE_MAIN_PUSH_TRUST,
)
TEST_ARCHIVE_SCCACHE_PR_ROLE_ENV = "PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}"
TEST_ARCHIVE_SCCACHE_READ_WRITE_ROLE = (
    'if [[ "$trusted" == "true" ]]; then\n'
    '            cache_mode="read_write"\n'
    '            role_arn="$ROLE_ARN"\n'
    "          fi"
)
TEST_ARCHIVE_SCCACHE_PR_READ_ONLY_ROLE = (
    'if [[ "$GITHUB_EVENT_NAME" == "pull_request" ]]; then\n'
    '            cache_mode="read_only"\n'
    '            role_arn="$PR_READONLY_ROLE_ARN"\n'
    "          fi"
)
TEST_ARCHIVE_SCCACHE_ROLE_OUTPUT = 'echo "role_arn=$role_arn" >> "$GITHUB_OUTPUT"'
TEST_ARCHIVE_SCCACHE_MODE_OUTPUT = 'echo "cache_mode=$cache_mode" >> "$GITHUB_OUTPUT"'
TEST_ARCHIVE_SCCACHE_RESOLVED_ROLE_ASSUME = "role-to-assume: ${{ steps.sccache-eligible.outputs.role_arn }}"
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
TEST_ARCHIVE_TARGET_CACHE_SAVE_GUARD = "if: ${{ (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}"
TEST_ARCHIVE_TEST_PROFILE_ENV = 'CARGO_PROFILE_TEST_DEBUG: "0"'
TEST_ARCHIVE_SIDECAR_PROFILE_ENV = 'CARGO_PROFILE_DEV_DEBUG: "0"'
TEST_ARCHIVE_SIDECAR_BUILD_COMMAND = (
    'python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo "$GITHUB_WORKSPACE" -- build --locked --bins'
)
TEST_ARCHIVE_SIDECAR_PACK_COMMAND = "python3 scripts/root_bin_sidecars.py pack"
TEST_ARCHIVE_RESTORE_ACTION = "uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_SAVE_ACTION = "uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_DOWNLOAD_ACTION = "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c"
UPLOAD_ARTIFACT_SHA_RE = re.compile(r"^\s*(?:-\s*)?uses:\s*([\"']?)actions/upload-artifact@[0-9a-fA-F]{40}\1\s*$")
CACHE_KEY_RE = re.compile(r"^\s+(?:key|shared-key):\s*\S+.*$")
SHARED_REGISTRY_CACHE_KEY = "cargo-registry-git-v1"
SHARED_REGISTRY_SAVE_IF = "${{ github.job == 'test-archive' }}"
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
BOLT_V2_BINARY_ARTIFACT_NAME = "bolt-v2-binary"
BOLT_V2_BINARY_RETENTION_DAYS = "3"
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
CI_SOURCE_BUILD_TOOLS = ("cargo-deny", "cargo-nextest", "cargo-zigbuild")
CI_INSTALL_ACTION_COMMANDS = {
    "deny": "just deny",
    "advisories": "just deny-advisories",
    "test-archive": 'just test-archive "$NEXTEST_ARCHIVE_PATH"',
    "build": "just build",
}
# Static-only option consumption keeps this local constant intentionally; the
# shared scanner has broader Cargo CLI coverage while preserving scan parity.
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT = {"--frozen", "--locked", "--offline", "--quiet", "-q", "--verbose", "-v"}


# verify_text re-parses the same shell strings tens of thousands of times across
# a run (e.g. `fi`, `exit 1`); these helpers are pure functions of a single str,
# so memoize. An unbounded cache is safe: the distinct-string set is bounded by
# the workflow corpus and the process is a short-lived CLI/test invocation.
@functools.cache
def strip_comment(line: str) -> str:
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
            continue
        if char == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index].rstrip()
    return line.rstrip()


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
def _normalize_concurrency_text(text: str) -> str:
    """Collapse all runs of whitespace to single spaces so formatting (YAML
    folding, indentation, line wrapping) does not affect the allowlist match."""
    return " ".join(text.split())


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

# Shared predicate used by advisory jobs to skip redundant runs on Mergify proof
# PR metadata-only edits (title/body) after the initial opened run. It must NOT
# be used for required merge-proof jobs. The predicate is true only for
# pull_request edited events where the head ref is a Mergify queue proof branch
# and the base ref did not change.
# Shared predicate used by advisory jobs to skip redundant runs on Mergify proof
# PR metadata-only edits (title/body) after the initial opened run. It is true
# only for pull_request edited events where the head ref is a Mergify queue
# proof branch and the base ref did not change. It must NOT be used for required
# merge-proof jobs.
MERGIFY_METADATA_EDIT_SKIP_PREDICATE = (
    "github.event.action == 'edited' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& !(github.event.changes.base.ref.from != '')"
)

EXPECTED_MERGE_READINESS_PROGRESS_IF = (
    "${{ github.event_name == 'pull_request' "
    "&& !(" + MERGIFY_METADATA_EDIT_SKIP_PREDICATE + ") }}"
)

EXPECTED_COVERAGE_ENFORCER_IF = (
    "${{ !(github.event_name == 'pull_request' "
    "&& " + MERGIFY_METADATA_EDIT_SKIP_PREDICATE + ") }}"
)


MERGE_GROUP_SAFE_GROUP_FORMS = frozenset({
    # .github/workflows/ci.yml — merge_group arm format('mq-{0}', github.ref)
    # wins under merge_group (the PR/workflow_dispatch arms are false then), before
    # the per-ref/sha fallback; PR-draft-deferral arms are gated off merge_group.
    "group: >- ${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id) "
    "|| github.event_name == 'pull_request' && github.event.pull_request.draft == true "
    "&& contains(fromJSON('[\"opened\",\"synchronize\",\"reopened\",\"converted_to_draft\",\"edited\"]'), github.event.action) "
    "&& format('pr-{0}-deferred', github.event.number) || github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false && (github.event.action == 'reopened' "
    "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))) "
    "&& format('pr-{0}-noop', github.event.number) || github.event_name == 'pull_request' "
    "&& format('pr-{0}-full', github.event.number) || github.event_name == 'workflow_dispatch' "
    "&& github.event.inputs.full_ci == 'true' && format('{0}-dispatch-full', github.ref_name) "
    "|| github.event_name == 'workflow_dispatch' && format('{0}-dispatch-iteration', github.ref_name) "
    "|| github.event_name == 'merge_group' "
    "&& format('mq-{0}', github.ref) || format('{0}-{1}', github.ref_name, github.sha) }}",
    # .github/workflows/actionlint.yml — simpler prefixed shape, same merge_group
    # arm before the per-ref/sha fallback.
    "group: >- actionlint-${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id) "
    "|| github.event_name == 'pull_request' && format('pr-{0}', github.event.number) "
    "|| github.event_name == 'merge_group' && format('mq-{0}', github.ref) "
    "|| format('{0}-{1}', github.ref_name, github.sha) }}",
    # .github/workflows/backtester-ci.yml — same draft/full PR split as ci.yml
    # with a backtester-prefixed merge_group arm before the per-ref/sha fallback.
    "group: >- ${{ github.event_name == 'pull_request' "
    "&& (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
    "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
    "&& format('bvs-pr-{0}-mergify-proof-{1}', github.event.number, github.run_id) "
    "|| github.event_name == 'pull_request' && github.event.pull_request.draft == true "
    "&& contains(fromJSON('[\"opened\",\"synchronize\",\"reopened\",\"converted_to_draft\",\"edited\"]'), github.event.action) "
    "&& format('bvs-pr-{0}-deferred', github.event.number) || github.event_name == 'pull_request' "
    "&& github.event.pull_request.draft == false && (github.event.action == 'reopened' "
    "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))) "
    "&& format('bvs-pr-{0}-noop', github.event.number) || github.event_name == 'pull_request' "
    "&& format('bvs-pr-{0}-full', github.event.number) || github.event_name == 'workflow_dispatch' "
    "&& github.event.inputs.full_ci == 'true' && format('bvs-{0}-dispatch-full', github.ref_name) "
    "|| github.event_name == 'workflow_dispatch' && format('bvs-{0}-dispatch-iteration', github.ref_name) "
    "|| github.event_name == 'merge_group' && format('bvs-mq-{0}', github.ref) "
    "|| format('bvs-{0}-{1}', github.ref_name, github.sha) }}",
})

# cancel-in-progress is fail-closed for merge_group only when it is provably
# false for the merge_group event. A bare substring check missed `true` and
# negations (`!= 'push'`, `!startsWith(github.ref, ...)`) that evaluate true for
# the queue ref and so cancel a queue validation. A positive allowlist — the
# literal false, or solely pull_request/workflow_dispatch equality arms — is the
# only form we can prove never cancels a merge_group run.
SAFE_CANCEL_EVENT_RE = re.compile(
    r"github\.event_name\s*==\s*(['\"])(pull_request|workflow_dispatch)\1"
)
KNOWN_SAFE_CANCEL_FORMS = frozenset(
    {
        "${{ github.event_name == 'pull_request' && !(github.event.pull_request.draft == false "
        "&& (github.event.action == 'reopened' || (github.event.action == 'edited' "
        "&& !(github.event.changes.base.ref.from != '')))) || github.event_name == 'workflow_dispatch' }}",
        "${{ github.event_name == 'pull_request' "
        "&& !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
        "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
        "&& !(github.event.pull_request.draft == false && (github.event.action == 'reopened' "
        "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from != '')))) "
        "|| github.event_name == 'workflow_dispatch' }}",
        "${{ github.event_name == 'pull_request' "
        "&& !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
        "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
    }
)


def _cancel_in_progress_value(cancel_text: str) -> str:
    """Extract the cancel-in-progress scalar as one normalized line, dropping
    the key and any YAML folding indicator (>-, |, …)."""
    marker = "cancel-in-progress:"
    idx = cancel_text.find(marker)
    raw = cancel_text[idx + len(marker):] if idx != -1 else cancel_text
    tokens = raw.split()
    if tokens and tokens[0] in {">-", ">+", ">", "|-", "|+", "|"}:
        tokens = tokens[1:]
    return " ".join(tokens)


def cancel_in_progress_is_merge_group_safe(cancel_text: str) -> bool:
    """True only when cancel-in-progress is provably false for the merge_group
    event: the literal false, or a ${{ }} expression whose only truthy operands
    are pull_request/workflow_dispatch equality arms. Any negation, function
    call, literal true, or other event name leaves residue and fails closed."""
    value = _cancel_in_progress_value(cancel_text)
    if value == "false":
        return True
    if _normalize_concurrency_text(value) in KNOWN_SAFE_CANCEL_FORMS:
        return True
    match = re.fullmatch(r"\$\{\{(.*)\}\}", value, re.DOTALL)
    if not match:
        return False
    inner = match.group(1)
    if "!" in inner:
        return False
    residue = SAFE_CANCEL_EVENT_RE.sub("", inner)
    for token in ("||", "(", ")"):
        residue = residue.replace(token, " ")
    return residue.strip() == ""


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
        or "github.run_id" not in normalized_group
    ):
        errors.append("concurrency group must isolate Mergify proof PR runs")
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
    if (
        "github.event.inputs.full_ci == 'true'" not in group_text
        or "dispatch-full" not in group_text
        or "dispatch-iteration" not in group_text
    ):
        errors.append("workflow_dispatch runs must split full and iteration concurrency groups")
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


MERGIFY_TEMP_PR_FULL_ACTIONS = frozenset({"opened", "synchronize", "reopened", "ready_for_review"})


def bool_like(value: bool | str) -> bool:
    if isinstance(value, bool):
        return value
    return value.strip().lower() == "true"


def mergify_temp_pr_requires_full_ci(*, action: str, pull_request_base_changed: bool | str) -> bool:
    base_changed = bool_like(pull_request_base_changed)
    return action in MERGIFY_TEMP_PR_FULL_ACTIONS or (action == "edited" and base_changed)


def evaluate_ci_policy(
    policy: dict[str, object],
    gate_names: dict[str, str],
    *,
    event_name: str,
    action: str,
    pull_request_draft: bool,
    pull_request_head_ref: str = "",
    pull_request_base_changed: bool = False,
    workflow_dispatch_full_ci: str = "",
    mergify_temp_pr_head_ref_prefix: str = "",
    mergify_temp_pr_actor_id: int = -1,
    event_sender_id: int = -1,
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
            workflow_dispatch_full_ci=workflow_dispatch_full_ci,
            docs_only=False,
            event_sender_id=event_sender_id,
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


def workflow_dispatch_input_errors(workflow_text: str, input_name: str) -> list[str]:
    block = workflow_trigger_block(workflow_text, "workflow_dispatch")
    if not block and "workflow_dispatch:" not in "\n".join(top_level_block(workflow_text, "on")):
        return ["workflow must define workflow_dispatch"]
    block_text = "\n".join(block)
    if "inputs:" not in block_text or not re.search(rf"^\s+{re.escape(input_name)}:\s*$", block_text, re.MULTILINE):
        return ["workflow_dispatch must define configured full CI input"]
    input_lines = block_text.splitlines()
    input_start = next(
        (
            index
            for index, line in enumerate(input_lines)
            if re.match(rf"^\s+{re.escape(input_name)}:\s*$", strip_comment(line))
        ),
        None,
    )
    if input_start is None:
        return ["workflow_dispatch must define configured full CI input"]
    input_indent = len(input_lines[input_start]) - len(input_lines[input_start].lstrip())
    input_end = len(input_lines)
    next_input_re = re.compile(rf"^\s{{{input_indent}}}[A-Za-z0-9_-]+:\s*$")
    for index in range(input_start + 1, len(input_lines)):
        if next_input_re.match(strip_comment(input_lines[index])):
            input_end = index
            break
    if not input_block_has_default_false(input_lines[input_start:input_end]):
        return ["workflow_dispatch full CI input must default to false"]
    return []


SHELL_ASSIGNMENT_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)(?:\+)?=[\s\S]*$")
CI_POLICY_SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "(", "{", ")", "}"}
PYTHON3_EXECUTABLE_RE = re.compile(r"^python3(?:\.\d+)?$")


def shell_assignment_name(token: str) -> str | None:
    match = SHELL_ASSIGNMENT_RE.match(token)
    return match.group(1) if match else None


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
    # sender id hygiene is not an unspoofable trust boundary. The queue-only boundary
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
    if f'--pull-request-base-changed "${{{{ {PR_BASE_CHANGED_EXPR} }}}}"' not in text:
        errors.append("ci-policy must pass pull_request base-change state")
    if '--workflow-dispatch-full-ci "${{ github.event.inputs.full_ci || \'\' }}"' not in text:
        errors.append("ci-policy must pass workflow_dispatch full_ci input")
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


def block_runs_command(block: list[str], command: str) -> bool:
    for index, line in enumerate(block):
        clean = strip_comment(line)
        inline = YAML_RUN_LINE_RE.match(clean)
        if inline is None:
            continue
        value = inline.group(2).strip().strip("'\"")
        if value == command:
            return True
        if value not in {"|", ">"}:
            continue
        for nested in block[index + 1 :]:
            nested_clean = strip_comment(nested).strip()
            if nested_clean == command:
                return True
        return False
    return False


def job_runs_command(job_lines: list[str], command: str) -> bool:
    return any(block_runs_command(block, command) for block in step_blocks(job_lines))


def block_has_target_dir_opt_in(block: list[str]) -> bool:
    return any(TARGET_DIR_OPT_IN_RE.match(strip_comment(line)) for line in block)


def unquote_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


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


def block_input_value(block: list[str], name: str) -> str | None:
    for item_name, item_value in block_input_items(block):
        if item_name == name:
            return unquote_yaml_scalar(item_value)
    return None


def job_has_setup_input(job_lines: list[str], name: str, value: str | None = None) -> bool:
    return any(block_has_input(block, name, value) for block in setup_action_blocks(job_lines))


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


def has_line_matching(lines: list[str], pattern: re.Pattern[str]) -> bool:
    return any(pattern.match(strip_comment(line)) for line in lines)


def job_if_value(job_lines: list[str]) -> str:
    for index, line in enumerate(job_lines):
        clean = strip_comment(line).rstrip()
        if clean.strip() == "steps:":
            return ""
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
            errors.append(f"{job} shared Cargo registry/git cache save must be single-owner")
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
    return any("actions/cache@" in strip_comment(line) for line in block) and block_has_input(
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
    if not cache_blocks or not all(block_has_input(block, "key", TEST_ARCHIVE_CACHE_KEY) for block in cache_blocks):
        return ["nextest archive cache key must use nextest fingerprint output"]
    if any("hashFiles(" in (block_input_value(block, "key") or "") for block in cache_blocks):
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
    target_blocks = [
        block
        for block in github_cache_blocks(job_lines)
        if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
    ]
    if not target_blocks:
        return [f"{job} must use isolated managed target cache"]

    expected_prefix = (
        f"managed-target-v1-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-{expected_key}-"
    )
    # The exact `key:` value must carry the job-specific prefix. Checking the
    # whole block's text would also match a prefix that only appears in
    # `restore-keys:`, masking key/restore-keys drift.
    if not any(block_key_value_has_prefix(block, expected_prefix) for block in target_blocks):
        return [f"{job} managed target cache key must isolate {expected_key}"]

    # #400: each managed-target cache MUST declare a restore-keys prefix fallback
    # matching the job's key prefix. Without it, any change to CI orchestration
    # files included in hashFiles (justfile, ci/rust-verification.toml,
    # scripts/rust_verification.py) misses the exact key and pays the full
    # ~22m aarch64 release cross-compile instead of an incremental rebuild.
    if not any(
        block_declares_restore_keys_prefix(block, expected_prefix) for block in target_blocks
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


def first_step_running_command(job_lines: list[str], command: str) -> int | None:
    for index, block in enumerate(step_blocks(job_lines)):
        if block_runs_command(block, command):
            return index
    return None


def shell_assignment_word(token: str) -> bool:
    return shell_assignment_name(token) is not None


def shell_name_word(token: str) -> bool:
    return re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", storage_strip_quotes(token)) is not None


SUDO_OPTIONS_WITH_ARGUMENT = {
    "-a",
    "-C",
    "-c",
    "-D",
    "-g",
    "-h",
    "-p",
    "-R",
    "-r",
    "-T",
    "-t",
    "-U",
    "-u",
    "--auth-type",
    "--chdir",
    "--close-from",
    "--command-timeout",
    "--group",
    "--host",
    "--login-class",
    "--prompt",
    "--role",
    "--type",
    "--user",
}
SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT = {
    "--preserve-env",
}
SUDO_OPTIONS_WITHOUT_ARGUMENT = {
    "-A",
    "-b",
    "-E",
    "-e",
    "-H",
    "-i",
    "-K",
    "-k",
    "-l",
    "-n",
    "-P",
    "-S",
    "-s",
    "-V",
    "-v",
    "--askpass",
    "--background",
    "--bell",
    "--edit",
    "--help",
    "--ignore-ticket",
    "--list",
    "--login",
    "--non-interactive",
    "--remove-timestamp",
    "--reset-timestamp",
    "--stdin",
    "--validate",
    "--version",
}
ENV_OPTIONS_WITH_ARGUMENT = {
    "-a",
    "-S",
    "-u",
    "-C",
    "--argv0",
    "--split-string",
    "--unset",
    "--chdir",
}
ENV_SIGNAL_OPTIONS = {"--block-signal", "--default-signal", "--ignore-signal"}
ENV_OPTIONS_WITHOUT_ARGUMENT = {
    "-0",
    "-i",
    "-v",
    "--debug",
    "--ignore-environment",
    "--null",
}
SU_SG_OPTIONS_WITH_ARGUMENT = {
    "-g",
    "-G",
    "-s",
    "-w",
    "--group",
    "--shell",
    "--supp-group",
    "--whitelist-environment",
}
SU_SG_OPTIONS_WITHOUT_ARGUMENT = {
    "-l",
    "-m",
    "-M",
    "-p",
    "-P",
    "--fast",
    "--login",
    "--preserve-environment",
    "--pty",
}
SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS = {"m", "M", "p", "P", "l"}
FLOCK_OPTIONS_WITH_ARGUMENT = {"-E", "-w", "--conflict-exit-code", "--wait", "--timeout"}
FLOCK_OPTIONS_WITHOUT_ARGUMENT = {
    "-F",
    "-n",
    "-o",
    "-s",
    "-u",
    "-x",
    "--close",
    "--exclusive",
    "--no-fork",
    "--nonblock",
    "--shared",
    "--unlock",
    "--verbose",
}
FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS = {"s", "x", "n", "u", "o", "F"}
TIME_OPTIONS_WITH_ARGUMENT = {"-f", "-o", "--format", "--output"}
TIME_OPTIONS_WITHOUT_ARGUMENT = {"-a", "-p", "-v", "--append", "--portability", "--verbose"}
SHELL_PUNCTUATION_CHARS = ";&|(){}!<>"
SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "if", "elif", "then", "else", "while", "until", "do", "!", "(", "{", ")", "}"}
SHELL_REDIRECTION_OPERATORS = {">", ">>", "<", "<<", "<>", ">|", ">&", "<&", "&>", "&>>", "<<<"}
SHELL_PUNCTUATION_OPERATORS = {
    "&>>",
    "&&",
    "||",
    ">>",
    "<<",
    "<>",
    ">|",
    ">&",
    "<&",
    "&>",
    "<<<",
}
SHELL_PUNCTUATION_OPERATORS_BY_LENGTH = tuple(sorted(SHELL_PUNCTUATION_OPERATORS, key=len, reverse=True))
RECURSIVE_WRAPPER_EXECUTABLES = {
    "catchsegv",
    "chrt",
    "command",
    "chroot",
    "doas",
    "docker",
    "env",
    "exec",
    "flock",
    "ionice",
    "nice",
    "nohup",
    "podman",
    "runuser",
    "rustup",
    "setsid",
    "sg",
    "stdbuf",
    "su",
    "sudo",
    "taskset",
    "time",
    "timeout",
    "xargs",
}
CARGO_PROCESS_SUBCOMMANDS = {
    "bench",
    "build",
    "check",
    "clean",
    "clippy",
    "doc",
    "fetch",
    "fmt",
    "install",
    "nextest",
    "run",
    "rustc",
    "test",
    "zigbuild",
}


def consume_assignment_words(tokens: list[str], index: int) -> int:
    while index < len(tokens) and shell_assignment_word(tokens[index]):
        index += 1
    return index


def consume_option_prefix(
    tokens: list[str],
    index: int,
    options_with_argument: set[str],
    options_without_argument: set[str],
    options_with_optional_argument: set[str] | None = None,
) -> int | None:
    options_with_optional_argument = options_with_optional_argument or set()
    short_options_with_argument = {option for option in options_with_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    short_options_without_argument = {option for option in options_without_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in options_with_argument:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_optional_argument if option.startswith("--")):
            index += 1
            continue
        if token in options_with_optional_argument:
            index += 1
            continue
        if token in options_without_argument:
            index += 1
            continue
        if len(token) > 2 and token.startswith("-") and not token.startswith("--"):
            offset = 1
            while offset < len(token):
                option = f"-{token[offset]}"
                if option in short_options_without_argument:
                    offset += 1
                    continue
                if option in short_options_with_argument:
                    if offset + 1 < len(token):
                        index += 1
                    elif index + 1 < len(tokens):
                        index += 2
                    else:
                        return None
                    break
                return None
            else:
                index += 1
            continue
        break
    return index


def command_prefix_allows_cargo(prefix: list[str]) -> bool:
    prefix = strip_shell_redirections(prefix)
    index = consume_assignment_words(prefix, 0)
    while index < len(prefix):
        token = prefix[index]
        if token == "command":
            index += 1
        elif token == "time":
            index = consume_option_prefix(prefix, index + 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "nice":
            index = nice_command_index(prefix, index + 1)
        elif token == "sudo":
            index = consume_option_prefix(
                prefix,
                index + 1,
                SUDO_OPTIONS_WITH_ARGUMENT,
                SUDO_OPTIONS_WITHOUT_ARGUMENT,
                SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
            )
        elif token == "doas":
            index = consume_option_prefix(prefix, index + 1, SUDO_OPTIONS_WITH_ARGUMENT, SUDO_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "env":
            index = env_command_prefix_index(prefix, index + 1)
        elif token == "flock":
            inner = flock_inner_tokens(prefix[index:])
            if inner is not None:
                index = len(prefix) - len(inner)
            else:
                return False
        elif token == "eval":
            index += 1
            if index < len(prefix) and prefix[index] == "--":
                index += 1
        elif token in {"catchsegv", "chrt", "exec", "ionice", "nohup", "setsid", "stdbuf", "taskset", "timeout", "xargs"}:
            inner = wrapper_inner_tokens(prefix[index:])
            if inner is None:
                return False
            index = len(prefix) - len(inner)
        else:
            return False
        if index is None:
            return False
        index = consume_assignment_words(prefix, index)
    return True


def cargo_token_is_command(tokens: list[str], index: int) -> bool:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        cursor -= 1
    prefix = tokens[cursor + 1 : index]
    return command_prefix_allows_cargo(prefix)


def split_shell_punctuation_tokens(tokens: list[str]) -> list[str]:
    split_tokens: list[str] = []
    for token in tokens:
        if not token or any(char not in SHELL_PUNCTUATION_CHARS for char in token):
            split_tokens.append(token)
            continue
        cursor = 0
        while cursor < len(token):
            operator = next(
                (candidate for candidate in SHELL_PUNCTUATION_OPERATORS_BY_LENGTH if token.startswith(candidate, cursor)),
                None,
            )
            if operator is not None:
                split_tokens.append(operator)
                cursor += len(operator)
                continue
            split_tokens.append(token[cursor])
            cursor += 1
    return split_tokens


def strip_shell_redirections(tokens: list[str]) -> list[str]:
    stripped: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        operator_index = index
        if (
            token.isdigit()
            and index + 1 < len(tokens)
            and tokens[index + 1] in SHELL_REDIRECTION_OPERATORS
        ):
            operator_index = index + 1
        if tokens[operator_index] in SHELL_REDIRECTION_OPERATORS:
            index = operator_index + 1
            if index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
                index += 1
            continue
        stripped.append(token)
        index += 1
    return stripped


# Pure shlex parse of a single command string; memoized because verify_text
# re-tokenizes the same strings thousands of times. Cache an immutable tuple and
# copy on return so callers that mutate the list cannot corrupt the cache.
@functools.cache
def _command_tokens_cached(command: str) -> tuple[str, ...]:
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=SHELL_PUNCTUATION_CHARS)
        lexer.whitespace_split = True
        return tuple(split_shell_punctuation_tokens(list(lexer)))
    except ValueError:
        return tuple(command.split())


def command_tokens(command: str) -> list[str]:
    return list(_command_tokens_cached(command))


def command_tokens_with_line_boundaries(command: str) -> list[str]:
    tokens: list[str] = []
    for line in shell_logical_lines(command):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        line_tokens = command_tokens(stripped)
        if not line_tokens:
            continue
        if tokens and tokens[-1] not in {"|", "&&", "||"}:
            tokens.append(";")
        tokens.extend(line_tokens)
    return tokens


def backtick_command_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        start = token.find("`")
        if start < 0:
            index += 1
            continue
        payload_parts: list[str] = []
        remainder = token[start + 1 :]
        end = remainder.find("`")
        if end >= 0:
            payload = remainder[:end].strip()
            if payload:
                payloads.append(command_tokens(payload))
            index += 1
            continue
        if remainder:
            payload_parts.append(remainder)
        cursor = index + 1
        while cursor < len(tokens):
            part = tokens[cursor]
            end = part.find("`")
            if end >= 0:
                if end:
                    payload_parts.append(part[:end])
                break
            payload_parts.append(part)
            cursor += 1
        if cursor < len(tokens):
            payload = " ".join(payload_parts).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor + 1
            continue
        index += 1
    return payloads


def inline_command_substitution_payloads(token: str) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 0
    while index + 1 < len(token):
        if token[index : index + 2] not in {"$(", "<("}:
            index += 1
            continue
        cursor = index + 2
        depth = 1
        payload_chars: list[str] = []
        while cursor < len(token) and depth:
            char = token[cursor]
            if char == "(":
                depth += 1
                payload_chars.append(char)
            elif char == ")":
                depth -= 1
                if depth:
                    payload_chars.append(char)
            else:
                payload_chars.append(char)
            cursor += 1
        if depth == 0:
            payload = "".join(payload_chars).strip()
            if payload:
                payloads.append(command_tokens(payload))
            index = cursor
            continue
        index += 1
    return payloads


def shell_command_substitution_payloads(tokens: list[str]) -> list[list[str]]:
    payloads = backtick_command_payloads(tokens)
    for token in tokens:
        payloads.extend(inline_command_substitution_payloads(token))
    index = 0
    while index + 1 < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$") or token == "<") and tokens[index + 1] == "(":
            cursor = index + 2
            depth = 1
            payload: list[str] = []
            while cursor < len(tokens) and depth:
                current = tokens[cursor]
                if current == "(":
                    depth += 1
                    payload.append(current)
                elif current == ")":
                    depth -= 1
                    if depth:
                        payload.append(current)
                else:
                    payload.append(current)
                cursor += 1
            if depth == 0:
                if payload:
                    payloads.append(payload)
                index = cursor
                continue
        index += 1
    return payloads


def shell_quotes_are_balanced(text: str) -> bool:
    quote: str | None = None
    escaped = False
    for char in text:
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
    return quote is None


def shell_logical_lines(text: str) -> list[str]:
    lines: list[str] = []
    pending = ""
    normalized = text.replace("\\\r\n", " ").replace("\\\n", " ")
    for line in normalized.splitlines():
        pending = f"{pending}\n{line}" if pending else line
        balance_text = "\n".join(strip_comment(pending_line) for pending_line in pending.splitlines())
        if shell_quotes_are_balanced(balance_text):
            lines.append(pending)
            pending = ""
    if pending:
        lines.append(pending)
    return lines


def shell_command(tokens: list[str]) -> str | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if index + 1 < len(tokens):
            if token == "-c":
                return tokens[index + 1]
            if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
                return tokens[index + 1]
        index += 1
    return None


def source_build_tool_from_token(token: str) -> str | None:
    token = token.rstrip("/")
    lower_token = token.lower()
    for tool in CI_SOURCE_BUILD_TOOLS:
        lower_tool = tool.lower()
        if lower_token == lower_tool or lower_token.startswith(f"{lower_tool}@"):
            return tool
        if lower_token.endswith(f"/{lower_tool}") or lower_token.endswith(f"/{lower_tool}.git"):
            return tool
    return None


def normalized_source_path(token: str) -> str:
    return token.rstrip("/")


def source_build_tool_for_path(
    token: str,
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None = None,
) -> str | None:
    normalized = normalized_source_path(token)
    if normalized == "." and cwd_source_tool is not None:
        return cwd_source_tool
    if source_path_tools and normalized in source_path_tools:
        return source_path_tools[normalized]
    return source_build_tool_from_token(token)


def executable_name(token: str) -> str:
    return pathlib.Path(token).name


def cargo_install_source_build_tools(
    tokens: list[str],
    command_index: int,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    tools: set[str] = set()
    for payload in shell_command_substitution_payloads(tokens[command_index + 1 :]):
        for token in payload:
            tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
    index = command_index + 1
    while index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[index]
        if token in ("--package", "-p") and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--package="):
            tool = source_build_tool_for_path(token.removeprefix("--package="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        if token == "--path" and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--path="):
            tool = source_build_tool_for_path(token.removeprefix("--path="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
        index += 1
    return tools


def source_build_tools_from_depth_exceeded_tokens(
    tokens: list[str],
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None,
) -> set[str]:
    if "install" not in tokens:
        return set()
    tools: set[str] = set()
    for token in tokens:
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
    return tools


def cd_source_tool(tokens: list[str], source_path_tools: dict[str, str] | None) -> tuple[bool, str | None]:
    if not tokens or tokens[0] != "cd":
        return False, None
    index = 1
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return True, None
    return True, source_build_tool_for_path(tokens[index], source_path_tools)


def cargo_install_source_build_tools_from_tokens(
    tokens: list[str],
    *,
    depth: int = 0,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return set()
    if depth > 6:
        return source_build_tools_from_depth_exceeded_tokens(tokens, source_path_tools, cwd_source_tool)
    tools: set[str] = set()
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        segment_cwd_source_tool = cwd_source_tool
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                tools.update(
                    cargo_install_source_build_tools_from_tokens(
                        segment,
                        depth=depth + 1,
                        source_path_tools=source_path_tools,
                        cwd_source_tool=segment_cwd_source_tool,
                    )
                )
                changed, cd_tool = cd_source_tool(segment, source_path_tools)
                if changed:
                    segment_cwd_source_tool = cd_tool
                segment = []
                continue
            segment.append(token)
        tools.update(
            cargo_install_source_build_tools_from_tokens(
                segment,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=segment_cwd_source_tool,
            )
        )
        return tools
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return cargo_install_source_build_tools_from_tokens(
            tokens[assignment_index:],
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    executable = pathlib.Path(tokens[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        if nested is None:
            return tools
        return cargo_install_source_build_tools_from_tokens(
            command_tokens(nested),
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    if executable.startswith("python"):
        for payload in python_inline_command_payloads(tokens):
            tools.update(
                cargo_install_source_build_tools_from_tokens(
                    command_tokens(payload),
                    depth=depth + 1,
                    source_path_tools=source_path_tools,
                    cwd_source_tool=cwd_source_tool,
                )
            )
        return tools
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return cargo_install_source_build_tools_from_tokens(
                inner,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=cwd_source_tool,
            )
        return tools
    if executable == "cargo":
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    elif path_invocation_may_have_cargo_subcommand(tokens):
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    return tools


def source_build_clone_path_tools(text: str) -> dict[str, str]:
    path_tools: dict[str, str] = {}
    for line in text.replace("\\\n", " ").splitlines():
        tokens = command_tokens(line)
        for index, token in enumerate(tokens[:-2]):
            if executable_name(token) != "git" or tokens[index + 1] != "clone":
                continue
            cursor = index + 2
            while cursor < len(tokens) and tokens[cursor].startswith("-"):
                if cursor + 1 < len(tokens) and not tokens[cursor + 1].startswith("-"):
                    cursor += 2
                else:
                    cursor += 1
            if cursor >= len(tokens):
                continue
            tool = source_build_tool_from_token(tokens[cursor])
            if tool is None:
                continue
            if cursor + 1 < len(tokens) and tokens[cursor + 1] not in SHELL_COMMAND_BOUNDARIES:
                path_tools[normalized_source_path(tokens[cursor + 1])] = tool
    return path_tools


def cargo_install_source_build_tools_in_text(text: str) -> set[str]:
    tools: set[str] = set()
    source_path_tools = source_build_clone_path_tools(text)
    cwd_source_tool: str | None = None
    for line in text.replace("\\\n", " ").splitlines():
        lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        try:
            tokens = list(lexer)
        except ValueError:
            continue
        if "install" in line:
            tools.update(
                cargo_install_source_build_tools_from_tokens(
                    tokens,
                    source_path_tools=source_path_tools,
                    cwd_source_tool=cwd_source_tool,
                )
            )
        for index, token in enumerate(tokens[:-1]):
            if executable_name(token) != "cargo":
                continue
            if not cargo_token_is_command(tokens, index):
                continue
            command_index = consume_cargo_global_options(tokens, index + 1)
            if command_index >= len(tokens) or tokens[command_index] != "install":
                continue
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
        changed, cd_tool = cd_source_tool(tokens, source_path_tools)
        if changed:
            cwd_source_tool = cd_tool
    return tools


def python_rust_verification_script_index(tokens: list[str]) -> int | None:
    if not tokens or not pathlib.Path(tokens[0]).name.startswith("python"):
        return None
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in {"-B", "-E", "-I", "-O", "-OO", "-S", "-s", "-u"}:
            index += 1
            continue
        if token in {"-W", "-X"} and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("-W", "-X")) and token not in {"-W", "-X"}:
            index += 1
            continue
        break
    if index < len(tokens) and pathlib.Path(tokens[index]).name == "rust_verification.py":
        return index
    return None


def managed_rust_verification_command_tokens(tokens: list[str], *, depth: int = 0) -> list[str] | None:
    if depth > 6:
        return None
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return None
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return managed_rust_verification_command_tokens(tokens[assignment_index:], depth=depth + 1)
    executable = pathlib.Path(tokens[0]).name
    if executable == "env":
        inner = env_inner_tokens(tokens)
        return managed_rust_verification_command_tokens(inner, depth=depth + 1) if inner is not None else None
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        return managed_rust_verification_command_tokens(inner, depth=depth + 1) if inner is not None else None
    script_index = python_rust_verification_script_index(tokens)
    if script_index is None or script_index + 1 >= len(tokens):
        return None
    command = tokens[script_index + 1]
    if command not in {"cargo", "run"}:
        return None
    return [tokens[0], tokens[script_index], *tokens[script_index + 1 :]]


def managed_rust_verification_tokens(tokens: list[str]) -> bool:
    return managed_rust_verification_command_tokens(tokens) is not None


def consume_rust_verification_repo_option(tokens: list[str], index: int) -> int:
    if index >= len(tokens):
        return index
    token = tokens[index]
    if token == "--repo" and index + 1 < len(tokens):
        return index + 2
    if token.startswith("--repo="):
        return index + 1
    return index


def managed_rust_verification_cargo_args(tokens: list[str]) -> list[str] | None:
    normalized_tokens = managed_rust_verification_command_tokens(tokens)
    if normalized_tokens is None:
        return None
    command = normalized_tokens[2]
    tail = normalized_tokens[3:]
    index = 0
    while index < len(tail):
        if tail[index] == "--":
            index += 1
            break
        next_index = consume_rust_verification_repo_option(tail, index)
        if next_index == index:
            break
        index = next_index
    if command == "cargo":
        return tail[index:]
    if index >= len(tail):
        return []
    managed_command = tail[index]
    managed_args = tail[index + 1 :]
    return [managed_command, *managed_args]


def target_routing_cargo_args(tokens: list[str]) -> list[str] | None:
    tokens = strip_shell_redirections(tokens)
    managed_args = managed_rust_verification_cargo_args(tokens)
    if managed_args is not None:
        return managed_args
    if not tokens:
        return None
    executable = pathlib.Path(tokens[0]).name
    if executable == "cargo" or path_invocation_may_have_cargo_subcommand(tokens):
        return tokens[1:]
    return None


def cargo_target_routing_scan_tokens(tokens: list[str]) -> list[str]:
    cargo_args = target_routing_cargo_args(tokens)
    if cargo_args is None:
        return []
    return cargo_args_for_target_routing_scan(cargo_args)


def tokens_have_target_routing_override(tokens: list[str]) -> bool:
    env_prefixes = (
        "BOLT_MANAGED_JUST=",
        "CARGO_BUILD_RUSTFLAGS=",
        "CARGO_BUILD_TARGET_DIR=",
        "CARGO_ENCODED_RUSTFLAGS=",
        "CARGO_HOME=",
        "CARGO_INCREMENTAL=",
        "CARGO_INSTALL_ROOT=",
        "CARGO_TARGET_DIR=",
        "CARGO_TARGET_TMPDIR=",
        "RUSTFLAGS=",
        "RUSTUP_HOME=",
    )
    value_options = {"--artifact-dir", "--out-dir", "--root", "--target-dir"}
    for token in tokens:
        if token.startswith(env_prefixes):
            return True
    scan_tokens = cargo_target_routing_scan_tokens(tokens)
    for index, token in enumerate(scan_tokens):
        if token in value_options:
            return True
        if any(token.startswith(f"{option}=") for option in value_options):
            return True
        if token == "--config" and index + 1 < len(scan_tokens) and cargo_config_has_storage_override(scan_tokens[index + 1]):
            return True
        if token.startswith("--config=") and cargo_config_has_storage_override(token.split("=", 1)[1]):
            return True
    return False


def cargo_config_has_storage_override(config: str) -> bool:
    if cargo_config_looks_like_path(config):
        return True
    scan_config = decode_toml_unicode_escapes(config)
    if "target-dir" in scan_config and ("build" in scan_config or "[build]" in scan_config):
        return True
    return "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config)


def decode_toml_unicode_escapes(value: str) -> str:
    def replace(match: re.Match[str]) -> str:
        digits = match.group(1) or match.group(2)
        return chr(int(digits, 16))

    return re.sub(r"\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})", lambda match: replace(match), value)


def cargo_config_looks_like_path(config: str) -> bool:
    stripped = config.strip()
    if not stripped:
        return False
    if stripped.startswith(("[", "{")):
        return False
    if "=" not in stripped:
        return True
    key_prefix = stripped.split("=", 1)[0]
    return "/" in key_prefix or "\\" in key_prefix or key_prefix.endswith(".toml")


def rustup_run_inner_tokens(tokens: list[str]) -> list[str]:
    index = 2
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return []
    index += 1
    while index < len(tokens) and tokens[index] == "--":
        index += 1
    return tokens[index:]


def exec_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return tokens[index + 1 :]
        if token == "-a" and index + 1 < len(tokens):
            index += 2
            continue
        if token in {"-c", "-l"}:
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            if set(cluster) <= {"c", "l"}:
                index += 1
                continue
            if cluster.endswith("a") and set(cluster[:-1]) <= {"c", "l"} and index + 1 < len(tokens):
                index += 2
                continue
        return tokens[index:]
    return []


def container_rust_payload_from_tokens(tokens: list[str], start: int) -> list[str] | None:
    for index in range(start, len(tokens)):
        token = tokens[index]
        executable = pathlib.Path(token).name
        if (
            raw_rust_tool_token(executable)
            or path_executable_looks_like_cargo(token)
            or path_executable_looks_like_rustc(token)
            or path_name_looks_like_renamed_cargo(executable)
            or path_name_looks_like_renamed_rustc(executable)
        ):
            return tokens[index:]
    return None


def container_inner_tokens(tokens: list[str]) -> list[str] | None:
    if len(tokens) < 3:
        return None
    executable = pathlib.Path(tokens[0]).name
    if executable not in {"docker", "podman"}:
        return None
    command = tokens[1]
    options_with_argument = {
        "--add-host",
        "--cpus",
        "--entrypoint",
        "--env",
        "--env-file",
        "--hostname",
        "--mount",
        "--name",
        "--network",
        "--platform",
        "--user",
        "--volume",
        "--workdir",
        "-e",
        "-h",
        "-m",
        "-u",
        "-v",
        "-w",
    }
    index = 2
    entrypoint: str | None = None
    uncertain_options = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token in options_with_argument and index + 1 < len(tokens):
            if token == "--entrypoint":
                entrypoint = tokens[index + 1]
            index += 2
            continue
        if token.startswith("--entrypoint="):
            entrypoint = token.split("=", 1)[1]
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-"):
            uncertain_options = True
            index += 1
            continue
        break
    if command == "run":
        if index >= len(tokens):
            return []
        tail = tokens[index + 1 :]
        if entrypoint is not None:
            return [entrypoint, *tail]
        if uncertain_options:
            fallback = container_rust_payload_from_tokens(tokens, 2)
            if fallback is not None:
                return fallback
        return tail
    if command == "exec":
        if index >= len(tokens):
            return []
        tail = tokens[index + 1 :]
        if uncertain_options:
            fallback = container_rust_payload_from_tokens(tokens, 2)
            if fallback is not None:
                return fallback
        return tail
    return None


def chroot_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token.startswith("--userspec=") or token.startswith("--groups="):
            index += 1
            continue
        if token in {"--userspec", "--groups"} and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return tokens[index + 1 :] if index < len(tokens) else []


def short_cluster_consumes_option_argument(
    tokens: list[str],
    index: int,
    argument_flags: set[str],
    no_argument_flags: set[str],
) -> int | None:
    token = tokens[index]
    if not token.startswith("-") or token.startswith("--"):
        return None
    offset = 1
    while offset < len(token):
        flag = token[offset]
        if flag in no_argument_flags:
            offset += 1
            continue
        if flag in argument_flags:
            return index + 1 if offset + 1 < len(token) or index + 1 >= len(tokens) else index + 2
        return None
    return index + 1


def su_sg_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in SU_SG_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in SU_SG_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token in SU_SG_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in {"-c", "--command"} and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"g", "G", "s", "w"},
            SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        index += 1
    return None


def wrapper_inner_tokens(tokens: list[str]) -> list[str] | None:
    executable = pathlib.Path(tokens[0]).name if tokens else ""
    if executable == "command":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token == "-p":
                index += 1
                continue
            if token in ("-v", "-V"):
                return []
            return tokens[index:]
        return []
    if executable in {"sudo", "doas"}:
        index = consume_option_prefix(
            tokens,
            1,
            SUDO_OPTIONS_WITH_ARGUMENT,
            SUDO_OPTIONS_WITHOUT_ARGUMENT,
            SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT if executable == "sudo" else None,
        )
        return tokens[index:] if index is not None else None
    if executable == "flock":
        return flock_inner_tokens(tokens)
    if executable == "timeout":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                continue
            if token in ("-k", "--kill-after", "-s", "--signal") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--kill-after=", "--signal=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index + 1 :]
        return []
    if executable == "stdbuf":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-i", "-o", "-e", "--input", "--output", "--error") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--input=", "--output=", "--error=")):
                index += 1
                continue
            if re.fullmatch(r"-[ioe].+", token):
                index += 1
                continue
            return tokens[index:]
        return []
    if executable == "env":
        return env_inner_tokens(tokens)
    if executable == "nice":
        index = nice_command_index(tokens, 1)
        return tokens[index:] if index is not None else None
    if executable == "rustup" and len(tokens) >= 3 and tokens[1] == "run":
        return rustup_run_inner_tokens(tokens)
    if executable == "exec":
        return exec_inner_tokens(tokens)
    if executable in {"docker", "podman"}:
        return container_inner_tokens(tokens)
    if executable == "chroot":
        return chroot_inner_tokens(tokens)
    if executable in {"catchsegv", "nohup"}:
        return tokens[1:]
    if executable == "time":
        index = consume_option_prefix(tokens, 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        return tokens[index:] if index is not None else None
    if executable == "setsid":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--ctty", "-f", "--fork", "-w", "--wait"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--") and set(token[1:]) <= {"c", "f", "w"}:
                index += 1
                continue
            return tokens[index:]
        return []
    if executable == "taskset":
        index = 1
        cpu_list_mode = False
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                continue
            if token in ("-c", "--cpu-list") and index + 1 < len(tokens):
                index += 2
                cpu_list_mode = True
                continue
            if token.startswith("--cpu-list=") or re.fullmatch(r"-c.+", token):
                index += 1
                cpu_list_mode = True
                continue
            if token in ("-a", "--all-tasks"):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token.startswith("-"):
                index += 1
                continue
            if not cpu_list_mode:
                index += 1
            return tokens[index:]
        return []
    if executable == "ionice":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--class", "-n", "--classdata") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--class=", "--classdata=")) or re.fullmatch(r"-[cn].+", token):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token in ("-t", "--ignore"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--"):
                cluster = token[1:]
                if cluster and (set(cluster) <= {"t"} or re.fullmatch(r"t*[cn].+", cluster)):
                    index += 1
                    continue
            return tokens[index:]
        return []
    if executable == "chrt":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                break
            if token in ("-p", "--pid"):
                return []
            if token in ("-T", "--sched-runtime", "-P", "--sched-period", "-D", "--sched-deadline") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--sched-runtime=", "--sched-period=", "--sched-deadline=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            break
        if index < len(tokens) and re.fullmatch(r"-?\d+", tokens[index]):
            index += 1
        return tokens[index:]
    if executable == "xargs":
        options_with_argument = {
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
        }
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in options_with_argument and index + 1 < len(tokens):
                index += 2
                continue
            if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
                index += 1
                continue
            if re.fullmatch(r"-(?:a|d|E|I|L|n|P|s).+", token):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index:]
        return []
    if executable in {"su", "sg"}:
        return su_sg_command_option_tokens(tokens)
    if executable == "runuser":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in {"-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell"} and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--user=", "--group=", "--supp-group=", "--shell=")):
                index += 1
                continue
            if token in {"-c", "--command"} and index + 1 < len(tokens):
                return command_tokens(tokens[index + 1])
            if token.startswith("--command="):
                return command_tokens(token.split("=", 1)[1])
            if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
                return command_tokens(token[2:])
            if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
                prefix, suffix = token[1:].split("c", 1)
                if set(prefix) <= {"m", "M", "p", "P", "l"}:
                    if suffix:
                        return command_tokens(suffix)
                    if index + 1 < len(tokens):
                        return command_tokens(tokens[index + 1])
            next_index = short_cluster_consumes_option_argument(
                tokens,
                index,
                {"G", "g", "s", "u"},
                SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
            )
            if next_index is not None:
                index = next_index
                continue
            if token.startswith("-"):
                index += 1
                continue
            command_index = index + 1
            while command_index < len(tokens):
                candidate = tokens[command_index]
                if candidate in {"-u", "--user", "-g", "--group", "-G", "--supp-group", "-s", "--shell"} and command_index + 1 < len(tokens):
                    command_index += 2
                    continue
                if candidate.startswith(("--user=", "--group=", "--supp-group=", "--shell=")):
                    command_index += 1
                    continue
                if candidate in {"-c", "--command"} and command_index + 1 < len(tokens):
                    return command_tokens(tokens[command_index + 1])
                if candidate.startswith("--command="):
                    return command_tokens(candidate.split("=", 1)[1])
                if candidate.startswith("-c") and not candidate.startswith("--") and len(candidate) > 2:
                    return command_tokens(candidate[2:])
                next_command_index = short_cluster_consumes_option_argument(
                    tokens,
                    command_index,
                    {"G", "g", "s", "u"},
                    SU_SG_COMMAND_CLUSTER_PREFIX_FLAGS,
                )
                if next_command_index is not None:
                    command_index = next_command_index
                    continue
                command_index += 1
            return tokens[index:]
        return None
    starters = {
        "bash",
        "catchsegv",
        "cargo",
        "cargo-clippy",
        "cargo-fmt",
        "cargo-nextest",
        "env",
        "flock",
        "nice",
        "python",
        "python3",
        "rustup",
        "sh",
        "stdbuf",
        "time",
        "zsh",
    }
    for index, token in enumerate(tokens[1:], start=1):
        if pathlib.Path(token).name in starters:
            return tokens[index:]
    return None


def find_exec_payloads(tokens: list[str]) -> list[list[str]]:
    payloads: list[list[str]] = []
    index = 1
    while index < len(tokens):
        if tokens[index] not in {"-exec", "-execdir"}:
            index += 1
            continue
        index += 1
        payload: list[str] = []
        while index < len(tokens) and tokens[index] not in {";", "+"}:
            payload.append(tokens[index])
            index += 1
        if payload:
            payloads.append(payload)
    return payloads


def shell_command_substitution_at(tokens: list[str], index: int) -> tuple[list[str], int] | None:
    if index + 1 >= len(tokens) or not (tokens[index] == "$" or tokens[index].endswith("$")) or tokens[index + 1] != "(":
        return None
    cursor = index + 2
    depth = 1
    payload: list[str] = []
    while cursor < len(tokens) and depth:
        token = tokens[cursor]
        if token == "(":
            depth += 1
            payload.append(token)
        elif token == ")":
            depth -= 1
            if depth:
                payload.append(token)
        else:
            payload.append(token)
        cursor += 1
    return (payload, cursor) if depth == 0 else None


def env_short_cluster_next_index(tokens: list[str], index: int, cluster: str) -> int | None:
    offset = 0
    while offset < len(cluster):
        option = cluster[offset]
        if option in "i0v":
            offset += 1
            continue
        if option in "uC":
            if offset + 1 < len(cluster):
                return index + 1
            if index + 1 < len(tokens):
                return index + 2
            return index + 1
        return None
    return index + 1


def env_short_split_tokens(tokens: list[str], index: int) -> list[str] | None:
    token = tokens[index]
    if not token.startswith("-") or token.startswith("--"):
        return None
    cluster = token[1:]
    if "S" not in cluster:
        return None
    suffix = cluster.split("S", 1)[1]
    if suffix:
        return command_tokens(" ".join([suffix, *tokens[index + 1 :]]))
    if index + 1 < len(tokens):
        return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
    return []


def env_assignment_argument(token: str) -> bool:
    return "=" in token and not token.startswith("-")


def env_command_prefix_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        redirection_index = shell_redirection_next_index(tokens, index)
        if redirection_index is not None:
            index = redirection_index
            continue
        if token == "--":
            return index + 1
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ENV_OPTIONS_WITH_ARGUMENT:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            if "S" in token[1:]:
                return index
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if env_assignment_argument(token):
            index += 1
            continue
        return index
    return index


def shell_redirection_next_index(tokens: list[str], index: int) -> int | None:
    token = tokens[index]
    if token in SHELL_REDIRECTION_OPERATORS:
        return min(index + 2, len(tokens))
    if re.match(r"^(?:\d?(?:>>?|<<?|<>|>\||>&|<&)|&>>?|<<<).+", token):
        return index + 1
    return None


def env_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        redirection_index = shell_redirection_next_index(tokens, index)
        if redirection_index is not None:
            index = redirection_index
            continue
        if token == "--":
            return tokens[index + 1 :]
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ("-S", "--split-string") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
        if token.startswith("--split-string="):
            return command_tokens(token.split("=", 1)[1]) + tokens[index + 1 :]
        if token in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            split_tokens = env_short_split_tokens(tokens, index)
            if split_tokens is not None:
                return split_tokens
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if env_assignment_argument(token):
            index += 1
            continue
        return tokens[index:]
    return []


def nice_command_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token == "-n" and index + 1 < len(tokens):
            index += 2
            continue
        if token == "--adjustment" and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("--adjustment="):
            index += 1
            continue
        if re.fullmatch(r"-n-?\d+", token) or re.fullmatch(r"-?\d+", token):
            index += 1
            continue
        return index
    return index


def flock_inner_tokens(tokens: list[str]) -> list[str] | None:
    command_option_tokens = flock_command_option_tokens(tokens)
    if command_option_tokens is not None:
        return command_option_tokens
    index = 1
    separator_seen = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            separator_seen = True
            break
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        if token in FLOCK_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token in FLOCK_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"E", "w"},
            FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        if token.startswith("-"):
            index += 1
            continue
        return tokens[index + 1 :]
    if separator_seen and index < len(tokens):
        return tokens[index + 1 :]
    return tokens[index:]


def flock_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return None
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-c") and not token.startswith("--") and len(token) > 2:
            return command_tokens(token[2:])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
            prefix, suffix = token[1:].split("c", 1)
            if set(prefix) <= FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS:
                if suffix:
                    return command_tokens(suffix)
                if index + 1 < len(tokens):
                    return command_tokens(tokens[index + 1])
        if token in FLOCK_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token in FLOCK_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        next_index = short_cluster_consumes_option_argument(
            tokens,
            index,
            {"E", "w"},
            FLOCK_COMMAND_CLUSTER_PREFIX_FLAGS,
        )
        if next_index is not None:
            index = next_index
            continue
        index += 1
    return None


def simple_cargo_aliases(tokens: list[str], known_aliases: set[str] | None = None) -> set[str]:
    known_aliases = known_aliases or set()
    aliases: set[str] = set()
    for name, value in shell_alias_payloads(tokens).items():
        value_tokens = command_tokens(value)
        value_names = {pathlib.Path(value_token).name for value_token in value_tokens}
        if any(raw_rust_tool_token(value_name) or value_name in known_aliases for value_name in value_names):
            aliases.add(name)
    return aliases


def shell_alias_payloads(tokens: list[str]) -> dict[str, str]:
    if not tokens or pathlib.Path(tokens[0]).name != "alias":
        return {}
    payloads: dict[str, str] = {}
    for token in tokens[1:]:
        name, separator, value = token.partition("=")
        name = name.strip("\"'")
        if separator and re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name):
            payloads[name] = value.strip()
    return payloads


def expand_cargo_aliases(tokens: list[str], aliases: set[str]) -> list[str]:
    if not aliases:
        return tokens
    return ["cargo" if token in aliases else token for token in tokens]


def no_mistakes_inner_tokens(tokens: list[str]) -> list[str] | None:
    for index, token in enumerate(tokens):
        if token == "--":
            return tokens[index + 1 :]
    return None


def rust_tool_name_has_script_extension(name: str) -> bool:
    return pathlib.Path(name).suffix.lower() in {".bash", ".fish", ".ksh", ".ps1", ".py", ".rb", ".sh", ".zsh"}


def raw_rust_tool_token(name: str) -> bool:
    if rust_tool_name_has_script_extension(name):
        return False
    return name in {"cargo", "clippy", "nextest", "rustc", "rustdoc"} or name.startswith(
        ("cargo-", "clippy-", "rust-")
    )


def path_name_looks_like_renamed_cargo(name: str) -> bool:
    return name == "c" or raw_rust_tool_token(name) or (name.endswith("cargo") and "_" not in name)


def path_executable_looks_like_cargo(token: str) -> bool:
    if "/" not in token:
        return False
    path = pathlib.Path(token)
    if path_name_looks_like_renamed_cargo(path.name):
        return True
    return False


def path_name_looks_like_renamed_rustc(name: str) -> bool:
    return name == "r" or name == "rustc" or (name.endswith("rustc") and "_" not in name)


def path_executable_looks_like_rustc(token: str) -> bool:
    if "/" not in token:
        return False
    path = pathlib.Path(token)
    if path_name_looks_like_renamed_rustc(path.name):
        return True
    return False


def path_invocation_has_cargo_subcommand(tokens: list[str]) -> bool:
    if not tokens:
        return False
    executable = pathlib.Path(tokens[0]).name
    if "/" in tokens[0]:
        if not path_executable_looks_like_cargo(tokens[0]):
            return False
    elif not path_name_looks_like_renamed_cargo(executable):
        return False
    command_index = consume_cargo_global_options(tokens, 1)
    return command_index < len(tokens) and tokens[command_index] in CARGO_PROCESS_SUBCOMMANDS


def path_invocation_may_have_cargo_subcommand(tokens: list[str]) -> bool:
    if not tokens:
        return False
    executable = pathlib.Path(tokens[0]).name
    if "/" not in tokens[0] and not path_name_looks_like_renamed_cargo(executable):
        return False
    command_index = consume_cargo_global_options(tokens, 1)
    return command_index < len(tokens) and tokens[command_index] in CARGO_PROCESS_SUBCOMMANDS


def shell_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    assignments: dict[str, str] = {}
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def export_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    if not tokens or pathlib.Path(tokens[0]).name != "export":
        return {}, 0
    assignments: dict[str, str] = {}
    cursor = 1
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "--" or token.startswith("-"):
            cursor += 1
            continue
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            if shell_name_word(token):
                cursor += 1
                continue
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def shell_declaration_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    if not tokens or pathlib.Path(tokens[0]).name not in {"declare", "local", "typeset"}:
        return {}, 0
    assignments: dict[str, str] = {}
    cursor = 1
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "--":
            cursor += 1
            continue
        if token.startswith("-") or token.startswith("+"):
            cursor += 1
            continue
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            if shell_name_word(token):
                cursor += 1
                continue
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def persistent_shell_assignment_values(tokens: list[str]) -> tuple[dict[str, str], bool]:
    assignments, assignment_index = shell_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = export_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = shell_declaration_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = shell_array_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    return {}, False


def shell_array_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    assignments: dict[str, str] = {}
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if not shell_assignment_word(token) or not token.endswith("="):
            break
        name, value = token.split("=", 1)
        if value:
            break
        cursor += 1
        if cursor >= len(tokens) or tokens[cursor] != "(":
            break
        cursor += 1
        depth = 1
        parts: list[str] = []
        while cursor < len(tokens) and depth:
            current = tokens[cursor]
            if current == "(":
                depth += 1
                parts.append(current)
            elif current == ")":
                depth -= 1
                if depth:
                    parts.append(current)
            else:
                parts.append(current)
            cursor += 1
        if depth:
            break
        assignments[name] = " ".join(parts)
    return assignments, cursor


def shell_variable_reference_token(token: str) -> str | None:
    clean = storage_strip_quotes(token)
    match = re.fullmatch(r"\$([A-Za-z_][A-Za-z0-9_]*)", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\[(?:@|\*)\]\}", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=].*)\}", clean)
    if match:
        return match.group(1)
    return None


def expand_known_shell_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in tokens:
        variable = shell_variable_reference_token(token)
        if variable is not None and variable in variables:
            expanded.extend(command_tokens(variables[variable]))
        else:
            expanded.append(token)
    return expanded


def shell_identifier_fragment(value: str) -> str | None:
    clean = storage_strip_quotes(value)
    return clean if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", clean) else None


def expand_known_shell_assignment_name(name: str, variables: dict[str, str]) -> str:
    def replace_reference(match: re.Match[str]) -> str:
        variable = match.group("bare") or match.group("braced")
        if variable is None or variable not in variables:
            return match.group(0)
        fragment = shell_identifier_fragment(variables[variable])
        return fragment if fragment is not None else match.group(0)

    return re.sub(
        r"\$(?P<bare>[A-Za-z_][A-Za-z0-9_]*)|\$\{(?P<braced>[A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=][^}]*)?\}",
        replace_reference,
        name,
    )


def expand_known_shell_assignment_value(value: str, variables: dict[str, str]) -> str:
    clean = storage_strip_quotes(value)

    def replace_reference(match: re.Match[str]) -> str:
        variable = match.group("bare") or match.group("braced")
        if variable is None or variable not in variables:
            return match.group(0)
        return variables[variable]

    return re.sub(
        r"\$(?P<bare>[A-Za-z_][A-Za-z0-9_]*)|\$\{(?P<braced>[A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=][^}]*)?\}",
        replace_reference,
        clean,
    )


def merge_split_shell_parameter_assignment_tokens(tokens: list[str]) -> list[str]:
    merged: list[str] = []
    index = 0
    while index < len(tokens):
        if tokens[index] == "$" and index + 3 < len(tokens) and tokens[index + 1] == "{":
            close = index + 2
            while close < len(tokens) and tokens[close] != "}":
                close += 1
            if close + 1 < len(tokens) and "=" in tokens[close + 1]:
                variable = "".join(tokens[index + 2 : close])
                merged.append("${" + variable + "}" + tokens[close + 1])
                index = close + 2
                continue
        merged.append(tokens[index])
        index += 1
    return merged


def expand_known_shell_assignment_names(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in merge_split_shell_parameter_assignment_tokens(tokens):
        if "=" not in token:
            expanded.append(token)
            continue
        name, value = token.split("=", 1)
        expanded_name = expand_known_shell_assignment_name(name, variables)
        if expanded_name != name and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", expanded_name):
            expanded.append(f"{expanded_name}={value}")
            continue
        expanded.append(token)
    return expanded


def expand_known_shell_command_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    if not tokens:
        return tokens
    executable = pathlib.Path(tokens[0]).name
    if executable == "eval":
        return [tokens[0], *expand_known_shell_variables(tokens[1:], variables)]
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        expanded = list(tokens)
        index = 1
        while index + 1 < len(expanded):
            token = expanded[index]
            if token == "-c" or (token.startswith("-") and not token.startswith("--") and "c" in token[1:]):
                variable = shell_variable_reference_token(expanded[index + 1])
                if variable is not None and variable in variables:
                    expanded[index + 1] = variables[variable]
                return expanded
            index += 1
        return expanded
    variable = shell_variable_reference_token(tokens[0])
    if variable is not None and variable in variables:
        return [*command_tokens(variables[variable]), *tokens[1:]]
    return tokens


def tokens_have_raw_cargo(
    tokens: list[str],
    *,
    depth: int = 0,
    allow_storage_only: bool = True,
    variables: dict[str, str] | None = None,
) -> bool:
    if not tokens:
        return False
    variables = variables or {}
    if variables:
        tokens = merge_split_shell_parameter_assignment_tokens(tokens)
        tokens = expand_known_shell_assignment_names(tokens, variables)
        tokens = expand_known_shell_command_variables(tokens, variables)
        if not tokens:
            return False
    if depth > 6:
        return True
    if allow_storage_only and tokens_have_target_routing_override(tokens):
        return True
    for payload in shell_command_substitution_payloads(tokens):
        if tokens_have_raw_cargo(
            payload,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        ):
            return True
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return False
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        cargo_aliases: set[str] = set()
        cargo_alias_payloads: dict[str, str] = {}
        shell_variables: dict[str, str] = dict(variables)
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    alias_payloads = shell_alias_payloads(segment)
                    for payload in alias_payloads.values():
                        if tokens_have_raw_cargo(
                            command_tokens(payload),
                            depth=depth + 1,
                            allow_storage_only=allow_storage_only,
                            variables=shell_variables,
                        ):
                            return True
                    cargo_alias_payloads.update(alias_payloads)
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                    segment = []
                    continue
                shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    shell_variables.update(shell_assignments)
                    segment = []
                    continue
                segment = expand_known_shell_assignment_names(segment, shell_variables)
                segment = expand_known_shell_command_variables(segment, shell_variables)
                if segment and segment[0] in cargo_alias_payloads:
                    alias_tokens = command_tokens(cargo_alias_payloads[segment[0]]) + segment[1:]
                    if tokens_have_raw_cargo(
                        alias_tokens,
                        depth=depth + 1,
                        allow_storage_only=allow_storage_only,
                        variables=shell_variables,
                    ):
                        return True
                segment = expand_cargo_aliases(segment, cargo_aliases)
                if segment and tokens_have_raw_cargo(
                    segment,
                    depth=depth + 1,
                    allow_storage_only=allow_storage_only,
                    variables=shell_variables,
                ):
                    return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            return any(
                tokens_have_raw_cargo(
                    command_tokens(payload),
                    depth=depth + 1,
                    allow_storage_only=allow_storage_only,
                    variables=shell_variables,
                )
                for payload in shell_alias_payloads(segment).values()
            )
        shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
        if is_persistent_assignment:
            return False
        segment = expand_known_shell_assignment_names(segment, shell_variables)
        segment = expand_known_shell_command_variables(segment, shell_variables)
        if segment and segment[0] in cargo_alias_payloads:
            alias_tokens = command_tokens(cargo_alias_payloads[segment[0]]) + segment[1:]
            return tokens_have_raw_cargo(
                alias_tokens,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=shell_variables,
            )
        segment = expand_cargo_aliases(segment, cargo_aliases)
        return bool(segment) and tokens_have_raw_cargo(
            segment,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=shell_variables,
        )
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        prefix_assignments, _assignment_cursor = shell_assignment_values_from_tokens(tokens[:assignment_index])
        local_variables = {**variables, **prefix_assignments}
        return assignment_index < len(tokens) and tokens_have_raw_cargo(
            tokens[assignment_index:],
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=local_variables,
        )
    if managed_rust_verification_tokens(tokens):
        return tokens_have_target_routing_override(tokens)
    executable = pathlib.Path(tokens[0]).name
    if path_invocation_has_cargo_subcommand(tokens):
        return True
    if path_executable_looks_like_rustc(tokens[0]) and any(
        token in {"--crate-name", "--emit", "--out-dir", "--artifact-dir"}
        or token.startswith(("--emit=", "--out-dir=", "--artifact-dir="))
        for token in tokens[1:]
    ):
        return True
    if path_name_looks_like_renamed_rustc(executable) and any(
        token in {"--crate-name", "--emit", "--out-dir", "--artifact-dir"}
        or token.startswith(("--emit=", "--out-dir=", "--artifact-dir="))
        for token in tokens[1:]
    ):
        return True
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        return nested is not None and tokens_have_raw_cargo(
            command_tokens(nested),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "eval":
        inner = tokens[1:]
        if inner and inner[0] == "--":
            inner = inner[1:]
        return bool(inner) and tokens_have_raw_cargo(
            command_tokens(" ".join(inner)),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "no-mistakes":
        inner = no_mistakes_inner_tokens(tokens)
        if inner is None:
            return False
        if inner and raw_rust_tool_token(pathlib.Path(inner[0]).name):
            return True
        return tokens_have_raw_cargo(
            inner,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "env":
        inner = env_inner_tokens(tokens)
        return inner is not None and tokens_have_raw_cargo(
            inner,
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable == "rustup" and len(tokens) >= 3 and tokens[1] == "run":
        return tokens_have_raw_cargo(
            rustup_run_inner_tokens(tokens),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
            variables=variables,
        )
    if executable.startswith("python"):
        return any(
            tokens_have_raw_cargo(
                command_tokens(payload),
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
            for payload in python_inline_command_payloads(tokens)
        )
    if executable == "flock":
        inner = flock_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(
                inner,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
    if executable == "find":
        return any(
            tokens_have_raw_cargo(
                payload,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
            for payload in find_exec_payloads(tokens)
        )
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(
                inner,
                depth=depth + 1,
                allow_storage_only=allow_storage_only,
                variables=variables,
            )
    for index, token in enumerate(tokens):
        name = pathlib.Path(token).name
        if name == "cargo" and cargo_token_is_command(tokens, index):
            return True
        if name in {"clippy", "nextest", "rustc", "rustdoc"} and command_prefix_allows_cargo(tokens[:index]):
            return True
        if name != "cargo" and raw_rust_tool_token(name) and command_prefix_allows_cargo(tokens[:index]):
            return True
    return False


def command_has_raw_cargo(command: str) -> bool:
    return tokens_have_raw_cargo(command_tokens(command))


def tokens_have_raw_cargo_launch(tokens: list[str], *, variables: dict[str, str] | None = None) -> bool:
    return tokens_have_raw_cargo(tokens, allow_storage_only=False, variables=variables)


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
        "managed-test": "cargo nextest run --locked {{args}}",
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
            and current_just_recipe in {"managed-build", "managed-clippy", "managed-test"}
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


def cargo_config_storage_override_message(tokens: list[str]) -> str | None:
    for index, token in enumerate(tokens):
        if token == "--config" and index + 1 < len(tokens) and cargo_config_has_storage_override(tokens[index + 1]):
            if cargo_config_looks_like_path(tokens[index + 1]):
                return "cargo --config file raw target override must be classified"
            scan_config = decode_toml_unicode_escapes(tokens[index + 1])
            if "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config):
                return "cargo --config build.rustflags raw output override must be classified"
            return "cargo --config build.target-dir raw target override must be classified"
        if token.startswith("--config="):
            config = token.split("=", 1)[1]
            if cargo_config_has_storage_override(config):
                if cargo_config_looks_like_path(config):
                    return "cargo --config file raw target override must be classified"
                scan_config = decode_toml_unicode_escapes(config)
                if "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config):
                    return "cargo --config build.rustflags raw output override must be classified"
                return "cargo --config build.target-dir raw target override must be classified"
    return None


def direct_raw_cargo_storage_override_messages(tokens: list[str]) -> set[str]:
    messages: set[str] = set()
    cargo_args = target_routing_cargo_args(tokens)
    cargo_scan_tokens = cargo_args_for_target_routing_scan(cargo_args) if cargo_args is not None else []
    cargo_command = cargo_subcommand(cargo_args) if cargo_args is not None else None
    if any(token == "--target-dir" or token.startswith("--target-dir=") for token in cargo_scan_tokens):
        messages.add("cargo --target-dir raw target override must be classified")
    if cargo_command == "rustc":
        if any(token == "--out-dir" or token.startswith("--out-dir=") for token in cargo_scan_tokens):
            messages.add("cargo rustc --out-dir raw output override must be classified")
        if any(token == "--artifact-dir" or token.startswith("--artifact-dir=") for token in cargo_scan_tokens):
            messages.add("cargo rustc --artifact-dir raw output override must be classified")
    if tokens and (
        pathlib.Path(tokens[0]).name == "rustc"
        or path_executable_looks_like_rustc(tokens[0])
        or path_name_looks_like_renamed_rustc(pathlib.Path(tokens[0]).name)
    ):
        if any(token == "--out-dir" or token.startswith("--out-dir=") for token in tokens):
            messages.add("rustc --out-dir raw output override must be classified")
        if any(token == "--artifact-dir" or token.startswith("--artifact-dir=") for token in tokens):
            messages.add("rustc --artifact-dir raw output override must be classified")
    config_message = cargo_config_storage_override_message(cargo_scan_tokens)
    if config_message is not None:
        messages.add(config_message)
    if cargo_command == "install":
        has_target_dir = any(token == "--target-dir" or token.startswith("--target-dir=") for token in cargo_scan_tokens)
        has_root = any(token == "--root" or token.startswith("--root=") for token in cargo_scan_tokens)
        if has_target_dir and has_root:
            messages.add("cargo install build target and install root ownership must be classified separately")
        if any(
            token == "--root"
            and index + 1 < len(cargo_scan_tokens)
            and cargo_scan_tokens[index + 1].startswith("s3://")
            for index, token in enumerate(cargo_scan_tokens)
        ):
            messages.add("cargo install S3 install root must be classified")
        if any(token.startswith("--root=s3://") for token in cargo_scan_tokens):
            messages.add("cargo install S3 install root must be classified")
    return messages


def raw_cargo_storage_override_messages_from_tokens(
    tokens: list[str],
    *,
    aliases: set[str] | None = None,
    variables: dict[str, str] | None = None,
    depth: int = 0,
) -> set[str]:
    if not tokens:
        return set()
    aliases = aliases or set()
    variables = variables or {}
    expanded = merge_split_shell_parameter_assignment_tokens(tokens)
    expanded = expand_known_shell_assignment_names(expanded, variables)
    expanded = expand_known_shell_command_variables(expanded, variables)
    expanded = expand_known_shell_variables(expanded, variables)
    expanded = expand_cargo_aliases(expanded, aliases)
    if not expanded:
        return set()
    if depth > 6:
        if tokens_have_raw_cargo_launch(expanded):
            return direct_raw_cargo_storage_override_messages(expanded)
        return set()
    messages: set[str] = set()
    if tokens_have_top_level_shell_boundary(expanded):
        segment: list[str] = []
        segment_aliases = set(aliases)
        segment_variables = dict(variables)
        substitution_depth = 0
        index = 0
        while index < len(expanded):
            token = expanded[index]
            if token == "$" and index + 1 < len(expanded) and expanded[index + 1] == "(":
                segment.extend([token, expanded[index + 1]])
                substitution_depth += 1
                index += 2
                continue
            if token == "(" and substitution_depth:
                substitution_depth += 1
            elif token == ")" and substitution_depth:
                substitution_depth -= 1
            elif token in SHELL_COMMAND_BOUNDARIES:
                shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    segment_variables.update(shell_assignments)
                    segment = []
                    index += 1
                    continue
                messages.update(
                    raw_cargo_storage_override_messages_from_tokens(
                        segment,
                        aliases=segment_aliases,
                        variables=segment_variables,
                        depth=depth + 1,
                    )
                )
                if segment and segment[0] == "alias":
                    segment_aliases.update(simple_cargo_aliases(segment, segment_aliases))
                segment = []
                index += 1
                continue
            segment.append(token)
            index += 1
        messages.update(
            raw_cargo_storage_override_messages_from_tokens(
                segment,
                aliases=segment_aliases,
                variables=segment_variables,
                depth=depth + 1,
            )
        )
        return messages
    if expanded[0] == "alias":
        return messages
    shell_assignments, assignment_index = shell_assignment_values_from_tokens(expanded)
    if assignment_index:
        local_variables = dict(variables)
        local_variables.update(shell_assignments)
        return raw_cargo_storage_override_messages_from_tokens(
            expanded[assignment_index:],
            aliases=aliases,
            variables=local_variables,
            depth=depth + 1,
        )
    executable = pathlib.Path(expanded[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(expanded)
        if nested is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(nested),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable == "eval":
        inner = expanded[1:]
        if inner and inner[0] == "--":
            inner = inner[1:]
        if inner:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(" ".join(inner)),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable.startswith("python"):
        for payload in python_inline_command_payloads(expanded):
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(payload),
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(expanded)
        if inner is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    inner,
                    aliases=aliases,
                    variables=variables,
                    depth=depth + 1,
                )
            )
        return messages
    if not tokens_have_raw_cargo_launch(expanded) and not tokens_have_target_routing_override(expanded):
        return messages
    messages.update(direct_raw_cargo_storage_override_messages(expanded))
    return messages


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


MERGIFY_REQUIRED_MERGE_CONDITIONS = frozenset(
    {
        "approved-reviews-by = sp-reviewer",
        "check-success = gate",
        "check-success = backtester-gate",
        "check-success = actionlint",
        "check-success = host-health",
    }
)


def top_level_yaml_block(config_text: str, key: str) -> str | None:
    lines = [strip_comment(line).rstrip() for line in config_text.splitlines()]
    for index, line in enumerate(lines):
        if re.fullmatch(rf"{re.escape(key)}\s*:\s*", line):
            block = [line]
            for nested in lines[index + 1 :]:
                if nested.strip() and not nested.startswith(" "):
                    break
                block.append(nested)
            return "\n".join(block)
    return None


def yaml_scalar_value(block_text: str, key: str) -> str | None:
    for line in block_text.splitlines():
        match = re.match(rf"^\s*{re.escape(key)}\s*:\s*(.*?)\s*$", line)
        if match is not None:
            return unquote_yaml_scalar(match.group(1))
    return None


def queue_rule_block(queue_rules_text: str, name: str) -> str | None:
    lines = queue_rules_text.splitlines()
    for index, line in enumerate(lines):
        match = re.match(r"^(\s*)-\s*name\s*:\s*(.*?)\s*$", line)
        if match is None or unquote_yaml_scalar(match.group(2)) != name:
            continue
        item_indent = len(match.group(1))
        block = [line]
        for nested in lines[index + 1 :]:
            if nested.strip():
                indent = len(nested) - len(nested.lstrip(" "))
                if indent == item_indent and nested.lstrip().startswith("- "):
                    break
            block.append(nested)
        return "\n".join(block)
    return None


def yaml_list_values(block_text: str, key: str) -> list[str] | None:
    lines = block_text.splitlines()
    for index, line in enumerate(lines):
        match = re.match(rf"^(\s*){re.escape(key)}\s*:\s*(.*?)\s*$", line)
        if match is None:
            continue
        scalar = unquote_yaml_scalar(match.group(2))
        if scalar == "[]":
            return []
        if scalar:
            return None
        parent_indent = len(match.group(1))
        values: list[str] = []
        for nested in lines[index + 1 :]:
            if not nested.strip():
                continue
            indent = len(nested) - len(nested.lstrip(" "))
            if indent <= parent_indent:
                break
            item_match = re.match(r"^\s*-\s*(.*?)\s*$", nested)
            if item_match is None:
                return None
            values.append(unquote_yaml_scalar(item_match.group(1)))
        return values
    return None


def verify_mergify_config(config_text: str, config_name: str = ".mergify.yml") -> list[str]:
    errors: list[str] = []
    uncommented = uncommented_text(config_text.splitlines())
    forbidden_keys = (
        "autoqueue",
        "auto_merge_conditions",
        "merge_protections",
        "merge_protections_settings",
        "pull_request_rules",
    )
    for key in forbidden_keys:
        if re.search(rf"(?m)^\s*{re.escape(key)}\s*:", uncommented):
            errors.append(f"{config_name} must keep manual queueing only; remove {key}")

    merge_queue = top_level_yaml_block(config_text, "merge_queue")
    if merge_queue is None:
        errors.append(f"{config_name} must define merge_queue")
    else:
        if yaml_scalar_value(merge_queue, "max_parallel_checks") != "1":
            errors.append(f"{config_name} merge_queue.max_parallel_checks must be 1")
        if yaml_scalar_value(merge_queue, "reset_on_external_merge") != "always":
            errors.append(f"{config_name} merge_queue.reset_on_external_merge must be always")

    queue_rules = top_level_yaml_block(config_text, "queue_rules")
    default_rule = queue_rule_block(queue_rules, "default") if queue_rules is not None else None
    if queue_rules is None:
        errors.append(f"{config_name} must define queue_rules")
    if default_rule is None:
        errors.append(f"{config_name} queue_rules must define default")
        return errors

    if yaml_list_values(default_rule, "queue_conditions") != []:
        errors.append(f"{config_name} default queue_conditions must be empty for manual entry")
    merge_conditions = yaml_list_values(default_rule, "merge_conditions")
    if merge_conditions is None:
        errors.append(f"{config_name} default merge_conditions must be a list")
    elif set(merge_conditions) != MERGIFY_REQUIRED_MERGE_CONDITIONS:
        errors.append(f"{config_name} default merge_conditions must require sp-reviewer and all four gates")

    expected_scalars = {
        "branch_protection_injection_mode": "merge",
        "batch_size": "1",
        "checks_timeout": "60 minutes",
        "draft_bot_account": "null",
        "merge_method": "squash",
    }
    for key, expected in expected_scalars.items():
        if yaml_scalar_value(default_rule, key) != expected:
            errors.append(f"{config_name} default {key} must be {expected}")
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
CI_LINT_WORKFLOW_INNER_REQUIRED_COMMANDS = (
    "python3 scripts/test_root_bin_sidecars.py",
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
        for required_command in CI_LINT_WORKFLOW_INNER_REQUIRED_COMMANDS:
            if not any(required_command in line for line in ci_lint_inner_lines):
                errors.append(f"justfile ci-lint-workflow-inner must run {required_command}")
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
    for command in (
        "python3 scripts/test_local_verification_gate.py",
        "python3 scripts/test_lane_governor.py",
        "python3 scripts/test_verify_lane_governance.py",
        "python3 scripts/verify_lane_governance.py",
    ):
        if command not in static_lines:
            errors.append(f"justfile source-fence-static must run {command}")
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
        if any(cache_input not in clean for cache_input in EXACT_HEAD_GOVERNANCE_CACHE_INPUTS):
            return ["cache keys must include exact-head CI/no-mistakes governance inputs"]
    return []


def text_has_path_style_cargo_config(text: str) -> bool:
    for match in re.finditer(r"\bcargo\b[^\n;&|]*", text):
        tokens = command_tokens(match.group(0))
        for index, token in enumerate(tokens):
            if pathlib.Path(token).name != "cargo":
                continue
            cursor = index + 1
            while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
                option = tokens[cursor]
                if option == "--config" and cursor + 1 < len(tokens):
                    if cargo_config_looks_like_path(tokens[cursor + 1]):
                        return True
                    cursor += 2
                    continue
                if option.startswith("--config=") and cargo_config_looks_like_path(option.split("=", 1)[1]):
                    return True
                cursor += 1
    return False


STORAGE_ROLE_S3 = "s3"
STORAGE_ROLE_ACTIVE_TARGET = "active_target"
AWS_S3_TRANSFER_COMMANDS = {"cp", "mv", "sync"}
ACTIVE_TARGET_STDOUT_COMMANDS = {
    "awk",
    "base64",
    "bzcat",
    "cat",
    "egrep",
    "fgrep",
    "grep",
    "gzip",
    "head",
    "sed",
    "tail",
    "tar",
    "xzcat",
    "zcat",
}
AWS_S3_OPTIONS_WITH_ARGUMENT = {
    "--acl",
    "--cache-control",
    "--content-disposition",
    "--content-encoding",
    "--content-language",
    "--content-type",
    "--copy-props",
    "--exclude",
    "--expires",
    "--expected-size",
    "--include",
    "--metadata",
    "--metadata-directive",
    "--page-size",
    "--profile",
    "--region",
    "--request-payer",
    "--sse",
    "--sse-c",
    "--sse-c-copy-source",
    "--sse-c-copy-source-key",
    "--sse-c-key",
    "--sse-kms-key-id",
    "--storage-class",
    "--website-redirect",
}


def storage_strip_quotes(value: str) -> str:
    return value.strip().strip("\"'")


def storage_without_trailing_current_dir(value: str) -> str:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "")
    while normalized.endswith("/.") or normalized.endswith("/"):
        normalized = normalized[:-2] if normalized.endswith("/.") else normalized[:-1]
    return normalized


def storage_variable_names(value: str) -> set[str]:
    names = {match.group(1) for match in re.finditer(r"\$([A-Za-z_][A-Za-z0-9_]*)\b", value)}
    names.update(match.group(1) for match in re.finditer(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:[^}]*)\}", value))
    names.update(match.group(1) for match in re.finditer(r"\$\{\{\s*env\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}", value))
    return names


def storage_command_substitution_has_target(value: str) -> bool:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    for payload in shell_command_substitution_payloads(command_tokens(compact)):
        if any(storage_value_has_target_component(token) for token in payload):
            return True
    if ("`" in compact or "$" in compact) and storage_value_has_target_component(storage_value_without_substitutions(compact)):
        return True
    return False


def storage_value_without_substitutions(value: str) -> str:
    compact = re.sub(r"`[^`]*`", "", value)
    tokens = command_tokens(compact)
    output: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(tokens) and tokens[index + 1] == "(":
            prefix = token[:-1] if token != "$" and token.endswith("$") else ""
            if prefix:
                output.append(prefix)
            index += 2
            depth = 1
            while index < len(tokens) and depth:
                if tokens[index] == "(":
                    depth += 1
                elif tokens[index] == ")":
                    depth -= 1
                index += 1
            continue
        output.append(token)
        index += 1
    return " ".join(output)


def storage_value_has_target_component(value: str) -> bool:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "").lstrip("<>")
    if not normalized or normalized.startswith("s3://"):
        return False
    parts = [part for part in re.split(r"[\\/]+", normalized) if part and part not in {".", ".."}]
    return "target" in parts


def storage_value_roles(
    value: str,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool = False,
    active_paths: set[str] | None = None,
) -> set[str]:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    root_compact = storage_without_trailing_current_dir(value)
    roles: set[str] = set()
    if active_paths is not None and storage_path_is_inside_active_path(root_compact, active_paths):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if "s3://" in compact:
        roles.add(STORAGE_ROLE_S3)
    if "rust_verification.py" in compact and "target-dir" in compact:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_command_substitution_has_target(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    for payload in shell_command_substitution_payloads(command_tokens(compact)):
        if any(storage_value_has_target_component(token) for token in payload):
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    for variable in storage_variable_names(compact):
        if variable in {"CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_TARGET_TMPDIR"}:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        if variable in {"GITHUB_WORKSPACE", "PWD"} and root_compact in {
            f"${variable}",
            f"${{{variable}}}",
        }:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        roles.update(variable_roles.get(variable, set()))
    if re.search(r"\$\{\{\s*(?:env\.CARGO_TARGET_DIR|steps\.setup\.outputs\.managed_target_dir(?:_relative)?)\s*\}\}", compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if re.search(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact) and (
        re.fullmatch(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact.strip()) is not None
        or storage_value_has_target_component(compact)
    ):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if root_compact in {".", "*", "$PWD", "${PWD}", "$GITHUB_WORKSPACE", "${GITHUB_WORKSPACE}"}:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_value_has_target_component(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if cwd_is_active_target and compact and not compact.startswith("-") and STORAGE_ROLE_S3 not in roles:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    return roles


def storage_path_key(value: str) -> str:
    return storage_without_trailing_current_dir(value).replace('"', "").replace("'", "")


def storage_path_is_inside_active_path(value: str, active_paths: set[str]) -> bool:
    key = storage_path_key(value)
    return any(key == active_path or key.startswith(f"{active_path}/") for active_path in active_paths if active_path)


def command_tail_until_boundary(tokens: list[str], start: int) -> list[str]:
    tail: list[str] = []
    cursor = start
    substitution_depth = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "$" and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            tail.extend([token, tokens[cursor + 1]])
            substitution_depth += 1
            cursor += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            break
        tail.append(token)
        cursor += 1
    return tail


def command_operand_roles(
    operand: str,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
) -> set[str]:
    return storage_value_roles(
        operand,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def operand_has_s3_path_role(operand: str, s3_paths: set[str]) -> bool:
    return storage_path_is_inside_active_path(storage_without_trailing_current_dir(operand), s3_paths)


def local_transfer_operands(tokens: list[str], index: int) -> tuple[list[str], str] | None:
    tail = command_tail_until_boundary(tokens, index + 1)
    operands: list[str] = []
    target_directory: str | None = None
    cluster_prefix_flags_without_argument = {"a", "d", "f", "H", "i", "L", "l", "n", "P", "p", "R", "r", "s", "u", "v", "x", "Z"}
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token == "--":
            cursor += 1
            continue
        if token in {"-t", "--target-directory"} and cursor + 1 < len(tail):
            target_directory = tail[cursor + 1]
            cursor += 2
            continue
        if token.startswith("-t") and not token.startswith("--") and len(token) > 2:
            target_directory = token[2:]
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--") and "t" in token[1:]:
            prefix, suffix = token[1:].split("t", 1)
            if set(prefix) <= cluster_prefix_flags_without_argument:
                if suffix:
                    target_directory = suffix
                    cursor += 1
                elif cursor + 1 < len(tail):
                    target_directory = tail[cursor + 1]
                    cursor += 2
                else:
                    cursor += 1
                continue
        if token.startswith("--target-directory="):
            target_directory = token.split("=", 1)[1]
            cursor += 1
            continue
        if token.startswith("-"):
            cursor += 1
            continue
        operands.append(token)
        cursor += 1
    if target_directory is not None:
        return (operands, target_directory) if operands else None
    if len(operands) < 2:
        return None
    return operands[:-1], operands[-1]


def command_copies_s3_path_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    operands = local_transfer_operands(tokens, index)
    if operands is None:
        return False
    sources, destination = operands
    if not any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        return False
    return STORAGE_ROLE_ACTIVE_TARGET in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def record_local_transfer_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    operands = local_transfer_operands(tokens, index)
    if operands is None:
        return
    sources, destination = operands
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(destination))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(destination))


TAR_SHORT_OPTION_CLUSTER_FLAGS = set("AacdtruxvzjJfCOpPsSMWUmhk")
TAR_SHORT_OPTIONS_WITH_ARGUMENT = {"C", "f"}


def tar_cluster_looks_like_options(cluster: str) -> bool:
    return bool(cluster) and set(cluster) <= TAR_SHORT_OPTION_CLUSTER_FLAGS


def tar_option_parts(token: str, tail: list[str], index: int) -> tuple[set[str], dict[str, str], int, bool]:
    flags: set[str] = set()
    arguments: dict[str, str] = {}
    consumed = 0
    if token == "--":
        return flags, arguments, consumed, True
    if token in {"c", "-c", "--create"}:
        flags.add("c")
        return flags, arguments, consumed, True
    if token in {"x", "-x", "--extract", "--get"}:
        flags.add("x")
        return flags, arguments, consumed, True
    if token in {"-f", "--file"}:
        if index + 1 < len(tail):
            arguments["f"] = tail[index + 1]
            consumed = 1
        return flags, arguments, consumed, True
    if token.startswith("--file="):
        arguments["f"] = token.split("=", 1)[1]
        return flags, arguments, consumed, True
    if token in {"-C", "--directory"}:
        if index + 1 < len(tail):
            arguments["C"] = tail[index + 1]
            consumed = 1
        return flags, arguments, consumed, True
    if token.startswith("--directory="):
        arguments["C"] = token.split("=", 1)[1]
        return flags, arguments, consumed, True
    if token.startswith("--"):
        return flags, arguments, consumed, True

    traditional_cluster = False
    cluster: str | None = None
    if token.startswith("-") and len(token) > 1:
        cluster = token[1:]
    elif tar_cluster_looks_like_options(token):
        cluster = token
        traditional_cluster = True
    if cluster is None:
        return flags, arguments, consumed, False

    argument_offset = 1
    position = 0
    while position < len(cluster):
        flag = cluster[position]
        if flag == "c":
            flags.add("c")
        elif flag == "x":
            flags.add("x")
        if flag in TAR_SHORT_OPTIONS_WITH_ARGUMENT:
            suffix = cluster[position + 1 :]
            if suffix and not (traditional_cluster or tar_cluster_looks_like_options(suffix)):
                arguments[flag] = suffix
                break
            if index + argument_offset < len(tail):
                arguments[flag] = tail[index + argument_offset]
                consumed = max(consumed, argument_offset)
                argument_offset += 1
            position += 1
            continue
        position += 1
    return flags, arguments, consumed, True


def tar_writes_archive_to_stdout(tail: list[str]) -> bool:
    creates_archive = False
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        flags, arguments, consumed, _option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "c" in flags:
            creates_archive = True
        if "f" in arguments:
            return arguments["f"] == "-"
    return creates_archive


def tar_archive_creation(tail: list[str]) -> tuple[str | None, list[str]]:
    creates_archive = False
    archive: str | None = None
    sources: list[str] = []
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        flags, arguments, consumed, option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "c" in flags:
            creates_archive = True
        if "f" in arguments:
            archive = arguments["f"]
        if option_like:
            continue
        sources.append(token)
    return (archive, sources) if creates_archive else (None, [])


def tar_archive_inputs(tail: list[str]) -> list[str]:
    archives: list[str] = []
    skip_count = 0
    for index, token in enumerate(tail):
        if skip_count:
            skip_count -= 1
            continue
        _flags, arguments, consumed, option_like = tar_option_parts(token, tail, index)
        skip_count = consumed
        if "f" in arguments and arguments["f"] != "-":
            archives.append(arguments["f"])
            continue
        if option_like:
            continue
        archives.append(token)
    return archives


def record_tar_archive_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    archive, sources = tar_archive_creation(command_tail_until_boundary(tokens, index + 1))
    if archive is None or archive == "-":
        return
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(archive))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(archive))


def tar_extracts_s3_archive_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    if not tar_extracts_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return False
    return any(operand_has_s3_path_role(archive, s3_paths) for archive in tar_archive_inputs(tail))


def zip_archive_operands(tokens: list[str], index: int) -> tuple[str, list[str]] | None:
    tail = command_tail_until_boundary(tokens, index + 1)
    operands: list[str] = []
    options_with_argument = {
        "-b",
        "-i",
        "-n",
        "-O",
        "-P",
        "-t",
        "-x",
        "--before-date",
        "--exclude",
        "--from-date",
        "--include",
        "--out",
        "--output-file",
        "--password",
        "--suffixes",
        "--temp-path",
    }
    short_options_with_argument = {"b", "i", "n", "O", "P", "t", "x"}
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token == "--":
            cursor += 1
            continue
        if token in options_with_argument and cursor + 1 < len(tail):
            cursor += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            argument_consumed = False
            for position, flag in enumerate(cluster):
                if flag not in short_options_with_argument:
                    continue
                if position + 1 < len(cluster):
                    cursor += 1
                elif cursor + 1 < len(tail):
                    cursor += 2
                else:
                    cursor += 1
                argument_consumed = True
                break
            if argument_consumed:
                continue
        if token.startswith("-"):
            cursor += 1
            continue
        operands.append(token)
        cursor += 1
    if len(operands) < 2:
        return None
    return operands[0], operands[1:]


def record_zip_archive_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    operands = zip_archive_operands(tokens, index)
    if operands is None:
        return
    archive, sources = operands
    if any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        active_paths.add(storage_path_key(archive))
    if any(operand_has_s3_path_role(source, s3_paths) for source in sources):
        s3_paths.add(storage_path_key(archive))


def unzip_extracts_s3_archive_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    archives: list[str] = []
    members: list[str] = []
    destination: str | None = None
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token in {"-d", "--directory"} and cursor + 1 < len(tail):
            destination = tail[cursor + 1]
            cursor += 2
            continue
        if token.startswith("--directory="):
            destination = token.split("=", 1)[1]
            cursor += 1
            continue
        if token in {"-x", "--exclude", "-P", "--password"} and cursor + 1 < len(tail):
            cursor += 2
            continue
        if token.startswith(("--exclude=", "--password=")):
            cursor += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            cluster = token[1:]
            argument_consumed = False
            for position, flag in enumerate(cluster):
                if flag == "d":
                    if position + 1 < len(cluster):
                        destination = cluster[position + 1 :]
                        cursor += 1
                    elif cursor + 1 < len(tail):
                        destination = tail[cursor + 1]
                        cursor += 2
                    else:
                        cursor += 1
                    argument_consumed = True
                    break
                if flag in {"x", "P"}:
                    if position + 1 < len(cluster):
                        cursor += 1
                    elif cursor + 1 < len(tail):
                        cursor += 2
                    else:
                        cursor += 1
                    argument_consumed = True
                    break
            if argument_consumed:
                continue
        if token == "--" or token.startswith("-"):
            cursor += 1
            continue
        if archives:
            members.append(token)
        else:
            archives.append(token)
        cursor += 1
    if not any(operand_has_s3_path_role(archive, s3_paths) for archive in archives):
        return False
    if cwd_is_active_target and destination is None:
        return True
    destination_is_active = destination is not None and STORAGE_ROLE_ACTIVE_TARGET in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )
    return destination_is_active or any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            member,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for member in members
    )


def command_streams_active_target_to_stdout(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
    command_name: str,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    if command_name == "tar" and not tar_writes_archive_to_stdout(tail):
        return False
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in command_operand_roles(
            token,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for token in tail
        if token != "-" and not token.startswith("-")
    )


def output_redirection_targets(tokens: list[str], index: int) -> list[str]:
    targets: list[str] = []
    tail = command_tail_until_boundary(tokens, index + 1)
    cursor = 0
    while cursor < len(tail):
        token = tail[cursor]
        if token in {">", ">>", "<>", ">|", ">&", "&>", "&>>"}:
            if cursor + 1 < len(tail):
                targets.append(tail[cursor + 1])
            cursor += 2
            continue
        match = re.match(r"^(?:\d?(?:>>?|<>|>\||>&)|&>>?)(.+)$", token)
        if match is not None:
            targets.append(match.group(1))
        cursor += 1
    return targets


def command_output_redirects_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            target,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for target in output_redirection_targets(tokens, index)
    )


def tar_extracts_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> bool:
    tail = command_tail_until_boundary(tokens, index + 1)
    extracts = False
    directories: list[str] = []
    members: list[str] = []
    skip_count = 0
    cursor = 0
    while cursor < len(tail):
        if skip_count:
            skip_count -= 1
            cursor += 1
            continue
        token = tail[cursor]
        flags, arguments, consumed, option_like = tar_option_parts(token, tail, cursor)
        skip_count = consumed
        if "x" in flags:
            extracts = True
        if "C" in arguments:
            directories.append(arguments["C"])
        if option_like:
            cursor += 1
            continue
        if token != "--":
            members.append(token)
        cursor += 1
    if not extracts:
        return False
    if cwd_is_active_target:
        return True
    return any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            directory,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for directory in directories
    ) or any(
        STORAGE_ROLE_ACTIVE_TARGET
        in storage_value_roles(
            member,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for member in members
    )


def command_writes_s3_stdin_to_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    cwd_is_active_target: bool,
    command_name: str,
) -> bool:
    if command_output_redirects_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return True
    if command_name == "tar" and tar_extracts_to_active_target(
        tokens,
        index,
        variable_roles,
        active_paths,
        cwd_is_active_target=cwd_is_active_target,
    ):
        return True
    if command_name == "tee":
        return any(
            STORAGE_ROLE_ACTIVE_TARGET
            in storage_value_roles(
                token,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            for token in command_tail_until_boundary(tokens, index + 1)
            if token != "-" and not token.startswith("-")
        )
    return False


def shell_assignment_from_tokens(tokens: list[str], index: int) -> tuple[str, str, int] | None:
    if index >= len(tokens) or not shell_assignment_word(tokens[index]):
        return None
    name, value = tokens[index].split("=", 1)
    cursor = index + 1
    if value == "$" and cursor < len(tokens) and tokens[cursor] == "(":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "(":
                depth += 1
            elif token == ")":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value == "$" and cursor < len(tokens) and tokens[cursor] == "{":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "{":
                depth += 1
            elif token == "}":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value.startswith("`") and not value.endswith("`"):
        parts = [value]
        while cursor < len(tokens):
            token = tokens[cursor]
            parts.append(token)
            cursor += 1
            if token.endswith("`"):
                break
        value = " ".join(parts)
    return name, value, cursor


def storage_assignment_values(text: str) -> list[tuple[str, str]]:
    assignments: list[tuple[str, str]] = []
    tokens = command_tokens(text)
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            cursor += 1
            continue
        name, value, cursor = assignment
        assignments.append((name, value))
    for line in text.splitlines():
        clean = strip_comment(line).strip()
        match = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+?)\s*$", clean)
        if match:
            assignments.append((match.group(1), match.group(2)))
    for github_env_assignment in github_env_assignment_lines(text):
        name, value = github_env_assignment.split("=", 1)
        assignments.append((name, value))
    return assignments


def storage_variable_roles(text: str) -> dict[str, set[str]]:
    assignments = storage_assignment_values(text)
    roles: dict[str, set[str]] = {}
    for _ in range(max(1, len(assignments))):
        changed = False
        for name, value in assignments:
            new_roles = storage_value_roles(value, roles)
            if new_roles and not new_roles.issubset(roles.get(name, set())):
                roles.setdefault(name, set()).update(new_roles)
                changed = True
        if not changed:
            break
    return roles


def consume_storage_option(tokens: list[str], index: int, options_with_argument: set[str]) -> int:
    token = tokens[index]
    if token in options_with_argument and index + 1 < len(tokens):
        return index + 2
    return index + 1


def aws_service_index(tokens: list[str], start: int) -> int | None:
    cursor = start + 1
    while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[cursor]
        if token in {"s3", "s3api"}:
            return cursor
        if token.startswith("-"):
            if (
                "=" not in token
                and cursor + 1 < len(tokens)
                and tokens[cursor + 1] not in {"s3", "s3api"}
                and not tokens[cursor + 1].startswith("-")
            ):
                cursor += 2
            else:
                cursor += 1
            continue
        cursor += 1
    return None


def aws_s3_operands(tokens: list[str]) -> list[str]:
    operands: list[str] = []
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            break
        if token == "-":
            operands.append(token)
            cursor += 1
            continue
        if token.startswith("-"):
            cursor = consume_storage_option(tokens, cursor, AWS_S3_OPTIONS_WITH_ARGUMENT)
            continue
        if "`" in token:
            parts = [token]
            cursor += 1
            backtick_count = token.count("`")
            while cursor < len(tokens) and backtick_count % 2 == 1:
                parts.append(tokens[cursor])
                backtick_count += tokens[cursor].count("`")
                cursor += 1
            operands.append(" ".join(parts))
            continue
        if (token == "$" or token.endswith("$")) and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            depth = 1
            parts = [token, tokens[cursor + 1]]
            substitution_tokens: list[str] = []
            cursor += 2
            while cursor < len(tokens) and depth:
                current = tokens[cursor]
                parts.append(current)
                if current != ")" or depth > 1:
                    substitution_tokens.append(current)
                if current == "(":
                    depth += 1
                elif current == ")":
                    depth -= 1
                cursor += 1
            if (
                cursor < len(tokens)
                and not any(part in SHELL_COMMAND_BOUNDARIES for part in substitution_tokens)
                and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES
                and not tokens[cursor].startswith("-")
                and not tokens[cursor].startswith("s3://")
            ):
                parts.append(tokens[cursor])
                cursor += 1
            operands.append(" ".join(parts))
            continue
        operands.append(token)
        cursor += 1
    return operands


def aws_s3_transfer_touches_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
    stdin_is_active_target: bool = False,
) -> bool:
    service_index = aws_service_index(tokens, index)
    if service_index is None:
        return False
    service = tokens[service_index]
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] in SHELL_COMMAND_BOUNDARIES:
        return False
    operation = tokens[op_index]
    tail: list[str] = []
    cursor = op_index + 1
    command_substitution_depth = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if (token == "$" or token.endswith("$")) and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            tail.extend([token, tokens[cursor + 1]])
            command_substitution_depth += 1
            cursor += 2
            continue
        if token == "(" and command_substitution_depth:
            command_substitution_depth += 1
        elif token == ")" and command_substitution_depth:
            command_substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not command_substitution_depth:
            break
        tail.append(token)
        cursor += 1
    if service == "s3api":
        return any(
            STORAGE_ROLE_ACTIVE_TARGET
            in storage_value_roles(
                token,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            for token in tail
        )
    if operation not in AWS_S3_TRANSFER_COMMANDS:
        return False
    operands = aws_s3_operands(tail)
    if len(operands) < 2:
        return False
    if stdin_is_active_target and operation == "cp" and "-" in operands:
        return True
    endpoint_roles = [
        storage_value_roles(
            endpoint,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for endpoint in operands
    ]
    return any(STORAGE_ROLE_ACTIVE_TARGET in roles for roles in endpoint_roles)


def aws_s3_transfer_streams_s3_to_stdout(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
    active_paths: set[str],
) -> bool:
    service_index = aws_service_index(tokens, index)
    if service_index is None or tokens[service_index] != "s3":
        return False
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] != "cp":
        return False
    operands = aws_s3_operands(command_tail_until_boundary(tokens, op_index + 1))
    if len(operands) < 2:
        return False
    source = operands[0]
    destination = operands[1]
    if destination != "-":
        return False
    return STORAGE_ROLE_S3 in storage_value_roles(
        source,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    )


def record_aws_s3_download_paths(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    s3_paths: set[str],
    *,
    cwd_is_active_target: bool,
) -> None:
    service_index = aws_service_index(tokens, index)
    if service_index is None or tokens[service_index] != "s3":
        return
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] in SHELL_COMMAND_BOUNDARIES:
        return
    operation = tokens[op_index]
    if operation not in {"cp", "mv", "sync"}:
        return
    operands = aws_s3_operands(command_tail_until_boundary(tokens, op_index + 1))
    if len(operands) < 2:
        return
    sources = operands[:-1]
    destination = operands[-1]
    if destination == "-" or STORAGE_ROLE_S3 in storage_value_roles(
        destination,
        variable_roles,
        cwd_is_active_target=cwd_is_active_target,
        active_paths=active_paths,
    ):
        return
    if any(
        STORAGE_ROLE_S3
        in storage_value_roles(
            source,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        )
        for source in sources
    ):
        s3_paths.add(storage_path_key(destination))


def command_prefix_before_token(tokens: list[str], index: int) -> list[str]:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        cursor -= 1
    return tokens[cursor + 1 : index]


def env_chdir_value(tokens: list[str]) -> str | None:
    command_index = env_command_prefix_index(tokens, 1)
    if command_index is None:
        return None
    index = 1
    while index < command_index:
        token = tokens[index]
        if token in ("-C", "--chdir") and index + 1 < command_index:
            return tokens[index + 1]
        if token.startswith("--chdir="):
            return token.split("=", 1)[1]
        if token.startswith("-") and not token.startswith("--") and "C" in token[1:]:
            offset = 1
            while offset < len(token):
                option = token[offset]
                if option in "0iv":
                    offset += 1
                    continue
                if option == "C":
                    suffix = token[offset + 1 :]
                    if suffix:
                        return suffix
                    if index + 1 < command_index:
                        return tokens[index + 1]
                    break
                if option in "Su":
                    index += 1 if offset + 1 < len(token) or index + 1 >= command_index else 2
                    break
                break
            else:
                index += 1
                continue
            if index >= command_index or token[offset] not in "Su":
                index += 1
            continue
        if token in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < command_index:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        index += 1
    return None


def sudo_chdir_value(tokens: list[str]) -> str | None:
    command_index = consume_option_prefix(
        tokens,
        1,
        SUDO_OPTIONS_WITH_ARGUMENT,
        SUDO_OPTIONS_WITHOUT_ARGUMENT,
        SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
    )
    if command_index is None:
        return None
    index = 1
    short_options_with_argument = {option[1] for option in SUDO_OPTIONS_WITH_ARGUMENT if re.match(r"^-[A-Za-z0-9]$", option)}
    short_options_without_argument = {option[1] for option in SUDO_OPTIONS_WITHOUT_ARGUMENT if re.match(r"^-[A-Za-z0-9]$", option)}
    while index < command_index:
        token = tokens[index]
        if token in ("-D", "--chdir") and index + 1 < command_index:
            return tokens[index + 1]
        if token.startswith("--chdir="):
            return token.split("=", 1)[1]
        if token.startswith("-") and not token.startswith("--") and "D" in token[1:]:
            offset = 1
            while offset < len(token):
                option = token[offset]
                if option in short_options_without_argument:
                    offset += 1
                    continue
                if option == "D":
                    suffix = token[offset + 1 :]
                    if suffix:
                        return suffix
                    if index + 1 < command_index:
                        return tokens[index + 1]
                    break
                if option in short_options_with_argument:
                    index += 1 if offset + 1 < len(token) or index + 1 >= command_index else 2
                    break
                break
            else:
                index += 1
                continue
            if index >= command_index or token[offset] not in short_options_with_argument - {"D"}:
                index += 1
            continue
        if token in SUDO_OPTIONS_WITH_ARGUMENT and index + 1 < command_index:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in SUDO_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        index += 1
    return None


def directory_wrapper_chdir_value(tokens: list[str]) -> str | None:
    if not tokens:
        return None
    executable = executable_name(tokens[0])
    if executable == "env":
        return env_chdir_value(tokens)
    if executable == "sudo":
        return sudo_chdir_value(tokens)
    return None


def cd_option_token(token: str) -> bool:
    if token in {"-L", "-P", "-e"}:
        return True
    return token.startswith("-") and not token.startswith("--") and len(token) > 1 and set(token[1:]) <= {"L", "P", "e"}


def shell_directory_change_target(tokens: list[str], cursor: int) -> tuple[str | None, int]:
    if cursor >= len(tokens):
        return None, cursor + 1
    name = executable_name(tokens[cursor])
    index = cursor + 1
    while name == "cd" and index < len(tokens) and cd_option_token(tokens[index]):
        index += 1
    while name == "pushd" and index < len(tokens) and tokens[index] == "-n":
        index += 1
    if index < len(tokens) and tokens[index] == "--":
        index += 1
    if index >= len(tokens) or tokens[index] in SHELL_COMMAND_BOUNDARIES:
        return None, index
    return tokens[index], index + 1


def shell_group_end_index(tokens: list[str], cursor: int) -> int | None:
    opener = tokens[cursor]
    closer = "}" if opener == "{" else ")"
    depth = 1
    index = cursor + 1
    while index < len(tokens):
        token = tokens[index]
        if token == opener:
            depth += 1
        elif token == closer:
            depth -= 1
            if depth == 0:
                return index
        index += 1
    return None


def skip_shell_redirections(tokens: list[str], cursor: int) -> int:
    while cursor < len(tokens):
        next_cursor = shell_redirection_next_index(tokens, cursor)
        if next_cursor is None:
            break
        cursor = next_cursor
    return cursor


def storage_stdout_roles_from_tokens(
    tokens: list[str],
    variable_roles: dict[str, set[str]],
    active_paths: set[str],
    *,
    depth: int,
    initial_cwd_is_active_target: bool,
) -> set[str]:
    if depth > 6:
        return set()
    roles: set[str] = set()
    cursor = 0
    cwd_is_active_target = initial_cwd_is_active_target
    pipe_stdin_is_active_target = False
    pipe_stdin_is_s3 = False
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is not None:
            cursor = assignment[2]
            continue
        token = tokens[cursor]
        if token in {"{", "("}:
            close_index = shell_group_end_index(tokens, cursor)
            if close_index is None:
                cursor += 1
                continue
            inner_roles = storage_stdout_roles_from_tokens(
                tokens[cursor + 1 : close_index],
                variable_roles,
                active_paths,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
            )
            roles.update(inner_roles)
            cursor = skip_shell_redirections(tokens, close_index + 1)
            continue
        if token in SHELL_COMMAND_BOUNDARIES:
            if token == "|":
                pipe_stdin_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in roles
                pipe_stdin_is_s3 = STORAGE_ROLE_S3 in roles
            else:
                pipe_stdin_is_active_target = False
                pipe_stdin_is_s3 = False
            cursor += 1
            continue
        name = executable_name(token)
        if name in {"cd", "pushd"}:
            directory_target, next_cursor = shell_directory_change_target(tokens, cursor)
            if directory_target is None:
                if name == "cd":
                    cwd_is_active_target = False
                cursor = next_cursor
                continue
            target_roles = storage_value_roles(
                directory_target,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            cwd_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in target_roles
            cursor = next_cursor
            continue
        if name in ACTIVE_TARGET_STDOUT_COMMANDS and (
            pipe_stdin_is_active_target
            or command_streams_active_target_to_stdout(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
                command_name=name,
            )
        ):
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        elif pipe_stdin_is_active_target and name != "aws":
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        if name == "aws" and aws_s3_transfer_streams_s3_to_stdout(
            tokens,
            cursor,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
        ):
            roles.add(STORAGE_ROLE_S3)
        elif pipe_stdin_is_s3:
            roles.add(STORAGE_ROLE_S3)
        cursor += 1
    return roles


def storage_transfer_policy_errors_from_tokens(
    tokens: list[str],
    variable_roles: dict[str, set[str]],
    *,
    depth: int = 0,
    initial_cwd_is_active_target: bool = False,
    initial_active_paths: set[str] | None = None,
    initial_s3_paths: set[str] | None = None,
    initial_pipe_stdin_is_active_target: bool = False,
    initial_pipe_stdin_is_s3: bool = False,
) -> list[str]:
    if depth > 6:
        return []
    cursor = 0
    cwd_is_active_target = initial_cwd_is_active_target
    active_paths: set[str] = set(initial_active_paths or set())
    s3_paths: set[str] = set(initial_s3_paths or set())
    pipe_stdout_is_active_target = False
    pipe_stdin_is_active_target = initial_pipe_stdin_is_active_target
    pipe_stdout_is_s3 = False
    pipe_stdin_is_s3 = initial_pipe_stdin_is_s3
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is not None:
            cursor = assignment[2]
            continue
        token = tokens[cursor]
        if token in {"{", "("}:
            close_index = shell_group_end_index(tokens, cursor)
            if close_index is None:
                cursor += 1
                continue
            inner_tokens = tokens[cursor + 1 : close_index]
            nested_errors = storage_transfer_policy_errors_from_tokens(
                inner_tokens,
                variable_roles,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
                initial_active_paths=active_paths,
                initial_s3_paths=s3_paths,
                initial_pipe_stdin_is_active_target=pipe_stdin_is_active_target,
                initial_pipe_stdin_is_s3=pipe_stdin_is_s3,
            )
            if nested_errors:
                return nested_errors
            group_stdout_roles = storage_stdout_roles_from_tokens(
                inner_tokens,
                variable_roles,
                active_paths,
                depth=depth + 1,
                initial_cwd_is_active_target=cwd_is_active_target,
            )
            if STORAGE_ROLE_S3 in group_stdout_roles and command_output_redirects_to_active_target(
                tokens,
                close_index,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            pipe_stdout_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in group_stdout_roles
            pipe_stdout_is_s3 = STORAGE_ROLE_S3 in group_stdout_roles
            cursor = skip_shell_redirections(tokens, close_index + 1)
            continue
        if token in SHELL_COMMAND_BOUNDARIES:
            if token == "|":
                pipe_stdin_is_active_target = pipe_stdout_is_active_target
                pipe_stdin_is_s3 = pipe_stdout_is_s3
            else:
                pipe_stdin_is_active_target = False
                pipe_stdin_is_s3 = False
            pipe_stdout_is_active_target = False
            pipe_stdout_is_s3 = False
            cursor += 1
            continue
        name = executable_name(token)
        if name in {"bash", "dash", "fish", "sh", "zsh"}:
            nested = shell_command(tokens[cursor:])
            if nested is not None:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    command_tokens(nested),
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        if name == "eval":
            inner = tokens[cursor + 1 :]
            if inner and inner[0] == "--":
                inner = inner[1:]
            if inner:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    command_tokens(" ".join(inner)),
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        chdir_value = directory_wrapper_chdir_value([token] + command_tail_until_boundary(tokens, cursor + 1))
        if chdir_value is not None:
            segment = [token] + command_tail_until_boundary(tokens, cursor + 1)
            inner = wrapper_inner_tokens(segment)
            if inner:
                chdir_roles = storage_value_roles(
                    chdir_value,
                    variable_roles,
                    cwd_is_active_target=cwd_is_active_target,
                    active_paths=active_paths,
                )
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    inner,
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=STORAGE_ROLE_ACTIVE_TARGET in chdir_roles,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                )
                if nested_errors:
                    return nested_errors
        if name in RECURSIVE_WRAPPER_EXECUTABLES:
            segment = [token] + command_tail_until_boundary(tokens, cursor + 1)
            inner = wrapper_inner_tokens(segment)
            if inner:
                nested_errors = storage_transfer_policy_errors_from_tokens(
                    inner,
                    variable_roles,
                    depth=depth + 1,
                    initial_cwd_is_active_target=cwd_is_active_target,
                    initial_active_paths=active_paths,
                    initial_s3_paths=s3_paths,
                    initial_pipe_stdin_is_active_target=pipe_stdin_is_active_target,
                    initial_pipe_stdin_is_s3=pipe_stdin_is_s3,
                )
                if nested_errors:
                    return nested_errors
        if name in {"cd", "pushd"}:
            directory_target, next_cursor = shell_directory_change_target(tokens, cursor)
            if directory_target is None:
                if name == "cd":
                    cwd_is_active_target = False
                cursor = next_cursor
                continue
            target_roles = storage_value_roles(
                directory_target,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            cwd_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in target_roles
            cursor = next_cursor
            continue
        if name in {"cp", "rsync", "mv"}:
            if command_copies_s3_path_to_active_target(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            record_local_transfer_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "tar":
            if tar_extracts_s3_archive_to_active_target(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            ):
                return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
            record_tar_archive_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "zip":
            record_zip_archive_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
        if name == "unzip" and unzip_extracts_s3_archive_to_active_target(
            tokens,
            cursor,
            variable_roles,
            active_paths,
            s3_paths,
            cwd_is_active_target=cwd_is_active_target,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name in ACTIVE_TARGET_STDOUT_COMMANDS:
            pipe_stdout_is_active_target = pipe_stdin_is_active_target or command_streams_active_target_to_stdout(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                cwd_is_active_target=cwd_is_active_target,
                command_name=name,
            )
        elif pipe_stdin_is_active_target and name != "aws":
            pipe_stdout_is_active_target = True
        if pipe_stdin_is_s3 and command_writes_s3_stdin_to_active_target(
            tokens,
            cursor,
            variable_roles,
            active_paths,
            cwd_is_active_target=cwd_is_active_target,
            command_name=name,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name == "aws" and aws_s3_transfer_touches_active_target(
            tokens,
            cursor,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
            active_paths=active_paths,
            stdin_is_active_target=pipe_stdin_is_active_target,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        if name == "aws":
            record_aws_s3_download_paths(
                tokens,
                cursor,
                variable_roles,
                active_paths,
                s3_paths,
                cwd_is_active_target=cwd_is_active_target,
            )
            pipe_stdout_is_s3 = aws_s3_transfer_streams_s3_to_stdout(
                tokens,
                cursor,
                variable_roles,
                cwd_is_active_target=cwd_is_active_target,
                active_paths=active_paths,
            )
            pipe_stdin_is_active_target = False
            pipe_stdin_is_s3 = False
        elif pipe_stdin_is_s3:
            pipe_stdout_is_s3 = True
        cursor += 1
    return []


def storage_transfer_policy_errors(text: str) -> list[str]:
    variable_roles = storage_variable_roles(text)
    return storage_transfer_policy_errors_from_tokens(command_tokens_with_line_boundaries(text), variable_roles)


def target_env_key_alias(value: str, target_keys: dict[str, str]) -> str | None:
    clean = storage_strip_quotes(value)
    compact = re.sub(r"\s+", "", clean)
    if clean in target_keys:
        return clean
    for target_key in target_keys:
        if target_key not in clean:
            continue
        if compact.startswith("$(") or compact.startswith("`") or compact.startswith("${"):
            return target_key
    return None


def shell_assignment_alias_value(value: str, target_keys: dict[str, str]) -> str | None:
    target_key = target_env_key_alias(value, target_keys)
    if target_key is not None:
        return target_key
    clean = storage_strip_quotes(value)
    for pattern in (r"\$\(\s*echo\s+([A-Za-z_][A-Za-z0-9_]*)\s*\)", r"`\s*echo\s+([A-Za-z_][A-Za-z0-9_]*)\s*`"):
        match = re.fullmatch(pattern, clean)
        if match and match.group(1) in target_keys:
            return match.group(1)
    return shell_identifier_fragment(value)


def shell_assignment_tracking_value(value: str, target_keys: dict[str, str]) -> str:
    alias_value = shell_assignment_alias_value(value, target_keys)
    return alias_value if alias_value is not None else storage_strip_quotes(value)


def target_env_key_from_assignment_name(
    name: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    clean = storage_strip_quotes(name)
    if clean in target_keys:
        return clean
    expanded = expand_known_shell_assignment_name(clean, assignments)
    if expanded in target_keys:
        return expanded
    return None


RUSTFLAGS_OUTPUT_OVERRIDE_KEYS = {
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTFLAGS",
}


def rustflags_value_has_output_override(value: str, assignments: dict[str, str] | None = None) -> bool:
    clean = expand_known_shell_assignment_value(value, assignments or {})
    return "--out-dir" in clean or "--artifact-dir" in clean


def dynamic_env_assignment_message(
    token: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    if "=" not in token:
        return None
    name, value = token.split("=", 1)
    target_key = target_env_key_from_assignment_name(name, assignments, target_keys)
    if target_key is None:
        return None
    if target_key in RUSTFLAGS_OUTPUT_OVERRIDE_KEYS and not rustflags_value_has_output_override(value, assignments):
        return None
    return target_keys[target_key]


def dynamic_env_segment_messages(
    segment: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    if not segment or depth > 4:
        return set()
    messages: set[str] = set()
    expanded = merge_split_shell_parameter_assignment_tokens(segment)
    local_assignments = dict(assignments)
    cursor = 0
    while cursor < len(expanded):
        current = expand_known_shell_assignment_names([expanded[cursor]], local_assignments)[0]
        if not shell_assignment_word(current):
            break
        expanded[cursor] = current
        message = dynamic_env_assignment_message(current, local_assignments, target_keys)
        if message is not None:
            messages.add(message)
        name, value = current.split("=", 1)
        local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        cursor += 1
    if cursor >= len(expanded):
        return messages
    expanded = expanded[:cursor] + expand_known_shell_assignment_names(expanded[cursor:], local_assignments)
    command = pathlib.Path(expanded[cursor]).name
    if command == "alias":
        for payload in shell_alias_payloads(expanded[cursor:]).values():
            messages.update(
                dynamic_env_tokens_messages(
                    command_tokens(payload),
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
        return messages
    if command == "export":
        for argument in expanded[cursor + 1 :]:
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        return messages
    if command in {"declare", "local", "typeset"}:
        for argument in expanded[cursor + 1 :]:
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            if argument == "--" or argument.startswith(("-", "+")):
                continue
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
        return messages
    if command == "env":
        index = cursor + 1
        while index < len(expanded):
            argument = expanded[index]
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            redirection_index = shell_redirection_next_index(expanded, index)
            if redirection_index is not None:
                index = redirection_index
                continue
            if argument == "--":
                index += 1
                continue
            if argument in ENV_OPTIONS_WITHOUT_ARGUMENT or argument in ENV_SIGNAL_OPTIONS:
                index += 1
                continue
            if any(argument.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
                index += 1
                continue
            if argument in {"-S", "--split-string"} and index + 1 < len(expanded):
                split_inner = command_tokens(expanded[index + 1]) + expanded[index + 2 :]
                messages.update(
                    dynamic_env_tokens_messages(
                        expand_known_shell_variables(split_inner, local_assignments),
                        local_assignments,
                        target_keys,
                        depth=depth + 1,
                    )
                )
                return messages
            if argument.startswith("--split-string="):
                split_inner = command_tokens(argument.split("=", 1)[1]) + expanded[index + 1 :]
                messages.update(
                    dynamic_env_tokens_messages(
                        expand_known_shell_variables(split_inner, local_assignments),
                        local_assignments,
                        target_keys,
                        depth=depth + 1,
                    )
                )
                return messages
            if argument in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(expanded):
                index += 2
                continue
            if any(
                argument.startswith(f"{option}=")
                for option in ENV_OPTIONS_WITH_ARGUMENT
                if option.startswith("--")
            ):
                index += 1
                continue
            if argument.startswith("-") and not argument.startswith("--"):
                split_inner = env_short_split_tokens(expanded, index)
                if split_inner is not None:
                    messages.update(
                        dynamic_env_tokens_messages(
                            expand_known_shell_variables(split_inner, local_assignments),
                            local_assignments,
                            target_keys,
                            depth=depth + 1,
                        )
                    )
                    return messages
                parsed_index = env_short_cluster_next_index(expanded, index, argument[1:])
                if parsed_index is not None:
                    index = parsed_index
                    continue
            message = dynamic_env_assignment_message(argument, local_assignments, target_keys)
            if message is None and not env_assignment_argument(argument):
                break
            if message is not None:
                messages.add(message)
            if shell_assignment_word(argument):
                name, value = argument.split("=", 1)
                local_assignments[name] = shell_assignment_tracking_value(value, target_keys)
            index += 1
        if index < len(expanded) and expanded[index] not in SHELL_COMMAND_BOUNDARIES:
            inner = expand_known_shell_variables(expanded[index:], local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    inner,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
        return messages
    if command == "eval":
        inner = expanded[cursor + 1 :]
        if inner and inner[0] == "--":
            inner = inner[1:]
        if inner:
            inner = expand_known_shell_variables(inner, local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    command_tokens(" ".join(inner)),
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    if command in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(expanded[cursor:])
        if nested is not None:
            nested_tokens = expand_known_shell_variables(command_tokens(nested), local_assignments)
            messages.update(
                dynamic_env_tokens_messages(
                    nested_tokens,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    if command in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(expanded[cursor:])
        if inner is not None:
            messages.update(
                dynamic_env_tokens_messages(
                    inner,
                    local_assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    return messages


def dynamic_env_tokens_messages(
    tokens: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    messages: set[str] = set()
    for segment in shell_command_segments_from_tokens(tokens):
        messages.update(dynamic_env_segment_messages(segment, assignments, target_keys, depth=depth))
    return messages


def shell_command_segments_from_tokens(tokens: list[str]) -> list[list[str]]:
    segments: list[list[str]] = []
    segment: list[str] = []
    expanded = merge_split_shell_parameter_assignment_tokens(tokens)
    index = 0
    substitution_depth = 0
    while index < len(expanded):
        assignment = shell_assignment_from_tokens(expanded, index)
        if assignment is not None:
            _name, _value, next_index = assignment
            segment.extend(expanded[index:next_index])
            index = next_index
            continue
        token = expanded[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(expanded) and expanded[index + 1] == "(":
            segment.extend([token, expanded[index + 1]])
            substitution_depth += 1
            index += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            if segment:
                segments.append(segment)
            segment = []
            index += 1
            continue
        segment.append(token)
        index += 1
    if segment:
        segments.append(segment)
    return segments


def tokens_have_top_level_shell_boundary(tokens: list[str]) -> bool:
    substitution_depth = 0
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if (token == "$" or token.endswith("$")) and index + 1 < len(tokens) and tokens[index + 1] == "(":
            substitution_depth += 1
            index += 2
            continue
        if token == "(" and substitution_depth:
            substitution_depth += 1
        elif token == ")" and substitution_depth:
            substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES and not substitution_depth:
            return True
        index += 1
    return False


def dynamic_env_target_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    target_keys = {
        "CARGO_TARGET_DIR": "CARGO_TARGET_DIR raw target override must be classified",
        "CARGO_BUILD_TARGET_DIR": "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        "CARGO_TARGET_TMPDIR": "CARGO_TARGET_TMPDIR raw target override must be classified",
        "CARGO_INCREMENTAL": "CARGO_INCREMENTAL raw cache override must be classified",
        "CARGO_BUILD_RUSTFLAGS": "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        "CARGO_ENCODED_RUSTFLAGS": "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        "CARGO_INSTALL_ROOT": "CARGO_INSTALL_ROOT install output override must be classified",
        "CARGO_HOME": "CARGO_HOME raw cache override must be classified",
        "RUSTUP_HOME": "RUSTUP_HOME raw toolchain override must be classified",
        "RUSTFLAGS": "RUSTFLAGS raw output override must be classified",
        "RUSTC_WRAPPER": "RUSTC_WRAPPER raw compiler wrapper must be classified",
        "RUSTC_WORKSPACE_WRAPPER": "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified",
        "BOLT_ALLOW_LOCAL_RUST": "BOLT_ALLOW_LOCAL_RUST local Rust break-glass must not be checked in",
        "BOLT_MANAGED_JUST": "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        "GITHUB_ACTIONS": "GITHUB_ACTIONS local CI spoof must not be checked in",
    }
    assignments: dict[str, str] = {}
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        for segment in shell_command_segments_from_tokens(command_tokens(stripped)):
            messages.update(dynamic_env_segment_messages(segment, assignments, target_keys))
            segment_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
            for name, value in segment_assignments.items():
                if is_persistent_assignment:
                    alias_value = shell_assignment_alias_value(value, target_keys)
                    if alias_value is not None:
                        assignments[name] = alias_value
                    else:
                        assignments[name] = shell_assignment_tracking_value(value, target_keys)
    return messages


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


def github_env_payload_assignments(payload: str, *, decode_newlines: bool = False) -> list[str]:
    if decode_newlines:
        payload = payload.replace("\\n", "\n")
    assignments: list[str] = []
    payload_lines = payload.splitlines() or [payload]
    index = 0
    while index < len(payload_lines):
        line = payload_lines[index]
        clean = line.strip()
        heredoc = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)<<(.+)", clean)
        if heredoc:
            name = heredoc.group(1)
            delimiter = storage_strip_quotes(heredoc.group(2).strip())
            body: list[str] = []
            index += 1
            while index < len(payload_lines):
                candidate = payload_lines[index]
                if candidate.strip() == delimiter:
                    break
                body.append(candidate.strip())
                index += 1
            assignments.append(f"{name}={shlex.quote(storage_strip_quotes(chr(10).join(body)))}")
            if index < len(payload_lines):
                index += 1
            continue
        if "=" not in clean:
            index += 1
            continue
        name, value = clean.split("=", 1)
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
            assignments.append(f"{name}={shlex.quote(storage_strip_quotes(value))}")
        index += 1
    return assignments


def github_env_assignments_from_echo_tokens(tokens: list[str]) -> list[str]:
    if len(tokens) < 4 or pathlib.Path(tokens[0]).name != "echo":
        return []
    for redirect_index, token in enumerate(tokens):
        if token != ">>":
            continue
        target = storage_strip_quotes(tokens[redirect_index + 1]) if redirect_index + 1 < len(tokens) else ""
        if target not in {"$GITHUB_ENV", "${GITHUB_ENV}"}:
            continue
        payload_start = 1
        decode_newlines = False
        while payload_start < redirect_index and re.fullmatch(r"-[neE]+", tokens[payload_start]):
            for option in tokens[payload_start][1:]:
                if option == "e":
                    decode_newlines = True
                elif option == "E":
                    decode_newlines = False
            payload_start += 1
        payload = " ".join(tokens[payload_start:redirect_index])
        return github_env_payload_assignments(payload, decode_newlines=decode_newlines)
    return []


def github_env_assignment_from_echo_tokens(tokens: list[str]) -> str | None:
    assignments = github_env_assignments_from_echo_tokens(tokens)
    return assignments[0] if assignments else None


def printf_rendered_payload(format_payload: str, argument_tokens: list[str]) -> str | None:
    chunks: list[str] = []
    argument_index = 0
    while True:
        chunk: list[str] = []
        consumed_argument = False
        index = 0
        while index < len(format_payload):
            if format_payload[index] != "%":
                chunk.append(format_payload[index])
                index += 1
                continue
            if index + 1 >= len(format_payload):
                return None
            conversion = format_payload[index + 1]
            if conversion == "%":
                chunk.append("%")
                index += 2
                continue
            if conversion not in {"s", "b"}:
                return None
            value = argument_tokens[argument_index] if argument_index < len(argument_tokens) else ""
            if argument_index < len(argument_tokens):
                argument_index += 1
            chunk.append(value.replace("\\n", "\n") if conversion == "b" else value)
            consumed_argument = True
            index += 2
        chunks.append("".join(chunk))
        if argument_index >= len(argument_tokens) or not consumed_argument:
            break
    return "".join(chunks)


def github_env_assignments_from_printf_tokens(tokens: list[str]) -> list[str]:
    if len(tokens) < 4 or pathlib.Path(tokens[0]).name != "printf":
        return []
    for redirect_index, token in enumerate(tokens):
        if token != ">>":
            continue
        target = storage_strip_quotes(tokens[redirect_index + 1]) if redirect_index + 1 < len(tokens) else ""
        if target not in {"$GITHUB_ENV", "${GITHUB_ENV}"}:
            continue
        payload_tokens = tokens[1:redirect_index]
        if not payload_tokens:
            return []
        if payload_tokens[0] == "--":
            payload_tokens = payload_tokens[1:]
        if not payload_tokens:
            return []
        format_payload = storage_strip_quotes(payload_tokens[0]).replace("\\n", "\n")
        argument_tokens = [storage_strip_quotes(value) for value in payload_tokens[1:]]
        payload = printf_rendered_payload(format_payload, argument_tokens)
        if payload is None:
            return []
        return github_env_payload_assignments(payload)
    return []


def github_env_assignment_from_printf_tokens(tokens: list[str]) -> str | None:
    assignments = github_env_assignments_from_printf_tokens(tokens)
    return assignments[0] if assignments else None


def github_env_assignments_from_line(line: str) -> list[str]:
    clean = strip_comment(line).strip()
    tokens = command_tokens(clean)
    assignments: list[str] = []
    for segment in shell_command_segments_from_tokens(tokens):
        for extractor in (github_env_assignments_from_echo_tokens, github_env_assignments_from_printf_tokens):
            assignments.extend(extractor(segment))
    return assignments


def github_env_line_assignments_around_cat_heredoc(
    line: str,
) -> tuple[list[str], tuple[str, bool, bool] | None, list[str]]:
    clean = strip_comment(line).strip()
    before: list[str] = []
    after: list[str] = []
    heredoc_spec: tuple[str, bool, bool] | None = None
    for segment in shell_command_segments_from_tokens(command_tokens(clean)):
        spec = github_env_cat_heredoc_spec(segment, clean)
        if spec is not None and heredoc_spec is None:
            heredoc_spec = spec
            continue
        target = after if heredoc_spec is not None else before
        for extractor in (github_env_assignments_from_echo_tokens, github_env_assignments_from_printf_tokens):
            target.extend(extractor(segment))
    return before, heredoc_spec, after


def shell_heredoc_quoted_delimiters(line: str) -> dict[str, bool]:
    delimiters: dict[str, bool] = {}
    for match in re.finditer(r"<<(-?)\s*(['\"]?)([A-Za-z_][A-Za-z0-9_-]*)\2", line):
        delimiters[match.group(3)] = bool(match.group(2))
    return delimiters


def github_env_cat_heredoc_spec(tokens: list[str], line: str) -> tuple[str, bool, bool] | None:
    if len(tokens) < 5 or pathlib.Path(tokens[0]).name != "cat":
        return None
    writes_github_env = any(
        token == ">>"
        and index + 1 < len(tokens)
        and storage_strip_quotes(tokens[index + 1]) in {"$GITHUB_ENV", "${GITHUB_ENV}"}
        for index, token in enumerate(tokens)
    )
    if not writes_github_env:
        return None
    quoted_delimiters = shell_heredoc_quoted_delimiters(line)
    for index, token in enumerate(tokens):
        if token in {"<<", "<<-"} and index + 1 < len(tokens):
            delimiter = storage_strip_quotes(tokens[index + 1])
            return (delimiter, token == "<<-", quoted_delimiters.get(delimiter, False))
    return None


def github_env_assignments_from_cat_heredocs(text: str) -> list[str]:
    assignments: list[str] = []
    lines = text.splitlines()
    index = 0
    while index < len(lines):
        clean = strip_comment(lines[index]).strip()
        heredoc_spec: tuple[str, bool, bool] | None = None
        for segment in shell_command_segments_from_tokens(command_tokens(clean)):
            heredoc_spec = github_env_cat_heredoc_spec(segment, clean)
            if heredoc_spec is not None:
                break
        if heredoc_spec is None:
            index += 1
            continue
        delimiter, strip_tabs, quoted_delimiter = heredoc_spec
        payload: list[str] = []
        index += 1
        while index < len(lines):
            candidate = lines[index]
            comparable = candidate.lstrip("\t") if strip_tabs else candidate
            if comparable == delimiter:
                break
            payload.append(candidate.lstrip("\t") if strip_tabs else candidate)
            index += 1
        payload_text = "\n".join(payload)
        if not quoted_delimiter:
            payload_text = payload_text.replace("\\\n", "")
        assignments.extend(github_env_payload_assignments(payload_text))
        if index < len(lines):
            index += 1
    return assignments


def github_env_assignment_line(line: str) -> str | None:
    assignments = github_env_assignments_from_line(line)
    return assignments[0] if assignments else None


def github_env_assignments_from_logical_text(text: str) -> list[str]:
    assignments: list[str] = []
    for line in shell_logical_lines(text):
        assignments.extend(github_env_assignments_from_line(line))
    return assignments


def github_env_assignment_lines(text: str) -> list[str]:
    assignments: list[str] = []
    pending = ""
    raw_lines = text.splitlines()
    index = 0
    while index < len(raw_lines):
        line = raw_lines[index]
        before, heredoc_spec, after = github_env_line_assignments_around_cat_heredoc(line)
        if heredoc_spec is None:
            pending = f"{pending}\n{line}" if pending else line
            balance_text = "\n".join(strip_comment(pending_line) for pending_line in pending.splitlines())
            if shell_quotes_are_balanced(balance_text) and not line.rstrip().endswith("\\"):
                assignments.extend(github_env_assignments_from_logical_text(pending))
                pending = ""
            index += 1
            continue

        if pending:
            assignments.extend(github_env_assignments_from_logical_text(pending))
            pending = ""
        assignments.extend(before)
        delimiter, strip_tabs, quoted_delimiter = heredoc_spec
        payload: list[str] = []
        index += 1
        while index < len(raw_lines):
            candidate = raw_lines[index]
            comparable = candidate.lstrip("\t") if strip_tabs else candidate
            if comparable == delimiter:
                break
            payload.append(candidate.lstrip("\t") if strip_tabs else candidate)
            index += 1
        payload_text = "\n".join(payload)
        if not quoted_delimiter:
            payload_text = payload_text.replace("\\\n", "")
        assignments.extend(github_env_payload_assignments(payload_text))
        assignments.extend(after)
        if index < len(raw_lines):
            index += 1
    if pending:
        assignments.extend(github_env_assignments_from_logical_text(pending))
    return assignments


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


def consume_cargo_global_options(tokens: list[str], index: int) -> int:
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("+"):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return index


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


def fingerprint_reuse_gates_on_pull_request(job_lines: list[str]) -> bool:
    return FINGERPRINT_REUSE_PR_EVENT_EXPR in job_if_value(job_lines)


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
        "nextest archive reused from run",
    )
    return all(item in text for item in required)


def ci_provenance_emit_runs_emitter(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
        "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
        'git check-ref-format "refs/heads/$base_branch"',
        'git archive "$base_ref" scripts/ ci/github-actions-runners.toml',
        'tested_workflow="$GITHUB_WORKSPACE/.github/workflows/ci.yml"',
        "tested workflow file is missing or not a regular file",
        'cp "$tested_workflow" "$base_tree/.github/workflows/ci.yml"',
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
            if expected not in text:
                errors.append("ci-provenance-emit must pass build.result from needs.build.result")
            continue
        expected = f"--required-job {need}=${{{{ needs.{need}.result }}}}"
        if expected not in text:
            errors.append(f"ci-provenance-emit must pass {need} result from needs.{need}.result")
    if "--conditional-job build.required=${{ needs.detector.outputs.build_required }}" not in text:
        errors.append("ci-provenance-emit must pass build.required from needs.detector.outputs.build_required")
    if (
        f'--nextest-fingerprint "{TEST_ARCHIVE_FINGERPRINT_OUTPUT}"' not in text
        or "--nextest-fingerprint-path" in text
    ):
        errors.append("ci-provenance-emit must use secure nextest fingerprint output")
    return errors


def ci_provenance_emit_upload_errors(job_lines: list[str], retention_days: int) -> list[str]:
    errors: list[str] = []
    upload_blocks = [
        block
        for block in action_blocks(job_lines, "actions/upload-artifact@")
        if block_has_input(block, "name", "ci-provenance-attempt-${{ github.run_attempt }}")
    ]
    if not upload_blocks:
        errors.append("ci-provenance-emit must upload configured provenance artifact")
        return errors
    if not any(block_has_input(block, "path", "ci-provenance.json") for block in upload_blocks):
        errors.append("ci-provenance-emit must upload ci-provenance.json")
    if retention_days > 0 and not any(
        block_has_input(block, "retention-days", str(retention_days)) for block in upload_blocks
    ):
        errors.append("ci-provenance-emit retention-days must match TOML")
    return errors


def ci_provenance_emit_records_secure_fingerprint(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return (
        f'--nextest-fingerprint "{TEST_ARCHIVE_FINGERPRINT_OUTPUT}"' in text
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
            if matches(block) and not has_line_matching(block, CHECK_AARCH64_STANDALONE_IF_RE):
                errors.append(message)
                break
    return errors


GATE_TAG_REUSE_CONDITION = '"$policy_path" == "tag_reuse"'
GATE_FULL_CONDITION = '"$policy_path" == "full"'
GATE_ITERATION_CONDITION = '"$policy_path" == "iteration"'
GATE_NOOP_CONDITION = '"$policy_path" == "noop"'
GATE_DEFER_CONDITION = '"$policy_path" == "defer" || "$full_ci_deferred" == "true"'
GATE_NAME_OUTPUT = "name: ${{ needs.ci-policy.outputs.gate_name }}"
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


def if_chain_bodies(gate_text: str, condition: str) -> dict[tuple[str, str], str] | None:
    lines = gate_text.splitlines()
    for start, line in enumerate(lines):
        match = IF_OR_ELIF_RE.match(line)
        if match and match.group(1) == "if" and match.group("condition") == condition:
            return collect_if_chain_bodies(lines, start, condition)
    return None


def collect_if_chain_bodies(lines: list[str], start: int, condition: str) -> dict[tuple[str, str], str] | None:
    bodies: dict[tuple[str, str], list[str]] = {("if", condition): []}
    current = ("if", condition)
    depth = 0
    for line in lines[start + 1 :]:
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            keyword = branch_match.group(1)
            branch_condition = branch_match.group("condition")
            if depth == 0 and keyword == "elif":
                current = ("elif", branch_condition)
                bodies[current] = []
                continue
            bodies[current].append(line)
            if keyword == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            if depth == 0:
                current = ("else", "")
                bodies[current] = []
            else:
                bodies[current].append(line)
            continue
        if FI_RE.match(line):
            if depth == 0:
                return {key: "\n".join(value) for key, value in bodies.items()}
            bodies[current].append(line)
            depth -= 1
            continue
        bodies[current].append(line)
    return None


def gate_checks_same_sha_reuse(gate_text: str) -> list[str]:
    errors: list[str] = []
    for job in (*TAG_SKIPPED_JOBS, "same-sha-main-evidence", "check-aarch64"):
        required_arg = f"--job {job}=${{{{ needs.{job}.result }}}}"
        if required_arg not in gate_text:
            errors.append(f"gate shared verdict call must include {required_arg}")
    return errors


def gate_checks_nextest_fingerprint_reuse(gate_text: str) -> list[str]:
    errors: list[str] = []
    for required in (
        "--reuse-found",
        "needs.nextest-fingerprint-reuse.outputs.reuse_found",
        "--job nextest-fingerprint-reuse=${{ needs.nextest-fingerprint-reuse.result }}",
        "--job ci-provenance-emit=${{ needs.ci-provenance-emit.result }}",
    ):
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    return errors


def gate_policy_truth_table_errors(gate_text: str) -> list[str]:
    errors: list[str] = []
    if GATE_NAME_OUTPUT not in gate_text:
        errors.append("gate name must come from ci-policy gate_name output")
    for required in (
        "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
        "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
        'git check-ref-format "refs/heads/$base_branch"',
        "git archive \"$base_ref\" scripts/ ci/github-actions-runners.toml",
        "steps.verdict_base.outputs.script",
        'python3 "$verdict_script" check-ci-gate',
    ):
        if required not in gate_text:
            errors.append(f"gate must use trusted base-tree ci_provenance.py check-ci-gate verdict ({required})")
    for required in (
        "--policy-path \"${{ needs.ci-policy.outputs.ci_policy_path }}\"",
        "--expected-event-class \"${{ needs.ci-policy.outputs.expected_event_class }}\"",
        "--full-ci-deferred \"${{ needs.ci-policy.outputs.full_ci_deferred }}\"",
        "--ignore-emit-failure \"${{ needs.ci-policy.outputs.ignore_emit_failure }}\"",
        "carry_forward_args=()",
        "carry_forward_verified=\"${{ steps.carry_forward.outputs.carry_forward_verified }}\"",
        "if [[ -n \"$carry_forward_verified\" ]]; then",
        "carry_forward_args+=(--carry-forward-verified \"$carry_forward_verified\")",
        "\"${carry_forward_args[@]}\"",
        "--build-required \"${{ needs.detector.outputs.build_required || 'false' }}\"",
        "--job ci-policy=${{ needs.ci-policy.result }}",
        "--job detector=${{ needs.detector.result }}",
        "--job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
    ):
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    for job in (
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
    ):
        required = f"--job {job}=${{{{ needs.{job}.result }}}}"
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    if 'python3 "$verdict_script" resolve-gate-carry-forward' not in gate_text:
        errors.append("gate must verify carry-forward through trusted base-tree ci_provenance.py")
    if "--require-provenance-base true" not in gate_text:
        errors.append("gate carry-forward must require provenance base match")
    return errors


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


def detector_fingerprint_reuse_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    text = uncommented_text(job_lines)
    fingerprint_inputs_text = ""
    allowance_text = ""
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
    allowance_chain = if_chain_bodies(allowance_text, '"${{ github.event_name }}" != "pull_request"')
    if allowance_chain is None:
        errors.append("detector must deny fingerprint reuse outside pull_request")
    elif (
        'echo "value=false" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(("if", '"${{ github.event_name }}" != "pull_request"'), "")
        or 'echo "value=false" >> "$GITHUB_OUTPUT"'
        not in allowance_chain.get(
            (
                "elif",
                '"${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true"',
            ),
            "",
        )
        or 'echo "value=true" >> "$GITHUB_OUTPUT"' not in allowance_chain.get(("else", ""), "")
        or allowance_text.count('echo "value=false" >> "$GITHUB_OUTPUT"') != 2
        or allowance_text.count('echo "value=true" >> "$GITHUB_OUTPUT"') != 1
    ):
        errors.append("detector must determine fingerprint_reuse_allowed")
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


def configured_ci_provenance_retention_days() -> int:
    try:
        config = load_github_actions_runners_config()
    except (ValueError, FileNotFoundError, tomllib.TOMLDecodeError):
        return -1
    ci_provenance = config.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        return -1
    artifacts = ci_provenance.get("artifacts")
    if not isinstance(artifacts, dict):
        return -1
    retention_days = artifacts.get("retention_days")
    return retention_days if isinstance(retention_days, int) else -1


def configured_ci_provenance_dispatch_input() -> str:
    try:
        config = load_github_actions_runners_config()
    except (ValueError, FileNotFoundError, tomllib.TOMLDecodeError):
        return ""
    ci_provenance = config.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        return ""
    dispatch = ci_provenance.get("dispatch")
    if not isinstance(dispatch, dict):
        return ""
    workflow_input = dispatch.get("workflow_input")
    return workflow_input if isinstance(workflow_input, str) else ""


def configured_ci_provenance_dispatch_names() -> dict[str, str]:
    try:
        config = load_github_actions_runners_config()
    except (ValueError, FileNotFoundError, tomllib.TOMLDecodeError):
        return {}
    ci_provenance = config.get("ci_provenance")
    if not isinstance(ci_provenance, dict):
        return {}
    dispatch = ci_provenance.get("dispatch")
    if not isinstance(dispatch, dict):
        return {}
    names = {
        key: value
        for key in ("workflow_input", "run_name_default", "run_name_full", "run_name_iteration")
        if isinstance((value := dispatch.get(key)), str) and value
    }
    return names if len(names) == 4 else {}


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
    names = configured_ci_provenance_dispatch_names()
    if not names:
        return []
    run_name_text = top_level_key_block_text(workflow_text, "run-name")
    full_predicate = f"github.event.inputs.{names['workflow_input']} == 'true'"
    errors: list[str] = []
    if "run-name: >-" not in run_name_text:
        errors.append("workflow must define run-name for dispatch class markers")
    if full_predicate not in run_name_text:
        errors.append("workflow run-name must use strict configured dispatch full predicate")
    if f"&& '{names['run_name_full']}'" not in run_name_text:
        errors.append("workflow run-name must publish configured dispatch full marker")
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
    errors.extend(workflow_steps_alias_errors(workflow_text))
    jobs = parse_jobs(workflow_text)
    triggers = workflow_trigger_keys(workflow_text)
    is_ci_topology = "pull_request" in triggers and "push" in triggers
    errors.extend(raw_rust_storage_errors(workflow_text))
    errors.extend(exact_head_governance_cache_errors(workflow_text))
    errors.extend(base_ref_archive_scripts_directory_errors(workflow_text))
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
        dispatch_input = configured_ci_provenance_dispatch_input()
        if dispatch_input:
            errors.extend(
                workflow_dispatch_input_errors(
                    workflow_text,
                    dispatch_input,
                )
            )
        errors.extend(workflow_run_name_errors(workflow_text))
        if "merge_group" not in triggers:
            # The merge queue dispatches merge_group/checks_requested; required
            # checks that do not declare it never report and block the merge.
            errors.append("on must define merge_group for merge queue full CI")

    errors.extend(verify_pr_concurrency(workflow_text))

    if not workflow_permissions_have_actions_read(workflow_text):
        errors.append("workflow permissions must include actions: read")

    for job in REQUIRED_JOBS:
        if job not in jobs:
            errors.append(f"missing required job {job}")

    if "detector" in jobs and not detector_forces_build_on_workflow_dispatch(jobs["detector"]):
        errors.append("detector must force build_required=true for workflow_dispatch full CI")
    if "detector" in jobs and not detector_forces_build_on_merge_group(jobs["detector"]):
        errors.append("detector must force build_required=true for merge_group full CI")
    if "detector" in jobs:
        errors.extend(detector_fingerprint_reuse_errors(jobs["detector"]))
        errors.extend(detector_docs_only_archive_errors(jobs["detector"]))

    if "ci-policy" in jobs:
        errors.extend(ci_policy_job_errors(jobs["ci-policy"]))

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
        if not job_gates_on_full_ci_required(jobs["source-fence"]):
            errors.append("source-fence must gate on full_ci_required")

    for job_name, recipe in JOB_REQUIRED_JUST_RECIPE.items():
        if job_name in jobs and not job_runs_command(jobs[job_name], f"just {recipe}"):
            errors.append(f"{job_name} must run just {recipe}")

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
        target_cache_keys = [
            block_input_value(block, "key") or ""
            for block in target_restore_blocks + target_save_blocks
        ]
        if TEST_ARCHIVE_PATH not in archive_text:
            errors.append("test-archive must declare nextest archive path")
        if TEST_ARCHIVE_SIDECAR_PATH not in archive_text:
            errors.append("test-archive must declare root binary sidecar path")
        if not archive_cache_blocks or not all(
            block_has_input(block, "key", TEST_ARCHIVE_CACHE_KEY)
            for block in archive_cache_blocks
        ):
            errors.append("nextest archive cache key must use nextest fingerprint output")
        if any("hashFiles(" in (block_input_value(block, "key") or "") for block in archive_cache_blocks):
            errors.append("nextest archive cache key must use nextest fingerprint output")
        if not sidecar_cache_blocks or not all(
            block_has_input(block, "key", TEST_ARCHIVE_SIDECAR_CACHE_KEY)
            for block in sidecar_cache_blocks
        ):
            errors.append("root binary sidecar cache key must use nextest fingerprint output")
        if any("hashFiles(" in (block_input_value(block, "key") or "") for block in sidecar_cache_blocks):
            errors.append("root binary sidecar cache key must use nextest fingerprint output")
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
        for required in ("src/**", "tests/**"):
            if not any(required in key for key in target_cache_keys):
                errors.append(f"test-archive managed target cache key must include {required}")
        if "nextest-archive-build-v1" in archive_text:
            errors.append("test-archive must not save a second archive-build cache")
        if not archive_restore_blocks:
            errors.append("test-archive must restore nextest archive cache")
        if not archive_save_blocks:
            errors.append("test-archive must save nextest archive cache")
        if archive_upload_blocks:
            errors.append("test-archive must not upload nextest archive artifact")
        if any(block_has_input(block, "restore-keys") for block in archive_cache_blocks):
            errors.append("test-archive cache must not use restore-keys")
        if any(block_has_input(block, "restore-keys") for block in sidecar_cache_blocks):
            errors.append("root binary sidecar cache must not use restore-keys")
        if archive_text.count(TEST_ARCHIVE_CACHE_PATH) < 2:
            errors.append("test-archive cache must use archive path env")
        if archive_text.count(TEST_ARCHIVE_SIDECAR_CACHE_PATH) < 2:
            errors.append("root binary sidecar cache must use sidecar path env")
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
        if (
            TEST_ARCHIVE_SIDECAR_CACHE_MISS_GUARD not in archive_text
            or TEST_ARCHIVE_SIDECAR_BUILD_COMMAND not in archive_text
        ):
            errors.append("test-archive must build CARGO_BIN_EXE sidecars on sidecar cache miss")
        sidecar_block = named_step_block(archive_lines, "Build root binary sidecars")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_BUILD_GUARD not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar cargo build must run only on archive-cache hit and sidecar-cache miss")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_PROFILE_ENV not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar build must use dev profile debug knob")
        if sidecar_block is None or TEST_ARCHIVE_SIDECAR_PACK_COMMAND not in uncommented_text(sidecar_block):
            errors.append("test-archive sidecar build must use tracked root binary sidecar helper")
        if not sidecar_restore_blocks:
            errors.append("test-archive must restore root binary sidecar cache")
        if not sidecar_save_blocks:
            errors.append("test-archive must save root binary sidecar cache")
        if any(TEST_ARCHIVE_SIDECAR_CACHE_MISS_GUARD not in uncommented_text(block) for block in sidecar_save_blocks):
            errors.append("test-archive must save root binary sidecar cache only on sidecar cache miss")
        if not job_runs_command(archive_lines, 'just test-archive "$NEXTEST_ARCHIVE_PATH"'):
            errors.append("test-archive must build through just test-archive")
        # Fail-open contract for the S3 sccache compile cache (#1011): when the
        # opt-in is wired, the cache must never be able to fail the required build,
        # and cache use must be gated to trusted refs (the IAM trust scope is the
        # real poison boundary, but keep the workflow itself honest too).
        if "BOLT_RUST_VERIFICATION_SCCACHE" in archive_text:
            if TEST_ARCHIVE_SCCACHE_OPT_IN not in archive_text:
                errors.append("test-archive sccache opt-in must stay conditional on the resolver, never hardcoded")
            for label in (
                "Resolve sccache eligibility",
                "Configure AWS credentials for sccache",
                "Install sccache",
                "Resolve sccache enablement",
            ):
                block = named_step_block(archive_lines, label)
                if block is None or "continue-on-error: true" not in uncommented_text(block):
                    errors.append(f"test-archive sccache step '{label}' must be continue-on-error (fail-open)")
            # Value, not mere presence: the flag must be "1" so a future edit cannot
            # silently flip it to "0" and make S3/server I/O errors fatal.
            if TEST_ARCHIVE_SCCACHE_IGNORE_IO not in archive_text:
                errors.append('test-archive sccache must set SCCACHE_IGNORE_SERVER_IO_ERROR: "1" (degrade S3 errors to local compile)')
            # Even a mid-build sccache server crash (which SCCACHE_IGNORE_SERVER_IO_ERROR
            # does not cover) must not fail the build: it retries once without sccache.
            build_block = named_step_block(archive_lines, "Build nextest archive")
            if build_block is None or TEST_ARCHIVE_SCCACHE_RETRY not in uncommented_text(build_block):
                errors.append("test-archive sccache must retry the build without sccache on failure (fail-open)")
            # Gate cache use to trusted refs in the eligibility step itself, not merely
            # somewhere in the job: main (post-merge) and the GitHub-controlled
            # merge_group queue ref are the only write-safe refs (IAM is the real boundary).
            eligibility_block = named_step_block(archive_lines, "Resolve sccache eligibility")
            eligibility_text = uncommented_text(eligibility_block) if eligibility_block is not None else ""
            if TEST_ARCHIVE_SCCACHE_PREFIX_PRECONDITION not in eligibility_text:
                errors.append("test-archive sccache eligibility must require CI_SCCACHE_S3_KEY_PREFIX")
            if TEST_ARCHIVE_SCCACHE_LOCATION_PRECONDITION not in eligibility_text:
                errors.append("test-archive sccache eligibility must pin bucket/region/prefix to the bolt-v2 CI cache")
            trusted_assignments = tuple(
                line.strip()
                for line in eligibility_text.splitlines()
                if "trusted=true" in line
            )
            if trusted_assignments != TEST_ARCHIVE_SCCACHE_TRUSTED_ASSIGNMENTS:
                errors.append("test-archive sccache must gate write-cache use exactly to main push/dispatch refs")
            if TEST_ARCHIVE_SCCACHE_PR_ROLE_ENV not in eligibility_text:
                errors.append("test-archive sccache must configure PR read-only sccache role path")
            if (
                TEST_ARCHIVE_SCCACHE_READ_WRITE_ROLE not in eligibility_text
                or TEST_ARCHIVE_SCCACHE_PR_READ_ONLY_ROLE not in eligibility_text
                or TEST_ARCHIVE_SCCACHE_ROLE_OUTPUT not in eligibility_text
                or TEST_ARCHIVE_SCCACHE_MODE_OUTPUT not in eligibility_text
            ):
                errors.append("test-archive sccache must configure PR read-only sccache role path")
            aws_block = named_step_block(archive_lines, "Configure AWS credentials for sccache")
            aws_text = uncommented_text(aws_block) if aws_block is not None else ""
            if TEST_ARCHIVE_SCCACHE_RESOLVED_ROLE_ASSUME not in aws_text:
                errors.append("test-archive sccache must assume the resolved sccache role")
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
        for fragment in (
            TEST_ARCHIVE_PARTITION_STATUS_INIT,
            TEST_ARCHIVE_PARTITION_STATUS_MARK,
            TEST_ARCHIVE_PARTITION_STATUS_EXIT,
            TEST_ARCHIVE_PARTITION_FAILURE_WRAPPER,
        ):
            if fragment not in archive_text:
                errors.append("test-archive must aggregate partition failures")
                break

    if "nextest-fingerprint-reuse" in jobs:
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
        if not fingerprint_reuse_gates_on_pull_request(reuse_lines):
            errors.append("nextest-fingerprint-reuse must be PR-only")
        if not fingerprint_reuse_skips_main_branch(reuse_lines):
            errors.append("nextest-fingerprint-reuse must skip main branch")
        if not fingerprint_reuse_gates_on_detector_allowed(reuse_lines):
            errors.append("nextest-fingerprint-reuse must gate on fingerprint_reuse_allowed")
        if not fingerprint_reuse_job_has_outputs(reuse_lines):
            errors.append("nextest-fingerprint-reuse must expose reuse provenance outputs")
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
        if NEXTEST_REUSE_MISS_EXPR not in uncommented_text(emit_lines):
            errors.append("ci-provenance-emit must skip validated nextest fingerprint reuse")
        if not ci_provenance_emit_runs_emitter(emit_lines):
            errors.append("ci-provenance-emit must run provenance emitter")
        errors.extend(ci_provenance_emit_checks_needs(emit_lines, (*CI_PROVENANCE_REQUIRED_JOBS, "build")))
        errors.extend(
            ci_provenance_emit_upload_errors(
                emit_lines,
                configured_ci_provenance_retention_days(),
            )
        )
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
        if block_has_input(block, "name", BOLT_V2_BINARY_ARTIFACT_NAME)
    ]
    if not binary_upload_blocks or any(
        block_input_values(block, "retention-days") != [BOLT_V2_BINARY_RETENTION_DAYS]
        for block in binary_upload_blocks
    ):
        errors.append(
            f"{workflow_name} {BOLT_V2_BINARY_ARTIFACT_NAME} retention-days must be {BOLT_V2_BINARY_RETENTION_DAYS}"
        )
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
    if not file_name.endswith("backtester-ci.yml"):
        return []
    errors: list[str] = []
    for line in text.splitlines():
        if "managed-target-bvs-v1-" not in line or "hashFiles(" not in line:
            continue
        for required in [
            "crates/backtesting-vertical-slice/src/**",
            "crates/backtesting-vertical-slice/tests/**",
        ]:
            if required not in line:
                errors.append(f"backtester managed-target cache key must include {required}")
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
    if archive_job is None:
        errors.append("backtester bvs-test must define archive producer job")
    if test_job is None:
        errors.append("backtester bvs-test must define matrix shard job")
    if issue_job is None:
        errors.append("backtester bvs-test must define dedicated issue-789 job")
    if archive_job is None or test_job is None:
        return errors
    archive_text = uncommented_text(archive_job)
    job_text = uncommented_text(test_job)
    issue_text = uncommented_text(issue_job) if issue_job is not None else ""
    gate_text = uncommented_text(gate_job) if gate_job is not None else ""
    consumer_text = f"{job_text}\n{issue_text}"
    combined_text = f"{archive_text}\n{consumer_text}"
    if "just bte-test --partition" in combined_text:
        errors.append("backtester bvs-test must not run direct per-shard target builds")
    if "for shard in $(seq" in archive_text or "just bte-test-archive-run" in archive_text:
        errors.append("backtester bvs-test archive producer must not run partitions serially")
    if "build --locked --bins" in consumer_text:
        errors.append("backtester bvs-test consumers must not build binary sidecars")
    if "managed-target-bvs-v1-" in consumer_text or "test-target-cache" in consumer_text:
        errors.append("backtester bvs-test consumers must not restore the managed target cache")
    if gate_job is not None and (
        "issue_789" in extract_needs(gate_job) or "needs.issue_789.result" in gate_text
    ):
        errors.append("backtester diagnostic issue-789 lane must not gate merge proof")

    archive_fragments = [
        ("backtester bvs-test archive must use archive job name", "name: bvs-test archive"),
        ("backtester bvs-test archive must declare archive path", "BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst"),
        ("backtester bvs-test archive must declare sidecar path", "BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz"),
        (
            "backtester bvs-test archive must restore nextest archive cache explicitly",
            "id: bvs-nextest-archive-cache",
        ),
        (
            "backtester bvs-test archive must restore caches through pinned actions/cache",
            "uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae",
        ),
        (
            "backtester bvs-test archive cache key must be exact and content-addressed",
            "key: bvs-nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles(",
        ),
        (
            "backtester bvs-test archive must restore binary sidecar cache",
            "id: bvs-bin-sidecars-cache",
        ),
        (
            "backtester bvs-test archive must upload the run-scoped test payload artifact",
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
        ),
        (
            "backtester bvs-test archive must publish the bvs-test-payload artifact",
            "name: bvs-test-payload",
        ),
        (
            "backtester bvs-test archive payload upload must include hidden files (.nextest-archive dot-dir)",
            "include-hidden-files: true",
        ),
        (
            "backtester bvs-test archive payload upload must fail closed when the payload is empty",
            "if-no-files-found: error",
        ),
        (
            "backtester bvs-test sidecar cache key must be exact and content-addressed",
            "key: bvs-bin-sidecars-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles(",
        ),
        (
            "backtester bvs-test archive must resolve target only for cache misses",
            "if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must save shared registry cache from the archive producer only",
            "save-if: ${{ github.job == 'test-archive' }}",
        ),
        (
            "backtester bvs-test archive must build archive only on cache miss",
            "if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must build a nextest archive",
            'just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH"',
        ),
        (
            "backtester bvs-test archive must save nextest archive cache explicitly",
            "uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae",
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
            "backtester bvs-test archive sidecars must use managed cargo",
            'python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo crates/backtesting-vertical-slice -- build --locked --bins',
        ),
        (
            "backtester bvs-test archive must pack sidecars from target debug",
            "find debug -maxdepth 1 -type f -perm -111 -print0",
        ),
        (
            "backtester bvs-test archive must save binary sidecar cache",
            "Save BVS binary sidecars",
        ),
        (
            "backtester bvs-test archive must restore target cache only while producing caches",
            "if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'",
        ),
        (
            "backtester bvs-test archive must save target cache only after archive/sidecar misses",
            "if: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}",
        ),
    ]
    test_fragments = [
        ("backtester bvs-test shards must name matrix shards", "name: bvs-test ${{ matrix.shard }} of 4"),
        (
            "backtester bvs-test shards must depend on archive producer",
            "needs: [ci-policy, detect, fmt, test-archive]",
        ),
        (
            "backtester bvs-test shards must only run after archive producer succeeds",
            "needs.test-archive.result == 'success'",
        ),
        ("backtester bvs-test shards must keep fail-fast disabled", "fail-fast: false"),
        ("backtester bvs-test shards must define four nextest shards", "shard: [1, 2, 3, 4]"),
        ("backtester bvs-test shards must declare archive path", "BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst"),
        ("backtester bvs-test shards must declare sidecar path", "BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz"),
        ("backtester bvs-test shards must declare four archive partitions", 'BVS_NEXTEST_SHARDS: "4"'),
        (
            "backtester bvs-test shards must download the run-scoped test payload artifact",
            "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
        ),
        (
            "backtester bvs-test shards must download the bvs-test-payload artifact by name",
            "name: bvs-test-payload",
        ),
        (
            "backtester bvs-test shards must extract binary sidecars",
            'tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"',
        ),
        (
            "backtester bvs-test shards must create archive extract root",
            'mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"',
        ),
        (
            "backtester bvs-test shards must exclude dedicated issue-789 lane",
            "-- --skip issue_789_first_real_free_data_taker_pl",
        ),
        (
            "backtester bvs-test shards must run one partition from local archive",
            'just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${{ matrix.shard }}/${{ env.BVS_NEXTEST_SHARDS }}" -- --skip issue_789_first_real_free_data_taker_pl',
        ),
    ]
    issue_fragments = [
        ("backtester bvs-test issue-789 must use dedicated job name", "name: bvs-test issue-789"),
        (
            "backtester bvs-test issue-789 must depend on archive producer and backtester-gate",
            "needs: [ci-policy, detect, test-archive, gate]",
        ),
        (
            "backtester bvs-test issue-789 must only run after archive producer succeeds",
            "needs.test-archive.result == 'success'",
        ),
        (
            "backtester bvs-test issue-789 must only run after required gate succeeds",
            "needs.gate.result == 'success'",
        ),
        ("backtester bvs-test issue-789 must declare archive path", "BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst"),
        ("backtester bvs-test issue-789 must declare sidecar path", "BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz"),
        (
            "backtester bvs-test issue-789 must write the first-P/L artifact path",
            "BOLT_ISSUE_789_RESULT_PATH:",
        ),
        (
            "backtester bvs-test issue-789 must download the run-scoped test payload artifact",
            "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c",
        ),
        (
            "backtester bvs-test issue-789 must download the bvs-test-payload artifact by name",
            "name: bvs-test-payload",
        ),
        (
            "backtester bvs-test issue-789 must extract binary sidecars",
            'tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"',
        ),
        (
            "backtester bvs-test issue-789 must create archive extract root",
            'mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"',
        ),
        (
            "backtester bvs-test issue-789 must run only the dedicated long test",
            'just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" issue_789_first_real_free_data_taker_pl',
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
    for message, fragment in test_fragments:
        if fragment not in job_text:
            errors.append(message)
    for scope_name, scope_job in (("bvs-test shards", test_job), ("bvs-test issue-789", issue_job)):
        if scope_job is None:
            continue
        for var_name, payload_name in (
            ("BVS_NEXTEST_ARCHIVE_PATH", "archive"),
            ("BVS_BIN_SIDECARS_PATH", "sidecars"),
        ):
            guard_prefix = f'test -s "${var_name}"'
            guard_lines: list[str] = []
            for block in step_blocks(scope_job):
                for line in block_run_body(block).splitlines():
                    guard_line = strip_comment(line).strip()
                    if guard_line.startswith(guard_prefix) and (
                        len(guard_line) == len(guard_prefix)
                        or guard_line[len(guard_prefix)] in " \t;|&)"
                    ):
                        guard_lines.append(guard_line)
            if not guard_lines:
                errors.append(
                    f"backtester consumer must fail closed if the downloaded {payload_name} "
                    f"is missing or empty ({scope_name})"
                )
                continue
            for guard_line in guard_lines:
                if not EXIT_ONE_RE.search(guard_line):
                    errors.append(
                        f"backtester consumer guard is not fail-closed for downloaded "
                        f"{payload_name} ({scope_name}): {guard_line}"
                    )
    if issue_job is not None:
        for message, fragment in issue_fragments:
            if fragment not in issue_text:
                errors.append(message)
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
    "`backtester-gate` after recomputing proof lanes for crate-changing noop/defer\n"
    "# paths. `backtester-gate-iteration` is feedback-only"
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
        errors.extend(workflow_dispatch_input_errors(text, "full_ci"))
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
            f'--pull-request-base-changed "${{{{ {PR_BASE_CHANGED_EXPR} }}}}"',
            '--workflow-dispatch-full-ci "${{ github.event.inputs.full_ci || \'\' }}"',
            "EVENT_SENDER_ID: ${{ github.event.sender.id }}",
            '--ref "${{ github.ref }}"',
        ]:
            if required not in policy_text:
                errors.append(f"backtester draft deferral ci-policy job must include {required}")
        errors.extend(ci_policy_event_sender_command_errors(policy))

    for heavy_job in ("clippy", "test-archive", "test"):
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
            "--job test=${{ needs.test.result }}",
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
    if (
        "github.event.inputs.full_ci == 'true'" not in group_text
        or "dispatch-full" not in group_text
        or "dispatch-iteration" not in group_text
    ):
        errors.append("backtester draft deferral concurrency must split workflow_dispatch full and iteration runs")
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
    for required in ("ci/github-actions-runners.toml", "scripts/ci_provenance.py"):
        if not any(required in line for line in detect_job):
            errors.append(f"backtester detect paths must include {required}")
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


def verify_repo_automation_texts(texts: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for file_name, text in texts.items():
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
        if file_name == "actionlint.yml" or file_name.endswith("/actionlint.yml"):
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
    if not isinstance(value, str) or not value:
        raise ValueError(f"{prefix}.{key} must be a non-empty string")
    return value


def require_config_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if not isinstance(value, int) or value <= 0:
        raise ValueError(f"{prefix}.{key} must be a positive integer")
    return value


def require_config_string_list(parent: dict[str, object], key: str, prefix: str) -> list[str]:
    value = parent.get(key)
    if not isinstance(value, list) or not all(isinstance(item, str) and item for item in value):
        raise ValueError(f"{prefix}.{key} must be a non-empty string list")
    return value


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
    if deploy.get("require_source_event") != "push":
        raise ValueError("ci_provenance.deploy.require_source_event must be push")
    if deploy.get("require_source_branch") != "main":
        raise ValueError("ci_provenance.deploy.require_source_branch must be main")
    if deploy.get("require_gate_check") is not True:
        raise ValueError("ci_provenance.deploy.require_gate_check must be true")

    dispatch = require_config_table(ci_provenance, "dispatch", "ci_provenance")
    require_config_string(dispatch, "workflow_input", "ci_provenance.dispatch")
    run_name_default = require_config_string(dispatch, "run_name_default", "ci_provenance.dispatch")
    run_name_full = require_config_string(dispatch, "run_name_full", "ci_provenance.dispatch")
    run_name_iteration = require_config_string(dispatch, "run_name_iteration", "ci_provenance.dispatch")
    proof_gate_job = require_config_string(dispatch, "proof_gate_job", "ci_provenance.dispatch")
    workflow_name = require_config_string(ci_provenance, "workflow_name", "ci_provenance")
    if run_name_default != workflow_name:
        raise ValueError("ci_provenance.dispatch.run_name_default must match workflow_name")
    if run_name_full == run_name_iteration:
        raise ValueError("ci_provenance.dispatch run_name_full and run_name_iteration must differ")

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
    if non_heavy_jobs != ["detector"]:
        raise ValueError("ci_provenance.docs.non_heavy_required_jobs must be ['detector']")

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
    if api_limits["max_lookback_age_seconds"] > retention_days * 24 * 60 * 60:
        raise ValueError(
            "ci_provenance.api_limits.max_lookback_age_seconds must not exceed artifact retention"
        )

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
    dispatch_cancel = validate_dispatch_cancel_config(data)
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
        if not isinstance(value, int) or value <= 0:
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
        "variables": sorted(tier_to_var.values()),
        "workflows": workflows,
        "ci_provenance": ci_provenance,
        "dispatch_cancel": dispatch_cancel,
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


def load_ci_runner_debug_config(path: pathlib.Path = DEFAULT_RUNNERS_CONFIG) -> dict[str, str]:
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


def verify_ci_runner_debug_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/ci-runner-debug.yml"
    if workflow_name not in workflows:
        return []
    if not DEFAULT_RUNNERS_CONFIG.exists():
        return []
    try:
        debug_config = load_ci_runner_debug_config()
    except (ValueError, tomllib.TOMLDecodeError) as exc:
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


def verify_dispatch_ci_cancel_workflow(workflows: dict[str, str]) -> list[str]:
    workflow_name = ".github/workflows/dispatch-ci-cancel.yml"
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f"{workflow_name} must exist to cancel stale branch workflow_dispatch CI runs"]
    if not DEFAULT_RUNNERS_CONFIG.exists():
        return []
    try:
        config = load_github_actions_runners_config()
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        return [f"github-actions runner config invalid: {exc}"]

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
            "merge-readiness-progress job if-condition must skip Mergify proof PR "
            "metadata-only edited events while preserving all other pull_request runs"
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
    if not DEFAULT_RUNNERS_CONFIG.exists():
        return []
    try:
        config = load_github_actions_runners_config()
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        return [f"github-actions runner config invalid: {exc}"]

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

    permissions = "\n".join(top_level_block(workflow_text, "permissions"))
    for required in (
        "  checks: write",
        "  contents: read",
        "  pull-requests: read",
    ):
        if required not in permissions:
            errors.append(f"{workflow_name} permissions must include {required.strip()}")
    for forbidden in (
        "  contents: write",
        "  pull-requests: write",
        "  actions:",
        "  id-token:",
        "  issues:",
    ):
        if forbidden in permissions:
            errors.append(f"{workflow_name} permissions must not include {forbidden.strip()}")

    jobs = parse_jobs(workflow_text)
    job = jobs.get("coverage-enforcer")
    if job is None:
        errors.append(f"{workflow_name} must define coverage-enforcer job")
        return errors
    job_text = "\n".join(job)
    job_if = job_if_value(job)
    if _normalize_concurrency_text(job_if) != EXPECTED_COVERAGE_ENFORCER_IF:
        errors.append(
            f"{workflow_name} coverage-enforcer job if-condition must skip Mergify "
            "proof PR metadata-only edited events while preserving merge_group and "
            "all other pull_request runs"
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
    for required in (
        "if [ ! -f scripts/coverage_enforcer.py ]; then",
        "coverage-enforcer bootstrap: trusted base tree lacks scripts/coverage_enforcer.py",
        "exit 0",
    ):
        if required not in job_text:
            errors.append(f"{workflow_name} job must guard first-run trusted-base bootstrap")
            break
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


def verify_github_actions_runner_contract(workflows: dict[str, str]) -> list[str]:
    if not DEFAULT_RUNNERS_CONFIG.exists():
        return []
    try:
        config = load_github_actions_runners_config()
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        return [f"github-actions runner config invalid: {exc}"]

    tier_to_var = config["tier_to_var"]
    meter_included_workflows = set(config["meter_included_workflows"])
    workflow_tables = config["workflows"]
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
    if not DEFAULT_RUNNERS_CONFIG.exists():
        return []
    try:
        config = load_github_actions_runners_config()
    except (ValueError, tomllib.TOMLDecodeError) as exc:
        return [f"github-actions runner config invalid: {exc}"]
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


def main() -> int:
    workflow_texts = repo_workflow_texts()
    action_text = DEFAULT_SETUP_ACTION.read_text()
    nextest_config_text = DEFAULT_NEXTEST_CONFIG.read_text()
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
    errors = verify_workflows(workflow_texts, action_text, nextest_config_text)
    errors.extend(verify_github_actions_runner_contract(workflow_texts))
    errors.extend(verify_ci_runner_debug_workflow(workflow_texts))
    errors.extend(verify_dispatch_ci_cancel_workflow(workflow_texts))
    ci_workflow = workflow_texts.get(".github/workflows/ci.yml")
    if ci_workflow is not None:
        errors.extend(verify_merge_readiness_ci_job(ci_workflow))
    errors.extend(verify_merge_readiness_finalizer_workflow(workflow_texts))
    errors.extend(verify_coverage_enforcer_workflow(workflow_texts))
    errors.extend(verify_actionlint_runner_contract(workflow_texts))
    errors.extend(verify_repo_automation_texts(repo_automation_texts))
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
    # Mirror verify_github_actions_runner_contract's gating: the runners config drives
    # this check, so a partial repo (or test harness) without it is tolerated, not failed.
    if DEFAULT_RUNNERS_CONFIG.exists():
        errors.extend(mergify_proof_prefix_alignment_errors(load_config(DEFAULT_RUNNERS_CONFIG)))
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: CI workflow hygiene verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
