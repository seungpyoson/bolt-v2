#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import ast
import contextlib
import dataclasses
import functools
import io
import importlib.util
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
from typing import Callable

from ci_test_manifest import CiTestManifest
from ci_workflow_hygiene_test_helpers import (
    BASE_ACTION,
    BASE_ADVISORY_WORKFLOW,
    BASE_BVS_RUST_VERIFICATION_POLICY,
    BASE_NEXTEST_CONFIG,
    BASE_RUST_VERIFICATION_POLICY,
    BASE_WORKFLOW,
    DEBUG_TEST_WORKFLOW_PATH,
    GATE_NAME,
    GATE_NEEDS,
    LOCAL_COMPILE_POLICY_TOML,
    LOCAL_LANE_POLICY_TOML,
    REPO_ROOT,
    TEST_HARNESS_MEMBER,
    test_harness_names,
    VERIFIER_PATH,
    all_standalone_live_node_manifest,
    assert_error,
    assert_no_inline_matrix_key,
    base_test_harness_manifest,
    ci_provenance_config_fixture,
    commit_repo,
    copy_self_authorizing_base_tree,
    inline_matrix_values,
    init_self_authorizing_fixture_repo,
    load_provenance,
    load_verifier,
    replace_once,
    replace_once_after,
    repo_source_text,
    repo_workflow_text,
    run_repo_git,
    run_verifier_main_with_no_mistakes,
    runner_config_load_error,
    shard_partition_argument_denominators,
    without_inline_need,
    workflow_with_detector_probe,
    write_repo_workflows,
    write_repo_text,
    write_runner_config_fixture,
    write_rust_verification_policy_fixtures,
    write_storage_tripwire_policy_fixture,
    write_temp_runner_config,
    write_test_harness_fixture,
    yaml_scalar_literal,
)

SYNC_CI_DEBUG_SSH_PATH = pathlib.Path(__file__).resolve().parents[1] / "scripts" / "sync_ci_debug_ssh_secret.py"
DEBUG_WORKFLOW_PATH = ".github/workflows/ci-runner-debug.yml"
SSH_RUNNER_ACTION = "ubicloud/ssh-runner@b6ccad69f047c476b84a54a990f89b1ea5f2a828"
DEPLOY_NEEDS = "needs: [gate, same-sha-main-evidence, build, detector, deny, clippy, check-aarch64, source-fence, test]"
FORBIDDEN_MANAGED_TARGET_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
    "'ci/rust-verification.toml'",
    "'justfile'",
    "'scripts/command_understanding.py'",
    "'scripts/rust_verification.py'",
)
MAIN_ONLY_SHARED_REGISTRY_SAVE_IF = "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}"
BINANCE_TIMESTAMP_PROOF_STEP_NAME = (
    "Run Binance SBE timestamp proof from nextest archive"
)
BINANCE_TIMESTAMP_PROOF_BINARY = "binance_sbe_quote_timestamps"
BINANCE_TIMESTAMP_PROOF_CASES = (
    "sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps",
    "sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps",
    "sbe_depth_snapshot_preserves_unequal_event_and_adapter_initialization_stamps",
    "sbe_depth_diff_preserves_unequal_event_and_adapter_initialization_stamps",
)
BINANCE_TIMESTAMP_PROOF_EXTRACT_ROOT = (
    "$RUNNER_TEMP/binance-sbe-timestamp-proof-extract"
)


def binance_timestamp_proof_command(case_name: str) -> str:
    return (
        'just test-archive-filtered-run "$NEXTEST_ARCHIVE_PATH" "$proof_extract_root" '
        f"'binary(={BINANCE_TIMESTAMP_PROOF_BINARY}) & "
        f"test(={case_name})'"
    )


BINANCE_TIMESTAMP_PROOF_COMMANDS = tuple(
    binance_timestamp_proof_command(case_name)
    for case_name in BINANCE_TIMESTAMP_PROOF_CASES
)
BINANCE_TIMESTAMP_PROOF_STEP = (
    f"      - name: {BINANCE_TIMESTAMP_PROOF_STEP_NAME}\n"
    "        shell: bash\n"
    "        run: |\n"
    "          set -euo pipefail\n"
    f'          proof_extract_root="{BINANCE_TIMESTAMP_PROOF_EXTRACT_ROOT}"\n'
    '          mkdir -p "$proof_extract_root"\n'
    + "".join(f"          {command}\n" for command in BINANCE_TIMESTAMP_PROOF_COMMANDS)
)


def load_sync_ci_debug_ssh_script(
    path: pathlib.Path = SYNC_CI_DEBUG_SSH_PATH, module_name: str = "sync_ci_debug_ssh_secret"
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load sync_ci_debug_ssh_secret.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module



BOLT_V2_BINARY_UPLOAD_IF = "${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}"
BOLT_V2_BINARY_UPLOAD_IF_LINE = f"        if: {BOLT_V2_BINARY_UPLOAD_IF}\n"
BOLT_V2_BINARY_UPLOAD_WITH_BLOCK = """          name: bolt-v2-binary
          path: |
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256
          retention-days: 3"""


BASE_DISPATCH_CI_CANCEL_WORKFLOW = """
name: Dispatch CI Cancel

on:
  workflow_run:
    workflows: ["CI"]
    types: [requested]

permissions:
  contents: read
  actions: write

jobs:
  cancel-obsolete-dispatch:
    name: cancel-obsolete-dispatch
    if: >-
      ${{ github.event.workflow_run.event == 'workflow_dispatch'
          && github.event.workflow_run.path == '.github/workflows/ci.yml' }}
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - uses: actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0
        with:
          python-version: "3.12"
      - name: Cancel older same-branch dispatch runs
        env:
          GITHUB_TOKEN: ${{ github.token }}
          GITHUB_EVENT_PATH: ${{ github.event_path }}
          GITHUB_REPOSITORY: ${{ github.repository }}
        run: python3 scripts/cancel_obsolete_dispatch_runs.py
"""


BASE_MERGE_READINESS_FINALIZER_WORKFLOW = """
name: Merge Readiness Finalizer

on:
  workflow_run:
    workflows: ["CI"]
    types: [completed]

permissions:
  contents: read
  checks: read
  actions: read
  pull-requests: write

jobs:
  mark-stalled:
    name: mark-stalled
    if: >-
      ${{ github.event.workflow_run.event == 'pull_request'
          && github.event.workflow_run.path == '.github/workflows/ci.yml' }}
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          persist-credentials: false

      - uses: actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0
        with:
          python-version: "3.12"

      - name: Mark stalled merge-readiness comment
        env:
          GITHUB_TOKEN: ${{ github.token }}
          GITHUB_EVENT_PATH: ${{ github.event_path }}
          GITHUB_REPOSITORY: ${{ github.repository }}
        run: python3 scripts/merge_readiness.py finalize-stalled
"""


BASE_COVERAGE_ENFORCER_WORKFLOW = """
name: Coverage Enforcer

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]
  merge_group:
    types: [checks_requested]

concurrency:
  group: >-
    coverage-${{ github.event_name == 'pull_request'
        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !((github.event.action == 'edited'
              && !(github.event.changes.base.ref.from && true || false))
             || (github.event.pull_request.draft == true
                 && github.event.action == 'converted_to_draft'))
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)
        || github.event_name == 'pull_request'
        && ((github.event.pull_request.draft == true
             && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action))
            || (github.event.pull_request.draft == false
                && github.event.action == 'edited'
                && !(github.event.changes.base.ref.from && true || false)))
        && format('pr-{0}-iteration', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'merge_group'
        && format('mq-{0}', github.ref)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}

permissions:
  checks: read
  contents: read
  pull-requests: read

jobs:
  coverage-enforcer:
    name: coverage-enforcer
    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          ref: ${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}
          persist-credentials: false

      - uses: actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0
        with:
          python-version: "3.12"

      - name: Enforce coverage map
        env:
          GITHUB_TOKEN: ${{ github.token }}
          GITHUB_EVENT_PATH: ${{ github.event_path }}
          GITHUB_REPOSITORY: ${{ github.repository }}
        run: |
          if [ ! -f scripts/coverage_enforcer.py ]; then
            echo "coverage-enforcer bootstrap fail-closed: trusted base tree lacks scripts/coverage_enforcer.py"
            exit 1
          fi
          if ! grep -q "def expected_registry_checks_for_policy" scripts/coverage_enforcer.py; then
            echo "coverage-enforcer bootstrap fail-closed: trusted base tree lacks event-aware scripts/coverage_enforcer.py"
            exit 1
          fi
          python3 scripts/coverage_enforcer.py
"""









def live_node_manifest_with(
    verifier,
    *,
    consolidated: dict[str, str] | None = None,
) -> CiTestManifest:
    member_to_harness = {member: member for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    for member, harness in (consolidated or {}).items():
        member_to_harness[member] = harness
        member_to_harness.setdefault(harness, harness)

    harness_members: dict[str, list[str]] = {}
    for member, harness in member_to_harness.items():
        harness_members.setdefault(harness, []).append(member)
    harness_to_members = {
        harness: tuple(members)
        for harness, members in harness_members.items()
    }
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)


def assert_nextest_clean(nextest_config: str, manifest: CiTestManifest) -> None:
    verifier = load_verifier()
    errors = verifier.verify_nextest_config(nextest_config, manifest=manifest)
    if errors:
        raise AssertionError(f"expected no nextest errors, got: {errors}")


def assert_nextest_error(fragment: str, nextest_config: str, manifest: CiTestManifest) -> None:
    verifier = load_verifier()
    errors = verifier.verify_nextest_config(nextest_config, manifest=manifest)
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected nextest error containing {fragment!r}, got: {errors}")








def test_harness_manifest_errors(
    *,
    manifest: CiTestManifest | None = None,
    cargo_autotests: str = "false",
    test_files: dict[str, str] | None = None,
    workflow_text: str = "jobs:\n  test:\n    steps:\n      - run: cargo test --test pricing\n",
    justfile_text: str = "ci-test:\n    cargo test --test iv\n",
) -> list[str]:
    verifier = load_verifier()
    manifest = manifest or base_test_harness_manifest()
    verifier.build_test_manifest = lambda _manifest_path, _tests_root: manifest
    with tempfile.TemporaryDirectory() as temp_dir:
        root = pathlib.Path(temp_dir)
        write_test_harness_fixture(
            root,
            manifest=manifest,
            cargo_autotests=cargo_autotests,
            test_files=test_files,
            workflow_text=workflow_text,
            justfile_text=justfile_text,
        )
        return verifier.verify_test_harness_manifest(
            cargo_manifest_path=root / "Cargo.toml",
            tests_root=root / "tests",
            workflow_path=root / ".github" / "workflows" / "ci.yml",
            justfile_path=root / "justfile",
        )


def assert_test_harness_manifest_clean(**kwargs) -> None:
    errors = test_harness_manifest_errors(**kwargs)
    if errors:
        raise AssertionError(f"expected no test harness manifest errors, got: {errors}")


def assert_test_harness_manifest_error(fragment: str, **kwargs) -> None:
    errors = test_harness_manifest_errors(**kwargs)
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected test harness manifest error containing {fragment!r}, got: {errors}")













def assert_clean(
    workflow: str = BASE_WORKFLOW,
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action, nextest_config)
    if workflow != repo_workflow_text(".github/workflows/ci.yml"):
        errors = [error for error in errors if "ROOT_TEST_ARCHIVE_JOB_SHA256" not in error]
    if errors:
        raise AssertionError(f"expected no errors, got: {errors}")


def assert_workflows_clean(
    workflows: dict[str, str],
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_workflows(workflows, action, nextest_config)
    if errors:
        raise AssertionError(f"expected no errors, got: {errors}")


def assert_workflows_error(
    fragment: str,
    workflows: dict[str, str],
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_workflows(workflows, action, nextest_config)
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected error containing {fragment!r}, got: {errors}")
















JULES_ADVISORY_GOOD_WORKFLOW = """\
name: Jules Weekly Cleanup

on:
  schedule:
    - cron: "17 8 * * 1"
  workflow_dispatch: {}

permissions: {}

jobs:
  jules-weekly-cleanup:
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    steps:
      - name: Start Jules advisory session
        id: invoke-jules
        continue-on-error: true
        timeout-minutes: ${{ fromJSON(vars.JULES_SESSION_TIMEOUT_MINUTES) }}
        env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
          JULES_SESSIONS_ENDPOINT: ${{ vars.JULES_SESSIONS_ENDPOINT }}
          JULES_STARTING_BRANCH: ${{ github.event.repository.default_branch }}
          JULES_PROMPT: |
            You are Jules running code-maintenance automation for this repository.
            - You are advisory only and not a required gate.
            - Create a draft pull request only when you have a small verified cleanup change.
            - Label any pull request with agent:jules.
            - No AWS access.
            - No trading, runtime, deploy, live, market data, or order execution access.
        run: |
          jq -n \\
            --arg repo_full_name "$GITHUB_REPOSITORY" \\
            --arg branch "$JULES_STARTING_BRANCH" \\
            --arg prompt "$JULES_PROMPT" \\
            '{
              prompt: $prompt,
              sourceContext: {
                source: "sources/github/\\($repo_full_name)",
                githubRepoContext: {
                  startingBranch: $branch
                }
              },
              automationMode: "AUTO_CREATE_PR",
              requirePlanApproval: true
            }' > "$RUNNER_TEMP/jules_payload.json"

          curl --silent --show-error --fail-with-body \\
            --request POST \\
            --header "Content-Type: application/json" \\
            --header "x-goog-api-key: ${JULES_API_KEY}" \\
            --data @"$RUNNER_TEMP/jules_payload.json" \\
            --output "$RUNNER_TEMP/jules_response.json" \\
            "$JULES_SESSIONS_ENDPOINT"

          jq -e '.name and .id' "$RUNNER_TEMP/jules_response.json" >/dev/null

      - name: Jules advisory success notice
        if: ${{ steps.invoke-jules.outcome == 'success' }}
        run: |
          echo "::notice::Jules advisory session started and returned a session id. Jules remains non-blocking and emits no merge signal."

      - name: Jules advisory unavailable warning
        if: ${{ steps.invoke-jules.outcome != 'success' }}
        run: |
          echo "::warning::Jules advisory session did not start. Unavailable Jules results remain advisory only and emit no merge signal."
"""


def assert_jules_advisory_workflow_contracts() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/weekly-cleanup.yml"
    clean_errors = verifier.verify_repo_automation_texts(
        {workflow_name: JULES_ADVISORY_GOOD_WORKFLOW}
    )
    if clean_errors:
        raise AssertionError(f"known-good Jules workflow must be clean, got: {clean_errors}")

    cases = (
        (
            "Jules advisory workflows must require plan approval",
            replace_once(JULES_ADVISORY_GOOD_WORKFLOW, "requirePlanApproval: true", "requirePlanApproval: false"),
        ),
        (
            "Jules advisory workflows must use configured session timeout variable",
            replace_once(
                JULES_ADVISORY_GOOD_WORKFLOW,
                "timeout-minutes: ${{ fromJSON(vars.JULES_SESSION_TIMEOUT_MINUTES) }}",
                "timeout-minutes: 10",
            ),
        ),
        (
            "Jules advisory workflows must use configured sessions endpoint variable",
            replace_once(
                JULES_ADVISORY_GOOD_WORKFLOW,
                '"$JULES_SESSIONS_ENDPOINT"',
                '"https://jules.googleapis.com/v1alpha/sessions"',
            ),
        ),
        (
            "Jules advisory workflows must emit verified session notice only on invoke success",
            replace_once(
                JULES_ADVISORY_GOOD_WORKFLOW,
                "if: ${{ steps.invoke-jules.outcome == 'success' }}",
                "if: ${{ always() }}",
            ),
        ),
        (
            "Jules advisory workflows must warn when invocation is unavailable",
            replace_once(
                JULES_ADVISORY_GOOD_WORKFLOW,
                "if: ${{ steps.invoke-jules.outcome != 'success' }}",
                "if: ${{ steps.invoke-jules.outcome == 'success' }}",
            ),
        ),
        (
            "Jules advisory workflows must not use AWS commands",
            replace_once(
                JULES_ADVISORY_GOOD_WORKFLOW,
                "          jq -e '.name and .id' \"$RUNNER_TEMP/jules_response.json\" >/dev/null",
                "          jq -e '.name and .id' \"$RUNNER_TEMP/jules_response.json\" >/dev/null\n          aws sts get-caller-identity",
            ),
        ),
    )
    for fragment, workflow in cases:
        errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected Jules contract error containing {fragment!r}, got: {errors}")

    secret_errors = verifier.verify_repo_automation_texts(
        {
            ".github/workflows/ci.yml": """\
jobs:
  test:
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    steps:
      - env:
          JULES_API_KEY: ${{ secrets.JULES_API_KEY }}
        run: echo no
"""
        }
    )
    if not any("JULES_API_KEY may only be used by Jules advisory workflows" in error for error in secret_errors):
        raise AssertionError(f"Jules secret boundary drift was silent: {secret_errors}")

    real_jules_workflows = {
        path: repo_workflow_text(path)
        for path in (
            ".github/workflows/weekly-cleanup.yml",
            ".github/workflows/performance-improver.yml",
            ".github/workflows/tech-debt-review.yml",
        )
    }
    real_errors = verifier.verify_repo_automation_texts(real_jules_workflows)
    if real_errors:
        raise AssertionError(f"real Jules workflows must satisfy advisory contract, got: {real_errors}")


def assert_jules_advisory_config_carries_repo_variable_values() -> None:
    verifier = load_verifier()
    config = verifier.load_github_actions_runners_config()
    jules_config = config["jules_advisory"]
    actual_values = jules_config.get("repository_variables")
    expected_keys = {
        jules_config["sessions_endpoint_variable"],
        jules_config["session_timeout_minutes_variable"],
    }
    if not isinstance(actual_values, dict) or set(actual_values) != expected_keys:
        raise AssertionError(
            "Jules advisory config must carry provisionable repository variable values, "
            f"got: {actual_values!r}"
        )
    for key, value in actual_values.items():
        if not isinstance(value, str) or not value:
            raise AssertionError(f"Jules advisory repository variable {key} must have a non-empty value")

    config_text = ci_provenance_config_fixture()
    cases = (
        (
            "jules_advisory.sessions_endpoint must be a non-empty string",
            re.sub(r'^sessions_endpoint = ".+"\n', "", config_text, count=1, flags=re.MULTILINE),
        ),
        (
            "jules_advisory.session_timeout_minutes must be a positive integer",
            re.sub(r"^session_timeout_minutes = [0-9]+\n", "", config_text, count=1, flags=re.MULTILINE),
        ),
    )
    for fragment, broken_config in cases:
        error = runner_config_load_error(broken_config, verifier)
        if fragment not in error:
            raise AssertionError(f"expected Jules config error containing {fragment!r}, got: {error!r}")


def artifact_retention_policy_errors(
    workflows: dict[str, str] | None = None,
    composite_actions: dict[str, str] | None = None,
    verifier=None,
) -> list[str]:
    if verifier is None:
        verifier = load_verifier()
    if workflows is None:
        workflows = {
            ".github/workflows/ci.yml": BASE_WORKFLOW,
            ".github/workflows/backtester-ci.yml": repo_workflow_text(".github/workflows/backtester-ci.yml"),
        }
    if composite_actions is None:
        composite_actions = {}
    return verifier.verify_artifact_retention_policy(workflows, composite_actions)


def assert_artifact_retention_error(
    fragment: str,
    workflows: dict[str, str] | None = None,
    composite_actions: dict[str, str] | None = None,
    verifier=None,
) -> None:
    errors = artifact_retention_policy_errors(workflows, composite_actions, verifier)
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected artifact retention error containing {fragment!r}, got: {errors}")


def without_job(workflow: str, job: str) -> str:
    lines = workflow.splitlines()
    start = next(i for i, line in enumerate(lines) if line == f"  {job}:")
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("  ") and not lines[i].startswith("    ") and lines[i].strip().endswith(":"):
            end = i
            break
    return "\n".join(lines[:start] + lines[end:]) + "\n"




def replace_config_key_line_once(config: str, section: str, key: str, new_line: str | None) -> str:
    # Tunable config values (e.g. cargo_build_jobs caps) change over time; anchor
    # fixture mutations on section+key so the mutation fails loud instead of
    # silently no-oping when the live value drifts.
    header = f"[{section}]\n"
    start = config.find(header)
    if start == -1:
        raise AssertionError(f"config section not found: {section!r}")
    body_start = start + len(header)
    next_section = config.find("\n[", body_start)
    body_end = len(config) if next_section == -1 else next_section + 1
    body = config[body_start:body_end]
    match = re.search(rf"(?m)^{re.escape(key)} = .+\n", body)
    if match is None:
        raise AssertionError(f"config key not found in section {section!r}: {key!r}")
    replacement = "" if new_line is None else new_line + "\n"
    return config[:body_start] + body[: match.start()] + replacement + body[match.end() :] + config[body_end:]


def base_upload_artifact_action_line() -> str:
    return next(
        line.strip()
        for line in BASE_WORKFLOW.splitlines()
        if "uses: actions/upload-artifact@" in line
    )












def mutate_named_step_after(
    text: str,
    anchor: str,
    step_name: str,
    mutator: Callable[[list[str]], list[str]],
) -> str:
    index = text.find(anchor)
    if index == -1:
        raise AssertionError(f"fixture anchor not found: {anchor!r}")
    before = text[:index]
    lines = text[index:].splitlines(keepends=True)
    start: int | None = None
    end = len(lines)
    step_indent = 0
    for line_index, line in enumerate(lines):
        if line.strip() != f"- name: {step_name}":
            continue
        start = line_index
        step_indent = len(line) - len(line.lstrip(" "))
        break
    if start is None:
        raise AssertionError(f"fixture step not found after {anchor!r}: {step_name!r}")
    for line_index in range(start + 1, len(lines)):
        line = lines[line_index]
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent < step_indent or (indent == step_indent and line.lstrip().startswith("- ")):
            end = line_index
            break
    return before + "".join(lines[:start] + mutator(lines[start:end]) + lines[end:])


def insert_step_before_named_step_after(text: str, anchor: str, step_name: str, step: str) -> str:
    step_lines = step.splitlines(keepends=True)
    if not step.endswith("\n"):
        step_lines.append("\n")
    return mutate_named_step_after(text, anchor, step_name, lambda block: step_lines + block)


def without_once_after(text: str, anchor: str, old: str) -> str:
    index = text.find(anchor)
    if index == -1:
        raise AssertionError(f"fixture anchor not found: {anchor!r}")
    before = text[:index]
    after = text[index:]
    return before + after.replace(old, "", 1)


def assert_binance_timestamp_archive_execution_proof_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = BASE_WORKFLOW
    expected_error = "test-archive Binance SBE timestamp execution proof"

    mutations = [
        workflow.replace(BINANCE_TIMESTAMP_PROOF_STEP, "", 1),
        replace_once(
            workflow,
            f"      - name: {BINANCE_TIMESTAMP_PROOF_STEP_NAME}\n",
            "      - name: Run renamed Binance SBE timestamp proof\n",
        ),
        replace_once_after(
            workflow,
            BINANCE_TIMESTAMP_PROOF_STEP_NAME,
            f"binary(={BINANCE_TIMESTAMP_PROOF_BINARY})",
            "binary(=renamed_timestamp_harness)",
        ),
    ]
    for case_name, command in zip(
        BINANCE_TIMESTAMP_PROOF_CASES,
        BINANCE_TIMESTAMP_PROOF_COMMANDS,
    ):
        mutations.extend(
            (
                replace_once_after(
                    workflow,
                    BINANCE_TIMESTAMP_PROOF_STEP_NAME,
                    f"          {command}\n",
                    "",
                ),
                replace_once_after(
                    workflow,
                    BINANCE_TIMESTAMP_PROOF_STEP_NAME,
                    f"test(={case_name})",
                    f"test(~{case_name})",
                ),
                replace_once_after(
                    workflow,
                    BINANCE_TIMESTAMP_PROOF_STEP_NAME,
                    command,
                    command.replace("test-archive-filtered-run", "test-archive-run", 1),
                ),
            )
        )

    for mutated in mutations:
        errors = [
            error
            for error in verifier.verify_workflow(mutated)
            if "ROOT_TEST_ARCHIVE_JOB_SHA256" not in error
        ]
        if not any(expected_error in error for error in errors):
            raise AssertionError(
                "Binance timestamp archive execution proof drift was silent: "
                f"{errors}"
            )


def assert_test_archive_filtered_run_recipe_preserves_filterset_argv() -> None:
    verifier = load_verifier()
    expected_error = "test-archive filtered run recipe"
    generic_recipe = """test-archive-run archive extract_root *args: check-workspace require-rust-verification-owner
    python3 \"{{rust_verification_owner}}\" cargo --repo \"{{repo_root}}\" -- nextest run --archive-file \"{{archive}}\" --extract-to \"{{extract_root}}\" --extract-overwrite --workspace-remap \"{{repo_root}}\" {{args}}
"""
    dedicated_recipe = """test-archive-filtered-run archive extract_root filterset: check-workspace require-rust-verification-owner
    python3 \"{{rust_verification_owner}}\" cargo --repo \"{{repo_root}}\" -- nextest run --archive-file \"{{archive}}\" --extract-to \"{{extract_root}}\" --extract-overwrite --workspace-remap \"{{repo_root}}\" --no-tests=fail -E {{quote(filterset)}}
"""

    def filtered_recipe_errors(justfile_text: str) -> list[str]:
        return [
            error
            for error in verifier.verify_repo_automation_texts({"justfile": justfile_text})
            if expected_error in error
        ]

    missing_errors = filtered_recipe_errors(generic_recipe)
    if not missing_errors:
        raise AssertionError(
            "missing dedicated filtered archive recipe was silently accepted"
        )

    unquoted_errors = filtered_recipe_errors(
        generic_recipe + dedicated_recipe.replace("{{quote(filterset)}}", "{{filterset}}")
    )
    if not unquoted_errors:
        raise AssertionError(
            "unquoted filtered archive recipe did not prove filterset argv delivery"
        )

    variadic_errors = filtered_recipe_errors(
        generic_recipe
        + dedicated_recipe.replace(
            "archive extract_root filterset:", "archive extract_root *filterset:"
        )
    )
    if not variadic_errors:
        raise AssertionError(
            "variadic filtered archive recipe was silently accepted"
        )

    safe_errors = filtered_recipe_errors(generic_recipe + dedicated_recipe)
    if safe_errors:
        raise AssertionError(
            "safely quoted filtered archive recipe was rejected: " f"{safe_errors}"
        )










def one_indexed_sequence(values: tuple[int, ...]) -> bool:
    return values == tuple(range(1, len(values) + 1))




def strip_ci_provenance_config(config_text: str) -> str:
    lines = config_text.splitlines()
    kept: list[str] = []
    skip = False
    for line in lines:
        if line.startswith("[ci_provenance"):
            skip = True
            continue
        if skip and line.startswith("["):
            skip = False
        if not skip:
            kept.append(line)
    return "\n".join(kept).rstrip() + "\n"






def assert_ci_provenance_config_contract() -> None:
    valid = ci_provenance_config_fixture()
    if runner_config_load_error(valid):
        raise AssertionError("valid ci_provenance fixture must load")

    cases = [
        (
            "ci/github-actions-runners.toml must define [ci_provenance]",
            strip_ci_provenance_config(valid),
        ),
        (
            "ci_provenance.full_ci.jobs.test missing",
            valid.replace(
                """
[ci_provenance.full_ci.jobs.test]
check_name = "test"
""",
                "",
            ),
        ),
        (
            "must reference [meter] fingerprint keys",
            valid.replace(
                'fingerprint_source = "meter"',
                'fingerprint_source = "meter"\nfingerprint_artifact_prefix = "nextest-archive-fingerprint-"',
            ),
        ),
        (
            "ci_provenance.policy.override.force_full_ci must default to false",
            valid.replace("force_full_ci = false\n", ""),
        ),
        (
            "ci_provenance.policy.override.ignore_emit_failure must default to false",
            valid.replace("ignore_emit_failure = false\n", ""),
        ),
        (
            "ci_provenance.dispatch.run_name_default must match workflow_name",
            valid.replace('workflow_name = "CI"', 'workflow_name = "CI Main"'),
        ),
        (
            "ci_provenance.dispatch.proof_gate_job must match required gate name",
            valid.replace('proof_gate_job = "gate"', 'proof_gate_job = "gate-iteration"'),
        ),
        (
            "ci_provenance.dispatch.proof_gate_job must match required gate name",
            valid.replace('gate_required = "gate"', 'gate_required = "renamed-gate"'),
        ),
        (
            "ci_provenance.gate_names.gate_iteration must not equal backtester_required",
            valid.replace('gate_iteration = "gate-iteration"', 'gate_iteration = "backtester-gate"'),
        ),
        (
            "ci_provenance.gate_names.gate_iteration must not equal gate_required",
            valid.replace('gate_iteration = "gate-iteration"', 'gate_iteration = "gate"'),
        ),
        (
            "ci_provenance.gate_names.backtester_iteration must not equal backtester_required",
            valid.replace('backtester_iteration = "backtester-gate-iteration"', 'backtester_iteration = "backtester-gate"'),
        ),
        (
            "ci_provenance.gate_names.gate_iteration must be a GitHub Actions output-safe check name",
            valid.replace('gate_iteration = "gate-iteration"', 'gate_iteration = "${{ github.ref }}"'),
        ),
        (
            "ci_provenance.policy.ready_pr is proof-affecting",
            valid.replace('ready_pr = "full"', 'ready_pr = "iteration"'),
        ),
        (
            "ci_provenance.policy.ready_for_review is proof-affecting",
            valid.replace('ready_for_review = "full"', 'ready_for_review = "iteration"'),
        ),
        (
            "ci_provenance.policy.main_push is proof-affecting",
            valid.replace('main_push = "full"', 'main_push = "iteration"'),
        ),
        (
            "ci_provenance.policy.main_push is proof-affecting and must be full",
            valid.replace('main_push = "full"', 'main_push = "tag_reuse"'),
        ),
        (
            "ci_provenance.policy.merge_group is proof-affecting",
            valid.replace('merge_group = "full"', 'merge_group = "iteration"'),
        ),
        (
            "ci_provenance.policy.merge_group is proof-affecting and must be full",
            valid.replace('merge_group = "full"', 'merge_group = "tag_reuse"'),
        ),
        (
            "ci_provenance.policy.mergify_temp_pr is proof-affecting",
            valid.replace('mergify_temp_pr = "full"', 'mergify_temp_pr = "iteration"'),
        ),
        (
            "ci_provenance.policy.unknown_event is proof-affecting",
            valid.replace('unknown_event = "full"', 'unknown_event = "iteration"'),
        ),
        (
            "ci_provenance.policy.unknown_event is proof-affecting and must be full",
            valid.replace('unknown_event = "full"', 'unknown_event = "tag_reuse"'),
        ),
        (
            "ci_provenance.policy has unexpected keys",
            valid.replace(
                "\nmain_push = \"full\"",
                '\nworkflow_dispatch_full_ci = "full"\nmain_push = "full"',
            ),
        ),
        (
            "ci_provenance.policy.workflow_dispatch must be iteration",
            valid.replace('workflow_dispatch = "iteration"', 'workflow_dispatch = "full"'),
        ),
        (
            "ci_provenance.policy.draft_pr_synchronize must be iteration",
            valid.replace('draft_pr_synchronize = "iteration"', 'draft_pr_synchronize = "full"'),
        ),
        (
            "ci_provenance.policy.converted_to_draft must be iteration",
            valid.replace('converted_to_draft = "iteration"', 'converted_to_draft = "full"'),
        ),
        (
            "ci_provenance.policy.ready_pr must be full",
            valid.replace('ready_pr = "full"', 'ready_pr = "iteration"'),
        ),
        (
            "ci_provenance.policy.ready_for_review must be full",
            valid.replace('ready_for_review = "full"', 'ready_for_review = "iteration"'),
        ),
        (
            "ci_provenance.policy.ready_pr_edited_no_base must be iteration",
            valid.replace('ready_pr_edited_no_base = "iteration"', 'ready_pr_edited_no_base = "full"'),
        ),
        (
            "ci_provenance.policy.ready_pr_reopened must be full",
            valid.replace('ready_pr_reopened = "full"', 'ready_pr_reopened = "iteration"'),
        ),
        (
            "ci_provenance.policy has unexpected keys",
            valid.replace(
                "[ci_provenance.mergify]",
                'unexpected_policy_row = "iteration"\n\n[ci_provenance.mergify]',
            ),
        ),
    ]
    for fragment, config_text in cases:
        error = runner_config_load_error(config_text)
        if fragment not in error:
            raise AssertionError(f"expected {fragment!r}, got {error!r}")

    verifier = load_verifier()

    active_policy = {
        "draft_pr_synchronize": "iteration",
        "draft_pr_opened": "iteration",
        "draft_pr_reopened": "iteration",
        "draft_pr_edited": "iteration",
        "converted_to_draft": "iteration",
        "ready_pr": "full",
        "ready_pr_edited_no_base": "iteration",
        "ready_pr_reopened": "full",
        "ready_for_review": "full",
        "docs": "docs",
        "workflow_dispatch": "iteration",
        "main_push": "full",
        "merge_group": "full",
        "mergify_temp_pr": "full",
        "tag": "tag_reuse",
        "unknown_event": "full",
    }
    active_policy_errors = verifier.policy_proof_invariant_errors(active_policy)
    if active_policy_errors:
        raise AssertionError(f"active proof policy must satisfy the invariant: {active_policy_errors}")

    original_rows = verifier.CI_PROVENANCE_POLICY_ROWS
    original_semantics = verifier.CI_POLICY_ROW_SEMANTICS
    verifier.CI_PROVENANCE_POLICY_ROWS = original_rows + ("synthetic_proof_affecting",)
    verifier.CI_POLICY_ROW_SEMANTICS = {
        **original_semantics,
        "synthetic_proof_affecting": verifier.PolicyRowSemantics(changes_head_sha=True),
    }
    try:
        synthetic = valid.replace(
            "[ci_provenance.mergify]",
            'synthetic_proof_affecting = "iteration"\n\n[ci_provenance.mergify]',
        )
        error = runner_config_load_error(synthetic, verifier=verifier)
        fragment = "ci_provenance.policy.synthetic_proof_affecting is proof-affecting"
        if fragment not in error:
            raise AssertionError(f"expected {fragment!r}, got {error!r}")
    finally:
        verifier.CI_PROVENANCE_POLICY_ROWS = original_rows
        verifier.CI_POLICY_ROW_SEMANTICS = original_semantics


def assert_ci_policy_matrix() -> None:
    verifier = load_verifier()
    config = verifier.validate_ci_provenance_config(
        verifier.tomllib.loads(ci_provenance_config_fixture())
    )
    policy = config["policy"]
    gate_names = config["gate_names"]
    mergify_prefix = str(config["mergify"]["temp_pr_head_ref_prefix"])
    actor_id = int(config["mergify"]["mergify_temp_pr_actor_id"])
    # Draft pull_request rows are the cheap iteration loop. Ready PR opened,
    # synchronize, ready_for_review, and base edits publish the automatic full
    # pull_request signal; ready metadata edits iterate and reopened PRs refresh full proof.
    # The actor-bound mergify temp PR is covered separately below because it
    # depends on the event sender id.
    cases = [
        ("push", "", False, False, "refs/heads/main", "full"),
        ("push", "", False, False, "refs/tags/v1.2.3", "tag_reuse"),
        ("pull_request", "opened", True, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "synchronize", True, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "reopened", True, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "edited", True, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "converted_to_draft", True, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "opened", False, False, "refs/pull/1/merge", "full"),
        ("pull_request", "synchronize", False, False, "refs/pull/1/merge", "full"),
        ("pull_request", "edited", False, False, "refs/pull/1/merge", "iteration"),
        ("pull_request", "edited", False, True, "refs/pull/1/merge", "full"),
        ("pull_request", "reopened", False, False, "refs/pull/1/merge", "full"),
        ("pull_request", "ready_for_review", False, False, "refs/pull/1/merge", "full"),
        ("workflow_dispatch", "", True, False, "refs/heads/codex/branch", "iteration"),
        ("merge_group", "checks_requested", False, False, "refs/heads/gh-readonly-queue/main/pr-1-deadbeef", "full"),
        ("unknown_event", "", True, False, "refs/heads/codex/branch", "full"),
    ]
    if any(expected not in {"full", "iteration", "docs", "tag_reuse"} for *_, expected in cases):
        raise AssertionError("policy matrix contains a removed policy path")
    for event_name, action, draft, base_changed, ref, expected in cases:
        result = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        if result.ci_policy_path != expected:
            raise AssertionError((event_name, action, draft, ref, expected, result))
        if result.full_ci_required != (expected == "full"):
            raise AssertionError(f"full_ci_required must derive from {expected}: {result}")

    try:
        verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name="pull_request",
            action="ready_for_review",
            pull_request_draft=True,
            pull_request_base_changed=False,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref="refs/pull/1/merge",
        )
    except ValueError as exc:
        if "ready_for_review cannot be on a draft PR" not in str(exc):
            raise AssertionError(f"unexpected ready_for_review draft error: {exc}") from exc
    else:
        raise AssertionError("ready_for_review draft event must fail closed")

    # The actor-bound Mergify temp PR also earns the required gate, but only for
    # Mergify full-CI actions.
    mergify_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_result.ci_policy_path != "full"
        or mergify_result.gate_name != "gate"
        or mergify_result.backtester_gate_name != "backtester-gate"
        or mergify_result.reason != "mergify_temp_pr"
    ):
        raise AssertionError(f"Mergify temp PR must resolve to required full CI: {mergify_result}")

    mergify_sync_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="synchronize",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_sync_result.ci_policy_path != "full"
        or mergify_sync_result.gate_name != "gate"
        or mergify_sync_result.backtester_gate_name != "backtester-gate"
        or mergify_sync_result.reason != "mergify_temp_pr"
    ):
        raise AssertionError(f"Mergify temp PR synchronize must resolve to required full CI: {mergify_sync_result}")

    mergify_ready_by_human = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="ready_for_review",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=1376128,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_ready_by_human.ci_policy_path != "full"
        or mergify_ready_by_human.gate_name != "gate"
        or mergify_ready_by_human.backtester_gate_name != "backtester-gate"
        or mergify_ready_by_human.reason != "mergify_temp_pr"
    ):
        raise AssertionError(
            f"human-ready Mergify-authored temp PR must resolve to required full CI: {mergify_ready_by_human}"
        )

    ready_spoof_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="ready_for_review",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=1376128,
        pull_request_author_id=1376128,
        ref="refs/pull/965/merge",
    )
    if (
        ready_spoof_result.reason == "mergify_temp_pr"
        or ready_spoof_result.gate_name != "gate"
        or ready_spoof_result.ci_policy_path != "full"
        or ready_spoof_result.reason != "ready_for_review"
    ):
        raise AssertionError(
            f"non-Mergify-authored ready spoof must fall back to normal ready PR proof: {ready_spoof_result}"
        )

    ready_split_identity_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="ready_for_review",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        pull_request_author_id=1376128,
        ref="refs/pull/965/merge",
    )
    if (
        ready_split_identity_result.reason == "mergify_temp_pr"
        or ready_split_identity_result.gate_name != "gate"
        or ready_split_identity_result.ci_policy_path != "full"
        or ready_split_identity_result.reason != "ready_for_review"
    ):
        raise AssertionError(
            "Mergify-sender ready event with non-Mergify author must fall back to "
            f"normal ready PR proof: {ready_split_identity_result}"
        )

    human_sync_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="synchronize",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=1376128,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        human_sync_result.reason == "mergify_temp_pr"
        or human_sync_result.gate_name != "gate"
        or human_sync_result.ci_policy_path != "full"
        or human_sync_result.reason != "ready_pr"
    ):
        raise AssertionError(f"human-sender Mergify sync must fall back to normal ready PR proof: {human_sync_result}")

    # Mergify title/body edits arrive as draft metadata edits without a base
    # change; they must stay cheap instead of publishing required gates.
    mergify_edited_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_edited_result.ci_policy_path != "iteration"
        or mergify_edited_result.gate_name != "gate-iteration"
        or mergify_edited_result.backtester_gate_name != "backtester-gate-iteration"
        or mergify_edited_result.reason != "draft_pr_edited"
    ):
        raise AssertionError(
            f"Mergify temp PR metadata edits must stay iteration-only: {mergify_edited_result}"
        )

    mergify_base_edit_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=True,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_base_edit_result.ci_policy_path != "full"
        or mergify_base_edit_result.gate_name != "gate"
        or mergify_base_edit_result.backtester_gate_name != "backtester-gate"
        or mergify_base_edit_result.reason != "mergify_temp_pr"
    ):
        raise AssertionError(
            f"Mergify temp PR base edits must publish required gates: {mergify_base_edit_result}"
        )

    ready_mergify_base_edit_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=True,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    if (
        ready_mergify_base_edit_result.ci_policy_path != "full"
        or ready_mergify_base_edit_result.gate_name != "gate"
        or ready_mergify_base_edit_result.backtester_gate_name != "backtester-gate"
        or ready_mergify_base_edit_result.reason != "mergify_temp_pr"
    ):
        raise AssertionError(
            "ready Mergify temp PR base edits must publish required gates: "
            f"{ready_mergify_base_edit_result}"
        )

    # GAP-1: a spoofed mergify head ref from a NON-actor sender must never earn the
    # required gate; it fails closed to the ordinary draft path -> gate-iteration.
    mergify_spoof_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id + 1,
        ref="refs/pull/965/merge",
    )
    if (
        mergify_spoof_result.reason == "mergify_temp_pr"
        or mergify_spoof_result.gate_name != "gate-iteration"
        or mergify_spoof_result.ci_policy_path != "iteration"
    ):
        raise AssertionError(
            f"spoofed mergify head ref must fail closed to gate-iteration: {mergify_spoof_result}"
        )

    forced = dict(policy)
    forced["override"] = dict(policy["override"])
    forced["override"]["force_full_ci"] = True
    forced_result = verifier.evaluate_ci_policy(
        forced,
        gate_names,
        event_name="pull_request",
        action="synchronize",
        pull_request_draft=True,
        pull_request_base_changed=False,
        ref="refs/pull/1/merge",
    )
    if forced_result.ci_policy_path != "full":
        raise AssertionError(f"force_full_ci must force PR events to full, got {forced_result}")


def assert_ci_policy_resolvers_agree() -> None:
    # The runtime resolver (ci_provenance.evaluate_ci_policy) and the static
    # contract resolver (verify_ci_workflow_hygiene.evaluate_ci_policy) are
    # independent hand-maintained mirrors with no shared implementation. #848
    # adds a merge_group row to both; this parity lock fails loud if the two ever
    # diverge on any event, so a future drift cannot let the verifier certify a
    # workflow the runtime actually under-validates (a skipped required check
    # counts as passing in GitHub). Both are fed the real production config.
    verifier = load_verifier()
    provenance = load_provenance()
    config_path = REPO_ROOT / "ci" / "github-actions-runners.toml"
    config_text = config_path.read_text()
    verifier_config = verifier.validate_ci_provenance_config(verifier.tomllib.loads(config_text))
    policy = verifier_config["policy"]
    gate_names = verifier_config["gate_names"]
    mergify_prefix = str(verifier_config["mergify"]["temp_pr_head_ref_prefix"])
    actor_id = int(verifier_config["mergify"]["mergify_temp_pr_actor_id"])
    prov_config = provenance.load_config(config_path)
    cases = [
        ("push", "", False, False, "refs/heads/main"),
        ("push", "", False, False, "refs/tags/v1.2.3"),
        ("pull_request", "opened", True, False, "refs/pull/1/merge"),
        ("pull_request", "synchronize", True, False, "refs/pull/1/merge"),
        ("pull_request", "reopened", True, False, "refs/pull/1/merge"),
        ("pull_request", "edited", True, False, "refs/pull/1/merge"),
        ("pull_request", "converted_to_draft", True, False, "refs/pull/1/merge"),
        ("pull_request", "opened", False, False, "refs/pull/1/merge"),
        ("pull_request", "edited", False, False, "refs/pull/1/merge"),
        ("pull_request", "edited", False, True, "refs/pull/1/merge"),
        ("pull_request", "reopened", False, False, "refs/pull/1/merge"),
        ("pull_request", "ready_for_review", False, False, "refs/pull/1/merge"),
        ("workflow_dispatch", "", True, False, "refs/heads/codex/branch"),
        ("merge_group", "checks_requested", False, False, "refs/heads/gh-readonly-queue/main/pr-1-deadbeef"),
        ("unknown_event", "", True, False, "refs/heads/codex/branch"),
    ]
    saw_full = saw_iteration = False
    for event_name, action, draft, base_changed, ref in cases:
        ver = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        prov = provenance.evaluate_ci_policy(
            prov_config,
            event_name=event_name,
            event_action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            ref=ref,
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.gate_name,
            prov.backtester_gate_name,
            prov.expected_event_class,
            prov.reason,
        )
        if ver_tuple != prov_tuple:
            raise AssertionError(
                f"ci_policy resolver drift for {event_name}/{action!r}: "
                f"verifier={ver_tuple} provenance={prov_tuple}"
            )
        saw_full = saw_full or ver.ci_policy_path == "full"
        saw_iteration = saw_iteration or ver.ci_policy_path == "iteration"
    # Non-vacuous: the matrix must exercise the ready/draft/metadata paths so the
    # parity assertion compares real divergent branches, not a constant.
    if not (saw_full and saw_iteration):
        raise AssertionError("parity matrix must cover full and iteration resolutions")
    # The merge_group row #848 adds must resolve to full on both sides.
    if not any(
        event_name == "merge_group"
        and verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        ).ci_policy_path
        == "full"
        for event_name, action, draft, base_changed, ref in cases
    ):
        raise AssertionError("merge_group must resolve to full in the parity matrix")
    # force_full_ci override: production keeps it false (asserted elsewhere), so
    # the matrix above never exercises this branch. Cover it explicitly with a
    # synthetic forced config on both sides — the verifier reads
    # policy["override"]["force_full_ci"], the runtime reads config.force_full_ci
    # — so a future drift in how either reads the override is caught, not just the
    # default-false path. Both must short-circuit a PR to full CI.
    forced_policy = {
        **policy,
        "override": {**(policy.get("override") or {}), "force_full_ci": True},
    }
    forced_prov = dataclasses.replace(prov_config, force_full_ci=True)
    for event_name, action, draft, base_changed, ref in [
        ("pull_request", "opened", True, False, "refs/pull/1/merge"),
        ("pull_request", "synchronize", False, False, "refs/pull/1/merge"),
    ]:
        ver = verifier.evaluate_ci_policy(
            forced_policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        prov = provenance.evaluate_ci_policy(
            forced_prov,
            event_name=event_name,
            event_action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            ref=ref,
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.gate_name,
            prov.backtester_gate_name,
            prov.expected_event_class,
            prov.reason,
        )
        if ver_tuple != prov_tuple:
            raise AssertionError(
                f"ci_policy resolver drift under force_full_ci for {event_name}/{action!r}: "
                f"verifier={ver_tuple} provenance={prov_tuple}"
            )
        # force_full_ci keeps ci_policy_path == "full" and publishes the required
        # gate names. Manual dispatch-full is gone; this override remains an
        # explicit config-level emergency path covered by policy-contract tests.
        if ver_tuple != ("full", True, "gate", "backtester-gate", "full", "force_full_ci"):
            raise AssertionError(
                f"force_full_ci must keep {event_name}/{action!r} full with required gates; got {ver_tuple}"
            )

    ver = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    prov = provenance.evaluate_ci_policy(
        prov_config,
        event_name="pull_request",
        event_action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.gate_name,
        prov.backtester_gate_name,
        prov.expected_event_class,
        prov.reason,
    )
    if ver_tuple != prov_tuple:
        raise AssertionError(f"ci_policy resolver drift for Mergify temp PR: verifier={ver_tuple} provenance={prov_tuple}")

    ver = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="ready_for_review",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=1376128,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    prov = provenance.evaluate_ci_policy(
        prov_config,
        event_name="pull_request",
        event_action="ready_for_review",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        event_sender_id=1376128,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.gate_name,
        prov.backtester_gate_name,
        prov.expected_event_class,
        prov.reason,
    )
    if ver_tuple != prov_tuple:
        raise AssertionError(
            f"ci_policy resolver drift for human-ready Mergify temp PR: verifier={ver_tuple} provenance={prov_tuple}"
        )
    if ver_tuple != (
        "full",
        True,
        "gate",
        "backtester-gate",
        "full",
        "mergify_temp_pr",
    ):
        raise AssertionError(f"human-ready Mergify temp PR must stay required full CI: {ver_tuple}")

    ver = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    prov = provenance.evaluate_ci_policy(
        prov_config,
        event_name="pull_request",
        event_action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.gate_name,
        prov.backtester_gate_name,
        prov.expected_event_class,
        prov.reason,
    )
    if ver_tuple != prov_tuple:
        raise AssertionError(
            f"ci_policy resolver drift for Mergify temp PR metadata edit: verifier={ver_tuple} provenance={prov_tuple}"
        )
    # Metadata-only edits stay on the cheap draft edit path; base edits remain
    # proof-affecting and keep the Mergify required-gate path.
    if ver_tuple != (
        "iteration",
        False,
        "gate-iteration",
        "backtester-gate-iteration",
        "iteration",
        "draft_pr_edited",
    ):
        raise AssertionError(f"Mergify temp PR metadata edits must stay iteration-only: {ver_tuple}")
    ver = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=True,
        mergify_temp_pr_head_ref_prefix=mergify_prefix,
        mergify_temp_pr_actor_id=actor_id,
        event_sender_id=actor_id,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    prov = provenance.evaluate_ci_policy(
        prov_config,
        event_name="pull_request",
        event_action="edited",
        pull_request_draft=False,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=True,
        event_sender_id=actor_id,
        pull_request_author_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.gate_name,
        prov.backtester_gate_name,
        prov.expected_event_class,
        prov.reason,
    )
    if ver_tuple != prov_tuple:
        raise AssertionError(
            f"ci_policy resolver drift for ready Mergify temp PR base edit: "
            f"verifier={ver_tuple} provenance={prov_tuple}"
        )
    if ver_tuple != (
        "full",
        True,
        "gate",
        "backtester-gate",
        "full",
        "mergify_temp_pr",
    ):
        raise AssertionError(f"ready Mergify temp PR base edits must publish required gates: {ver_tuple}")
    for string_base_changed, expected in [
        (
            "false",
            (
                "iteration",
                False,
                "gate-iteration",
                "backtester-gate-iteration",
                "iteration",
                "draft_pr_edited",
            ),
        ),
        (
            "true",
            (
                "full",
                True,
                "gate",
                "backtester-gate",
                "full",
                "mergify_temp_pr",
            ),
        ),
    ]:
        ver = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name="pull_request",
            action="edited",
            pull_request_draft=True,
            pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
            pull_request_base_changed=string_base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            mergify_temp_pr_actor_id=actor_id,
            event_sender_id=actor_id,
            ref="refs/pull/965/merge",
        )
        prov = provenance.evaluate_ci_policy(
            prov_config,
            event_name="pull_request",
            event_action="edited",
            pull_request_draft=True,
            pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
            pull_request_base_changed=string_base_changed,
            event_sender_id=actor_id,
            ref="refs/pull/965/merge",
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.gate_name,
            prov.backtester_gate_name,
            prov.expected_event_class,
            prov.reason,
        )
        if ver_tuple != prov_tuple:
            raise AssertionError(
                f"ci_policy resolver drift for string base_changed={string_base_changed!r}: "
                f"verifier={ver_tuple} provenance={prov_tuple}"
            )
        if ver_tuple != expected:
            raise AssertionError(
                f"Mergify temp PR string base_changed={string_base_changed!r} resolved incorrectly: {ver_tuple}"
            )

    def assert_mergify_non_proof_case(
        *,
        label: str,
        action: str,
        base_changed: bool,
        event_sender_id: int,
        pull_request_author_id: int,
        expected_reason: str,
    ) -> None:
        ver = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name="pull_request",
            action=action,
            pull_request_draft=False,
            pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
            pull_request_base_changed=base_changed,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            mergify_temp_pr_actor_id=actor_id,
            event_sender_id=event_sender_id,
            pull_request_author_id=pull_request_author_id,
            ref="refs/pull/965/merge",
        )
        prov = provenance.evaluate_ci_policy(
            prov_config,
            event_name="pull_request",
            event_action=action,
            pull_request_draft=False,
            pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
            pull_request_base_changed=base_changed,
            event_sender_id=event_sender_id,
            pull_request_author_id=pull_request_author_id,
            ref="refs/pull/965/merge",
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.gate_name,
            prov.backtester_gate_name,
            prov.expected_event_class,
            prov.reason,
        )
        if expected_reason == "ready_pr":
            expected = ("full", True, "gate", "backtester-gate", "full", expected_reason)
        elif expected_reason == "ready_pr_reopened":
            expected = ("full", True, "gate", "backtester-gate", "full", expected_reason)
        else:
            raise AssertionError(f"unexpected fallback reason for {label}: {expected_reason}")
        if ver_tuple != prov_tuple:
            raise AssertionError(
                f"ci_policy resolver drift for {label}: verifier={ver_tuple} provenance={prov_tuple}"
            )
        if ver_tuple != expected:
            raise AssertionError(f"{label} must fall back to normal ready policy, not Mergify proof: {ver_tuple}")

    assert_mergify_non_proof_case(
        label="non-draft Mergify base edit from wrong author",
        action="edited",
        base_changed=True,
        event_sender_id=actor_id,
        pull_request_author_id=actor_id + 1,
        expected_reason="ready_pr",
    )
    for action, expected_reason in [
        ("synchronize", "ready_pr"),
        ("reopened", "ready_pr_reopened"),
    ]:
        # Non-draft synchronize/reopened events are intentionally not proof-affecting
        # author-bound Mergify transitions; only ready_for_review and base edits are.
        assert_mergify_non_proof_case(
            label=f"non-draft Mergify {action}",
            action=action,
            base_changed=False,
            event_sender_id=actor_id,
            pull_request_author_id=actor_id,
            expected_reason=expected_reason,
        )


def assert_ci_policy_rejects_literal_event_sender_id_argument() -> None:
    ci_ref_arg = '            --ref "${{ github.ref }}"'
    backtester_literal_arg = '            --event-sender-id 37929162 \\\n'
    backtester_ref_arg = """            --ref "${{ github.ref }}" \\
"""
    ci_mutated = replace_once(
        BASE_WORKFLOW,
        ci_ref_arg,
        ci_ref_arg + " --event-sender-id 37929162",
    )
    assert_error(
        "ci-policy must not pass --event-sender-id on the resolver command line",
        workflow=ci_mutated,
    )

    verifier = load_verifier()
    backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
    backtester_mutated = replace_once(
        backtester,
        backtester_ref_arg,
        backtester_ref_arg + backtester_literal_arg,
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
    if not any("ci-policy must not pass --event-sender-id on the resolver command line" in error for error in errors):
        raise AssertionError(f"backtester ci-policy literal event sender id must be rejected, got: {errors}")


def assert_ci_policy_rejects_inline_event_sender_id_override() -> None:
    command = '          python3 "$policy_script" ci-policy'
    mutated = replace_once(
        BASE_WORKFLOW,
        command,
        '          EVENT_SENDER_ID=37929162 python3 "$policy_script" ci-policy',
    )
    assert_error(
        "ci-policy must not override EVENT_SENDER_ID inline on the resolver command line",
        workflow=mutated,
    )

    verifier = load_verifier()
    backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
    backtester_mutated = replace_once(
        backtester,
        command,
        '          EVENT_SENDER_ID=37929162 python3 "$policy_script" ci-policy',
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
    if not any(
        "ci-policy must not override EVENT_SENDER_ID inline on the resolver command line" in error
        for error in errors
    ):
        raise AssertionError(f"backtester inline EVENT_SENDER_ID override must be rejected, got: {errors}")


def assert_ci_policy_rejects_backslash_split_event_sender_id_argument() -> None:
    ci_ref_arg = '            --ref "${{ github.ref }}"'
    split_arg = "            --event-\\\n            sender-id 37929162 \\"
    mutated = replace_once(
        BASE_WORKFLOW,
        ci_ref_arg,
        f"{split_arg}\n{ci_ref_arg}",
    )
    assert_error(
        "ci-policy must not pass --event-sender-id on the resolver command line",
        workflow=mutated,
    )

    verifier = load_verifier()
    backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
    backtester_ref_arg = """            --ref "${{ github.ref }}" \\
"""
    backtester_mutated = replace_once(
        backtester,
        backtester_ref_arg,
        backtester_ref_arg + split_arg + "\n",
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
    if not any("ci-policy must not pass --event-sender-id on the resolver command line" in error for error in errors):
        raise AssertionError(f"backtester backslash-split event sender id must be rejected, got: {errors}")


def assert_ci_policy_rejects_env_command_event_sender_id_override() -> None:
    command = '          python3 "$policy_script" ci-policy'
    mutated = replace_once(
        BASE_WORKFLOW,
        command,
        '          env EVENT_SENDER_ID=37929162 python3 "$policy_script" ci-policy',
    )
    assert_error(
        "ci-policy must not override EVENT_SENDER_ID inline on the resolver command line",
        workflow=mutated,
    )

    verifier = load_verifier()
    backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
    backtester_mutated = replace_once(
        backtester,
        command,
        '          env EVENT_SENDER_ID=37929162 python3 "$policy_script" ci-policy',
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
    if not any(
        "ci-policy must not override EVENT_SENDER_ID inline on the resolver command line" in error
        for error in errors
    ):
        raise AssertionError(f"backtester env EVENT_SENDER_ID override must be rejected, got: {errors}")


def assert_ci_policy_rejects_prior_event_sender_id_exports() -> None:
    command = '          python3 "$policy_script" ci-policy'
    for label, prefix in (
        ("standalone", "          EVENT_SENDER_ID=37929162\n"),
        ("export", "          export EVENT_SENDER_ID=37929162\n"),
    ):
        mutated = replace_once(BASE_WORKFLOW, command, prefix + command)
        assert_error(
            "ci-policy must not override EVENT_SENDER_ID before the resolver command",
            workflow=mutated,
        )

        verifier = load_verifier()
        backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
        backtester_mutated = replace_once(backtester, command, prefix + command)
        errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
        if not any("ci-policy must not override EVENT_SENDER_ID before the resolver command" in error for error in errors):
            raise AssertionError(f"backtester {label} EVENT_SENDER_ID override must be rejected, got: {errors}")


def assert_ci_policy_rejects_event_sender_id_append_assignment() -> None:
    command = '          python3 "$policy_script" ci-policy'
    mutated = replace_once(
        BASE_WORKFLOW,
        command,
        '          EVENT_SENDER_ID+=37929162 python3 "$policy_script" ci-policy',
    )
    assert_error(
        "ci-policy must not override EVENT_SENDER_ID inline on the resolver command line",
        workflow=mutated,
    )


def assert_ci_policy_rejects_alternate_python_event_sender_id_argument() -> None:
    command = '          python3 "$policy_script" ci-policy'
    ref_arg = '            --ref "${{ github.ref }}"'
    decoy = '          : \'python3 "$policy_script" ci-policy\'\n'
    mutated = replace_once(BASE_WORKFLOW, command, decoy + '          /usr/bin/python3 "$policy_script" ci-policy')
    mutated = replace_once(mutated, ref_arg, ref_arg + " --event-sender-id 37929162")
    assert_error(
        "ci-policy must not pass --event-sender-id on the resolver command line",
        workflow=mutated,
    )

    verifier = load_verifier()
    backtester = repo_workflow_text(".github/workflows/backtester-ci.yml")
    backtester_ref_arg = """            --ref "${{ github.ref }}" \\
"""
    backtester_mutated = replace_once(
        backtester,
        command,
        decoy + '          python3.12 "$policy_script" ci-policy',
    )
    backtester_mutated = replace_once(
        backtester_mutated,
        backtester_ref_arg,
        backtester_ref_arg + "            --event-sender-id 37929162 \\\n",
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": backtester_mutated})
    if not any("ci-policy must not pass --event-sender-id on the resolver command line" in error for error in errors):
        raise AssertionError(f"backtester alternate Python event sender id argument must be rejected, got: {errors}")


def assert_ci_policy_rejects_split_and_boundary_event_sender_id_arguments() -> None:
    ci_ref_arg = '            --ref "${{ github.ref }}"'
    mid_token_split = "            --event-send\\\n            er-id 37929162 \\"
    mutated = replace_once(
        BASE_WORKFLOW,
        ci_ref_arg,
        f"{mid_token_split}\n{ci_ref_arg}",
    )
    assert_error(
        "ci-policy must not pass --event-sender-id on the resolver command line",
        workflow=mutated,
    )

    boundary_arg = "            --event-name if --event-sender-id 37929162"
    mutated = replace_once(
        BASE_WORKFLOW,
        '            --event-name "${{ github.event_name }}"',
        boundary_arg,
    )
    assert_error(
        "ci-policy must not pass --event-sender-id on the resolver command line",
        workflow=mutated,
    )


def assert_ci_policy_counts_structural_event_sender_id_env_keys() -> None:
    env_line = "          EVENT_SENDER_ID: ${{ github.event.sender.id }}"
    duplicate = replace_once(BASE_WORKFLOW, env_line, env_line + "\n          EVENT_SENDER_ID : 37929162")
    assert_error(
        "ci-policy must declare EVENT_SENDER_ID env exactly once",
        workflow=duplicate,
    )

    command = '          python3 "$policy_script" ci-policy'
    diagnostic = (
        '          : "${EVENT_SENDER_ID:?missing sender id}"\n'
        '          echo "EVENT_SENDER_ID: ${EVENT_SENDER_ID}" >> "$GITHUB_STEP_SUMMARY"\n'
    )
    assert_clean(workflow=replace_once(BASE_WORKFLOW, command, diagnostic + command))


def assert_ci_policy_real_workflows_keep_event_sender_binding_clean() -> None:
    verifier = load_verifier()
    workflows = {
        ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml"),
        ".github/workflows/backtester-ci.yml": repo_workflow_text(".github/workflows/backtester-ci.yml"),
    }
    errors = verifier.verify_workflows(workflows, BASE_ACTION, BASE_NEXTEST_CONFIG)
    if errors:
        raise AssertionError(f"real ci-policy workflows must remain clean, got: {errors}")


def assert_pull_request_type_parser_accepts_block_list_indentation() -> None:
    verifier = load_verifier()
    workflow = """\
name: CI

on:
  pull_request:
    types:
    - opened
    - synchronize
    - reopened
    - ready_for_review
    - converted_to_draft
    - edited
  push:
    branches: [main]
"""
    errors = verifier.workflow_pull_request_type_errors(workflow)
    if errors:
        raise AssertionError(errors)


def assert_ci_workflow_requires_policy_trigger_and_dispatch_input() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cases = [
        (
            "workflow must define workflow_dispatch",
            re.sub(r"\n  workflow_dispatch:\n(?:    .+\n)+", "\n", workflow, count=1),
        ),
        (
            "workflow_dispatch must not define a full_ci input",
            replace_once(
                workflow,
                "      credential_ssm_gate:\n",
                "      full_ci:\n"
                "        description: Run full CI\n"
                "        required: false\n"
                "        type: boolean\n"
                "        default: false\n"
                "      credential_ssm_gate:\n",
            ),
        ),
        (
            "pull_request types must include ready_for_review",
            replace_once(
                workflow,
                "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]",
                "types: [opened, synchronize, reopened, converted_to_draft, edited]",
            ),
        ),
        (
            "pull_request types must include converted_to_draft",
            replace_once(
                workflow,
                "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]",
                "types: [opened, synchronize, reopened, ready_for_review, edited]",
            ),
        ),
        (
            "pull_request types must include edited",
            replace_once(
                workflow,
                "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]",
                "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft]",
            ),
        ),
        ("missing required job ci-policy", without_job(workflow, "ci-policy"))
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_ci_workflow_dispatch_config_errors_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    config_text = ci_provenance_config_fixture()
    invalid_config = replace_once(
        config_text,
        "\n[ci_provenance.dispatch]\n",
        "\n[ci_provenance.dispatch_disabled]\n",
    )
    expected = "github-actions runner config invalid: ci_provenance.dispatch must be a table"
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        verifier.DEFAULT_RUNNERS_CONFIG = write_temp_runner_config(pathlib.Path(tmp), invalid_config)
        try:
            dispatch_names, name_errors = verifier.configured_ci_provenance_dispatch_names()
            workflow_errors = verifier.verify_workflow(workflow)
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config

    if dispatch_names is not None or not any(expected in error for error in name_errors):
        raise AssertionError(f"dispatch name helper must fail closed on invalid config, got: {name_errors}")
    if not any(expected in error for error in workflow_errors):
        raise AssertionError(f"workflow verification must report invalid dispatch config, got: {workflow_errors}")


def assert_test_archive_sccache_fail_open_contract() -> None:
    # #1011: the S3 sccache compile cache must never be able to fail the required
    # test-archive build. Lock the fail-open invariants so a future edit can't
    # silently make the cache fatal.
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    clean = [error for error in verifier.verify_workflow(workflow) if "sccache" in error]
    if clean:
        raise AssertionError(f"real ci.yml must satisfy the sccache fail-open contract, got: {clean}")
    required_pr_read_fragments = [
        "uses: ./.github/actions/sccache-setup",
        "role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}",
        "write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}",
    ]
    for fragment in required_pr_read_fragments:
        if fragment not in workflow:
            raise AssertionError(f"real ci.yml must use the shared sccache action: missing {fragment!r}")
    cases = [
        (
            "test-archive sccache opt-in must stay conditional",
            replace_once(
                workflow,
                "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}",
                'BOLT_RUST_VERIFICATION_SCCACHE: "1"',
            ),
        ),
        (
            "test-archive sccache setup must route through the shared sccache action",
            replace_once(
                workflow,
                "uses: ./.github/actions/sccache-setup",
                "uses: ./.github/actions/not-sccache-setup",
            ),
        ),
        (
            "test-archive sccache setup must include",
            replace_once(
                workflow,
                "          active: ${{ steps.nextest-archive-cache.outputs.cache-hit != 'true' && 'true' || 'false' }}\n",
                "",
            ),
        ),
        (
            "test-archive sccache setup must include",
            replace_once(
                workflow,
                "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
                "",
            ),
        ),
        (
            "test-archive sccache build must route through the Rust verification owner",
            replace_once(
                workflow,
                'just test-archive "$NEXTEST_ARCHIVE_PATH"',
                "true",
            ),
        ),
        (
            "test-archive sccache retry must be owned by rust_verification.py",
            replace_once(
                workflow,
                'just test-archive "$NEXTEST_ARCHIVE_PATH"',
                'bash .github/scripts/sccache-fail-open.sh --on any "$RUNNER_TEMP/test-archive-build.log" just test-archive "$NEXTEST_ARCHIVE_PATH"',
            ),
        ),
        (
            "test-archive sccache must print stats after the compile step",
            workflow.replace("      - name: Print sccache stats\n", "      - name: Print early sccache stats\n", 1),
        ),
        (
            "test-archive sccache must print stats after the compile step",
            workflow.replace(
                "      - name: Print sccache stats\n        if: always()\n",
                "      - name: Print sccache stats\n        if: success()\n",
                1,
            ),
        ),
        (
            "test-archive sccache setup must include",
            replace_once(
                workflow,
                "          active: ${{ steps.nextest-archive-cache.outputs.cache-hit != 'true' && 'true' || 'false' }}\n",
                "",
            ),
        ),
        (
            "write role must only be passed to the shared sccache action",
            replace_once(
                workflow,
                "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
                "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n"
                "          extra-write-role: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
            ),
        ),
        (
            "BOLT_RUST_VERIFICATION_SCCACHE opt-in must stay scoped",
            replace_once(
                workflow,
                "  detector:\n    name: detector\n",
                "  detector:\n    name: detector\n    env:\n      BOLT_RUST_VERIFICATION_SCCACHE: \"1\"\n",
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def _test_archive_build_script(verifier) -> str:
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    archive_job = verifier.parse_jobs(workflow).get("test-archive")
    if archive_job is None:
        raise AssertionError("test-archive job missing")
    build_block = verifier.named_step_block(archive_job, "Build nextest archive")
    if build_block is None:
        raise AssertionError("Build nextest archive step missing")
    script = verifier.block_run_body(build_block)
    if not script:
        raise AssertionError("Build nextest archive run body missing")
    return script


def _run_test_archive_build_script(script: str, *, sccache: str, fake_just_mode: str) -> tuple[int, int]:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        counter = root / "just-count"
        fake_just = root / "just"
        fake_just.write_text(
            textwrap.dedent(
                """\
                #!/usr/bin/env bash
                set -euo pipefail
                count=0
                if [[ -f "$JUST_COUNT_FILE" ]]; then
                  count="$(cat "$JUST_COUNT_FILE")"
                fi
                count=$((count + 1))
                echo "$count" > "$JUST_COUNT_FILE"
                case "$JUST_MODE" in
                  transient-cache-failure)
                    if [[ "$count" -eq 1 ]]; then exit 86; fi
                    exit 0
                    ;;
                  compile-error)
                    exit 42
                    ;;
                  no-cache-failure)
                    exit 43
                    ;;
                  *)
                    echo "unknown JUST_MODE=$JUST_MODE" >&2
                    exit 99
                    ;;
                esac
                """
            ),
            encoding="utf-8",
        )
        fake_just.chmod(0o755)
        env = {
            **os.environ,
            "PATH": f"{root}:{os.environ['PATH']}",
            "JUST_COUNT_FILE": str(counter),
            "JUST_MODE": fake_just_mode,
            "NEXTEST_ARCHIVE_PATH": str(root / "out" / "nextest-archive.tar.zst"),
            "RUNNER_TEMP": str(root),
            "BOLT_RUST_VERIFICATION_SCCACHE": sccache,
        }
        script_path = root / "test-archive-build.sh"
        script_path.write_text(script, encoding="utf-8")
        result = subprocess.run(
            ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", str(script_path)],
            cwd=REPO_ROOT,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        count = int(counter.read_text(encoding="utf-8")) if counter.exists() else 0
        return result.returncode, count


def assert_test_archive_build_script_delegates_sccache_retry_to_owner() -> None:
    verifier = load_verifier()
    script = _test_archive_build_script(verifier)
    if "sccache-fail-open.sh" in script:
        raise AssertionError("test-archive workflow shell must not own sccache retry")
    rc, count = _run_test_archive_build_script(
        script,
        sccache="1",
        fake_just_mode="transient-cache-failure",
    )
    if (rc, count) != (86, 1):
        raise AssertionError(f"workflow shell must delegate retry to the Rust owner, got rc={rc} count={count}")

    rc, count = _run_test_archive_build_script(
        script,
        sccache="1",
        fake_just_mode="compile-error",
    )
    if (rc, count) != (42, 1):
        raise AssertionError(f"workflow shell must not retry compile failures, got rc={rc} count={count}")

    rc, count = _run_test_archive_build_script(
        script,
        sccache="0",
        fake_just_mode="no-cache-failure",
    )
    if (rc, count) != (43, 1):
        raise AssertionError(f"workflow shell must call the owner once without sccache, got rc={rc} count={count}")


def assert_ci_workflow_run_name_matches_dispatch_config() -> None:
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    assert_error(
        "workflow run-name must not publish a dispatch full marker",
        replace_once(
            workflow,
            "&& 'CI [dispatch:iteration]'",
            "&& github.event.inputs.full_ci == 'true'\n"
            "      && 'CI [dispatch:full]'\n"
            "      || github.event_name == 'workflow_dispatch'\n"
            "      && 'CI [dispatch:iteration]'",
        ),
    )
    assert_error(
        "workflow run-name must publish configured dispatch iteration marker",
        replace_once(workflow, "&& 'CI [dispatch:iteration]'", "&& 'CI [manual:iteration]'"),
    )
    assert_error(
        "workflow run-name must preserve configured non-dispatch name",
        replace_once(workflow, "|| 'CI' }}", "|| 'CI default' }}"),
    )


def assert_ci_detector_forces_build_on_workflow_dispatch() -> None:
    # Negative test: removing workflow_dispatch from the combined if-arm must trigger the guard.
    # The production shape is: if [[ "..." == "push" || "..." == "workflow_dispatch" ]]; then
    # Stripping the || clause leaves only push coverage and breaks the workflow_dispatch invariant.
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    dispatch_clause = ' || "${{ github.event_name }}" == "workflow_dispatch"'
    mutated = workflow.replace(dispatch_clause, "", 1)
    errors = verifier.verify_workflow(mutated)
    if not any("detector must force build_required=true for workflow_dispatch runs" in error for error in errors):
        raise AssertionError(f"expected workflow_dispatch detector guard error, got: {errors}")


def assert_capture_artifact_metadata_is_config_derived() -> None:
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    metadata_command = (
        '          python3 scripts/ci_provenance.py artifact-metadata \\\n'
        '            --config "$CAPTURE_PROVENANCE_CONFIG" \\\n'
        '            --run-attempt "${{ github.run_attempt }}" >> "$GITHUB_OUTPUT"\n'
    )
    assert_error(
        "capture must derive artifact metadata from ci_provenance.py artifact-metadata",
        workflow.replace(metadata_command, "") if metadata_command in workflow else workflow,
    )


def assert_ci_base_ref_archives_use_scripts_directory() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    marker = 'git archive "$base_ref" scripts/ ci/github-actions-runners.toml | tar -x -C "$base_tree"'
    for replacement in (
        'git archive "$base_ref" scripts/ci_provenance.py scripts/rust_verification.py ci/github-actions-runners.toml | tar -x -C "$base_tree"',
        'git archive "$base_ref" scripts/ scripts/ci_provenance.py ci/github-actions-runners.toml | tar -x -C "$base_tree"',
    ):
        mutated = replace_once(workflow, marker, replacement)
        errors = verifier.verify_workflow(mutated)
        if not any(
            "base_ref git archive must archive scripts/ wholesale" in error
            for error in errors
        ):
            raise AssertionError(f"expected base_ref scripts/ archive error for {replacement!r}, got: {errors}")


def assert_ci_detector_docs_only_archive_includes_lane_policy() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    archive_without_policy = (
        'git archive "$base_ref" scripts/ ci/github-actions-runners.toml .github/workflows/ci.yml | tar -x -C "$base_tree"'
    )
    archive_with_policy = (
        'git archive "$base_ref" scripts/ ci/rust-verification.toml ci/github-actions-runners.toml .github/workflows/ci.yml | tar -x -C "$base_tree"'
    )
    if archive_without_policy in workflow:
        workflow = replace_once(workflow, archive_without_policy, archive_with_policy)
    mutated = replace_once(workflow, " ci/rust-verification.toml", "")
    errors = verifier.verify_workflow(mutated)
    if not any(
        "detector docs-only classifier base archive must include ci/rust-verification.toml" in error
        for error in errors
    ):
        raise AssertionError(f"expected detector docs-only lane policy archive error, got: {errors}")






def assert_ci_policy_heavy_lane_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cases = [
        (
            "source-fence needs ci-policy",
            replace_once(
                workflow,
                "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]",
                "  source-fence:\n    name: source-fence\n    needs: detector",
            ),
        ),
        (
            "source-fence must run for full_ci_required or docs policy",
            replace_once(
                workflow,
                "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]\n    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs' }}",
                "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]\n    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}",
            ),
        ),
        (
            "source-fence must run for full_ci_required or docs policy",
            replace_once(
                workflow,
                "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs' }}",
                "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && needs.ci-policy.outputs.ci_policy_path == 'docs' }}",
            ),
        ),
        (
            "source-fence checkout must use pull_request head SHA for docs policy and github.sha otherwise",
            replace_once(
                workflow,
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n        with:\n          ref: ${{ needs.ci-policy.outputs.ci_policy_path == 'docs' && github.event.pull_request.head.sha || github.sha }}",
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2",
            ),
        ),
        (
            "test-archive needs ci-policy",
            replace_once(
                workflow,
                "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]",
                "  test-archive:\n    name: nextest archive\n    needs: [detector, nextest-fingerprint, nextest-fingerprint-reuse]",
            ),
        ),
        (
            "nextest-fingerprint needs ci-policy",
            replace_once(
                workflow,
                "  nextest-fingerprint:\n    name: nextest fingerprint\n    needs: [ci-policy, detector]",
                "  nextest-fingerprint:\n    name: nextest fingerprint\n    needs: detector",
            ),
        ),
        (
            "test needs ci-policy",
            replace_once(
                workflow,
                "  test:\n    name: test\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
                "  test:\n    name: test\n    needs: [detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
            ),
        ),
        (
            "build needs ci-policy",
            replace_once(
                workflow,
                "  build:\n    name: build\n    needs: [ci-policy, detector]",
                "  build:\n    name: build\n    needs: detector",
            ),
        ),
        (
            "ci-provenance-emit needs ci-policy",
            replace_once(
                workflow,
                "needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]",
                "needs: [detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]",
            ),
        ),
        (
            "ci-provenance-emit must gate on full_ci_required",
            replace_once(
                workflow,
                "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') }}",
                "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ always() && !startsWith(github.ref, 'refs/tags/v') }}",
            ),
        ),
        (
            "check-aarch64 needs ci-policy",
            replace_once(
                workflow,
                "  check-aarch64:\n    name: check-aarch64\n    needs: [ci-policy, detector]",
                "  check-aarch64:\n    name: check-aarch64\n    needs: detector",
            ),
        ),
        (
            "check-aarch64 must run on full CI or tag reuse",
            replace_once(
                workflow,
                "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'tag_reuse' }}",
                "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}",
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_cache_persistence_audit_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cases = [
        (
            "test-archive must expose cache persistence audit outputs",
            replace_once(
                workflow,
                "      nextest_archive_cache_key: ${{ steps.root-nextest-cache-keys.outputs.nextest_archive_cache_key }}\n",
                "",
            ),
        ),
        (
            "test-archive archive build target cache hit output must be explicit when restore is skipped",
            replace_once(
                workflow,
                "      archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || (steps.test-target-cache.outputs.cache-hit || 'false') }}\n",
                "      archive_build_target_cache_hit: ${{ steps.test-target-cache.outputs.cache-hit }}\n",
            ),
        ),
        (
            "test-archive archive build target cache hit output must default cache misses to false",
            replace_once(
                workflow,
                "      archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || (steps.test-target-cache.outputs.cache-hit || 'false') }}\n",
                "      archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || steps.test-target-cache.outputs.cache-hit }}\n",
            ),
        ),
        (
            "test-archive must expose cache persistence audit outputs",
            replace_once(
                workflow,
                "      nextest_archive_cache_hit: ${{ steps.nextest-archive-cache.outcome == 'skipped' && 'skipped' || (steps.nextest-archive-cache.outputs.cache-hit || 'false') }}\n",
                "      nextest_archive_cache_hit: ${{ steps.nextest-archive-cache.outputs.cache-hit || 'false' }}\n",
            ),
        ),
        (
            "test-archive must expose cache persistence audit outputs",
            replace_once(
                workflow,
                "      root_bin_sidecars_cache_hit: ${{ steps.root-bin-sidecars-cache.outcome == 'skipped' && 'skipped' || (steps.root-bin-sidecars-cache.outputs.cache-hit || 'false') }}\n",
                "      root_bin_sidecars_cache_hit: ${{ steps.root-bin-sidecars-cache.outputs.cache-hit || 'false' }}\n",
            ),
        ),
        (
            "test-archive must expose cache persistence save outcomes",
            replace_once(
                workflow,
                "      nextest_archive_cache_save_outcome: ${{ steps.nextest-archive-cache-save.outputs.save-status || (steps.nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}\n",
                "",
            ),
        ),
        (
            "test-archive cache save steps must have stable ids for persistence evidence",
            replace_once(
                workflow,
                "        id: nextest-archive-cache-save\n",
                "        id: nextest-archive-save\n",
            ),
        ),
        (
            "test-archive cache restore steps must have stable ids for persistence evidence",
            replace_once(
                workflow,
                "        id: nextest-archive-cache\n",
                "        id: nextest-archive-restore\n",
            ),
        ),
        (
            "test-archive cache persistence keys must come from single-source cache key outputs",
            replace_once(
                workflow,
                "      - name: Restore nextest archive from S3\n",
                """      - name: Emit cache persistence audit keys
        id: cache-audit-keys
        shell: bash
        run: |
          echo "nextest_archive_cache_key=duplicated-template" >> "$GITHUB_OUTPUT"

      - name: Restore nextest archive from S3
""",
            ),
        ),
        (
            "test-archive must resolve root nextest cache keys",
            replace_once(workflow, "        id: root-nextest-cache-keys\n", "        id: cache-keys\n"),
        ),
        (
            "test-archive cache persistence keys must come from single-source cache key outputs",
            replace_once(
                workflow,
                "root_bin_sidecars_cache_key=root-bin-sidecars-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}",
                "root_bin_sidecars_cache_key=root-bin-sidecars-bad-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}",
            ),
        ),
        (
            "test-archive cache persistence keys must come from single-source cache key outputs",
            replace_once(
                workflow,
                "archive_build_target_cache_key=managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
                "archive_build_target_cache_key=managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ github.sha }}",
            ),
        ),
        (
            "cache-persistence-audit job is required",
            replace_once(workflow, "  cache-persistence-audit:\n", "  cache-persistence-audit-disabled:\n"),
        ),
        (
            "cache-persistence-audit needs test-archive",
            replace_once(
                workflow,
                "    needs: [ci-policy, nextest-fingerprint-reuse, test-archive]",
                "    needs: [ci-policy, nextest-fingerprint-reuse]",
            ),
        ),
        (
            "cache-persistence-audit permissions must include actions: read",
            replace_once(
                workflow,
                "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    permissions:\n      contents: read\n      actions: read\n",
                "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    permissions:\n      contents: read\n",
            ),
        ),
        (
            "cache-persistence-audit permissions must include actions: read",
            replace_once(
                replace_once(
                    workflow,
                    "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    permissions:\n      contents: read\n      actions: read\n",
                    "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    permissions:\n      contents: read\n",
                ),
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          echo \"actions: read\" >/dev/null\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must use the workflow token for cache API reads",
            replace_once(
                replace_once(
                    workflow,
                    "          GH_TOKEN: ${{ github.token }}\n",
                    "",
                ),
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          echo 'GH_TOKEN: ${{ github.token }}' >/dev/null\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must run ci_storage_audit exact-key probes",
            replace_once(workflow, "          python3 scripts/ci_storage_audit.py \\\n", "          python3 scripts/ci_storage_audit_probe.py \\\n"),
        ),
        (
            "cache-persistence-audit must run ci_storage_audit exact-key probes",
            replace_once(workflow, "          python3 scripts/ci_storage_audit.py \\\n", "          python3 scripts/ci_storage_audit.py.bak \\\n"),
        ),
        (
            "cache-persistence-audit probe must be non-blocking",
            replace_once(
                workflow,
                "      - name: Probe saved cache keys\n        continue-on-error: true\n",
                "      - name: Probe saved cache keys\n",
            ),
        ),
        (
            "cache-persistence-audit probe must be non-blocking",
            replace_once(
                workflow,
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n\n      - name: Probe saved cache keys\n        continue-on-error: true\n",
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n        continue-on-error: true\n\n      - name: Probe saved cache keys\n",
            ),
        ),
        (
            "cache-persistence-audit probe must be non-blocking",
            replace_once(
                replace_once(
                    workflow,
                    "      - name: Probe saved cache keys\n        continue-on-error: true\n",
                    "      - name: Probe saved cache keys\n",
                ),
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          echo 'continue-on-error: true' >/dev/null\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must not add extra run steps",
            replace_once(
                workflow,
                "      - name: Probe saved cache keys\n",
                "      - name: Accidental blocking step\n        run: exit 1\n\n      - name: Probe saved cache keys\n",
            ),
        ),
        (
            "cache-persistence-audit must not add extra steps",
            replace_once(
                workflow,
                "      - name: Probe saved cache keys\n",
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2\n\n      - name: Probe saved cache keys\n",
            ),
        ),
        (
            "cache-persistence-audit must delegate audit policy to ci_storage_audit",
            replace_once(
                workflow,
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          echo preparing cache audit\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must not suppress audit contract failures",
            replace_once(
                workflow,
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"\n',
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}" || true\n',
            ),
        ),
        (
            "cache-persistence-audit must not suppress audit contract failures",
            replace_once(
                workflow,
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"\n',
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}" || echo suppressed\n',
            ),
        ),
        (
            "cache-persistence-audit must not suppress audit contract failures",
            replace_once(
                workflow,
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"\n',
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"; true\n',
            ),
        ),
        (
            "cache-persistence-audit must not suppress audit contract failures",
            replace_once(
                workflow,
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"\n',
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}" | cat\n',
            ),
        ),
        (
            "cache-persistence-audit must delegate audit policy to ci_storage_audit",
            replace_once(
                workflow,
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          grep -q ': missing;' \"$audit_log\"\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must delegate audit policy to ci_storage_audit",
            replace_once(
                workflow,
                "          python3 scripts/ci_storage_audit.py \\\n",
                "          set +e\n          python3 scripts/ci_storage_audit.py \\\n",
            ),
        ),
        (
            "cache-persistence-audit must probe all root nextest cache keys",
            replace_once(
                workflow,
                '            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"\n',
                "",
            ),
        ),
        (
            "cache-persistence-audit must limit exact-key probes to restorable cache refs",
            replace_once(
                workflow,
                '            --github-ref "$GITHUB_REF" \\\n',
                "",
            ),
        ),
        (
            "cache-persistence-audit must write probe results to the job summary",
            replace_once(workflow, '            --github-step-summary "$GITHUB_STEP_SUMMARY" \\\n', ""),
        ),
        (
            "cache-persistence-audit must emit audit annotations from ci_storage_audit",
            replace_once(workflow, "            --github-annotations \\\n", ""),
        ),
        (
            "cache-persistence-audit must summarize cache restore hits",
            replace_once(
                workflow,
                '            --restore-hit "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_hit }}" \\\n',
                "",
            ),
        ),
        (
            "cache-persistence-audit must summarize cache save outcomes",
            replace_once(
                workflow,
                '            --save-outcome "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_save_outcome }}" \\\n',
                "",
            ),
        ),
    ]
    for expected, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(expected in error for error in errors):
            raise AssertionError(f"expected verifier error containing {expected!r}, got: {errors}")




def assert_ci_concurrency_split_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cancel_in_progress_for_pr_and_dispatch = """  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        || github.event_name == 'workflow_dispatch' }}
"""
    cases = [
        (
            "concurrency group must split iteration PR runs from full CI runs",
            replace_once(workflow, "pr-{0}-iteration", "pr-{0}"),
        ),
        (
            "workflow_dispatch runs must use the iteration concurrency group",
            replace_once(
                workflow,
                "        || github.event_name == 'workflow_dispatch'\n"
                "        && format('{0}-dispatch-iteration', github.ref_name)\n",
                "",
            ),
        ),
        (
            "cancel-in-progress must apply only to pull_request and workflow_dispatch runs",
            replace_once(
                workflow,
                cancel_in_progress_for_pr_and_dispatch,
                "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n",
            ),
        ),
        (
            "cancel-in-progress must not cancel push, tag, or deploy flows",
            replace_once(
                workflow,
                "        || github.event_name == 'workflow_dispatch' }}",
                "        || github.event_name == 'workflow_dispatch'\n        || github.event_name == 'push' }}",
            ),
        ),
        (
            "cancel-in-progress must not cancel push, tag, or deploy flows",
            replace_once(
                workflow,
                "        || github.event_name == 'workflow_dispatch' }}",
                "        || github.event_name == 'workflow_dispatch'\n        || github.ref == 'refs/tags/v1.2.3' }}",
            ),
        ),
        (
            "cancel-in-progress must not cancel push, tag, or deploy flows",
            replace_once(
                workflow,
                "        || github.event_name == 'workflow_dispatch' }}",
                "        || github.event_name == 'workflow_dispatch'\n        || startsWith(github.ref, 'refs/tags/v') }}",
            ),
        ),
        (
            "workflow-level concurrency must not reference job outputs",
            replace_once(workflow, "github.event.number", "needs.ci-policy.outputs.reason"),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_mergify_proof_prefix_alignment_holds() -> None:
    # The resolver and workflow concurrency layer must agree on both documented
    # Mergify proof-PR head-ref forms, so either form gets the required gate only
    # when the workflow also isolates it from cancellation.
    verifier = load_verifier()
    provenance = load_provenance()
    config = provenance.load_config(REPO_ROOT / "ci" / "github-actions-runners.toml")
    errors = verifier.mergify_proof_prefix_alignment_errors(config)
    if errors:
        raise AssertionError(f"real config must keep resolver/workflow prefixes aligned: {errors}")


def assert_base_change_predicate_avoids_loose_empty_string_equality() -> None:
    verifier = load_verifier()
    forbidden = "github.event.changes.base.ref.from != ''"
    expected = "github.event.changes.base.ref.from && true || false"
    if verifier.PR_BASE_CHANGED_EXPR != expected:
        raise AssertionError(
            "base-change predicate must use truthiness instead of loose empty-string equality: "
            f"{verifier.PR_BASE_CHANGED_EXPR!r}"
        )
    predicate_surfaces = [
        ("PR_BASE_CHANGED_EXPR", verifier.PR_BASE_CHANGED_EXPR),
        (
            "MERGIFY_PROOF_PR_METADATA_ONLY_EDIT_PREDICATE",
            verifier.MERGIFY_PROOF_PR_METADATA_ONLY_EDIT_PREDICATE,
        ),
        (
            "READY_PR_METADATA_ITERATION_EXPR",
            verifier.READY_PR_METADATA_ITERATION_EXPR,
        ),
    ]
    predicate_surfaces.extend(
        (path, repo_workflow_text(path))
        for path in (
            ".github/workflows/actionlint.yml",
            ".github/workflows/backtester-ci.yml",
            ".github/workflows/ci.yml",
            ".github/workflows/coverage-enforcer.yml",
        )
    )
    offenders = [name for name, text in predicate_surfaces if forbidden in text]
    if offenders:
        raise AssertionError(
            "base-change predicate must not compare against an empty string on: "
            + ", ".join(offenders)
        )


def assert_mergify_proof_prefix_alignment_detects_drift() -> None:
    verifier = load_verifier()
    provenance = load_provenance()
    config = provenance.load_config(REPO_ROOT / "ci" / "github-actions-runners.toml")

    original_predicate = verifier.MERGIFY_PROOF_PR_HEAD_REF_PREDICATE
    try:
        verifier.MERGIFY_PROOF_PR_HEAD_REF_PREDICATE = (
            "startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')"
        )
        resolver_only_tmp_errors = verifier.mergify_proof_prefix_alignment_errors(config)
    finally:
        verifier.MERGIFY_PROOF_PR_HEAD_REF_PREDICATE = original_predicate
    if not resolver_only_tmp_errors:
        raise AssertionError("resolver-only tmp Mergify proof-PR handling must be reported")
    if not any("workflow concurrency layer does not isolate" in error for error in resolver_only_tmp_errors):
        raise AssertionError(f"resolver-only tmp drift must name workflow isolation gap: {resolver_only_tmp_errors}")

    original_matcher = verifier.mergify_temp_pr_matches

    def bare_only_matcher(
        *,
        event_name: str,
        event_action: str,
        pull_request_draft: bool,
        pull_request_head_ref: str,
        temp_pr_head_ref_prefix: str,
        event_sender_id: int,
        temp_pr_actor_id: int,
    ) -> bool:
        return (
            event_name == "pull_request"
            and pull_request_draft
            and pull_request_head_ref.startswith(temp_pr_head_ref_prefix)
            and event_sender_id == temp_pr_actor_id
        )

    try:
        verifier.mergify_temp_pr_matches = bare_only_matcher
        workflow_only_tmp_errors = verifier.mergify_proof_prefix_alignment_errors(config)
    finally:
        verifier.mergify_temp_pr_matches = original_matcher
    if not workflow_only_tmp_errors:
        raise AssertionError("workflow-only tmp Mergify proof-PR handling must be reported")
    if not any("resolver does not promote" in error for error in workflow_only_tmp_errors):
        raise AssertionError(f"workflow-only tmp drift must name resolver promotion gap: {workflow_only_tmp_errors}")


def assert_mergify_proof_pr_concurrency_gaps_are_reported() -> None:
    verifier = load_verifier()

    def workflow_errors(workflow_name: str, workflow: str) -> list[str]:
        if workflow_name.endswith("/coverage-enforcer.yml"):
            return verifier.verify_coverage_enforcer_workflow({workflow_name: workflow})
        if workflow_name.endswith("/ci.yml"):
            return verifier.verify_workflow(workflow)
        return verifier.verify_repo_automation_texts({workflow_name: workflow})

    cases = [
        (
            ".github/workflows/ci.yml",
            "format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)",
            "format('pr-{0}-iteration', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
            "",
            "concurrency group must isolate Mergify proof PR runs",
        ),
        (
            ".github/workflows/backtester-ci.yml",
            "format('bvs-pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)",
            "format('bvs-pr-{0}-iteration', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
            "",
            "concurrency group must isolate Mergify proof PR runs",
        ),
        (
            ".github/workflows/actionlint.yml",
            "format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)",
            "format('pr-{0}-iteration', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}\n",
            " }}\n",
            "concurrency group must isolate Mergify proof PR runs",
        ),
        (
            ".github/workflows/coverage-enforcer.yml",
            "format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)",
            "format('pr-{0}-iteration', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}\n",
            " }}\n",
            "concurrency group must isolate Mergify proof PR runs",
        ),
    ]
    for workflow_name, group_fragment, group_replacement, cancel_guard, cancel_replacement, expected_error in cases:
        workflow = repo_workflow_text(workflow_name)
        if group_fragment not in workflow:
            raise AssertionError(f"{workflow_name} must isolate Mergify proof PR runs")
        missing_group = replace_once_after(
            workflow,
            "concurrency:",
            group_fragment,
            group_replacement,
        )
        group_errors = workflow_errors(workflow_name, missing_group)
        if not any(expected_error in error for error in group_errors):
            raise AssertionError(f"{workflow_name} must reject missing Mergify proof PR group, got: {group_errors}")

        missing_cancel_guard = replace_once_after(
            workflow,
            "cancel-in-progress:",
            cancel_guard,
            cancel_replacement,
        )
        cancel_errors = workflow_errors(workflow_name, missing_cancel_guard)
        if not any("cancel-in-progress must not cancel Mergify proof PR validations" in error for error in cancel_errors):
            raise AssertionError(f"{workflow_name} must reject cancelling Mergify proof PRs, got: {cancel_errors}")

        run_id_keyed = replace_once(group_fragment, "github.event.pull_request.head.sha", "github.run_id")
        old_group = replace_once_after(
            workflow,
            "concurrency:",
            group_fragment,
            run_id_keyed,
        )
        old_group_errors = workflow_errors(workflow_name, old_group)
        if not any("must key on head SHA" in error for error in old_group_errors):
            raise AssertionError(f"{workflow_name} must reject run_id-keyed Mergify proof groups, got: {old_group_errors}")

        if workflow_name.endswith(("/ci.yml", "/backtester-ci.yml", "/coverage-enforcer.yml")):
            mergify_iteration_exclusion = (
                "        && !((github.event.action == 'edited'\n"
                "              && !(github.event.changes.base.ref.from && true || false))\n"
                "             || (github.event.pull_request.draft == true\n"
                "                 && github.event.action == 'converted_to_draft'))\n"
            )
            if mergify_iteration_exclusion not in workflow:
                raise AssertionError(
                    f"{workflow_name} must isolate Mergify iteration events from pending proof runs"
                )
            shared_group = replace_once_after(
                workflow,
                "concurrency:",
                mergify_iteration_exclusion,
                "",
            )
            shared_group_errors = workflow_errors(workflow_name, shared_group)
            if not any(
                "Mergify proof PR concurrency must exclude iteration events" in error
                for error in shared_group_errors
            ):
                raise AssertionError(
                    f"{workflow_name} must reject a shared Mergify full/iteration group, got: {shared_group_errors}"
                )


def assert_dispatch_cancel_watchdog_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/dispatch-ci-cancel.yml")
    cases = [
        (
            "must trigger only on workflow_run",
            replace_once(workflow, "  workflow_run:\n", "  pull_request:\n"),
        ),
        (
            "workflow_run trigger must watch",
            replace_once(workflow, '    workflows: ["CI"]\n', '    workflows: ["Backtester CI"]\n'),
        ),
        (
            "workflow_run trigger must use requested only",
            replace_once(workflow, "    types: [requested]\n", "    types: [completed]\n"),
        ),
        (
            "permissions must include actions: write",
            replace_once(workflow, "  actions: write\n", "  actions: read\n"),
        ),
        (
            "permissions must include contents: read",
            replace_once(workflow, "  contents: read\n", "  contents: none\n"),
        ),
        (
            "must define cancel-obsolete-dispatch job",
            replace_once(workflow, "  cancel-obsolete-dispatch:\n", "  cancel-stale-dispatch:\n"),
        ),
        (
            "job must filter workflow_dispatch runs",
            replace_once(
                workflow,
                "github.event.workflow_run.event == 'workflow_dispatch'",
                "github.event.workflow_run.event == 'pull_request'",
            ),
        ),
        (
            "job must filter the configured CI workflow by path",
            replace_once(
                workflow,
                "github.event.workflow_run.path == '.github/workflows/ci.yml'",
                "github.event.workflow_run.path == '.github/workflows/backtester-ci.yml'",
            ),
        ),
        (
            "job must not filter the configured CI workflow by mutable name",
            replace_once(
                workflow,
                "github.event.workflow_run.path == '.github/workflows/ci.yml'",
                "github.event.workflow_run.name == 'CI'",
            ),
        ),
        (
            "job must join workflow_dispatch and CI filters with &&",
            replace_once(
                workflow,
                "          && github.event.workflow_run.path == '.github/workflows/ci.yml' }}\n",
                "          || github.event.workflow_run.path == '.github/workflows/ci.yml' }}\n",
            ),
        ),
        (
            "job must run scripts/cancel_obsolete_dispatch_runs.py",
            replace_once(
                workflow,
                "python3 scripts/cancel_obsolete_dispatch_runs.py",
                "python3 scripts/ci_provenance.py ci-policy",
            ),
        ),
        (
            "job must pass github.token",
            replace_once(
                workflow,
                "          GITHUB_TOKEN: ${{ github.token }}\n",
                "",
            ),
        ),
        (
            "job must pass github.event_path",
            replace_once(
                workflow,
                "          GITHUB_EVENT_PATH: ${{ github.event_path }}\n",
                "",
            ),
        ),
        (
            "job must pass github.repository",
            replace_once(
                workflow,
                "          GITHUB_REPOSITORY: ${{ github.repository }}\n",
                "",
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_dispatch_ci_cancel_workflow(
            {".github/workflows/dispatch-ci-cancel.yml": mutated_workflow}
        )
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_merge_readiness_progress_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    missing_job_workflow = workflow
    if "  merge-readiness-progress:\n" in workflow:
        missing_job_workflow = replace_once(
            workflow,
            "  merge-readiness-progress:\n",
            "  merge-readiness-progress-renamed:\n",
        )
    missing_job_errors = verifier.verify_merge_readiness_ci_job(
        missing_job_workflow
    )
    if not any("ci.yml must define merge-readiness-progress job" in error for error in missing_job_errors):
        raise AssertionError(f"expected missing progress job error, got: {missing_job_errors}")

    cases = [
        (
            "merge-readiness-progress permissions must include checks: read",
            replace_once(workflow, "      checks: read\n", ""),
        ),
        (
            "merge-readiness-progress permissions must include pull-requests: write",
            replace_once(workflow, "      pull-requests: write\n", "      pull-requests: read\n"),
        ),
        (
            "merge-readiness-progress must not request issues: write",
            replace_once(
                workflow,
                "      pull-requests: write\n",
                "      pull-requests: write\n      issues: write\n",
            ),
        ),
        (
            "merge-readiness-progress must check out the PR base SHA only",
            replace_once(
                workflow,
                "          ref: ${{ github.event.pull_request.base.sha }}\n",
                "          ref: ${{ github.event.pull_request.head.sha }}\n",
            ),
        ),
        (
            "merge-readiness-progress must run merge_readiness.py comment",
            replace_once(
                workflow,
                "python3 scripts/merge_readiness.py comment",
                "python3 scripts/merge_readiness.py status",
            ),
        ),
        (
            "merge-readiness-progress job if-condition must run only on non-draft Mergify proof PRs",
            replace_once(
                workflow,
                "          && github.event.pull_request.draft == false\n",
                "",
            ),
        ),
        (
            "merge-readiness-progress job if-condition must run only on non-draft Mergify proof PRs",
            replace_once(
                workflow,
                "          && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
                "              || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
                "",
            ),
        ),
        (
            "merge-readiness-progress job if-condition must run only on non-draft Mergify proof PRs",
            replace_once(
                workflow,
                "    if: >-\n"
                "      ${{ github.event_name == 'pull_request'\n"
                "          && github.event.pull_request.draft == false\n"
                "          && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
                "              || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
                "          && !(github.event.action == 'edited'\n"
                "               && !(github.event.changes.base.ref.from && true || false)) }}\n",
                "    if: ${{ github.event_name == 'pull_request' }}\n",
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_merge_readiness_ci_job(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_merge_readiness_finalizer_gaps_are_reported() -> None:
    verifier = load_verifier()
    clean_errors = verifier.verify_merge_readiness_finalizer_workflow(
        {".github/workflows/merge-readiness-finalizer.yml": BASE_MERGE_READINESS_FINALIZER_WORKFLOW}
    )
    if clean_errors:
        raise AssertionError(f"expected clean finalizer workflow, got: {clean_errors}")

    cases = [
        (
            "workflow_run trigger must use completed only",
            replace_once(BASE_MERGE_READINESS_FINALIZER_WORKFLOW, "    types: [completed]\n", "    types: [requested]\n"),
        ),
        (
            "permissions must include checks: read",
            replace_once(BASE_MERGE_READINESS_FINALIZER_WORKFLOW, "  checks: read\n", ""),
        ),
        (
            "permissions must include actions: read",
            replace_once(BASE_MERGE_READINESS_FINALIZER_WORKFLOW, "  actions: read\n", ""),
        ),
        (
            "permissions must include pull-requests: write",
            replace_once(BASE_MERGE_READINESS_FINALIZER_WORKFLOW, "  pull-requests: write\n", "  pull-requests: read\n"),
        ),
        (
            "permissions must not include actions: write",
            replace_once(
                BASE_MERGE_READINESS_FINALIZER_WORKFLOW,
                "  actions: read\n",
                "  actions: write\n",
            ),
        ),
        (
            "job must filter pull_request runs",
            replace_once(
                BASE_MERGE_READINESS_FINALIZER_WORKFLOW,
                "github.event.workflow_run.event == 'pull_request'",
                "github.event.workflow_run.event == 'workflow_dispatch'",
            ),
        ),
        (
            "job must run scripts/merge_readiness.py finalize-stalled",
            replace_once(
                BASE_MERGE_READINESS_FINALIZER_WORKFLOW,
                "python3 scripts/merge_readiness.py finalize-stalled",
                "python3 scripts/merge_readiness.py comment",
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_merge_readiness_finalizer_workflow(
            {".github/workflows/merge-readiness-finalizer.yml": mutated_workflow}
        )
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_coverage_enforcer_workflow_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/coverage-enforcer.yml"
    clean_errors = verifier.verify_coverage_enforcer_workflow(
        {workflow_name: BASE_COVERAGE_ENFORCER_WORKFLOW}
    )
    if clean_errors:
        raise AssertionError(f"expected clean coverage-enforcer workflow, got: {clean_errors}")
    quoted_permissions = (
        BASE_COVERAGE_ENFORCER_WORKFLOW.replace("  checks: read\n", "  'checks': 'read'\n", 1)
        .replace("  contents: read\n", '  "contents": "read"\n', 1)
        .replace("  pull-requests: read\n", "  'pull-requests': 'read'\n", 1)
    )
    quoted_permissions_errors = verifier.verify_coverage_enforcer_workflow(
        {workflow_name: quoted_permissions}
    )
    if quoted_permissions_errors:
        raise AssertionError(
            "expected quoted exact coverage-enforcer permissions to pass, "
            f"got: {quoted_permissions_errors}"
        )

    cases = [
        (
            "must exist as its own workflow",
            {},
        ),
        (
            "must trigger only on pull_request and merge_group",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  merge_group:\n", "  workflow_dispatch:\n")},
        ),
        (
            "pull_request types must include converted_to_draft",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "converted_to_draft, ", "")},
        ),
        (
            "on.pull_request must not define paths filters",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "    branches: [main]\n", "    branches: [main]\n    paths: ['src/**']\n")},
        ),
        (
            "merge_group trigger must use checks_requested",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "    types: [checks_requested]\n", "    types: [requested]\n")},
        ),
        (
            "permissions must match the exact read-only map",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  checks: read\n", "")},
        ),
        (
            "permissions must match the exact read-only map",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  checks: read\n", "  checks: write\n")},
        ),
        (
            "permissions must match the exact read-only map",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  pull-requests: read\n", "")},
        ),
        (
            "permissions must match the exact read-only map",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  contents: read\n", "  contents: write\n")},
        ),
        (
            "permissions must match the exact read-only map",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "  pull-requests: read\n",
                    "  pull-requests: read\n"
                    "  statuses: write\n",
                )
            },
        ),
        (
            "must define coverage-enforcer job",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  coverage-enforcer:\n", "  renamed:\n")},
        ),
        (
            "must check out only the trusted base tree",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "          ref: ${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha }}\n", "")},
        ),
        (
            "must not check out PR head code",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "github.event.pull_request.base.sha", "github.event.pull_request.head.sha")},
        ),
        (
            "checkout must not persist credentials",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "          persist-credentials: false\n", "")},
        ),
        (
            "job must run scripts/coverage_enforcer.py",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "python3 scripts/coverage_enforcer.py", "python3 scripts/merge_readiness.py status")},
        ),
        (
            "job must guard first-run trusted-base bootstrap",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n"
                    "            echo \"coverage-enforcer bootstrap fail-closed: trusted base tree lacks scripts/coverage_enforcer.py\"\n"
                    "            exit 1\n"
                    "          fi\n",
                    "",
                )
            },
        ),
        (
            "job must guard first-run trusted-base bootstrap",
            {
                workflow_name: BASE_COVERAGE_ENFORCER_WORKFLOW.replace(
                    "            echo \"coverage-enforcer bootstrap fail-closed: trusted base tree lacks scripts/coverage_enforcer.py\"\n"
                    "            exit 1\n",
                    "            echo \"coverage-enforcer bootstrap fail-closed: trusted base tree lacks scripts/coverage_enforcer.py\"\n"
                    "            exit 0\n",
                )
            },
        ),
        (
            "job must guard first-run trusted-base bootstrap",
            {
                workflow_name: BASE_COVERAGE_ENFORCER_WORKFLOW.replace(
                    "            echo \"coverage-enforcer bootstrap fail-closed: trusted base tree lacks event-aware scripts/coverage_enforcer.py\"\n"
                    "            exit 1\n",
                    "            echo \"coverage-enforcer bootstrap fail-closed: trusted base tree lacks event-aware scripts/coverage_enforcer.py\"\n"
                    "            exit 0\n",
                )
            },
        ),
        (
            "job must guard first-run trusted-base bootstrap",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n",
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n"
                    "            echo \"coverage-enforcer bootstrap fail-open: trusted base tree lacks scripts/coverage_enforcer.py\"\n"
                    "            exit 0\n"
                    "          fi\n"
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n",
                )
            },
        ),
        (
            "job must guard first-run trusted-base bootstrap",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n",
                    "          python3 scripts/coverage_enforcer.py\n"
                    "          if [ ! -f scripts/coverage_enforcer.py ]; then\n",
                )
            },
        ),
        (
            "coverage-enforcer Enforce coverage map step must be canonical",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      - name: Enforce coverage map\n",
                    "      - name: Enforce coverage map\n"
                    "        if: ${{ github.event_name == 'pull_request' }}\n",
                )
            },
        ),
        (
            "coverage-enforcer Enforce coverage map step must be canonical",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      - name: Enforce coverage map\n",
                    "      - name: Enforce coverage map\n"
                    "        continue-on-error: true\n",
                )
            },
        ),
        (
            "coverage-enforcer Enforce coverage map step must be canonical",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "        run: |\n",
                    "        run: >\n",
                )
            },
        ),
        (
            "coverage-enforcer Enforce coverage map step must be canonical",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "        run: |\n",
                    "        run: \"|\"\n",
                )
            },
        ),
        (
            "coverage-enforcer job steps must match the pinned trusted-base topology",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      - name: Enforce coverage map\n",
                    "      - name: Accidental pre-enforcer\n"
                    "        run: python3 scripts/coverage_enforcer.py\n\n"
                    "      - name: Enforce coverage map\n",
                )
            },
        ),
        (
            "coverage-enforcer job steps must match the pinned trusted-base topology",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      - name: Enforce coverage map\n",
                    "      - uses: actions/cache@0400d5f644dc74513175e3cd8d07132dd4860809 # v4.2.4\n\n"
                    "      - name: Enforce coverage map\n",
                )
            },
        ),
        (
            "coverage-enforcer job steps must match the pinned trusted-base topology",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      - name: Enforce coverage map\n",
                    "      - name: Quoted run pre-enforcer\n"
                    "        'run': |\n"
                    "          python3 scripts/coverage_enforcer.py\n\n"
                    "      - name: Enforce coverage map\n",
                )
            },
        ),
        (
            "coverage-enforcer must not be defined inside another workflow",
            {
                workflow_name: BASE_COVERAGE_ENFORCER_WORKFLOW,
                ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                    "jobs:\n",
                    "jobs:\n  coverage-enforcer:\n    name: coverage-enforcer\n    steps:\n      - run: python3 scripts/coverage_enforcer.py\n",
                    1,
                ),
            },
        ),
        (
            "coverage-enforcer job must not define a job-level if-condition",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n",
                    "    if: ${{ github.event_name == 'pull_request' }}\n"
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n",
                )
            },
        ),
        (
            "coverage-enforcer job must not define a job-level if-condition",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "          python3 scripts/coverage_enforcer.py\n",
                    "          python3 scripts/coverage_enforcer.py\n"
                    "    if: ${{ github.event_name == 'pull_request' }}\n",
                )
            },
        ),
        (
            "coverage-enforcer job must not define a job-level if-condition",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n"
                    "    'if': ${{ github.event_name == 'pull_request' }}\n",
                )
            },
        ),
        (
            "coverage-enforcer job must not define job-level continue-on-error",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n"
                    "    continue-on-error: true\n",
                )
            },
        ),
        (
            "coverage-enforcer job must not define job-level permissions",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}\n"
                    "    permissions:\n"
                    "      contents: write\n"
                    "      checks: write\n"
                    "      pull-requests: write\n",
                )
            },
        ),
    ]
    for fragment, workflows in cases:
        errors = verifier.verify_coverage_enforcer_workflow(workflows)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_coverage_enforcer_bootstrap_marker_matches_script() -> None:
    workflow = repo_workflow_text(".github/workflows/coverage-enforcer.yml")
    script = repo_workflow_text("scripts/coverage_enforcer.py")
    match = re.search(r'if ! grep -q "([^"]+)" scripts/coverage_enforcer\.py; then', workflow)
    if match is None:
        raise AssertionError("coverage-enforcer workflow must define event-aware bootstrap marker")
    marker = match.group(1)
    if marker not in script:
        raise AssertionError(
            "coverage-enforcer event-aware bootstrap marker must match scripts/coverage_enforcer.py"
        )


def assert_runner_contract_rejects_missing_and_extra_jobs() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/ci.yml"
    workflow = repo_workflow_text(workflow_name)
    renamed = replace_once(workflow, "  deny:\n", "  deny-renamed:\n")
    errors = verifier.verify_github_actions_runner_contract({workflow_name: renamed})
    if not any("deny" in error and "missing from workflow" in error for error in errors):
        raise AssertionError(f"runner contract must reject TOML job without workflow job, got: {errors}")
    if not any(
        "deny-renamed" in error and "ci/github-actions-runners.toml" in error
        for error in errors
    ):
        raise AssertionError(f"runner contract must reject workflow job without TOML mapping, got: {errors}")


def assert_runner_contract_rejects_unmapped_workflow_jobs() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/actionlint.yml"
    workflow = repo_workflow_text(workflow_name)
    rogue = replace_once(
        workflow,
        "jobs:\n",
        """jobs:
  rogue:
    name: rogue
    runs-on: ubuntu-latest
    steps:
      - run: echo rogue

""",
    )
    errors = verifier.verify_github_actions_runner_contract({workflow_name: rogue})
    if not any("rogue" in error and "ci/github-actions-runners.toml" in error for error in errors):
        raise AssertionError(f"runner contract must reject unmapped workflow jobs, got: {errors}")


def assert_deleted_ai_review_model_freshness_workflow_is_unmapped() -> None:
    verifier = load_verifier()
    deleted_names = {
        "ai-review-model-freshness.yml",
        ".github/workflows/ai-review-model-freshness.yml",
    }
    mapped_names = set(verifier.WORKFLOW_RUNNER_CONFIG_KEYS)
    stale_names = sorted(mapped_names & deleted_names)
    if stale_names:
        raise AssertionError(
            f"deleted workflow must not remain in WORKFLOW_RUNNER_CONFIG_KEYS: {stale_names}"
        )
    if "ai_review_model_freshness" in verifier.WORKFLOW_RUNNER_CONFIG_KEYS.values():
        raise AssertionError(
            "deleted workflow runner config key ai_review_model_freshness must not remain mapped"
        )


def assert_runner_contract_accepts_flaky_detection_workflow_mapping() -> None:
    verifier = load_verifier()
    detection_workflow_name = ".github/workflows/flaky-test-detection.yml"
    detection_workflow = """name: Flaky Test Detection

on:
  schedule:
    - cron: '0 */12 * * 1-5'

jobs:
  flaky-detection-rust-root:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_detection.flaky-detection-rust-root

  flaky-detection-rust-backtester:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_detection.flaky-detection-rust-backtester

  flaky-detection-rust-backtester-issue-789:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_detection.flaky-detection-rust-backtester-issue-789
"""
    smoke_workflow_name = ".github/workflows/flaky-test-smoke.yml"
    smoke_workflow = """name: Flaky Test Smoke

on:
  workflow_dispatch:

jobs:
  flaky-smoke-rust-root:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_smoke.flaky-smoke-rust-root

  flaky-smoke-rust-backtester:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_smoke.flaky-smoke-rust-backtester

  flaky-smoke-rust-backtester-issue-789:
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          build-jobs-key: flaky_test_smoke.flaky-smoke-rust-backtester-issue-789
"""
    errors = verifier.verify_github_actions_runner_contract(
        {
            detection_workflow_name: detection_workflow,
            smoke_workflow_name: smoke_workflow,
        }
    )
    if errors:
        raise AssertionError(f"flaky detection workflow runner contract must be mapped, got: {errors}")


def assert_runner_config_floor_handles_missing_and_empty_inputs() -> None:
    verifier = load_verifier()
    real_workflows = {
        ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml"),
        DEBUG_WORKFLOW_PATH: repo_workflow_text(DEBUG_WORKFLOW_PATH),
        ".github/workflows/dispatch-ci-cancel.yml": repo_workflow_text(
            ".github/workflows/dispatch-ci-cancel.yml"
        ),
        ".github/workflows/merge-readiness-finalizer.yml": repo_workflow_text(
            ".github/workflows/merge-readiness-finalizer.yml"
        ),
    }

    def assert_floor(errors: list[str], label: str) -> None:
        if not any(
            "github-actions-runners.toml" in error and "enforcement set is empty" in error
            for error in errors
        ):
            raise AssertionError(f"{label} must fail closed on empty runner config, got: {errors}")

    cases: tuple[tuple[str, Callable[[], list[str]]], ...] = (
        (
            "artifact retention policy",
            lambda: verifier.verify_artifact_retention_policy({}, {}),
        ),
        (
            "ci runner debug workflow",
            lambda: verifier.verify_ci_runner_debug_workflow(real_workflows),
        ),
        (
            "dispatch CI cancel workflow",
            lambda: verifier.verify_dispatch_ci_cancel_workflow(real_workflows),
        ),
        (
            "merge readiness finalizer workflow",
            lambda: verifier.verify_merge_readiness_finalizer_workflow(real_workflows),
        ),
        (
            "github actions runner contract",
            lambda: verifier.verify_github_actions_runner_contract(real_workflows),
        ),
        (
            "actionlint runner contract",
            lambda: verifier.verify_actionlint_runner_contract(real_workflows),
        ),
        (
            "ci provenance dispatch names",
            lambda: verifier.configured_ci_provenance_dispatch_names()[1],
        ),
        (
            "workflow run name",
            lambda: verifier.workflow_run_name_errors(real_workflows[".github/workflows/ci.yml"]),
        ),
    )

    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        missing_config = root / "ci" / "github-actions-runners.toml"
        empty_config = write_temp_runner_config(root / "empty", "")
        for config_path in (missing_config, empty_config):
            verifier.DEFAULT_RUNNERS_CONFIG = config_path
            try:
                for label, run in cases:
                    assert_floor(run(), f"{label} with {config_path}")
                stderr = io.StringIO()
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
                    result = verifier.main()
                if result != 1 or "enforcement set is empty" not in stderr.getvalue():
                    raise AssertionError(
                        "main must report the runner config enforcement floor, "
                        f"got result={result}, stderr={stderr.getvalue()!r}"
                    )
                expected_error = f"ERROR: {verifier.RUNNERS_CONFIG_LABEL}: enforcement set is empty"
                error_lines = stderr.getvalue().splitlines()
                if error_lines.count(expected_error) != 1:
                    raise AssertionError(
                        "main must deduplicate the shared runner config floor, "
                        f"got {error_lines.count(expected_error)} copies in {error_lines!r}"
                    )
            finally:
                verifier.DEFAULT_RUNNERS_CONFIG = original_config


def assert_flaky_detection_workflow_uses_supported_mergify_contract() -> None:
    workflows = (
        ".github/workflows/flaky-test-detection.yml",
        ".github/workflows/flaky-test-smoke.yml",
    )
    pinned_v14_action = "uses: mergifyio/gha-mergify-ci@d01f69e6275942be9a9066fd22cda1c49b0c85e3 # v14"
    expected_job_names = (
        "job_name: nextest archive",
        "job_name: bvs-test archive",
        "job_name: bvs-test issue-789",
    )
    expected_cli_job_env = (
        "MERGIFY_TEST_JOB_NAME: nextest archive",
        "MERGIFY_TEST_JOB_NAME: bvs-test archive",
        "MERGIFY_TEST_JOB_NAME: bvs-test issue-789",
    )
    for workflow_path in workflows:
        workflow = repo_workflow_text(workflow_path)
        if workflow.count(pinned_v14_action) != 3:
            raise AssertionError(f"{workflow_path} must pin all Mergify uploads to the v14 action SHA")
        if "flaky_test_detection:" in workflow:
            raise AssertionError(f"{workflow_path} must not pass unsupported Mergify inputs")
        if "MERGIFY_JOB_NAME:" in workflow:
            raise AssertionError(f"{workflow_path} must pass job names through the Mergify job_name input")
        for job_name in expected_job_names:
            if job_name not in workflow:
                raise AssertionError(f"{workflow_path} missing Mergify upload {job_name!r}")
        for job_env in expected_cli_job_env:
            if job_env not in workflow:
                raise AssertionError(f"{workflow_path} missing current Mergify CLI env {job_env!r}")
        if workflow.count("MERGIFY_TEST_EXIT_CODE=%s") != 3:
            raise AssertionError(f"{workflow_path} must pass test runner exit codes to Mergify")
        if "python3 scripts/ci_input_sets.py hash backtester_cache" in workflow:
            raise AssertionError(f"{workflow_path} must not compute a dead BVS target-cache digest")
        if "managed-target-bvs-" in workflow:
            raise AssertionError(f"{workflow_path} must not restore a BVS whole-target cache")
        if "managed-target-bvs-v1-" in workflow or "flaky-root-test-" in workflow:
            raise AssertionError(f"{workflow_path} must restore from production cache namespaces")


def assert_flaky_backtester_jobs_require_shared_minio_action() -> None:
    verifier = load_verifier()
    action_step = """      - name: Setup BVS MinIO S3 smoke
        uses: ./.github/actions/setup-bvs-minio-s3-smoke

"""
    for workflow_path, job_anchor in (
        (".github/workflows/flaky-test-detection.yml", "  flaky-detection-rust-backtester:\n"),
        (".github/workflows/flaky-test-smoke.yml", "  flaky-smoke-rust-backtester:\n"),
    ):
        workflow = repo_workflow_text(workflow_path)
        mutated = replace_once_after(workflow, job_anchor, action_step, "")
        errors = verifier.flaky_test_detection_workflow_errors(
            mutated,
            verifier.FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS[workflow_path],
        )
        if not any("backtester" in error and "must keep BVS job steps unchanged" in error for error in errors):
            raise AssertionError(f"{workflow_path} may silently omit the shared MinIO action: {errors!r}")

        reordered = replace_once_after(
            mutated,
            job_anchor,
            "      - name: Stage JUnit report\n",
            action_step + "      - name: Stage JUnit report\n",
        )
        reordered_errors = verifier.flaky_test_detection_workflow_errors(
            reordered,
            verifier.FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS[workflow_path],
        )
        if not any(
            "backtester" in error and "must keep BVS job steps unchanged" in error
            for error in reordered_errors
        ):
            raise AssertionError(
                f"{workflow_path} may set up MinIO after running BVS tests: {reordered_errors!r}"
            )






def assert_runner_contract_requires_meter_workflows_for_managed_workflows() -> None:
    verifier = load_verifier()
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_text = original_config.read_text()
        config_path = write_temp_runner_config(
            pathlib.Path(tmp),
            config_text.replace(
                '  "backtester_ci",\n',
                "",
            ),
        )
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            errors = verifier.verify_github_actions_runner_contract(
                {".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml")}
            )
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if not any("meter.included_workflows" in error and "backtester_ci" in error for error in errors):
        raise AssertionError(f"runner contract must reject unmanaged meter workflow drift, got: {errors}")


def assert_runner_contract_requires_meter_api_limits() -> None:
    verifier = load_verifier()
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_text = original_config.read_text()
        config_path = write_temp_runner_config(
            pathlib.Path(tmp),
            config_text.replace(
                """
[meter.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
branch_pull_requests_per_page = 20
draft_timeline_items = 100
""",
                "",
            ),
        )
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            errors = verifier.verify_github_actions_runner_contract(
                {".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml")}
            )
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if not any("meter.api_limits" in error for error in errors):
        raise AssertionError(f"runner contract must reject missing meter api limits, got: {errors}")

    bool_config = original_config.read_text().replace(
        "workflow_runs_per_page = 100",
        "workflow_runs_per_page = true",
        1,
    )
    error = runner_config_load_error(bool_config, verifier=verifier)
    if "meter.api_limits.workflow_runs_per_page must be a positive integer" not in error:
        raise AssertionError(f"runner contract must reject boolean meter api limit, got: {error!r}")


def assert_runner_contract_requires_fingerprint_archive_tier_coupling() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/ci.yml"
    workflow = repo_workflow_text(workflow_name)
    recoupled_workflow = replace_once(
        workflow,
        """  nextest-fingerprint:
    name: nextest fingerprint
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}
""",
        """  nextest-fingerprint:
    name: nextest fingerprint
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ${{ vars.CI_RUNNER_MANAGED_LIGHT }}
""",
    )
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_text = original_config.read_text()
        config_path = write_temp_runner_config(
            pathlib.Path(tmp),
            replace_once(
                config_text,
                'nextest-fingerprint = "managed_heavy"',
                'nextest-fingerprint = "managed_light"',
            ),
        )
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            errors = verifier.verify_github_actions_runner_contract({workflow_name: recoupled_workflow})
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if not any("nextest-fingerprint and test-archive must use the same runner tier" in error for error in errors):
        raise AssertionError(f"runner contract must reject nextest fingerprint/archive tier split, got: {errors}")


def assert_runner_contract_requires_configured_cargo_build_jobs() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/ci.yml"
    workflow = repo_workflow_text(workflow_name)
    missing_key = workflow.replace("          build-jobs-key: ci.clippy\n", "", 1)
    errors = verifier.verify_github_actions_runner_contract({workflow_name: missing_key})
    if not any("clippy must resolve CARGO_BUILD_JOBS from cargo_build_jobs.ci.clippy" in error for error in errors):
        raise AssertionError(f"runner contract must reject uncapped compile lanes, got: {errors}")
    inline_cap = replace_once(
        workflow,
        "  clippy:\n    name: clippy\n    needs: [ci-policy, detector]\n    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && !startsWith(github.ref, 'refs/tags/v') }}\n    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n    steps:\n",
        '  clippy:\n    name: clippy\n    needs: [ci-policy, detector]\n    if: ${{ needs.ci-policy.outputs.full_ci_required == \'true\' && !startsWith(github.ref, \'refs/tags/v\') }}\n    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n    env:\n      CARGO_BUILD_JOBS: "9"\n    steps:\n',
    )
    inline_errors = verifier.verify_github_actions_runner_contract({workflow_name: inline_cap})
    if not any("clippy CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml" in error for error in inline_errors):
        raise AssertionError(f"runner contract must reject inline CARGO_BUILD_JOBS, got: {inline_errors}")
    workflow_env_cap = replace_once(
        workflow,
        "env:\n  JUST_VERSION: \"1.49.0\"\n",
        "env:\n  JUST_VERSION: \"1.49.0\"\n  CARGO_BUILD_JOBS: \"9\"\n",
    )
    workflow_env_errors = verifier.verify_github_actions_runner_contract(
        {workflow_name: workflow_env_cap}
    )
    if not any(
        "workflow-level CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml" in error
        for error in workflow_env_errors
    ):
        raise AssertionError(
            f"runner contract must reject workflow-level CARGO_BUILD_JOBS, got: {workflow_env_errors}"
        )
    shell_inline_anchor = (
        "  clippy:\n"
        "    name: clippy\n"
        "    needs: [ci-policy, detector]\n"
        "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && !startsWith(github.ref, 'refs/tags/v') }}\n"
        "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n"
        "    steps:\n"
    )
    shell_inline_cases = {
        "github env append": '      - run: echo "CARGO_BUILD_JOBS=9" >> "$GITHUB_ENV"\n',
        "export": "      - run: export CARGO_BUILD_JOBS=9\n",
        "inline env prefix": "      - run: CARGO_BUILD_JOBS=9 just clippy\n",
    }
    for case, injected_step in shell_inline_cases.items():
        shell_inline_cap = replace_once(
            workflow,
            shell_inline_anchor,
            shell_inline_anchor + injected_step,
        )
        shell_inline_errors = verifier.verify_github_actions_runner_contract(
            {workflow_name: shell_inline_cap}
        )
        if not any(
            "clippy CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml" in error
            for error in shell_inline_errors
        ):
            raise AssertionError(
                f"runner contract must reject shell inline CARGO_BUILD_JOBS via {case}, got: {shell_inline_errors}"
            )
    conditional_setup = replace_once(
        workflow,
        """      - name: Setup environment
        id: setup
        uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
          toolchain-components: clippy, rustfmt
          include-managed-target-dir: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.clippy
""",
        """      - name: Setup environment
        id: setup
        if: ${{ false }}
        uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
          toolchain-components: clippy, rustfmt
          include-managed-target-dir: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.clippy
""",
    )
    conditional_setup_errors = verifier.verify_github_actions_runner_contract(
        {workflow_name: conditional_setup}
    )
    if not any(
        (
            "clippy build-jobs-key setup-environment step must be unconditional "
            "or match the cargo/just compile step condition"
        )
        in error
        for error in conditional_setup_errors
    ):
        raise AssertionError(
            f"runner contract must reject conditional CARGO_BUILD_JOBS setup, got: {conditional_setup_errors}"
        )
    late_setup = replace_once(
        workflow,
        shell_inline_anchor,
        shell_inline_anchor + "      - run: just clippy\n",
    )
    late_setup_errors = verifier.verify_github_actions_runner_contract({workflow_name: late_setup})
    if not any(
        "clippy build-jobs-key setup-environment step must run before cargo/just compile commands" in error
        for error in late_setup_errors
    ):
        raise AssertionError(
            f"runner contract must reject compile commands before CARGO_BUILD_JOBS setup, got: {late_setup_errors}"
        )
    invalid_config = replace_config_key_line_once(
        ci_provenance_config_fixture(), "cargo_build_jobs.ci", "clippy", "clippy = true"
    )
    error = runner_config_load_error(invalid_config, verifier=verifier)
    if "cargo_build_jobs.ci.clippy must be a positive integer" not in error:
        raise AssertionError(f"runner contract must reject invalid cargo build jobs config, got: {error!r}")
    missing_config = replace_config_key_line_once(
        ci_provenance_config_fixture(), "cargo_build_jobs.ci", "clippy", None
    )
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_path = write_temp_runner_config(pathlib.Path(tmp), missing_config)
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            missing_config_errors = verifier.verify_github_actions_runner_contract({workflow_name: workflow})
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if not any(
        "clippy has build-jobs-key but is missing from cargo_build_jobs.ci" in error
        for error in missing_config_errors
    ):
        raise AssertionError(
            f"runner contract must reject workflow build-jobs-key without TOML config, got: {missing_config_errors}"
        )


def assert_ci_workflow_rejects_spike_probe_markers() -> None:
    verifier = load_verifier()
    workflow = textwrap.dedent(
        """
        name: CI
        jobs:
          clippy:
            steps:
              - name: clippy
                run: |
                  BOLT_SPIKE_TIME_FILE="${RUNNER_TEMP:-/tmp}/clippy.time"
                  just clippy
        """
    )
    errors = verifier.verify_workflow(workflow)
    if not any("BOLT_SPIKE" in error for error in errors):
        raise AssertionError(f"ci workflow must reject spike probe markers, got: {errors}")


def assert_debug_workflow_rejects_non_manual_trigger() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(DEBUG_WORKFLOW_PATH)
    with_push = replace_once(
        workflow,
        "on:\n  workflow_dispatch:\n",
        "on:\n  push:\n    branches: [main]\n  workflow_dispatch:\n",
    )
    errors = verifier.verify_ci_runner_debug_workflow({DEBUG_WORKFLOW_PATH: with_push})
    if not any("manual-only" in error and "workflow_dispatch" in error for error in errors):
        raise AssertionError(f"debug workflow must reject non-manual triggers, got: {errors}")


def assert_debug_workflow_checks_each_ssh_runner_step() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(DEBUG_WORKFLOW_PATH)
    unpinned_first_job = replace_once(
        workflow,
        f"uses: {SSH_RUNNER_ACTION} # v2.0",
        "uses: ubicloud/ssh-runner@v2",
    )
    errors = verifier.verify_ci_runner_debug_workflow({DEBUG_WORKFLOW_PATH: unpinned_first_job})
    if not any("debug-heavy" in error and SSH_RUNNER_ACTION in error for error in errors):
        raise AssertionError(f"debug verifier must check each SSH runner step, got: {errors}")




def assert_debug_lane_compile_cache_parity_contract() -> None:
    verifier = load_verifier()
    workflows = {
        ".github/workflows/debug-test.yml": repo_workflow_text(".github/workflows/debug-test.yml"),
        ".github/workflows/rust-probe.yml": repo_workflow_text(".github/workflows/rust-probe.yml"),
        ".github/workflows/flaky-test-detection.yml": repo_workflow_text(".github/workflows/flaky-test-detection.yml"),
        ".github/workflows/flaky-test-smoke.yml": repo_workflow_text(".github/workflows/flaky-test-smoke.yml"),
    }
    bvs_policy = repo_source_text("crates/backtesting-vertical-slice/ci/rust-verification.toml")
    if not (REPO_ROOT / ".github" / "actions" / "sccache-setup" / "action.yml").exists():
        raise AssertionError("debug lanes must route sccache setup through the shared sccache action")
    if not (REPO_ROOT / ".github" / "actions" / "sccache-stats" / "action.yml").exists():
        raise AssertionError("debug lanes must route post-compile stats through the shared sccache stats action")
    for workflow_name, workflow_text in workflows.items():
        if "uses: ./.github/actions/sccache-setup" not in workflow_text:
            raise AssertionError(f"{workflow_name} must use the shared sccache setup action")
        if "uses: ./.github/actions/sccache-stats" not in workflow_text:
            raise AssertionError(f"{workflow_name} must use the shared sccache stats action after compile")
    if "workflow_dispatch:" not in workflows[".github/workflows/flaky-test-smoke.yml"]:
        raise AssertionError("flaky smoke workflow must be manually dispatchable for exact-head evidence")
    if workflows[".github/workflows/flaky-test-smoke.yml"].count('exit "$rc"') != 3:
        raise AssertionError("flaky smoke run steps must exit with the captured rc")
    if 'run: cp "' in workflows[".github/workflows/flaky-test-smoke.yml"]:
        raise AssertionError("flaky smoke JUnit staging must synthesize missing-report failures")
    errors = verifier.verify_debug_lane_compile_cache_parity(workflows, bvs_policy)
    if errors:
        raise AssertionError(f"debug lanes must satisfy compile-cache parity, got: {errors}")

    flaky_jobs = (
        (".github/workflows/flaky-test-detection.yml", "flaky-detection-rust-backtester"),
        (".github/workflows/flaky-test-detection.yml", "flaky-detection-rust-backtester-issue-789"),
        (".github/workflows/flaky-test-smoke.yml", "flaky-smoke-rust-backtester"),
        (".github/workflows/flaky-test-smoke.yml", "flaky-smoke-rust-backtester-issue-789"),
    )
    setup_block = (
        "      - name: Setup read-only sccache\n"
        "        id: sccache\n"
        "        uses: ./.github/actions/sccache-setup\n"
        "        with:\n"
        "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n"
    )
    for workflow_name, job_name in flaky_jobs:
        workflow = workflows[workflow_name]
        anchor = f"  {job_name}:\n"
        mutations = [
            replace_once_after(workflow, anchor, setup_block, ""),
            replace_once_after(
                workflow,
                anchor,
                "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
                "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}",
                'BOLT_RUST_VERIFICATION_SCCACHE: "1"',
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Print sccache stats\n",
                "      - name: Omit sccache stats\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Restore forbidden BVS target cache\n"
                "        uses: actions/cache/restore@example\n"
                "        with:\n"
                "          path: crates/backtesting-vertical-slice/target\n"
                "          key: forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Block-scalar third-party BVS target cache\n"
                "        uses: buildjet/cache@example\n"
                "        with:\n"
                "          path: |\n"
                "            ./crates/backtesting-vertical-slice/target/\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Third-party forbidden BVS target cache\n"
                "        uses: buildjet/cache@example\n"
                "        with:\n"
                "          path: ${{ steps.crate_target.outputs.dir }}\n"
                "          key: forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Save forbidden BVS target cache\n"
                "        uses: actions/cache/save@example\n"
                "        with:\n"
                "          path: crates/backtesting-vertical-slice/target\n"
                "          key: forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Combined forbidden BVS target cache\n"
                "        uses: actions/cache@example\n"
                "        with:\n"
                "          path: crates/backtesting-vertical-slice/target\n"
                "          key: forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Inject forbidden BVS target key\n"
                "        env:\n"
                "          FORBIDDEN_KEY: managed-target-bvs-evil\n"
                "        run: echo forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Inject forbidden broad restore prefix\n"
                "        uses: example/cache@example\n"
                "        with:\n"
                "          restore-keys: forbidden\n"
                "      - name: Install cargo-nextest\n",
            ),
            replace_once_after(
                workflow,
                anchor,
                "      - name: Install cargo-nextest\n",
                "      - name: Compute BVS cache input hash\n"
                "        id: bvs_cache_inputs\n"
                "        run: echo digest=$(python3 scripts/ci_input_sets.py hash backtester_cache)\n"
                "      - name: Install cargo-nextest\n",
            ),
        ]
        expected_substrings = [
            "must route read-only sccache through the shared sccache action",
            "must use only the PR-readonly sccache role",
            "compile step must opt into managed sccache conditionally",
            "must print sccache stats after compile",
            "must not restore or save a BVS whole-target cache",
            "must not pass the BVS target directory to an action",
            "must not pass the BVS target directory to an action",
            "must not restore or save a BVS whole-target cache",
            "must not restore or save a BVS whole-target cache",
            "must not contain BVS target-cache fragment 'managed-target-bvs-'",
            "must not contain BVS target-cache fragment 'restore-keys:'",
            "must not contain BVS target-cache fragment 'bvs_cache_inputs'",
        ]
        for field, replacement, expected_substring in (
            ("          cache-targets: false\n", "          cache-targets: true\n", "must set cache-targets: false"),
            ("          cache-bin: false\n", "          cache-bin: true\n", "must set cache-bin: false"),
            (
                "          cache-targets: false\n",
                '          cache-targets: false\n          "cache-directories": crates/backtesting-vertical-slice/target\n',
                "must not set cache-directories",
            ),
        ):
            mutations.append(replace_once_after(workflow, anchor, field, replacement))
            expected_substrings.append(expected_substring)
        for forbidden_env in (
            "RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_INCREMENTAL",
        ):
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n",
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n"
                    f'          {forbidden_env}: "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n"
                    "    env: {"
                    f'{forbidden_env}: "forbidden"'
                    "}\n",
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n",
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n"
                    f'          {forbidden_env} : "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n",
                    "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n"
                    f'          "{forbidden_env}": "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n"
                    "    env:\n"
                    f'      "{forbidden_env}": "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n"
                    "    env:\n"
                    f'      {forbidden_env}: "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
            mutations.append(
                replace_once_after(
                    workflow,
                    anchor,
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n",
                    "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n"
                    "    env:\n"
                    f'      {forbidden_env} : "forbidden"\n',
                )
            )
            expected_substrings.append(f"must not bypass managed_env with {forbidden_env}")
        for mutation_index, (mutated, expected_substring) in enumerate(zip(mutations, expected_substrings, strict=True)):
            mutated_workflows = {**workflows, workflow_name: mutated}
            mutation_errors = verifier.verify_debug_lane_compile_cache_parity(mutated_workflows, bvs_policy)
            mutation_errors.extend(
                verifier.flaky_test_detection_workflow_errors(
                    mutated,
                    verifier.FLAKY_TEST_DETECTION_WORKFLOW_CONTRACTS[workflow_name],
                )
            )
            if not any(expected_substring in error for error in mutation_errors):
                raise AssertionError(
                    f"{workflow_name} {job_name} sccache mutation {mutation_index} did not emit "
                    f"{expected_substring!r}: {mutation_errors!r}"
                )

    cases = (
        (
            "debug-test package compiles must keep sccache active",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    "steps.debug-archive-ready.outputs.value != 'true' || inputs.package != ''",
                    "steps.debug-archive-ready.outputs.value != 'true'",
                    1,
                ),
            },
            bvs_policy,
            "package debug-test compiles",
        ),
        (
            "rust-probe must keep the configured linker",
            {
                **workflows,
                ".github/workflows/rust-probe.yml": workflows[".github/workflows/rust-probe.yml"].replace(
                    '          install-rust-linker: "true"\n',
                    "",
                    1,
                ),
            },
            bvs_policy,
            "probe-heavy must install configured Rust linker",
        ),
        (
            "rust-probe must use the read-only role",
            {
                **workflows,
                ".github/workflows/rust-probe.yml": workflows[".github/workflows/rust-probe.yml"].replace(
                    "AWS_CI_CACHE_PR_READONLY_ROLE_ARN",
                    "AWS_CI_CACHE_ROLE_ARN",
                    1,
                ),
            },
            bvs_policy,
            "must use only the PR-readonly sccache role",
        ),
        (
            "flaky smoke must use the shared action",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    "uses: ./.github/actions/sccache-setup",
                    "uses: ./.github/actions/not-sccache-setup",
                    1,
                ),
            },
            bvs_policy,
            "must route read-only sccache through the shared sccache action",
        ),
        (
            "flaky smoke must opt into the managed wrapper",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}",
                    'BOLT_RUST_VERIFICATION_SCCACHE: "1"',
                    1,
                ),
            },
            bvs_policy,
            "compile step must opt into managed sccache conditionally",
        ),
        (
            "debug sccache setup must use the read-only role",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
                    "",
                    1,
                ),
            },
            bvs_policy,
            "must route read-only sccache through the shared sccache action",
        ),
        (
            "debug compile step must not use workflow-level retry helpers",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    'bash .github/scripts/sccache-fail-open.sh --on any "$log" just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "debug compile step must match the test-archive profile",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    '          CARGO_PROFILE_TEST_DEBUG: "0"\n',
                    "",
                    1,
                ),
            },
            bvs_policy,
            "compile step must match the test-archive debug profile env",
        ),
        (
            "flaky smoke run step must not use workflow-level retry helpers",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    'bash .github/scripts/sccache-fail-open.sh --on any "$log" just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "flaky smoke compile step must match the test-archive profile",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    '          CARGO_PROFILE_DEV_DEBUG: "0"\n',
                    "",
                    1,
                ),
            },
            bvs_policy,
            "compile step must match the test-archive debug profile env",
        ),
        (
            "flaky smoke must exit with captured rc",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    '          exit "$rc"\n',
                    "",
                    1,
                ),
            },
            bvs_policy,
            "flaky smoke run step must exit with captured rc",
        ),
        (
            "flaky smoke run step must not continue on error",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    "      - name: Run tests\n        env:\n",
                    "      - name: Run tests\n        continue-on-error: true\n        env:\n",
                    1,
                ),
            },
            bvs_policy,
            "compile/test run step must not use continue-on-error",
        ),
        (
            "flaky smoke JUnit staging must synthesize missing-report failures",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    "missing-nextest-junit",
                    "missing-nextest-report",
                    1,
                ),
            },
            bvs_policy,
            "JUnit staging must synthesize a missing-report failure",
        ),
        (
            "debug test execution must not retry test execution in workflow shell",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    'bash .github/scripts/sccache-fail-open.sh --on any "$log" just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "debug test execution must not retry test execution via cache-error helper",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    'bash .github/scripts/sccache-fail-open.sh --on cache-error "$log" just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "debug test execution must not force sccache off",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    'BOLT_RUST_VERIFICATION_SCCACHE=0 just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    1,
                ),
            },
            bvs_policy,
            "test execution must not force sccache off",
        ),
        (
            "debug test execution must not be an echo spoof",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"',
                    'echo \'just debug-test "$DEBUG_TEST_FILTER" "$DEBUG_TEST_PACKAGE" 2>&1 | tee -a "$log"\'',
                    1,
                ),
            },
            bvs_policy,
            "run step must execute debug-test through one managed just invocation",
        ),
        (
            "rust-probe must not use workflow-level retry helpers",
            {
                **workflows,
                ".github/workflows/rust-probe.yml": workflows[".github/workflows/rust-probe.yml"].replace(
                    "bash .github/scripts/run-rust-probe.sh",
                    "bash .github/scripts/sccache-fail-open.sh --on any \"$RUNNER_TEMP/rust-probe.log\" bash .github/scripts/run-rust-probe.sh",
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "debug stats must run on failures",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    "      - name: Print sccache stats\n        if: always()\n",
                    "      - name: Print sccache stats\n        if: success()\n",
                    1,
                ),
            },
            bvs_policy,
            "must print sccache stats after compile",
        ),
        (
            "debug stats must require step-level always",
            {
                **workflows,
                ".github/workflows/debug-test.yml": workflows[".github/workflows/debug-test.yml"].replace(
                    "      - name: Print sccache stats\n        if: always()\n",
                    "      - name: Print sccache stats\n        env:\n          SPOOF: \"if: always()\"\n",
                    1,
                ),
            },
            bvs_policy,
            "must print sccache stats after compile",
        ),
        (
            "flaky smoke execution must not retry test execution in workflow shell",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    'bash .github/scripts/sccache-fail-open.sh --on any "$log" just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    1,
                ),
            },
            bvs_policy,
            "retry and compile/test split must be owned by rust_verification.py",
        ),
        (
            "flaky smoke execution must not chain another test command",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast 2>&1 | tee -a "$log"',
                    'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast 2>&1 | tee -a "$log"; just test --config-file "$RUNNER_TEMP/nextest-junit.toml"',
                    1,
                ),
            },
            bvs_policy,
            "run step must execute tests through one managed just invocation",
        ),
        (
            "flaky smoke execution must not force sccache off",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    'just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    'BOLT_RUST_VERIFICATION_SCCACHE=0 just test --config-file "$RUNNER_TEMP/nextest-junit.toml" --no-fail-fast',
                    1,
                ),
            },
            bvs_policy,
            "test execution must not force sccache off",
        ),
        (
            "flaky smoke JUnit staging must not be an echo spoof",
            {
                **workflows,
                ".github/workflows/flaky-test-smoke.yml": workflows[".github/workflows/flaky-test-smoke.yml"].replace(
                    'python3 - > "$staged" <<\'PY\'',
                    'echo missing-nextest-junit > "$staged"',
                    1,
                ),
            },
            bvs_policy,
            "JUnit staging must synthesize a missing-report failure",
        ),
        (
            "BVS policy must activate the remote compile cache",
            workflows,
            bvs_policy.replace(
                "[remote_compile_cache]\n"
                "enabled = true\n"
                'enable_env = "BOLT_RUST_VERIFICATION_SCCACHE"\n'
                'ci_env = "GITHUB_ACTIONS"\n'
                'wrapper_env = "SCCACHE_PATH"\n'
                'wrapper_program = "sccache"\n\n',
                "",
                1,
            ),
            "backtesting-vertical-slice rust policy must include [remote_compile_cache]",
        ),
        (
            "BVS policy must not disable the remote compile cache",
            workflows,
            bvs_policy.replace("[remote_compile_cache]\nenabled = true", "[remote_compile_cache]\nenabled = false", 1),
            "remote_compile_cache.enabled=True",
        ),
        (
            "BVS policy must activate the remote fast linker",
            workflows,
            bvs_policy.replace(
                "[remote_fast_linker]\n"
                "enabled = true\n"
                'ci_env = "GITHUB_ACTIONS"\n'
                'linker_env = "BOLT_RUST_FAST_LINKER"\n'
                'programs = ["mold", "lld"]\n\n',
                "",
                1,
            ),
            "backtesting-vertical-slice rust policy must include [remote_fast_linker]",
        ),
        (
            "BVS policy must keep the CI fast-linker env",
            workflows,
            bvs_policy.replace('ci_env = "GITHUB_ACTIONS"\nlinker_env = "BOLT_RUST_FAST_LINKER"', 'ci_env = "NOT_CI"\nlinker_env = "BOLT_RUST_FAST_LINKER"', 1),
            "remote_fast_linker.ci_env='GITHUB_ACTIONS'",
        ),
    )
    for label, mutated_workflows, mutated_policy, expected in cases:
        errors = verifier.verify_debug_lane_compile_cache_parity(mutated_workflows, mutated_policy)
        if not any(expected in error for error in errors):
            raise AssertionError(f"{label}: expected {expected!r}, got: {errors}")

    action_text = repo_source_text(".github/actions/sccache-setup/action.yml")
    stats_action_text = repo_source_text(".github/actions/sccache-stats/action.yml")
    config_text = repo_source_text("ci/sccache-location.toml")
    action_cases = (
        (
            "shared action must keep eligibility setup fail-open",
            action_text.replace("      continue-on-error: true\n", "", 1),
            config_text,
            "Resolve sccache eligibility must be continue-on-error",
        ),
        (
            "shared action must use repo-pinned Python",
            action_text.replace("python3.12 scripts/sccache_eligibility.py", "python3 scripts/sccache_eligibility.py", 1),
            config_text,
            "must include 'python3.12 scripts/sccache_eligibility.py'",
        ),
        (
            "shared action must keep enablement fail-open",
            action_text.replace("      id: enable\n      continue-on-error: true\n", "      id: enable\n", 1),
            config_text,
            "Resolve sccache enablement must be continue-on-error",
        ),
        (
            "shared action must disable vendor stats annotations",
            action_text.replace('        disable_annotations: "true"\n', "", 1),
            config_text,
            "must disable vendor sccache stats annotations",
        ),
        (
            "shared action must summarize cache state",
            action_text.replace("        echo \"sccache cache:", "        echo \"cache:", 1),
            config_text,
            "must summarize sccache state under always()",
        ),
        (
            "shared action must own trusted write policy",
            action_text.replace(
                "python3.12 scripts/sccache_eligibility.py",
                "echo 'event_name == \"push\" and github_ref == \"refs/heads/main\"'\n        echo 'read_allowed = '",
                1,
            ),
            config_text,
            "must include 'python3.12 scripts/sccache_eligibility.py'",
        ),
        (
            "sccache location config owns the prefix",
            action_text,
            re.sub(r'key_prefix = "[^"]+"', 'key_prefix = ""', config_text, count=1),
            "location.key_prefix must be a non-empty string ending in '/'",
        ),
    )
    for label, mutated_action, mutated_config, expected in action_cases:
        errors = verifier.sccache_setup_action_contract_errors(mutated_action, mutated_config)
        if not any(expected in error for error in errors):
            raise AssertionError(f"{label}: expected {expected!r}, got: {errors}")
    eligibility_script_text = repo_source_text("scripts/sccache_eligibility.py")
    script_cases = (
        (
            "eligibility script must keep trusted write policy",
            eligibility_script_text.replace('event_name == "push" and github_ref == "refs/heads/main"', 'event_name == "push"', 1),
            'event_name == "push" and github_ref == "refs/heads/main"',
        ),
        (
            "eligibility script must not grant PR write access through dead-text spoofing",
            eligibility_script_text.replace(
                "trusted_write = write_requested and (",
                'trusted_write = write_requested and ((event_name == "pull_request") or ',
                1,
            ),
            "trusted_write expression must restrict write access",
        ),
        (
            "eligibility script must set sccache ignore I/O",
            eligibility_script_text.replace("SCCACHE_IGNORE_SERVER_IO_ERROR=1", "SCCACHE_IGNORE_SERVER_IO_ERROR=0", 1),
            "SCCACHE_IGNORE_SERVER_IO_ERROR=1",
        ),
    )
    for label, mutated_script, expected in script_cases:
        errors = verifier.sccache_eligibility_script_contract_errors(mutated_script)
        if not any(expected in error for error in errors):
            raise AssertionError(f"{label}: expected {expected!r}, got: {errors}")
    stats_cases = (
        (
            "stats action must show stats",
            stats_action_text.replace('"$SCCACHE_PATH" --show-stats || true', "true", 1),
            "must include '\"$SCCACHE_PATH\" --show-stats || true'",
        ),
        (
            "stats action must be gated by enabled input",
            stats_action_text.replace('SCCACHE_ENABLED: ${{ inputs.enabled }}', 'SCCACHE_ENABLED: "true"', 1),
            "must be gated by the caller's enabled input",
        ),
    )
    for label, mutated_action, expected in stats_cases:
        errors = verifier.sccache_stats_action_contract_errors(mutated_action)
        if not any(expected in error for error in errors):
            raise AssertionError(f"{label}: expected {expected!r}, got: {errors}")


def assert_cache_docs_cover_debug_schedule_consumers() -> None:
    text = repo_source_text("docs/ci/nextest-artifact-cache.md")
    sccache_config = REPO_ROOT / "ci" / "sccache-location.toml"
    if not sccache_config.exists():
        raise AssertionError("ci/sccache-location.toml must own the shared sccache location contract")
    required_fragments = (
        "ci/sccache-location.toml",
        "key_prefix",
        "CI test-archive",
        "workflow_dispatch",
        "nextest-archive restores",
        "sccache read-only access",
        "Rust Probe",
        "Debug Test",
        "scheduled and manually dispatched Flaky Test Smoke",
        "AWS_CI_CACHE_PR_READONLY_ROLE_ARN",
    )
    missing = [fragment for fragment in required_fragments if fragment not in text]
    if missing:
        raise AssertionError(f"cache docs must name debug read-only consumers, missing: {missing}")


def assert_bootstrap_uses_onepassword_key_generation() -> None:
    sync_script = load_sync_ci_debug_ssh_script()
    commands: list[tuple[list[str], str | None]] = []
    private_key = "-----BEGIN OPENSSH PRIVATE KEY-----\nprivate\n-----END OPENSSH PRIVATE KEY-----"

    def fake_run_checked(
        command: list[str], *, input_text: str | None = None
    ) -> subprocess.CompletedProcess[str]:
        commands.append((command, input_text))
        if command and command[0] == "ssh-keygen":
            key_path = pathlib.Path(command[command.index("-f") + 1])
            key_path.write_text(private_key, encoding="utf-8")
            key_path.with_suffix(".pub").write_text("ssh-ed25519 AAAATEST test\n", encoding="utf-8")
        return subprocess.CompletedProcess(command, 0, "", "")

    sync_script.run_checked = fake_run_checked
    sync_script.onepassword_item_exists = lambda config: False
    config = {
        "ssh_public_key_secret": "SSH_PUBLIC_KEY",
        "onepassword_vault": "Private",
        "onepassword_item_title": "bolt-v2 CI runner debug SSH",
        "onepassword_public_key_field": "public key",
        "onepassword_private_key_field": "private key",
    }
    with contextlib.redirect_stdout(io.StringIO()):
        sync_script.bootstrap_onepassword_item(config)
    if any(command and command[0] == "ssh-keygen" for command, _ in commands):
        raise AssertionError("bootstrap must let 1Password generate the SSH key, not local ssh-keygen")
    create_commands = [command for command, _ in commands if command[:3] == ["op", "item", "create"]]
    if not create_commands or not any(
        arg == "--ssh-generate-key" or arg.startswith("--ssh-generate-key=")
        for arg in create_commands[0]
    ):
        raise AssertionError(f"bootstrap must use op item create --ssh-generate-key, got: {commands}")
    if any(private_key in arg for command, _ in commands for arg in command):
        raise AssertionError(f"bootstrap must not pass private key material on argv, got: {commands}")


def assert_sync_errors_redact_command_arguments() -> None:
    sync_script = load_sync_ci_debug_ssh_script()
    secret_arg = "-----BEGIN OPENSSH PRIVATE KEY-----private-----END OPENSSH PRIVATE KEY-----"
    exc = subprocess.CalledProcessError(
        1,
        ["op", "item", "create", f"private key[password]={secret_arg}"],
        output="",
        stderr="",
    )
    message = sync_script.called_process_error_message(exc)
    if secret_arg in message or "private key[password]" in message or "op item create" in message:
        raise AssertionError(f"CalledProcessError message must redact command arguments, got: {message!r}")
    if "exit 1" not in message:
        raise AssertionError(f"CalledProcessError message must include exit status, got: {message!r}")


def assert_sync_public_key_uses_stdin() -> None:
    sync_script = load_sync_ci_debug_ssh_script()
    public_key = "ssh-ed25519 AAAATEST operator@example"
    commands: list[tuple[list[str], str | None]] = []

    def fake_run_checked(
        command: list[str], *, input_text: str | None = None
    ) -> subprocess.CompletedProcess[str]:
        commands.append((command, input_text))
        return subprocess.CompletedProcess(command, 0, "", "")

    sync_script.read_onepassword_field = lambda config, field: public_key
    sync_script.github_repository = lambda: "seungpyoson/bolt-v2"
    sync_script.run_checked = fake_run_checked
    config = {
        "ssh_public_key_secret": "SSH_PUBLIC_KEY",
        "onepassword_vault": "Private",
        "onepassword_item_title": "bolt-v2 CI runner debug SSH",
        "onepassword_public_key_field": "public key",
    }
    with contextlib.redirect_stdout(io.StringIO()):
        sync_script.sync_public_key_to_github(config)

    if len(commands) != 1:
        raise AssertionError(f"sync must run exactly one gh command, got: {commands}")
    command, input_text = commands[0]
    if "--body" in command or public_key in command:
        raise AssertionError(f"sync must not pass public key on argv, got: {command}")
    if input_text != public_key:
        raise AssertionError(f"sync must pass public key on stdin, got: {input_text!r}")


def assert_security_key_public_prefix_is_validated() -> None:
    sync_script = load_sync_ci_debug_ssh_script()
    sync_script.validate_public_key("sk-ssh-ed25519@openssh.com AAAATEST")
    try:
        sync_script.validate_public_key("ssh-ed25519-sk@openssh.com AAAATEST")
    except RuntimeError:
        return
    raise AssertionError("validate_public_key must reject the invalid ssh-ed25519-sk@ prefix")


def assert_backtester_detect_uses_ci_input_set() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")
    validate_required = "python3 scripts/ci_input_sets.py validate backtester_cache backtester_detect"
    missing_validate = replace_once(workflow, f"          {validate_required}\n", "")
    validate_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": missing_validate})
    if not any("backtester detect must validate CI input sets before skip decisions" in error for error in validate_errors):
        raise AssertionError(f"backtester detector must reject missing input-set validation, got: {validate_errors}")
    bootstrap_required = 'git diff --name-only "${base_sha}...HEAD" -- scripts/ci_input_sets.py ci/rust-ci-inputs.toml > "$bootstrap_changed_path"'
    missing_bootstrap = replace_once(workflow, bootstrap_required, 'true')
    bootstrap_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": missing_bootstrap})
    if not any("backtester detect must force-run on CI input-set bootstrap changes" in error for error in bootstrap_errors):
        raise AssertionError(f"backtester detector must reject missing bootstrap guard, got: {bootstrap_errors}")
    missing_bootstrap_marker = replace_once_after(
        workflow,
        '          if [[ -s "$bootstrap_changed_path" ]]; then\n',
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        "            exit 0\n",
        "            exit 0\n",
    )
    bootstrap_marker_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": missing_bootstrap_marker}
    )
    if not any("backtester detect must mark CI input-set bootstrap changes" in error for error in bootstrap_marker_errors):
        raise AssertionError(f"backtester detector must reject missing bootstrap marker, got: {bootstrap_marker_errors}")
    required = 'changed="$(python3 scripts/ci_input_sets.py changed backtester_detect --base "$base_sha" --head HEAD)"'
    bad = replace_once(
        workflow,
        required,
        'changed="$(git diff --name-only "${base_sha}...HEAD" -- ci/github-actions-runners.toml scripts/ci_provenance.py)"',
    )
    bad_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    if not any("backtester detect paths must come from ci_input_sets backtester_detect" in error for error in bad_errors):
        raise AssertionError(f"backtester detector must reject non-systematic path detection, got: {bad_errors}")
    if not any("backtester detect paths must not be duplicated inline" in error for error in bad_errors):
        raise AssertionError(f"backtester detector must reject inline duplicated path lists, got: {bad_errors}")
    good_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": workflow})
    if any("backtester detect" in error for error in good_errors):
        raise AssertionError(f"backtester detector path check must pass when present, got: {good_errors}")


def assert_backtester_ci_input_set_config_covers_systematic_inputs() -> None:
    verifier = load_verifier()
    config_path = "ci/rust-ci-inputs.toml"
    config = (REPO_ROOT / config_path).read_text(encoding="utf-8")
    missing_manifest = config.replace('  "gated_source_roots.manifest",\n', "")
    manifest_errors = verifier.verify_repo_automation_texts({config_path: missing_manifest})
    if not any("backtester_cache input set must include gated_source_roots.manifest" in error for error in manifest_errors):
        raise AssertionError(f"CI input set config must reject missing root build manifest, got: {manifest_errors}")
    missing_gitignore = config.replace('  ".gitignore",\n', "")
    gitignore_errors = verifier.verify_repo_automation_texts({config_path: missing_gitignore})
    if not any("backtester_cache input set must include .gitignore" in error for error in gitignore_errors):
        raise AssertionError(f"CI input set config must reject missing .gitignore input, got: {gitignore_errors}")
    missing_reference = config.replace('  "specs/023-nt-research-analytics-platform/reference/**",\n', "")
    reference_errors = verifier.verify_repo_automation_texts({config_path: missing_reference})
    if not any(
        "backtester_cache input set must include specs/023-nt-research-analytics-platform/reference/**" in error
        for error in reference_errors
    ):
        raise AssertionError(f"CI input set config must reject missing BVS reference fixture input, got: {reference_errors}")
    missing_helper = config.replace('  "scripts/rust_test_targets.py",\n', "")
    helper_errors = verifier.verify_repo_automation_texts({config_path: missing_helper})
    if not any("backtester_cache input set must include scripts/rust_test_targets.py" in error for error in helper_errors):
        raise AssertionError(f"CI input set config must reject missing target discovery helper cache input, got: {helper_errors}")
    if not any("backtester_detect input set must include scripts/rust_test_targets.py" in error for error in helper_errors):
        raise AssertionError(f"CI input set config must reject missing target discovery helper detect input, got: {helper_errors}")
    workflow_in_cache = config.replace(
        '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
        '  "scripts/rust_test_targets.py",\n  ".github/workflows/backtester-ci.yml",\n]\n\n[sets.backtester_detect]\n',
        1,
    )
    workflow_in_cache_errors = verifier.verify_repo_automation_texts({config_path: workflow_in_cache})
    if not any(
        "backtester_cache input set must not include CI governance input .github/workflows/backtester-ci.yml" in error
        for error in workflow_in_cache_errors
    ):
        raise AssertionError(f"CI input set config must reject workflow governance cache input, got: {workflow_in_cache_errors}")
    input_script_in_cache = config.replace(
        '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
        '  "scripts/rust_test_targets.py",\n  "scripts/ci_input_sets.py",\n]\n\n[sets.backtester_detect]\n',
        1,
    )
    input_script_in_cache_errors = verifier.verify_repo_automation_texts({config_path: input_script_in_cache})
    if not any(
        "backtester_cache input set must not include CI governance input scripts/ci_input_sets.py" in error
        for error in input_script_in_cache_errors
    ):
        raise AssertionError(
            f"CI input set config must reject input-set helper governance cache input, got: {input_script_in_cache_errors}"
        )
    input_config_in_cache = config.replace(
        '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
        '  "scripts/rust_test_targets.py",\n  "ci/rust-ci-inputs.toml",\n]\n\n[sets.backtester_detect]\n',
        1,
    )
    input_config_in_cache_errors = verifier.verify_repo_automation_texts({config_path: input_config_in_cache})
    if not any(
        "backtester_cache input set must not include CI governance input ci/rust-ci-inputs.toml" in error
        for error in input_config_in_cache_errors
    ):
        raise AssertionError(
            f"CI input set config must reject input-set config governance cache input, got: {input_config_in_cache_errors}"
        )
    setup_action_in_cache = config.replace(
        '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
        '  "scripts/rust_test_targets.py",\n  ".github/actions/setup-environment/**",\n]\n\n[sets.backtester_detect]\n',
        1,
    )
    setup_action_in_cache_errors = verifier.verify_repo_automation_texts({config_path: setup_action_in_cache})
    if not any(
        "backtester_cache input set must not include CI governance input .github/actions/setup-environment/**" in error
        for error in setup_action_in_cache_errors
    ):
        raise AssertionError(
            f"CI input set config must reject setup-environment cache input, got: {setup_action_in_cache_errors}"
        )
    for setup_pathspec in (
        ".github/actions/setup-environment/action.yml",
        ".github/actions/setup-environment",
        ".github/actions/**",
    ):
        overlapping_setup_action_in_cache = config.replace(
            '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
            f'  "scripts/rust_test_targets.py",\n  "{setup_pathspec}",\n]\n\n[sets.backtester_detect]\n',
            1,
        )
        overlapping_setup_action_errors = verifier.verify_repo_automation_texts(
            {config_path: overlapping_setup_action_in_cache}
        )
        if not any(
            f"backtester_cache input set must not include CI governance input {setup_pathspec}" in error
            for error in overlapping_setup_action_errors
        ):
            raise AssertionError(
                f"CI input set config must reject setup-environment-overlapping cache input "
                f"{setup_pathspec}, got: {overlapping_setup_action_errors}"
            )
    for pathspec in (
        "./scripts/ci_input_sets.py",
        "scripts/../scripts/ci_input_sets.py",
        ":(top)scripts/ci_input_sets.py",
        ":(top).github/actions/setup-environment/action.yml",
        ":/.github/actions/setup-environment/action.yml",
        "./.github/actions/**",
    ):
        normalized_governance_input_in_cache = config.replace(
            '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
            f'  "scripts/rust_test_targets.py",\n  "{pathspec}",\n]\n\n[sets.backtester_detect]\n',
            1,
        )
        normalized_governance_input_errors = verifier.verify_repo_automation_texts(
            {config_path: normalized_governance_input_in_cache}
        )
        if not any(
            f"backtester_cache input set must not include CI governance input {pathspec}" in error
            for error in normalized_governance_input_errors
        ):
            raise AssertionError(
                f"CI input set config must reject normalized governance cache input "
                f"{pathspec}, got: {normalized_governance_input_errors}"
            )
    unsupported_magic_in_cache = config.replace(
        '  "scripts/rust_test_targets.py",\n]\n\n[sets.backtester_detect]\n',
        '  "scripts/rust_test_targets.py",\n  ":(glob).github/actions/**",\n]\n\n[sets.backtester_detect]\n',
        1,
    )
    unsupported_magic_errors = verifier.verify_repo_automation_texts({config_path: unsupported_magic_in_cache})
    if not any(
        "backtester_cache input set must not use unsupported Git pathspec magic :(glob).github/actions/**" in error
        for error in unsupported_magic_errors
    ):
        raise AssertionError(f"CI input set config must reject unsupported pathspec magic, got: {unsupported_magic_errors}")


def assert_backtester_ci_requires_pr_event_types() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/backtester-ci.yml"
    workflow = repo_workflow_text(workflow_name)
    errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
    if any("pull_request types must include" in error for error in errors):
        raise AssertionError(f"backtester-ci workflow must satisfy PR type policy, got: {errors}")
    for missing_type, fragment in (
        ("ready_for_review", "types: [opened, synchronize, reopened, converted_to_draft, edited]"),
        ("edited", "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft]"),
        ("converted_to_draft", "types: [opened, synchronize, reopened, ready_for_review, edited]"),
    ):
        bad = replace_once(workflow, "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]", fragment)
        bad_errors = verifier.verify_repo_automation_texts({workflow_name: bad})
        if not any(f"pull_request types must include {missing_type}" in error for error in bad_errors):
            raise AssertionError(
                f"backtester-ci workflow must require {missing_type} in pull_request types, got: {bad_errors}"
            )


def assert_backtester_ci_uses_iteration_for_feedback_paths() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/backtester-ci.yml"
    workflow = repo_workflow_text(workflow_name)
    errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
    if any("backtester iteration policy" in error for error in errors):
        raise AssertionError(f"backtester-ci workflow must satisfy iteration policy, got: {errors}")

    missing_required_gate_note = replace_once(
        workflow,
        verifier.BACKTESTER_REQUIRED_GATE_COMMENT,
        "",
    )
    missing_required_gate_note_errors = verifier.verify_repo_automation_texts({workflow_name: missing_required_gate_note})
    if not any("backtester iteration policy must document that only backtester-gate should be required" in error for error in missing_required_gate_note_errors):
        raise AssertionError(
            f"backtester-ci workflow must document the required-capable gate context, got: {missing_required_gate_note_errors}"
        )

    required_gate_note_decoy = (
        missing_required_gate_note + "\n# " + verifier.BACKTESTER_REQUIRED_GATE_COMMENT + "\n"
    )
    required_gate_note_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: required_gate_note_decoy})
    if not any(
        "backtester iteration policy must document that only backtester-gate should be required" in error
        for error in required_gate_note_decoy_errors
    ):
        raise AssertionError(
            "backtester-ci workflow must reject required-gate documentation decoys outside the header, "
            f"got: {required_gate_note_decoy_errors}"
        )

    missing_policy_gate = replace_once(
        workflow,
        "if: ${{ needs.detect.outputs.bvs_changed == 'true' && needs.ci-policy.outputs.full_ci_required == 'true' }}",
        "if: ${{ needs.detect.outputs.bvs_changed == 'true' }}",
    )
    missing_policy_errors = verifier.verify_repo_automation_texts({workflow_name: missing_policy_gate})
    if not any("backtester iteration policy managed-heavy jobs must require full CI policy" in error for error in missing_policy_errors):
        raise AssertionError(f"backtester-ci workflow must reject unmanaged heavy policy gates, got: {missing_policy_errors}")

    static_gate_name = replace_once(
        workflow,
        "name: ${{ needs.ci-policy.outputs.backtester_gate_name }}",
        "name: backtester-gate",
    )
    static_gate_errors = verifier.verify_repo_automation_texts({workflow_name: static_gate_name})
    if not any("backtester iteration policy gate name must come from ci-policy backtester_gate_name output" in error for error in static_gate_errors):
        raise AssertionError(f"backtester-ci workflow must reject static gate names, got: {static_gate_errors}")

    missing_expected_event_class = replace_once(
        workflow,
        '--expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}"',
        '--expected-event-class "iteration"',
    )
    missing_expected_event_class_errors = verifier.verify_repo_automation_texts({workflow_name: missing_expected_event_class})
    if not any(
        "backtester iteration policy shared gate call must include --expected-event-class" in error
        for error in missing_expected_event_class_errors
    ):
        raise AssertionError(
            f"backtester-ci workflow must reject missing resolver event class, got: {missing_expected_event_class_errors}"
        )

    missing_shared_gate = replace_once(
        workflow,
        'python3 "$verdict_script" check-backtester-gate',
        'python3 "$verdict_script" check-not-backtester-gate',
    )
    missing_shared_gate_errors = verifier.verify_repo_automation_texts({workflow_name: missing_shared_gate})
    if not any("backtester iteration policy gate must use trusted base-tree check-backtester-gate verdict" in error for error in missing_shared_gate_errors):
        raise AssertionError(
            f"backtester-ci workflow must reject missing shared gate command, got: {missing_shared_gate_errors}"
        )

    carry_forward_reintroduced = replace_once(
        workflow,
        'python3 "$verdict_script" check-backtester-gate',
        'python3 "$verdict_script" resolve-gate-carry-forward\n          python3 "$verdict_script" check-backtester-gate',
    )
    missing_carry_forward_errors = verifier.verify_repo_automation_texts({workflow_name: carry_forward_reintroduced})
    if not any(
        "backtester iteration policy must not retain PR metadata carry-forward plumbing" in error
        for error in missing_carry_forward_errors
    ):
        raise AssertionError(
            f"backtester-ci workflow must reject carry-forward resolver, got: {missing_carry_forward_errors}"
        )

    issue_gate_workflow = workflow
    if "needs: [ci-policy, detect, fmt, clippy, test-archive, issue_789]" not in issue_gate_workflow:
        issue_gate_workflow = replace_once(
            issue_gate_workflow,
            "needs: [ci-policy, detect, fmt, clippy, test-archive]",
            "needs: [ci-policy, detect, fmt, clippy, test-archive, issue_789]",
        )
    issue_gate_workflow = replace_once(
        issue_gate_workflow,
        "--job test-archive=${{ needs.test-archive.result }}",
        "--job test-archive=${{ needs.test-archive.result }} \\\n            --job issue_789=${{ needs.issue_789.result }}",
    )
    issue_gate_errors = verifier.verify_repo_automation_texts({workflow_name: issue_gate_workflow})
    if not any("backtester diagnostic issue-789 lane must not gate merge proof" in error for error in issue_gate_errors):
        raise AssertionError(
            f"backtester-ci workflow must reject issue-789 as a merge-gating lane, got: {issue_gate_errors}"
        )

    broken_concurrency = replace_once(
        replace_once(
            workflow,
            "format('bvs-pr-{0}-iteration', github.event.number)",
            "format('bvs-pr-{0}', github.event.number)",
        ),
        "format('bvs-pr-{0}-full', github.event.number)",
        "format('bvs-pr-{0}', github.event.number)",
    )
    broken_concurrency += "\n# format('bvs-pr-{0}-iteration', github.event.number)\n# format('bvs-pr-{0}-full', github.event.number)\n"
    broken_concurrency_errors = verifier.verify_repo_automation_texts({workflow_name: broken_concurrency})
    if not any("backtester iteration policy concurrency must split iteration PR runs" in error for error in broken_concurrency_errors):
        raise AssertionError(f"backtester-ci workflow must reject broken concurrency even with comment decoys, got: {broken_concurrency_errors}")

    missing_concurrency_action_filter = replace_once(
        workflow,
        '             && contains(fromJSON(\'["opened","synchronize","reopened","converted_to_draft","edited"]\'), github.event.action))\n',
        "             && true)\n",
    )
    missing_concurrency_action_filter_errors = verifier.verify_repo_automation_texts({workflow_name: missing_concurrency_action_filter})
    if not any("backtester iteration policy concurrency must use the draft iteration action filter" in error for error in missing_concurrency_action_filter_errors):
        raise AssertionError(
            "backtester-ci workflow must reject concurrency groups that drift from the draft iteration action filter, got: "
            f"{missing_concurrency_action_filter_errors}"
        )

def assert_actionlint_rejects_stale_config_variables() -> None:
    verifier = load_verifier()
    actionlint = (REPO_ROOT / ".github" / "actionlint.yaml").read_text(encoding="utf-8")
    stale_actionlint = replace_once(
        actionlint,
        "\nconfig-secrets:\n",
        "\n  - CI_RUNNER_REMOVED\n\nconfig-secrets:\n",
    )
    with tempfile.TemporaryDirectory() as tmp:
        actionlint_path = pathlib.Path(tmp) / "actionlint.yaml"
        actionlint_path.write_text(stale_actionlint, encoding="utf-8")
        errors = verifier.verify_actionlint_runner_contract(
            verifier.repo_workflow_texts(),
            actionlint_path=actionlint_path,
        )
    if not any("stale config variable 'CI_RUNNER_REMOVED'" in error for error in errors):
        raise AssertionError(f"actionlint contract must reject stale config variables, got: {errors}")


def assert_actionlint_requires_pr_event_types() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/actionlint.yml"
    workflow = repo_workflow_text(workflow_name)
    errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
    if any("pull_request types must include" in error for error in errors):
        raise AssertionError(f"actionlint workflow must satisfy PR type policy, got: {errors}")
    for missing_type, fragment in (
        ("ready_for_review", "types: [opened, synchronize, reopened, edited]"),
        ("edited", "types: [opened, synchronize, reopened, ready_for_review]"),
    ):
        bad = replace_once(workflow, "types: [opened, synchronize, reopened, ready_for_review, edited]", fragment)
        bad_errors = verifier.verify_repo_automation_texts({workflow_name: bad})
        if not any(f"pull_request types must include {missing_type}" in error for error in bad_errors):
            raise AssertionError(
                f"actionlint workflow must require {missing_type} in pull_request types, got: {bad_errors}"
            )


def assert_actionlint_runs_ci_storage_audit_tests() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/actionlint.yml"
    workflow = repo_workflow_text(workflow_name)
    errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
    if any("must run python3 scripts/test_ci_storage_audit.py" in error for error in errors):
        raise AssertionError(f"actionlint workflow must run storage audit tests, got: {errors}")

    missing = replace_once(workflow, "          python3 scripts/test_ci_storage_audit.py\n", "")
    missing_errors = verifier.verify_repo_automation_texts({workflow_name: missing})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in missing_errors
    ):
        raise AssertionError(f"actionlint storage audit test wiring drift was silent, got: {missing_errors}")

    name_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "      - name: python3 scripts/test_ci_storage_audit.py\n",
    )
    name_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: name_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in name_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit step-name decoy was silent, got: {name_decoy_errors}")

    echo_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          echo 'python3 scripts/test_ci_storage_audit.py'\n",
    )
    echo_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: echo_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in echo_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit echo decoy was silent, got: {echo_decoy_errors}")

    comment_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          # python3 scripts/test_ci_storage_audit.py\n",
    )
    comment_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: comment_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in comment_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit comment decoy was silent, got: {comment_decoy_errors}")

    duplicated = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          python3 scripts/test_ci_storage_audit.py\n",
    )
    duplicated_errors = verifier.verify_repo_automation_texts({workflow_name: duplicated})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py exactly once" in error
        for error in duplicated_errors
    ):
        raise AssertionError(f"actionlint storage audit duplicate command was silent, got: {duplicated_errors}")

    skipped_job = replace_once(
        workflow,
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    steps:\n",
        "    if: false\n    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    steps:\n",
    )
    skipped_job_errors = verifier.verify_repo_automation_texts({workflow_name: skipped_job})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in skipped_job_errors
    ):
        raise AssertionError(f"actionlint storage audit skipped-job decoy was silent, got: {skipped_job_errors}")

    skipped_step = replace_once(
        workflow,
        "      - name: Verify AI review governance and storage audit safety tests\n        run: |\n",
        "      - name: Verify AI review governance and storage audit safety tests\n"
        "        if: false\n"
        "        run: |\n",
    )
    skipped_step_errors = verifier.verify_repo_automation_texts({workflow_name: skipped_step})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in skipped_step_errors
    ):
        raise AssertionError(f"actionlint storage audit skipped-step decoy was silent, got: {skipped_step_errors}")

    folded_scalar_decoy = replace_once(workflow, "        run: |\n", "        run: >\n")
    folded_scalar_errors = verifier.verify_repo_automation_texts({workflow_name: folded_scalar_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in folded_scalar_errors
    ):
        raise AssertionError(f"actionlint storage audit folded-scalar decoy was silent, got: {folded_scalar_errors}")

    heredoc_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          cat <<'EOF'\n"
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          EOF\n",
    )
    heredoc_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: heredoc_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in heredoc_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit heredoc decoy was silent, got: {heredoc_decoy_errors}")

    false_branch = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          if false; then\n"
        "            python3 scripts/test_ci_storage_audit.py\n"
        "          fi\n",
    )
    false_branch_errors = verifier.verify_repo_automation_texts({workflow_name: false_branch})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in false_branch_errors
    ):
        raise AssertionError(f"actionlint storage audit false-branch decoy was silent, got: {false_branch_errors}")

    function_body_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          run_storage_audit() {\n"
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          }\n",
    )
    function_body_errors = verifier.verify_repo_automation_texts({workflow_name: function_body_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in function_body_errors
    ):
        raise AssertionError(f"actionlint storage audit function-body decoy was silent, got: {function_body_errors}")

    split_brace_function_body_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          run_storage_audit()\n"
        "          {\n"
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          }\n",
    )
    split_brace_function_body_errors = verifier.verify_repo_automation_texts(
        {workflow_name: split_brace_function_body_decoy}
    )
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in split_brace_function_body_errors
    ):
        raise AssertionError(
            "actionlint storage audit split-brace function-body decoy was silent, "
            f"got: {split_brace_function_body_errors}"
        )

    subshell_function_body_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          run_storage_audit() (\n"
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          )\n",
    )
    subshell_function_body_errors = verifier.verify_repo_automation_texts(
        {workflow_name: subshell_function_body_decoy}
    )
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in subshell_function_body_errors
    ):
        raise AssertionError(
            "actionlint storage audit subshell function-body decoy was silent, "
            f"got: {subshell_function_body_errors}"
        )

    multiline_quote_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          echo \"\n"
        "          python3 scripts/test_ci_storage_audit.py\n"
        "          \"\n",
    )
    multiline_quote_errors = verifier.verify_repo_automation_texts({workflow_name: multiline_quote_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in multiline_quote_errors
    ):
        raise AssertionError(f"actionlint storage audit multiline-quote decoy was silent, got: {multiline_quote_errors}")

    env_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          echo \"not storage audit\"\n"
        "        env:\n"
        "          STORAGE_AUDIT_TEST: |\n"
        "            python3 scripts/test_ci_storage_audit.py\n",
    )
    env_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: env_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in env_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit env decoy was silent, got: {env_decoy_errors}")

    with_decoy = replace_once(
        workflow,
        "          python3 scripts/test_ci_storage_audit.py\n",
        "          echo \"not storage audit\"\n"
        "        with:\n"
        "          storage_audit_command: |\n"
        "            python3 scripts/test_ci_storage_audit.py\n",
    )
    with_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: with_decoy})
    if not any(
        "actionlint workflow must run python3 scripts/test_ci_storage_audit.py" in error
        for error in with_decoy_errors
    ):
        raise AssertionError(f"actionlint storage audit with decoy was silent, got: {with_decoy_errors}")


def assert_ci_docs_pass_stub_is_absent() -> None:
    workflow_path = REPO_ROOT / ".github/workflows/ci-docs-pass-stub.yml"
    if workflow_path.exists():
        raise AssertionError("ci-docs-pass-stub workflow must stay deleted")


def assert_source_fence_static_ignores_comments() -> None:
    verifier = load_verifier()
    justfile_text = """
source-fence-static:
    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner

source-fence-static-inner: require-local-verification-gate
    # cargo fetch and scripts/verify_runtime_capture_yaml.py stay in source-fence
    # python3 scripts/rust_verification.py cargo --repo . -- test stays remote-only
    python3 scripts/run_fences.py

source-fence: source-fence-static
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}" -- fetch --locked
    python3 scripts/verify_runtime_capture_yaml.py
"""
    errors = verifier.verify_source_fence_static_recipe(justfile_text)
    if errors:
        raise AssertionError(f"source-fence-static comments must not trigger compile-heavy errors, got: {errors}")

    active_bad = justfile_text.replace(
        "    # python3 scripts/rust_verification.py cargo --repo . -- test stays remote-only",
        "    python3 scripts/rust_verification.py cargo --repo . -- test",
    )
    bad_errors = verifier.verify_source_fence_static_recipe(active_bad)
    if not any("must not invoke wrapper-routed Cargo" in error for error in bad_errors):
        raise AssertionError(f"source-fence-static active wrapper cargo must still fail, got: {bad_errors}")

    ungated = justfile_text.replace(
        "    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner\n\n",
        "",
    )
    ungated_errors = verifier.verify_source_fence_static_recipe(ungated)
    if not any("must run through scripts/local_verification_gate.py" in error for error in ungated_errors):
        raise AssertionError(f"source-fence-static must require the local gate, got: {ungated_errors}")

    extra_public_work = justfile_text.replace(
        "    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner",
        "    python3 scripts/local_verification_gate.py source-fence-static -- just source-fence-static-inner\n"
        "    python3 scripts/verify_lane_governance.py",
    )
    extra_public_errors = verifier.verify_source_fence_static_recipe(extra_public_work)
    if not any("source-fence-static must contain only the local verification gate command" in error for error in extra_public_errors):
        raise AssertionError(f"source-fence-static public recipe extra work was silent, got: {extra_public_errors}")

    nested_public_gate = justfile_text.replace(
        "    python3 scripts/run_fences.py",
        "    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner",
    )
    nested_public_errors = verifier.verify_source_fence_static_recipe(nested_public_gate)
    if not any("source-fence-static-inner must not invoke local verification gate recipes" in error for error in nested_public_errors):
        raise AssertionError(f"source-fence-static nested gate call was silent, got: {nested_public_errors}")

    nested_public_dependency = justfile_text.replace(
        "source-fence-static-inner: require-local-verification-gate",
        "source-fence-static-inner: require-local-verification-gate ci-lint-workflow",
    )
    nested_dependency_errors = verifier.verify_source_fence_static_recipe(nested_public_dependency)
    if not any("source-fence-static-inner must not depend on local verification gate recipes" in error for error in nested_dependency_errors):
        raise AssertionError(f"source-fence-static nested gate dependency was silent, got: {nested_dependency_errors}")

    missing_runner = justfile_text.replace("    python3 scripts/run_fences.py\n", "")
    missing_errors = verifier.verify_source_fence_static_recipe(missing_runner)
    if not any("must run python3 scripts/run_fences.py" in error for error in missing_errors):
        raise AssertionError(f"source-fence-static must require run_fences harness, got: {missing_errors}")

    extra_inner_work = justfile_text.replace(
        "    python3 scripts/run_fences.py",
        "    python3 scripts/run_fences.py\n"
        "    python3 scripts/verify_lane_governance.py",
    )
    extra_inner_errors = verifier.verify_source_fence_static_recipe(extra_inner_work)
    if not any("source-fence-static-inner must contain only python3 scripts/run_fences.py" in error for error in extra_inner_errors):
        raise AssertionError(f"source-fence-static extra inner work was silent, got: {extra_inner_errors}")

    commented_lane_test = justfile_text.replace(
        "    python3 scripts/run_fences.py",
        "    # python3 scripts/run_fences.py",
    )
    commented_errors = verifier.verify_source_fence_static_recipe(commented_lane_test)
    if not any("must run python3 scripts/run_fences.py" in error for error in commented_errors):
        raise AssertionError(f"source-fence-static comments must not satisfy runner wiring, got: {commented_errors}")


def assert_local_verification_gate_recipes_are_enforced() -> None:
    verifier = load_verifier()
    justfile_text = """
fmt-check:
    python3 scripts/local_verification_gate.py fmt-check -- just fmt-check-inner

fmt-check-inner: require-local-verification-gate
    python3 scripts/test_verify_ci_workflow_hygiene.py

ci-lint-workflow:
    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner

ci-lint-workflow-inner: require-local-verification-gate
    if [ -n "${BOLT_CI_LINT_WORKFLOW_WORKERS:-}" ]; then
        set -- --workers "$BOLT_CI_LINT_WORKFLOW_WORKERS"
    else
        set --
    fi
    if ! python3 scripts/run_ci_lint_suites.py "$@"; then
        failed=1
    fi
"""
    errors = verifier.verify_local_verification_gate_recipes(justfile_text)
    if errors:
        raise AssertionError(f"local gate recipe wiring should pass, got: {errors}")

    runner_line = '    if ! python3 scripts/run_ci_lint_suites.py "$@"; then'

    missing_runner = justfile_text.replace(f"{runner_line}\n", "")
    missing_runner_errors = verifier.verify_local_verification_gate_recipes(missing_runner)
    if not any("justfile ci-lint-workflow-inner must run python3 scripts/run_ci_lint_suites.py" in error for error in missing_runner_errors):
        raise AssertionError(f"ci-lint runner wiring drift was silent, got: {missing_runner_errors}")

    duplicate_runner = justfile_text.replace(
        f"{runner_line}\n",
        f"{runner_line}\n"
        f"{runner_line}\n",
    )
    duplicate_runner_errors = verifier.verify_local_verification_gate_recipes(duplicate_runner)
    if not any("justfile ci-lint-workflow-inner must run python3 scripts/run_ci_lint_suites.py exactly once" in error for error in duplicate_runner_errors):
        raise AssertionError(f"duplicate ci-lint runner wiring was silent, got: {duplicate_runner_errors}")

    extra_inner_work = justfile_text.replace(
        runner_line,
        f"{runner_line}\n"
        "    python3 scripts/test_ci_storage_audit.py",
    )
    extra_inner_errors = verifier.verify_local_verification_gate_recipes(extra_inner_work)
    if not any("justfile ci-lint-workflow-inner must not run CI lint suite commands outside scripts/run_ci_lint_suites.py" in error for error in extra_inner_errors):
        raise AssertionError(f"ci-lint extra inner work was silent, got: {extra_inner_errors}")

    echo_decoy = justfile_text.replace(
        runner_line,
        '    echo python3 scripts/run_ci_lint_suites.py "$@"',
    )
    echo_decoy_errors = verifier.verify_local_verification_gate_recipes(echo_decoy)
    if not any("justfile ci-lint-workflow-inner must run python3 scripts/run_ci_lint_suites.py" in error for error in echo_decoy_errors):
        raise AssertionError(f"ci-lint runner echo decoy satisfied wiring, got: {echo_decoy_errors}")

    noncanonical_runner = justfile_text.replace(
        runner_line,
        f"{runner_line}\n"
        "    python3 scripts/run_ci_lint_suites.py --workers 2",
    )
    noncanonical_runner_errors = verifier.verify_local_verification_gate_recipes(noncanonical_runner)
    if not any("justfile ci-lint-workflow-inner must not invoke the runner outside the pinned line" in error for error in noncanonical_runner_errors):
        raise AssertionError(f"ci-lint noncanonical runner line was silent, got: {noncanonical_runner_errors}")

    commented_runner = justfile_text.replace(
        runner_line,
        f"    # {runner_line.strip()}",
    )
    commented_runner_errors = verifier.verify_local_verification_gate_recipes(commented_runner)
    if not any("justfile ci-lint-workflow-inner must run python3 scripts/run_ci_lint_suites.py" in error for error in commented_runner_errors):
        raise AssertionError(f"ci-lint runner comments must not satisfy wiring, got: {commented_runner_errors}")

    ungated = justfile_text.replace(
        "    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner",
        "    python3 scripts/test_verify_ci_workflow_hygiene.py",
    )
    ungated_errors = verifier.verify_local_verification_gate_recipes(ungated)
    if not any("justfile ci-lint-workflow must run through scripts/local_verification_gate.py" in error for error in ungated_errors):
        raise AssertionError(f"ci-lint-workflow gate drift was silent, got: {ungated_errors}")

    fmt_ungated = justfile_text.replace(
        "    python3 scripts/local_verification_gate.py fmt-check -- just fmt-check-inner",
        "    python3 scripts/test_verify_ci_workflow_hygiene.py",
    )
    fmt_ungated_errors = verifier.verify_local_verification_gate_recipes(fmt_ungated)
    if not any("justfile fmt-check must run through scripts/local_verification_gate.py" in error for error in fmt_ungated_errors):
        raise AssertionError(f"fmt-check gate drift was silent, got: {fmt_ungated_errors}")

    extra_public_work = justfile_text.replace(
        "    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner",
        "    python3 scripts/local_verification_gate.py ci-lint-workflow -- just ci-lint-workflow-inner\n"
        "    python3 scripts/test_verify_ci_workflow_hygiene.py",
    )
    extra_public_errors = verifier.verify_local_verification_gate_recipes(extra_public_work)
    if not any("justfile ci-lint-workflow must contain only the local verification gate command" in error for error in extra_public_errors):
        raise AssertionError(f"ci-lint-workflow public recipe extra work was silent, got: {extra_public_errors}")

    broken_runner = object()
    original_runner_module = sys.modules.get("run_ci_lint_suites")
    sys.modules["run_ci_lint_suites"] = broken_runner
    try:
        broken_runner_errors = verifier.verify_local_verification_gate_recipes(justfile_text)
    finally:
        if original_runner_module is None:
            sys.modules.pop("run_ci_lint_suites", None)
        else:
            sys.modules["run_ci_lint_suites"] = original_runner_module
    if not any("ci-lint workflow runner suite table must be importable" in error for error in broken_runner_errors):
        raise AssertionError(f"broken ci-lint runner import was not governed, got: {broken_runner_errors}")

    nested_public_gate = justfile_text.replace(runner_line, "    just ci-lint-workflow")
    nested_public_errors = verifier.verify_local_verification_gate_recipes(nested_public_gate)
    if not any("justfile ci-lint-workflow-inner must not invoke local verification gate recipes" in error for error in nested_public_errors):
        raise AssertionError(f"ci-lint-workflow nested public gate call was silent, got: {nested_public_errors}")

    nested_public_dependency = justfile_text.replace(
        "ci-lint-workflow-inner: require-local-verification-gate",
        "ci-lint-workflow-inner: require-local-verification-gate ci-lint-workflow",
    )
    nested_dependency_errors = verifier.verify_local_verification_gate_recipes(nested_public_dependency)
    if not any("justfile ci-lint-workflow-inner must not depend on local verification gate recipes" in error for error in nested_dependency_errors):
        raise AssertionError(f"ci-lint-workflow nested public gate dependency was silent, got: {nested_dependency_errors}")

    missing_guard = justfile_text.replace("ci-lint-workflow-inner: require-local-verification-gate", "ci-lint-workflow-inner:")
    missing_guard_errors = verifier.verify_local_verification_gate_recipes(missing_guard)
    if not any("justfile ci-lint-workflow-inner must require the local verification gate" in error for error in missing_guard_errors):
        raise AssertionError(f"ci-lint-workflow inner guard drift was silent, got: {missing_guard_errors}")


def assert_nextest_fingerprint_reuse_governance_covers_sidecar_helper() -> None:
    verifier = load_verifier()
    required_paths = (
        "scripts/config_validators.py",
        "scripts/root_bin_sidecars.py",
        "scripts/test_root_bin_sidecars.py",
    )
    missing = [path for path in required_paths if path not in verifier.FINGERPRINT_REUSE_GOVERNANCE_PATHS]
    if missing:
        raise AssertionError(f"fingerprint-reuse governance pathspec must include root sidecar helper files: {missing}")


def assert_nextest_fingerprint_reuse_governance_pathspec_order_is_pinned() -> None:
    verifier = load_verifier()
    expected_paths = (
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
    if verifier.FINGERPRINT_REUSE_GOVERNANCE_PATHS != expected_paths:
        raise AssertionError(
            "fingerprint-reuse governance pathspec order drifted: "
            f"{verifier.FINGERPRINT_REUSE_GOVERNANCE_PATHS!r}"
        )


def assert_nextest_fingerprint_reuse_governance_excludes_whole_ci_workflow() -> None:
    verifier = load_verifier()
    if ".github/workflows/ci.yml" in verifier.FINGERPRINT_REUSE_GOVERNANCE_PATHS:
        raise AssertionError("fingerprint-reuse governance pathspec must not include whole ci.yml")

    whole_workflow_detector = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        """          changed="$(git diff --name-only "$diff_range" --             .github/actions/setup-environment/action.yml""",
        """          changed="$(git diff --name-only "$diff_range" --             .github/workflows/ci.yml             .github/actions/setup-environment/action.yml""",
    )
    assert_error("detector must detect fingerprint-reuse governance changes", whole_workflow_detector)


def assert_reuse_neutral_env_comment_states_fail_closed_classification_rule() -> None:
    verifier_text = repo_source_text(VERIFIER_PATH)
    comment = (
        "# A key may be listed here only if it must not influence compiler/test-runner\n"
        "# behavior or archive content; when in doubt classify reuse-relevant\n"
        "# (fails toward rebuild). Build-affecting keys must instead go into\n"
        "# ci_provenance.REUSE_RELEVANT_WORKFLOW_ENV_KEYS so they invalidate reuse."
    )
    if comment not in verifier_text:
        raise AssertionError("reuse-neutral env comment must state the fail-closed classification rule")


def assert_rust_verification_policy_parse_errors_are_domain_specific() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        policy_path = pathlib.Path(tmp) / "rust-verification.toml"
        policy_path.write_text("schema_version = [\n", encoding="utf-8")
        try:
            verifier.load_rust_verification_policy_toml(policy_path, "ci/rust-verification.toml")
        except verifier.PolicyError as exc:
            if "ci/rust-verification.toml is invalid TOML" not in str(exc):
                raise AssertionError(str(exc)) from exc
            return
    raise AssertionError("invalid rust-verification TOML must raise PolicyError")


def without_pr_concurrency(workflow: str) -> str:
    return replace_once(
        workflow,
        """concurrency:
  group: >-
    ${{ github.event_name == 'pull_request'
        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !((github.event.action == 'edited'
              && !(github.event.changes.base.ref.from && true || false))
             || (github.event.pull_request.draft == true
                 && github.event.action == 'converted_to_draft'))
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)
        || github.event_name == 'pull_request'
        && ((github.event.pull_request.draft == true
             && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action))
            || (github.event.pull_request.draft == false
                && github.event.action == 'edited'
                && !(github.event.changes.base.ref.from && true || false)))
        && format('pr-{0}-iteration', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'workflow_dispatch'
        && format('{0}-dispatch-iteration', github.ref_name)
        || github.event_name == 'merge_group'
        && format('mq-{0}', github.ref)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        || github.event_name == 'workflow_dispatch' }}

""",
        "",
    )




def without_job_if(workflow: str, job: str) -> str:
    lines = workflow.splitlines()
    start = next(i for i, line in enumerate(lines) if line == f"  {job}:")
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("  ") and not lines[i].startswith("    ") and lines[i].strip().endswith(":"):
            end = i
            break
    filtered = [line for i, line in enumerate(lines) if not (start < i < end and line.startswith("    if: "))]
    return "\n".join(filtered) + "\n"


def assert_parse_jobs_strips_comments() -> None:
    verifier = load_verifier()
    jobs = verifier.parse_jobs(
        """
name: CI
jobs:
  clippy:
    name: clippy
    steps:
      # include-managed-target-dir: "true"
      - run: echo "${{ steps.setup.outputs.managed_target_dir }}"
""",
    )
    clippy = jobs["clippy"]
    if any("#" in line or "include-managed-target-dir" in line for line in clippy):
        raise AssertionError(f"parse_jobs must store stripped job lines, got: {clippy!r}")


def assert_strip_comment_handles_single_quoted_backslash() -> None:
    verifier = load_verifier()
    line = r"pattern: 'path\' # trailing comment"
    expected = r"pattern: 'path\'"
    actual = verifier.strip_comment(line)
    if actual != expected:
        raise AssertionError(f"single-quoted backslash comment stripping failed: {actual!r}")


def assert_command_parse_cache_is_transparent() -> None:
    """Differential proof that the @functools.cache on strip_comment and
    _command_tokens_cached is behavior-transparent. A memoization bug that
    returned a stale or wrong value would make this verifier silently miss a
    violation (fail open), so the equivalence is fenced by a test rather than
    argued by inspection.

    Three layers: (1) cached output is byte-identical to the un-memoized
    function (__wrapped__) over an adversarial corpus; (2) the public wrapper
    copies on return so a caller mutating the result cannot corrupt the cache;
    (3) verify_text produces identical findings with the cache live vs bypassed.
    """
    import cargo_command_analysis
    import shell_dataflow_analysis
    import workflow_expression_analysis

    verifier = load_verifier()

    # functools.cache is exactly lru_cache(maxsize=None): unbounded, no eviction.
    if verifier.strip_comment.cache_info().maxsize is not None:
        raise AssertionError("strip_comment cache must be unbounded (maxsize=None)")
    if verifier._command_tokens_cached.cache_info().maxsize is not None:
        raise AssertionError("_command_tokens_cached cache must be unbounded (maxsize=None)")

    # Quoting, escapes, embedded '#', shell punctuation, trailing comments, and
    # the high-frequency repeats that motivate the cache. __wrapped__ is the
    # undecorated, cache-free function.
    samples = [
        "",
        "fi",
        "exit 1",
        "echo ok # trailing comment",
        "URL=https://example.com/api#fragment ; cargo build --target-dir /tmp/raw",
        r"pattern: 'path\' # trailing comment",
        'echo "a # b" && cargo test',
        "cargo build && cargo test || echo fail",
        "a|b|c",
        "(cd x && cargo build); echo done",
    ]
    # Each sample twice so cache hits actually occur, keeping the differential
    # non-vacuous: a never-hit cache cannot diverge.
    for sample in samples + samples:
        if verifier.strip_comment(sample) != verifier.strip_comment.__wrapped__(sample):
            raise AssertionError(
                f"strip_comment cache diverged from the un-memoized result for {sample!r}"
            )
        if verifier.command_tokens(sample) != list(
            verifier._command_tokens_cached.__wrapped__(sample)
        ):
            raise AssertionError(
                f"command_tokens cache diverged from the un-memoized result for {sample!r}"
            )
    if verifier.strip_comment.cache_info().hits == 0:
        raise AssertionError("strip_comment cache was never hit; differential is vacuous")
    if verifier._command_tokens_cached.cache_info().hits == 0:
        raise AssertionError("_command_tokens_cached cache was never hit; differential is vacuous")

    # Copy-on-return: mutating a returned token list must not corrupt the cache.
    repeated = "cargo build && cargo test"
    first = verifier.command_tokens(repeated)
    first.append("__POISON__")
    if "__POISON__" in verifier.command_tokens(repeated):
        raise AssertionError("command_tokens must copy on return; caller mutation corrupted the cache")

    # Verifier-level: a command whose classification flows through the memoized
    # tokenizer. Findings must be identical -- and non-empty, so the comparison
    # is meaningful -- with the cache live vs bypassed via __wrapped__.
    probe = replace_once(
        BASE_WORKFLOW,
        "      - run: just fmt-check",
        "      - run: cargo build --target-dir /tmp/raw # inline comment\n      - run: just fmt-check",
    )
    cached_strip_fn = verifier.strip_comment
    cached_token_fn = verifier._command_tokens_cached
    cached_workflow_strip_fn = workflow_expression_analysis.strip_comment
    cached_cargo_strip_fn = cargo_command_analysis.strip_comment
    cached_shell_strip_fn = shell_dataflow_analysis.strip_comment
    cached_cargo_token_fn = cargo_command_analysis._command_tokens_cached
    findings_live = verifier.verify_text(probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
    if not findings_live:
        raise AssertionError("verifier-level differential probe must produce a finding to be meaningful")
    hits_before = cached_strip_fn.cache_info().hits + cached_token_fn.cache_info().hits
    try:
        verifier.strip_comment = cached_strip_fn.__wrapped__
        verifier._command_tokens_cached = cached_token_fn.__wrapped__
        workflow_expression_analysis.strip_comment = cached_workflow_strip_fn.__wrapped__
        cargo_command_analysis.strip_comment = cached_cargo_strip_fn.__wrapped__
        shell_dataflow_analysis.strip_comment = cached_shell_strip_fn.__wrapped__
        cargo_command_analysis._command_tokens_cached = cached_cargo_token_fn.__wrapped__
        findings_bypassed = verifier.verify_text(probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
        hits_after = cached_strip_fn.cache_info().hits + cached_token_fn.cache_info().hits
    finally:
        verifier.strip_comment = cached_strip_fn
        verifier._command_tokens_cached = cached_token_fn
        workflow_expression_analysis.strip_comment = cached_workflow_strip_fn
        cargo_command_analysis.strip_comment = cached_cargo_strip_fn
        shell_dataflow_analysis.strip_comment = cached_shell_strip_fn
        cargo_command_analysis._command_tokens_cached = cached_cargo_token_fn
    if findings_live != findings_bypassed:
        raise AssertionError(
            f"verify_text findings changed when the parse cache was bypassed: "
            f"{findings_live!r} != {findings_bypassed!r}"
        )
    if hits_after != hits_before:
        raise AssertionError(
            "parse-cache bypass did not take effect; the verifier-level differential is vacuous"
        )


def decorators_separated_from_targets(path: pathlib.Path, source: str) -> list[str]:
    tree = ast.parse(source, filename=str(path))
    lines = source.splitlines()
    offenders: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        if not node.decorator_list:
            continue
        last_decorator_line = max(
            getattr(decorator, "end_lineno", decorator.lineno) or decorator.lineno
            for decorator in node.decorator_list
        )
        if any(
            not lines[line_number - 1].strip()
            for line_number in range(last_decorator_line + 1, node.lineno)
        ):
            offenders.append(f"{path.relative_to(REPO_ROOT)}:{node.lineno}:{node.name}")
    return offenders


def assert_decorator_spacing_guard_covers_reviewed_evasions() -> None:
    alias_source = "from functools import cache\n@cache\n\ndef cached_alias():\n    return 1\n"
    sibling_source = "import functools\n@functools.cache\n\ndef sibling_cache():\n    return 1\n"
    clean_source = "import functools\n@functools.cache\ndef clean_cache():\n    return 1\n"
    if not decorators_separated_from_targets(REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py", alias_source):
        raise AssertionError("decorator spacing guard must catch aliased cache decorators")
    if not decorators_separated_from_targets(REPO_ROOT / "scripts" / "cargo_command_analysis.py", sibling_source):
        raise AssertionError("decorator spacing guard must catch sibling module decorators")
    if decorators_separated_from_targets(REPO_ROOT / "scripts" / "workflow_expression_analysis.py", clean_source):
        raise AssertionError("decorator spacing guard must accept adjacent decorators")


def assert_no_decorators_separated_from_targets() -> None:
    offenders: list[str] = []
    for path in sorted((REPO_ROOT / "scripts").glob("*.py")):
        offenders.extend(decorators_separated_from_targets(path, path.read_text(encoding="utf-8")))
    if offenders:
        raise AssertionError(f"decorators must stay adjacent to their targets: {offenders}")


def assert_relocated_symbols_keep_legacy_exports() -> None:
    verifier = load_verifier()
    moved_modules = (
        REPO_ROOT / "scripts" / "cargo_command_analysis.py",
        REPO_ROOT / "scripts" / "shell_dataflow_analysis.py",
        REPO_ROOT / "scripts" / "governance_diff_analysis.py",
        REPO_ROOT / "scripts" / "workflow_expression_analysis.py",
    )
    missing: list[str] = []
    for path in moved_modules:
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in tree.body:
            names: list[str] = []
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                names.append(node.name)
            elif isinstance(node, ast.Assign):
                names.extend(target.id for target in node.targets if isinstance(target, ast.Name))
            elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
                names.append(node.target.id)
            for name in names:
                if not name.startswith("__") and not hasattr(verifier, name):
                    missing.append(f"{path.name}:{name}")
    import merge_queue_preflight as preflight
    for name in (
        "MERGIFY_REQUIRED_MERGE_CONDITIONS",
        "MERGIFY_REQUIRED_QUEUE_RULES",
        "MERGIFY_REQUIRED_PRIORITY_RULES",
        "MERGIFY_TOP_LEVEL_KEYS",
        "MERGIFY_FORBIDDEN_TOP_LEVEL_KEYS",
        "MERGIFY_MERGE_QUEUE_KEYS",
        "MERGIFY_QUEUE_RULE_KEYS",
        "MERGIFY_DYNAMIC_BATCH_KEYS",
        "MERGIFY_PRIORITY_RULE_KEYS",
        "MERGIFY_YAML_PARSER_RUBY",
        "parse_mergify_yaml",
        "scalar_equals",
        "yaml_display",
        "unsupported_mapping_keys",
        "mergify_mapping",
        "mergify_list",
        "required_mergify_mapping",
        "required_mergify_list",
        "named_mergify_rules",
        "mergify_condition_list",
        "mergify_required_conditions",
        "expect_scalar",
        "verify_mergify_config",
    ):
        if not hasattr(preflight, name):
            missing.append(f"merge_queue_preflight.py:{name}")
        if not hasattr(verifier, name):
            missing.append(f"merge_queue_preflight.py:{name}")
    if missing:
        raise AssertionError(f"legacy verifier re-exports missing moved symbols: {missing}")


def imports_giant_twin(source: str) -> bool:
    tree = ast.parse(source)
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            if any(alias.name.split(".")[-1] == "test_verify_ci_workflow_hygiene" for alias in node.names):
                return True
        elif isinstance(node, ast.ImportFrom):
            if node.module and node.module.split(".")[-1] == "test_verify_ci_workflow_hygiene":
                return True
            if any(alias.name == "test_verify_ci_workflow_hygiene" for alias in node.names):
                return True
    return False


def assert_giant_twin_import_guard_ignores_fixture_strings() -> None:
    fixture_only = 'COMMAND = "python3 scripts/test_verify_ci_workflow_hygiene.py"\n'
    if imports_giant_twin(fixture_only):
        raise AssertionError("giant twin import guard must ignore fixture strings")
    if not imports_giant_twin("import test_verify_ci_workflow_hygiene\n"):
        raise AssertionError("giant twin import guard must catch direct imports")
    if not imports_giant_twin("from scripts import test_verify_ci_workflow_hygiene\n"):
        raise AssertionError("giant twin import guard must catch package imports")


def assert_relocated_tests_do_not_import_giant_twin() -> None:
    offenders = []
    for path in (
        REPO_ROOT / "scripts" / "test_cargo_command_analysis.py",
        REPO_ROOT / "scripts" / "test_shell_dataflow_analysis.py",
        REPO_ROOT / "scripts" / "test_governance_diff_analysis.py",
        REPO_ROOT / "scripts" / "test_workflow_expression_analysis.py",
        REPO_ROOT / "scripts" / "test_merge_queue_preflight.py",
        REPO_ROOT / "scripts" / "ci_workflow_hygiene_test_helpers.py",
    ):
        if imports_giant_twin(path.read_text(encoding="utf-8")):
            offenders.append(str(path.relative_to(REPO_ROOT)))
    if offenders:
        raise AssertionError(f"relocated tests must import shared helpers, not the giant twin: {offenders}")


def assert_workflow_hygiene_reviewer_regressions() -> None:
    verifier = load_verifier()

    url_fragment_command = "URL=https://example.com/api#fragment ; cargo build --target-dir /tmp/raw"
    if verifier.strip_comment(url_fragment_command) != url_fragment_command:
        raise AssertionError("unquoted # inside a shell word must not hide the rest of the command")

    trailing_comment = "run: echo ok # trailing comment"
    if verifier.strip_comment(trailing_comment) != "run: echo ok":
        raise AssertionError("whitespace-prefixed trailing comments must still be stripped")

    upload_probe = replace_once(
        BASE_WORKFLOW,
        "      - run: just fmt-check",
        '      - run: echo "actions/upload-artifact@"\n      - run: just fmt-check',
    )
    upload_errors = verifier.verify_text(upload_probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
    if any("actions/upload-artifact must be pinned" in error for error in upload_errors):
        raise AssertionError(f"action prose must not be treated as an upload-artifact action: {upload_errors!r}")

    rust_cache_probe = replace_once(
        BASE_WORKFLOW,
        "      - run: just deny",
        '      - run: echo "Swatinem/rust-cache@"\n      - run: just deny',
    )
    rust_cache_errors = verifier.verify_text(rust_cache_probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
    if any("shared Cargo registry/git" in error for error in rust_cache_errors):
        raise AssertionError(f"action prose must not be treated as a rust-cache action: {rust_cache_errors!r}")

    dynamic_env_cases = {
        "RUSTFLAGS raw output override must be classified": """
            E=RUSTFLAGS
            export $E='--out-dir=/tmp/raw-out'
            cargo build
        """,
        "CARGO_BUILD_RUSTFLAGS raw output override must be classified": """
            E=CARGO_BUILD_RUSTFLAGS
            export $E='--artifact-dir=/tmp/raw-artifacts'
            cargo build
        """,
        "CARGO_ENCODED_RUSTFLAGS raw output override must be classified": """
            E=CARGO_ENCODED_RUSTFLAGS
            export $E='--out-dir=/tmp/raw-out'
            cargo build
        """,
        "CARGO_HOME raw cache override must be classified": """
            E=CARGO_HOME
            export $E=/tmp/cargo-home
            cargo build
        """,
        "RUSTUP_HOME raw toolchain override must be classified": """
            E=RUSTUP_HOME
            export $E=/tmp/rustup-home
            cargo build
        """,
        "CARGO_INCREMENTAL raw cache override must be classified": """
            E=CARGO_INCREMENTAL
            export $E=1
            cargo build
        """,
        "CARGO_INSTALL_ROOT install output override must be classified": """
            E=CARGO_INSTALL_ROOT
            export $E=/tmp/install-root
            cargo install cargo-deny
        """,
        "RUSTC_WRAPPER raw compiler wrapper must be classified": """
            E=RUSTC_WRAPPER
            export $E=/tmp/wrapper
            cargo build
        """,
        "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified": """
            E=RUSTC_WORKSPACE_WRAPPER
            export $E=/tmp/workspace-wrapper
            cargo build
        """,
    }
    for expected, script in dynamic_env_cases.items():
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(script))
        if expected not in errors:
            raise AssertionError(f"dynamic env alias did not classify {expected!r}: {errors!r}")

    rustflags_expected = "RUSTFLAGS raw output override must be classified"
    rustflags_variable_cases = [
        'OUT="--out-dir=/tmp/raw"; RUSTFLAGS="$OUT" cargo build',
        'OUT="--artifact-dir=/tmp/raw"; env RUSTFLAGS="$OUT" cargo build',
        'OUT="--out-dir=/tmp/raw"\nRUSTFLAGS="$OUT" cargo build',
    ]
    for script in rustflags_variable_cases:
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(script))
        if rustflags_expected not in errors:
            raise AssertionError(f"rustflags variable output override was not rejected: {script!r} -> {errors!r}")

    redirection_expected = "cargo --target-dir raw target override must be classified"
    advanced_redirection_cases = [
        "env &> out -u FOO cargo build --target-dir /tmp/raw",
        "env &>> out -u FOO cargo build --target-dir /tmp/raw",
        "env >& out -u FOO cargo build --target-dir /tmp/raw",
        "env <& input -u FOO cargo build --target-dir /tmp/raw",
        "env <<< input -u FOO cargo build --target-dir /tmp/raw",
    ]
    for script in advanced_redirection_cases:
        errors = verifier.raw_rust_storage_errors(script)
        if redirection_expected not in errors:
            raise AssertionError(f"advanced redirection hid raw cargo target override: {script!r} -> {errors!r}")

    s3_expected = "S3 active mutable target cache must be rejected"
    s3_stdout_cases = {
        "aws stdout extracted into target": "aws s3 cp s3://bolt-v2-active-cache/target.tar - | tar -x -C target",
        "aws stdout extracted into target with traditional tar options": "aws s3 cp s3://bolt-v2-active-cache/target.tar - | tar xf - -C target",
        "aws stdout piped through cat into target": "aws s3 cp s3://bolt-v2-active-cache/target.tar - | cat > target/cache.tar",
        "aws stdout piped through cat with >& redirection into target": "aws s3 cp s3://bolt-v2-active-cache/target.tar - | cat >& target/cache.tar",
        "aws stdout redirected into target": "aws s3 cp s3://bolt-v2-active-cache/target.tar - > target/cache.tar",
    }
    for name, script in s3_stdout_cases.items():
        errors = verifier.raw_rust_storage_errors(script)
        if s3_expected not in errors:
            raise AssertionError(f"{name} was not rejected: {errors!r}")

    env_chdir_cases = [
        "env -Ctarget aws s3 sync debug s3://bolt-v2-active-cache/target/debug",
        "env -iC target aws s3 sync debug s3://bolt-v2-active-cache/target/debug",
        "env -u -C -C target aws s3 sync debug s3://bolt-v2-active-cache/target/debug",
    ]
    for script in env_chdir_cases:
        errors = verifier.raw_rust_storage_errors(script)
        if s3_expected not in errors:
            raise AssertionError(f"env chdir active target context was not rejected: {script!r} -> {errors!r}")

    sudo_chdir_cases = [
        "sudo -ED target aws s3 sync debug s3://bolt-v2-active-cache/target/debug",
        "sudo -u -D -D target aws s3 sync debug s3://bolt-v2-active-cache/target/debug",
    ]
    for script in sudo_chdir_cases:
        errors = verifier.raw_rust_storage_errors(script)
        if s3_expected not in errors:
            raise AssertionError(f"sudo chdir active target context was not rejected: {script!r} -> {errors!r}")


def assert_required_job_indentation_is_actionable() -> None:
    assert_error(
        "job clippy must use two-space top-level indentation",
        replace_once(BASE_WORKFLOW, "  clippy:\n    name: clippy", "    clippy:\n    name: clippy"),
    )


def assert_body_exits_requires_top_level_exit() -> None:
    verifier = load_verifier()
    nested_only = """
            if [[ "$inner_result" != "success" ]]; then
              exit 1
            fi
"""
    if verifier.body_exits(nested_only):
        raise AssertionError("body_exits must ignore exits nested inside inner conditionals")
    nested_then_top_level = nested_only + "            exit 1\n"
    if not verifier.body_exits(nested_then_top_level):
        raise AssertionError("body_exits must accept one top-level exit 1")


def assert_nextest_live_node_group_required() -> None:
    verifier = load_verifier()
    manifest = all_standalone_live_node_manifest(verifier)
    assert_nextest_error(
        "nextest config missing live-node test group",
        BASE_NEXTEST_CONFIG.replace("live-node = { max-threads = 1 }", ""),
        manifest,
    )
    assert_nextest_error(
        "nextest live-node test group max-threads must be 1",
        BASE_NEXTEST_CONFIG.replace("max-threads = 1", "max-threads = 2"),
        manifest,
    )
    assert_nextest_error(
        "nextest config must assign LiveNode test paths to live-node group",
        BASE_NEXTEST_CONFIG.replace("binary(=venue_contract)", "binary(=config_schema)"),
        manifest,
    )
    assert_nextest_error(
        "nextest config must assign LiveNode test paths to live-node group",
        BASE_NEXTEST_CONFIG.replace("test-group = 'live-node'", "test-group = 'other'"),
        manifest,
    )
    assert_nextest_error(
        "missing test(~bolt_v3_live_node::tests::)",
        BASE_NEXTEST_CONFIG.replace(
            " | test(~bolt_v3_live_node::tests::)",
            "",
        ),
        manifest,
    )


def assert_nextest_live_node_group_covers_bolt_v3_builders() -> None:
    verifier = load_verifier()
    manifest = all_standalone_live_node_manifest(verifier)
    for binary in verifier.LIVE_NODE_NEXTEST_BINARIES:
        assert_nextest_error(
            f"missing binary(={binary})",
            BASE_NEXTEST_CONFIG.replace(f"binary(={binary}) | ", "").replace(
                f" | binary(={binary})",
                "",
            ),
            manifest,
        )


def assert_nextest_live_node_group_uses_manifest_harness_scope() -> None:
    verifier = load_verifier()
    member = "bolt_v3_client_registration"
    harness = "wiring_registration"
    manifest = live_node_manifest_with(verifier, consolidated={member: harness})
    expected_clause = f"(binary(={harness}) & test(/^{member}::/))"
    canonical_config = BASE_NEXTEST_CONFIG.replace(
        f"binary(={member})",
        expected_clause,
    )
    assert_nextest_clean(canonical_config, manifest)
    assert_nextest_error(
        f"missing {expected_clause}",
        BASE_NEXTEST_CONFIG,
        manifest,
    )
    assert_nextest_error(
        f"missing {expected_clause}",
        BASE_NEXTEST_CONFIG.replace(f"binary(={member})", f"binary(={harness})"),
        manifest,
    )


def assert_nextest_live_node_group_accepts_manifest_standalone_member() -> None:
    verifier = load_verifier()
    manifest = live_node_manifest_with(
        verifier,
        consolidated={"bolt_v3_client_registration": "bolt_v3_client_registration"},
    )
    assert_nextest_clean(BASE_NEXTEST_CONFIG, manifest)


def test_harness_manifest_requires_autotests_false() -> None:
    assert_test_harness_manifest_clean()
    assert_test_harness_manifest_error(
        "Cargo.toml [package].autotests must be false",
        cargo_autotests="true",
    )


def test_harness_manifest_rejects_orphan_test_members() -> None:
    assert_test_harness_manifest_clean()
    assert_test_harness_manifest_error(
        "tests/bolt_v3_orphan.rs has #[test] but is not registered in any explicit test harness",
        test_files={"bolt_v3_orphan": "#[test]\nfn orphan_runs() {}\n"},
    )


def test_harness_manifest_rejects_double_modded_members() -> None:
    assert_test_harness_manifest_clean()
    harness_to_members = {
        harness: ((harness, TEST_HARNESS_MEMBER) if harness in {"iv", "pricing"} else (harness,))
        for harness in test_harness_names()
    }
    assert_test_harness_manifest_error(
        f"tests/{TEST_HARNESS_MEMBER}.rs is registered by multiple harnesses: iv, pricing",
        manifest=base_test_harness_manifest(harness_to_members),
    )


def test_harness_manifest_rejects_unreferenced_top_level_files() -> None:
    assert_test_harness_manifest_clean()
    assert_test_harness_manifest_error(
        "tests/bolt_v3_unreferenced.rs is neither a harness root, a #[test]-bearing registered member, nor a declared test helper",
        test_files={"bolt_v3_unreferenced": "pub fn helper_only() {}\n"},
    )


def test_harness_manifest_enforces_expected_harness_count() -> None:
    assert_test_harness_manifest_clean()
    harness_to_members = {
        **base_test_harness_manifest().harness_to_members,
        "extra_harness": ("extra_harness",),
    }
    expected_count = load_verifier().EXPECTED_HARNESS_COUNT
    actual_count = len(harness_to_members)
    assert_test_harness_manifest_error(
        f"Cargo.toml explicit test harness count must be {expected_count}, got {actual_count}",
        manifest=base_test_harness_manifest(harness_to_members),
    )


def test_harness_manifest_rejects_harness_roots_as_members() -> None:
    assert_test_harness_manifest_clean()
    harness_to_members = dict(base_test_harness_manifest().harness_to_members)
    harness_to_members["iv"] = ("iv", TEST_HARNESS_MEMBER, "pricing")
    assert_test_harness_manifest_error(
        "tests/pricing.rs is a harness root and must not be mod-ed by harness iv",
        manifest=base_test_harness_manifest(harness_to_members),
    )


def test_harness_manifest_masks_inner_attrs_and_rejects_crate_attrs() -> None:
    source = (REPO_ROOT / "tests" / "bolt_v3_binary_oracle_edge_taker_a10_structure.rs").read_text(encoding="utf-8")
    harness_to_members = {
        harness: ((harness, "bolt_v3_binary_oracle_edge_taker_a10_structure") if harness == "maker_taker" else (harness,))
        for harness in test_harness_names()
    }
    assert_test_harness_manifest_clean(
        manifest=base_test_harness_manifest(harness_to_members),
        test_files={"bolt_v3_binary_oracle_edge_taker_a10_structure": source},
    )
    assert_test_harness_manifest_error(
        "tests/bolt_v3_fixture_member.rs uses banned module-level inner attribute #![feature(...)]",
        test_files={TEST_HARNESS_MEMBER: "#![feature(test)]\n#[test]\nfn fixture_member_runs() {}\n"},
    )


def test_harness_manifest_rejects_retired_member_test_filters() -> None:
    assert_test_harness_manifest_clean()
    assert_test_harness_manifest_error(
        f"justfile references retired integration-test member {TEST_HARNESS_MEMBER!r} with --test; use harness 'iv'",
        justfile_text=f"ci-test:\n    cargo test --test {TEST_HARNESS_MEMBER}\n",
    )


def test_harness_manifest_rejects_typo_positional_test_filter() -> None:
    assert_test_harness_manifest_clean(
        justfile_text=f"ci-test:\n    cargo test --test iv -- {TEST_HARNESS_MEMBER}:: --nocapture\n",
    )
    assert_test_harness_manifest_error(
        "does not belong to --test harness 'iv'",
        justfile_text="ci-test:\n    cargo test --test iv -- bolt_v3_fixture_TYPO:: --nocapture\n",
    )


def test_harness_manifest_rejects_quoted_retired_member_test_flag() -> None:
    assert_test_harness_manifest_error(
        f"references retired integration-test member {TEST_HARNESS_MEMBER!r}",
        justfile_text=f"ci-test:\n    cargo test '--test' {TEST_HARNESS_MEMBER}\n",
    )


def test_shared_registry_cache_save_is_main_push_only() -> None:
    verifier = load_verifier()
    old_job_only_guard = "${{ github.job == 'test-archive' }}"
    workflow = f"""jobs:
  test-archive:
    steps:
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: {old_job_only_guard}
"""
    errors = verifier.shared_registry_cache_errors(
        "test-archive",
        verifier.parse_jobs(workflow)["test-archive"],
    )
    expected = "test-archive shared Cargo registry/git cache save must be main-only"
    if not any(expected in error for error in errors):
        raise AssertionError(f"expected {expected!r}, got: {errors}")

    main_only = workflow.replace(old_job_only_guard, MAIN_ONLY_SHARED_REGISTRY_SAVE_IF)
    main_only_errors = verifier.shared_registry_cache_errors(
        "test-archive",
        verifier.parse_jobs(main_only)["test-archive"],
    )
    if any(expected in error for error in main_only_errors):
        raise AssertionError(f"main-only save guard was rejected: {main_only_errors}")


def test_nextest_artifact_cache_doc_names_rollout_evidence() -> None:
    doc = (REPO_ROOT / "docs" / "ci" / "nextest-artifact-cache.md").read_text(encoding="utf-8")
    required = (
        "## Post-Merge Acceptance Evidence",
        "push-to-main run",
        "restore HIT",
        "CI Storage Tripwire",
        "week-one metrics",
        "Tag pushes intentionally rebuild nextest archive payloads locally",
    )
    missing = [fragment for fragment in required if fragment not in doc]
    if missing:
        raise AssertionError(f"nextest artifact cache doc is missing rollout evidence terms: {missing!r}")


def test_nextest_config_rejects_surprise_binary_overrides() -> None:
    verifier = load_verifier()
    manifest = all_standalone_live_node_manifest(verifier)
    assert_nextest_clean(BASE_NEXTEST_CONFIG, manifest)
    assert_nextest_error(
        "nextest config has unregistered per-binary override",
        BASE_NEXTEST_CONFIG
        + """

[[profile.default.overrides]]
filter = 'binary(=platform_config)'
retries = 2
""",
        manifest,
    )


def test_nextest_config_rejects_regex_form_binary_overrides() -> None:
    # findings 2+3: a regex-form binary(/.../) filter parses to an empty binary set,
    # so with a non-sensitive key it slips past the skip-guard entirely.
    verifier = load_verifier()
    manifest = all_standalone_live_node_manifest(verifier)
    assert_nextest_clean(BASE_NEXTEST_CONFIG, manifest)
    assert_nextest_error(
        "nextest config has unregistered per-binary override",
        BASE_NEXTEST_CONFIG
        + """

[[profile.default.overrides]]
filter = 'binary(/^venue_contract/)'
threads-required = 4
""",
        manifest,
    )


def test_nextest_config_rejects_regex_binary_smuggled_into_live_node_override() -> None:
    # finding 3: a regex-form binary appended to an otherwise-valid live-node filter
    # is invisible to the <= whitelist (empty set), so the override is wrongly accepted.
    verifier = load_verifier()
    manifest = all_standalone_live_node_manifest(verifier)
    smuggled = BASE_NEXTEST_CONFIG.replace(
        "binary(=venue_contract)'",
        "binary(=venue_contract) | binary(/^retired_test_binary$/)'",
    )
    assert_nextest_error(
        "nextest config has unregistered per-binary override",
        smuggled,
        manifest,
    )


def test_nextest_config_rejects_foreign_test_prefix_in_live_node_override() -> None:
    # finding 5: a non-live-node member's tests smuggled into the serialization group
    # via an already-recognized harness binary adds no new binary, so only exact
    # test-prefix-set equality catches it.
    verifier = load_verifier()
    member = "bolt_v3_client_registration"
    harness = "wiring_registration"
    manifest = live_node_manifest_with(verifier, consolidated={member: harness})
    expected_clause = f"(binary(={harness}) & test(/^{member}::/))"
    canonical = BASE_NEXTEST_CONFIG.replace(f"binary(={member})", expected_clause)
    assert_nextest_clean(canonical, manifest)
    smuggled = canonical.replace(
        expected_clause,
        f"{expected_clause} | (binary(={harness}) & test(/^cli::/))",
    )
    assert_nextest_error(
        "nextest config has unregistered per-binary override",
        smuggled,
        manifest,
    )


# Pin-consistency fixtures. The base SHA already appears throughout BASE_WORKFLOW
# and BASE_ADVISORY_WORKFLOW; SHA_ALT is a different valid 40-hex SHA used to
# exercise drift, and SHA_BASE_UPPER is the base SHA in uppercase to exercise
# normalization.
PIN_CONSISTENCY_SHA_BASE = "e49978b799e49ff429d162b7a30601a569ab6538"
PIN_CONSISTENCY_SHA_ALT = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
PIN_CONSISTENCY_SHA_BASE_UPPER = PIN_CONSISTENCY_SHA_BASE.upper()


def assert_pin_consistency_cross_file_mismatch_errors() -> None:
    """Finding 1: two workflows with different valid SHA pins must report drift."""
    verifier = load_verifier()
    advisory_alt = BASE_ADVISORY_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_ALT}",
    )
    errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory_alt},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    drift_errors = [e for e in errors if "taiki-e/install-action pin drift" in e]
    if len(drift_errors) < 1:
        raise AssertionError(
            f"expected at least one pin-drift error, got: {drift_errors!r} (full: {errors!r})"
        )
    drift = drift_errors[0]
    if PIN_CONSISTENCY_SHA_BASE not in drift or PIN_CONSISTENCY_SHA_ALT not in drift:
        raise AssertionError(
            f"pin-drift error must list both SHAs, got: {drift!r}"
        )
    if "ci.yml" not in drift or "advisory.yml" not in drift:
        raise AssertionError(
            f"pin-drift error must list both files, got: {drift!r}"
        )


def assert_pin_consistency_same_sha_no_error() -> None:
    """Finding 1: identical SHAs across workflows must not error."""
    verifier = load_verifier()
    errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    drift_errors = [e for e in errors if "pin drift" in e]
    if drift_errors:
        raise AssertionError(
            f"expected no pin-drift errors for identical SHAs, got: {drift_errors!r}"
        )


def assert_pin_consistency_includes_setup_action() -> None:
    """The composite setup action must be in the same install-action pin bucket."""
    verifier = load_verifier()
    setup_action_alt = BASE_ACTION.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_ALT}",
        1,
    )
    errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW},
        setup_action_alt,
        BASE_NEXTEST_CONFIG,
    )
    drift_errors = [e for e in errors if "taiki-e/install-action pin drift" in e]
    if not drift_errors:
        raise AssertionError(
            f"expected setup-action pin drift to be reported, got: {errors!r}"
        )
    drift = drift_errors[0]
    if ".github/actions/setup-environment/action.yml" not in drift:
        raise AssertionError(
            f"pin-drift error must include the setup action path, got: {drift!r}"
        )


def assert_pin_consistency_rejects_mutable_tag() -> None:
    """Finding 2: mutable tags (e.g. @v2) must fail with a 40-hex SHA message."""
    verifier = load_verifier()
    mutable_advisory = BASE_ADVISORY_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        "taiki-e/install-action@v2",
    )
    errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": mutable_advisory},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    matching = [
        e
        for e in errors
        if "taiki-e/install-action" in e
        and "40-hex-SHA" in e
        and "advisory.yml" in e
    ]
    if not matching:
        raise AssertionError(
            f"expected mutable-tag rejection mentioning '40-hex-SHA' and the file, got: {errors!r}"
        )


def assert_pin_consistency_ignores_non_uses_mentions() -> None:
    """Mentioning the action outside a `uses:` key must not be treated as a pin."""
    verifier = load_verifier()
    workflow = f"""
name: mention probe
jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - name: documents taiki-e/install-action@v2 without invoking it
        run: echo ok
      - uses: taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}
"""
    errors = verifier.verify_install_action_pin_consistency({"ci.yml": workflow})
    if errors:
        raise AssertionError(
            f"non-uses mentions must not produce install-action pin errors, got: {errors!r}"
        )


def assert_pin_consistency_accepts_uppercase_sha() -> None:
    """Finding 3: uppercase hex SHAs must be detected AND normalized to lowercase."""
    verifier = load_verifier()
    advisory_upper = BASE_ADVISORY_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE_UPPER}",
    )
    ci_alt = BASE_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_ALT}",
    )
    errors = verifier.verify_workflows(
        {"ci.yml": ci_alt, "advisory.yml": advisory_upper},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    drift_errors = [e for e in errors if "taiki-e/install-action pin drift" in e]
    if not drift_errors:
        raise AssertionError(
            f"expected drift error proving uppercase SHA was detected and bucketed, got errors: {errors!r}"
        )
    drift = drift_errors[0]
    if PIN_CONSISTENCY_SHA_BASE_UPPER in drift:
        raise AssertionError(
            f"pin-drift error must report normalized lowercase SHA, found uppercase: {drift!r}"
        )
    if PIN_CONSISTENCY_SHA_BASE not in drift or PIN_CONSISTENCY_SHA_ALT not in drift:
        raise AssertionError(
            f"pin-drift error must list lowercased base SHA and alt SHA, got: {drift!r}"
        )


def assert_pin_consistency_intra_file_mismatch_uses_pin_drift_wording() -> None:
    """Finding 5: intra-file drift must use 'pin drift:' wording, not 'across workflows'."""
    verifier = load_verifier()
    workflow_with_two_pins = BASE_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_ALT}",
        1,
    )
    errors = verifier.verify_workflows(
        {"ci.yml": workflow_with_two_pins},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    drift_errors = [e for e in errors if "taiki-e/install-action pin drift:" in e]
    if not drift_errors:
        raise AssertionError(
            f"expected intra-file drift to use 'pin drift:' wording, got: {errors!r}"
        )
    if any("across workflows" in e for e in errors):
        raise AssertionError(
            f"intra-file drift must not say 'across workflows', got: {errors!r}"
        )


def _replace_advisory_pin_with(replacement: str) -> str:
    """Replace the first taiki-e/install-action pin in BASE_ADVISORY_WORKFLOW.

    The full `uses: taiki-e/install-action@<sha>` line (without a leading
    dash — the advisory fixture uses the multi-key step form) is replaced
    verbatim so callers can inject mutable tags, quoted forms, mismatched
    quotes, or YAML multi-line scalars without altering surrounding job
    structure.
    """
    original = f"uses: taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}"
    if original not in BASE_ADVISORY_WORKFLOW:
        raise RuntimeError(
            "advisory fixture missing canonical install-action `uses:` line"
        )
    return BASE_ADVISORY_WORKFLOW.replace(original, replacement, 1)


def assert_pin_consistency_rejects_multi_line_mutable_tag() -> None:
    """BLOCK 1: multi-line `uses:` with mutable tag must emit malformed-form error."""
    verifier = load_verifier()
    multi_line = "uses:\n          taiki-e/install-action@v2"
    advisory = _replace_advisory_pin_with(multi_line)
    errors = verifier.verify_install_action_pin_consistency(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory}
    )
    matching = [
        e
        for e in errors
        if "advisory.yml:" in e
        and "40-hex-SHA" in e
        and "taiki-e/install-action@v2" in e
    ]
    if not matching:
        raise AssertionError(
            f"expected multi-line @v2 to be flagged with file:line and 40-hex-SHA wording, got: {errors!r}"
        )


def assert_pin_consistency_rejects_block_scalar_mutable_tag() -> None:
    """Gemini follow-up: block-scalar `uses:` with mutable tag must not bypass."""
    verifier = load_verifier()
    cases = [
        "uses: >\n          taiki-e/install-action@v2",
        "uses: |-\n          taiki-e/install-action@v2",
        "uses: # bypass\n          taiki-e/install-action@v2",
    ]
    for replacement in cases:
        advisory = _replace_advisory_pin_with(replacement)
        errors = verifier.verify_install_action_pin_consistency(
            {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory}
        )
        matching = [
            e
            for e in errors
            if "advisory.yml:" in e
            and "40-hex-SHA" in e
            and "taiki-e/install-action@v2" in e
        ]
        if not matching:
            raise AssertionError(
                f"expected block/comment scalar @v2 to be flagged with file:line and 40-hex-SHA wording, got: {errors!r}"
            )


def assert_pin_consistency_rejects_multi_line_valid_sha() -> None:
    """BLOCK 1: multi-line `uses:` with valid SHA still emits error AND does not bucket."""
    verifier = load_verifier()
    multi_line = f"uses:\n          taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}"
    advisory = _replace_advisory_pin_with(multi_line)
    # ci.yml uses SHA_ALT (single line), advisory uses SHA_BASE (multi-line, malformed).
    ci_alt = BASE_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_ALT}",
    )
    errors = verifier.verify_install_action_pin_consistency(
        {"ci.yml": ci_alt, "advisory.yml": advisory}
    )
    style_errors = [
        e for e in errors if "advisory.yml:" in e and "40-hex-SHA" in e
    ]
    if not style_errors:
        raise AssertionError(
            f"expected multi-line valid SHA to be flagged as malformed, got: {errors!r}"
        )
    # The malformed multi-line SHA must NOT phantom-bucket: there is only one
    # well-formed bucket (SHA_ALT in ci.yml), so no drift error should appear.
    drift_errors = [e for e in errors if "taiki-e/install-action pin drift" in e]
    if drift_errors:
        raise AssertionError(
            f"multi-line SHA must not contribute to the bucket map, got drift: {drift_errors!r}"
        )


def assert_pin_consistency_accepts_double_quoted_sha() -> None:
    """BLOCK 2: double-quoted valid SHA must not emit a malformed-form error."""
    verifier = load_verifier()
    quoted = f'uses: "taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}"'
    advisory = _replace_advisory_pin_with(quoted)
    errors = verifier.verify_install_action_pin_consistency(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory}
    )
    malformed = [e for e in errors if "advisory.yml:" in e and "40-hex-SHA" in e]
    if malformed:
        raise AssertionError(
            f"double-quoted valid SHA must not be flagged as malformed, got: {malformed!r}"
        )
    drift = [e for e in errors if "taiki-e/install-action pin drift" in e]
    if drift:
        raise AssertionError(
            f"double-quoted same SHA must not produce drift, got: {drift!r}"
        )


def assert_pin_consistency_accepts_single_quoted_sha() -> None:
    """BLOCK 2: single-quoted valid SHA must not emit a malformed-form error."""
    verifier = load_verifier()
    quoted = f"uses: 'taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}'"
    advisory = _replace_advisory_pin_with(quoted)
    errors = verifier.verify_install_action_pin_consistency(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory}
    )
    malformed = [e for e in errors if "advisory.yml:" in e and "40-hex-SHA" in e]
    if malformed:
        raise AssertionError(
            f"single-quoted valid SHA must not be flagged as malformed, got: {malformed!r}"
        )


def assert_pin_consistency_rejects_mismatched_quotes() -> None:
    """BLOCK 2: mismatched quotes must still fail strictly (backreference)."""
    verifier = load_verifier()
    mismatched = f"uses: \"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}'"
    advisory = _replace_advisory_pin_with(mismatched)
    errors = verifier.verify_install_action_pin_consistency(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory}
    )
    matching = [
        e
        for e in errors
        if "advisory.yml:" in e and "40-hex-SHA" in e
    ]
    if not matching:
        raise AssertionError(
            f"mismatched quotes must be flagged as malformed, got: {errors!r}"
        )


def assert_prebuilt_tool_installs_accepts_uppercase_pinned_install_action() -> None:
    """NIT C: verify_prebuilt_tool_installs must accept uppercase 40-hex pins.

    Uppercase 40-hex SHAs are valid pins now that the shared regex accepts
    [0-9a-fA-F]{40}; the broader prebuilt-tool-install check must not emit a
    'must install ... with pinned taiki-e/install-action' error for them.
    """
    verifier = load_verifier()
    advisory_upper = BASE_ADVISORY_WORKFLOW.replace(
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE}",
        f"taiki-e/install-action@{PIN_CONSISTENCY_SHA_BASE_UPPER}",
    )
    errors = verifier.verify_prebuilt_tool_installs(advisory_upper, "advisory.yml")
    pinning_errors = [
        e
        for e in errors
        if "with pinned taiki-e/install-action" in e
    ]
    if pinning_errors:
        raise AssertionError(
            f"uppercase SHA must be accepted as pinned, got: {pinning_errors!r}"
        )














def workflow_with_exact_head_governance_cache_inputs(workflow: str) -> str:
    if all(cache_input in workflow for cache_input in FORBIDDEN_MANAGED_TARGET_CACHE_INPUTS):
        return workflow
    governance_inputs = ", " + ", ".join(FORBIDDEN_MANAGED_TARGET_CACHE_INPUTS)
    return workflow.replace("'.cargo/config.toml') }}", f"'.cargo/config.toml'{governance_inputs}) }}").replace(
        "'specs/**/*.md') }}",
        f"'specs/**/*.md'{governance_inputs}) }}",
    )


def write_base_workflows(workflow_dir: pathlib.Path) -> None:
    workflow_dir.mkdir(parents=True)
    (workflow_dir / "ci.yml").write_text(BASE_WORKFLOW)
    (workflow_dir / "dispatch-ci-cancel.yml").write_text(BASE_DISPATCH_CI_CANCEL_WORKFLOW)
    (workflow_dir / "merge-readiness-finalizer.yml").write_text(BASE_MERGE_READINESS_FINALIZER_WORKFLOW)
    (workflow_dir / "coverage-enforcer.yml").write_text(BASE_COVERAGE_ENFORCER_WORKFLOW)
    (workflow_dir / "flaky-test-detection.yml").write_text(repo_workflow_text(".github/workflows/flaky-test-detection.yml"))
    (workflow_dir / "flaky-test-smoke.yml").write_text(
        repo_workflow_text(".github/workflows/flaky-test-smoke.yml")
    )






def assert_storage_cleanup_alert_workflow_contract() -> None:
    verifier = load_verifier()
    workflow_path = ".github/workflows/ci-storage-cleanup-alert.yml"
    workflow = repo_workflow_text(workflow_path)
    runners_config = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: workflow}, runners_config)
    if errors:
        raise AssertionError(f"storage cleanup alert workflow contract should pass, got: {errors}")

    missing_errors = verifier.verify_storage_cleanup_alert_workflow({}, runners_config)
    if not any(f"{workflow_path} must exist" in error for error in missing_errors):
        raise AssertionError(f"storage cleanup alert workflow missing file drift was silent, got: {missing_errors}")

    missing_permissions = replace_once(
        workflow,
        """permissions:
  contents: read
  actions: read

""",
        "",
    )
    missing_permissions_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: missing_permissions}, runners_config)
    if not any("permissions must match storage_audit.cleanup_feasibility_alert.workflow.permissions" in error for error in missing_permissions_errors):
        raise AssertionError(f"storage cleanup alert missing permissions drift was silent, got: {missing_permissions_errors}")

    malformed_permissions = replace_once(
        workflow,
        """permissions:
  contents: read
  actions: read
""",
        "permissions: read\n",
    )
    malformed_permissions_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: malformed_permissions}, runners_config)
    if not any(
        "permissions must match storage_audit.cleanup_feasibility_alert.workflow.permissions" in error
        for error in malformed_permissions_errors
    ):
        raise AssertionError(f"storage cleanup alert malformed permissions drift was silent, got: {malformed_permissions_errors}")

    missing_concurrency = replace_once(
        workflow,
        """concurrency:
  group: ci-storage-cleanup-alert
  cancel-in-progress: false

""",
        "",
    )
    missing_concurrency_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: missing_concurrency}, runners_config)
    if not any("concurrency must match storage_audit.cleanup_feasibility_alert.workflow" in error for error in missing_concurrency_errors):
        raise AssertionError(f"storage cleanup alert missing concurrency drift was silent, got: {missing_concurrency_errors}")

    malformed_concurrency = replace_once(
        workflow,
        """concurrency:
  group: ci-storage-cleanup-alert
  cancel-in-progress: false
""",
        "concurrency: ci-storage-cleanup-alert\n",
    )
    malformed_concurrency_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: malformed_concurrency}, runners_config)
    if not any("concurrency must match storage_audit.cleanup_feasibility_alert.workflow" in error for error in malformed_concurrency_errors):
        raise AssertionError(f"storage cleanup alert malformed concurrency drift was silent, got: {malformed_concurrency_errors}")

    missing_alert_command = replace_once(workflow, " --cleanup-alert", "")
    missing_alert_errors = verifier.verify_storage_cleanup_alert_workflow(
        {workflow_path: missing_alert_command},
        runners_config,
    )
    if not any("run step must match storage_audit.cleanup_feasibility_alert.workflow contract" in error for error in missing_alert_errors):
        raise AssertionError(f"storage cleanup alert command drift was silent, got: {missing_alert_errors}")

    missing_json_output = replace_once(workflow, ' --cleanup-json-output "$RUNNER_TEMP/cleanup-feasibility.json"', "")
    missing_json_output_errors = verifier.verify_storage_cleanup_alert_workflow(
        {workflow_path: missing_json_output},
        runners_config,
    )
    if not any(
        "run step must match storage_audit.cleanup_feasibility_alert.workflow contract" in error
        for error in missing_json_output_errors
    ):
        raise AssertionError(f"storage cleanup alert JSON output drift was silent, got: {missing_json_output_errors}")

    injected_job_if = replace_once(
        workflow,
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n",
        "    if: ${{ github.event_name == 'schedule' || github.event_name == 'workflow_dispatch' }}\n"
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n",
    )
    injected_job_if_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: injected_job_if}, runners_config)
    if not any("storage cleanup alert job keys must match the workflow contract" in error for error in injected_job_if_errors):
        raise AssertionError(f"storage cleanup alert redundant job if drift was silent, got: {injected_job_if_errors}")

    missing_timeout = replace_once(workflow, "    timeout-minutes: 30\n", "")
    missing_timeout_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: missing_timeout}, runners_config)
    if not any(
        "storage cleanup alert timeout-minutes must match storage_audit.cleanup_feasibility_alert.workflow.job_timeout_minutes" in error
        for error in missing_timeout_errors
    ):
        raise AssertionError(f"storage cleanup alert timeout drift was silent, got: {missing_timeout_errors}")

    missing_upload_step = replace_once(
        workflow,
        """      - name: Upload cleanup feasibility JSON
        id: upload-cleanup-feasibility-json
        if: ${{ always() }}
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ci-storage-cleanup-feasibility
          path: ${{ runner.temp }}/cleanup-feasibility.json
          if-no-files-found: error
          retention-days: 7
""",
        "",
    )
    missing_upload_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: missing_upload_step}, runners_config)
    if not any(
        "storage cleanup alert job must contain exactly checkout, run, and upload steps" in error
        for error in missing_upload_errors
    ):
        raise AssertionError(f"storage cleanup alert upload step drift was silent, got: {missing_upload_errors}")

    schema_drift_config = replace_once(
        runners_config,
        "[storage_audit.cleanup_feasibility_alert]\nschema_version = 1",
        "[storage_audit.cleanup_feasibility_alert]\nschema_version = 2",
    )
    schema_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: workflow}, schema_drift_config)
    if not any("cleanup_feasibility_alert.schema_version must be 1" in error for error in schema_errors):
        raise AssertionError(f"storage cleanup alert schema drift was silent, got: {schema_errors}")

    delete_fragment = workflow + "\n# --method DELETE\n"
    delete_errors = verifier.verify_storage_cleanup_alert_workflow({workflow_path: delete_fragment}, runners_config)
    if not any("must not contain forbidden workflow fragment" in error for error in delete_errors):
        raise AssertionError(f"storage cleanup alert destructive fragment drift was silent, got: {delete_errors}")




def run_verifier_main_with_extra_action(
    extra_action_text: str,
    action_file_name: str = "action.yml",
) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(repo_source_text(VERIFIER_PATH))

        workflow_dir = tmp_path / ".github" / "workflows"
        write_repo_workflows(workflow_dir)
        write_test_harness_fixture(
            tmp_path,
            manifest=base_test_harness_manifest(),
            write_workflow=False,
            write_justfile=False,
        )

        action_path = tmp_path / ".github" / "actions" / "setup-environment" / "action.yml"
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)

        extra_action_path = tmp_path / ".github" / "actions" / "evade" / action_file_name
        extra_action_path.parent.mkdir(parents=True)
        extra_action_path.write_text(extra_action_text)

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)
        write_rust_verification_policy_fixtures(tmp_path)
        write_runner_config_fixture(tmp_path)
        storage_policy_path = write_storage_tripwire_policy_fixture(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_extra_action_entrypoint")
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        original_discover_policy = temp_verifier.ci_storage_tripwire.discover_policy_path
        temp_verifier.ci_storage_tripwire.discover_policy_path = lambda _root: storage_policy_path
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = temp_verifier.main()
        finally:
            temp_verifier.ci_storage_tripwire.discover_policy_path = original_discover_policy
        return result, stdout.getvalue() + stderr.getvalue()


def run_verifier_main_with_extra_workflow(workflow_name: str, workflow_text: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(repo_source_text(VERIFIER_PATH))

        workflow_dir = tmp_path / ".github" / "workflows"
        write_repo_workflows(workflow_dir)
        (workflow_dir / workflow_name).write_text(workflow_text)
        write_test_harness_fixture(
            tmp_path,
            manifest=base_test_harness_manifest(),
            write_workflow=False,
            write_justfile=False,
        )

        action_path = tmp_path / ".github" / "actions" / "setup-environment" / "action.yml"
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)
        write_rust_verification_policy_fixtures(tmp_path)
        write_runner_config_fixture(tmp_path)
        storage_policy_path = write_storage_tripwire_policy_fixture(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_extra_workflow_entrypoint")
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        original_discover_policy = temp_verifier.ci_storage_tripwire.discover_policy_path
        temp_verifier.ci_storage_tripwire.discover_policy_path = lambda _root: storage_policy_path
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = temp_verifier.main()
        finally:
            temp_verifier.ci_storage_tripwire.discover_policy_path = original_discover_policy
        return result, stdout.getvalue() + stderr.getvalue()






def assert_v6_red_yaml_steps_aliases_are_rejected() -> None:
    verifier = load_verifier()
    workflow = """
name: Probe
on: [push]
.shared_steps: &shared_steps
  - run: echo ok
jobs:
  hidden:
    runs-on: ubuntu-latest
    steps: *shared_steps
"""
    expected = "workflow steps must be explicit; YAML steps aliases are unsupported"
    errors = verifier.verify_workflow(textwrap.dedent(workflow))
    if expected not in errors:
        raise AssertionError(f"workflow steps alias was not rejected: {errors!r}")
    workflow = """
name: Probe
on: [push]
.raw_step: &raw_step
  run: cargo build --target-dir /tmp/raw
jobs:
  hidden:
    runs-on: ubuntu-latest
    steps:
      - *raw_step
"""
    errors = verifier.verify_workflow(textwrap.dedent(workflow))
    if expected not in errors:
        raise AssertionError(f"workflow step item alias was not rejected: {errors!r}")




def assert_v6_red_local_composite_actions_are_scanned() -> None:
    extra_action = """
name: Evade
runs:
  using: composite
  steps:
    - shell: bash
      run: cargo build
    - shell: bash
      run: aws s3 sync target s3://bolt-v2-active-cache/target
"""
    result, output = run_verifier_main_with_extra_action(textwrap.dedent(extra_action))
    expected_raw_cargo = ".github/actions/evade/action.yml: repo automation raw Cargo must use managed rust_verification wrapper"
    expected_s3 = ".github/actions/evade/action.yml: S3 active mutable target cache must be rejected"
    if result == 0 or expected_raw_cargo not in output or expected_s3 not in output:
        raise AssertionError(f"local composite action drift was silent: exit={result}, output={output!r}")


def assert_v6_red_additional_workflows_are_scanned() -> None:
    extra_workflow = """
name: Release
on: [workflow_dispatch]
jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - run: cargo build
      - run: aws s3 sync target s3://bolt-v2-active-cache/target
"""
    result, output = run_verifier_main_with_extra_workflow("release.yml", textwrap.dedent(extra_workflow))
    expected_raw_cargo = ".github/workflows/release.yml: repo automation raw Cargo must use managed rust_verification wrapper"
    expected_s3 = ".github/workflows/release.yml: S3 active mutable target cache must be rejected"
    if result == 0 or expected_raw_cargo not in output or expected_s3 not in output:
        raise AssertionError(f"additional workflow drift was silent: exit={result}, output={output!r}")


def assert_artifact_retention_policy_spine_gaps_are_reported() -> None:
    upload_artifact_action = base_upload_artifact_action_line()
    verifier = load_verifier()
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        verifier.DEFAULT_RUNNERS_CONFIG = pathlib.Path(tmp) / "missing-github-actions-runners.toml"
        try:
            missing_config_errors = verifier.verify_artifact_retention_policy(
                {".github/workflows/ci.yml": BASE_WORKFLOW},
                {},
            )
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    expected_missing_config = "ci/github-actions-runners.toml: enforcement set is empty"
    if not any(expected_missing_config in error for error in missing_config_errors):
        raise AssertionError(
            "artifact retention policy must fail closed without runner config, "
            f"got: {missing_config_errors}"
        )
    assert_artifact_retention_error(
        "artifact retention policy upload .github/workflows/backtester-ci.yml::issue_789::"
        "upload-issue-789-first-pl source .github/workflows/backtester-ci.yml is missing from scanned sources",
        workflows={".github/workflows/ci.yml": BASE_WORKFLOW},
    )

    unexpected_upload = BASE_WORKFLOW.replace(
        "      - name: Detect build-affecting changes\n",
        f"""      - name: Upload unexpected artifact
        id: upload-unexpected-artifact
        {upload_artifact_action}
        with:
          name: unexpected-artifact
          path: unexpected.txt
          retention-days: 1

      - name: Detect build-affecting changes
""",
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml::detector::upload-unexpected-artifact missing from artifact retention policy",
        {".github/workflows/ci.yml": unexpected_upload},
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml nextest-fingerprint upload-nextest-fingerprint artifact ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }} must set retention-days",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                "          retention-days: 14\n",
                "",
                1,
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary retention-days 31 does not match configured retention-days 3",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK.replace("retention-days: 3", "retention-days: 31"),
                1,
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary "
        f"must set if to configured if {BOLT_V2_BINARY_UPLOAD_IF}",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                BOLT_V2_BINARY_UPLOAD_IF_LINE,
                "",
                1,
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary "
        f"if ${{{{ github.event_name == 'pull_request' }}}} does not match configured if {BOLT_V2_BINARY_UPLOAD_IF}",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                BOLT_V2_BINARY_UPLOAD_IF,
                "${{ github.event_name == 'pull_request' }}",
                1,
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary must set exactly one retention-days",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK + "\n          retention-days: 31",
                1,
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-artifact step must set id for artifact retention policy",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                "        id: upload-bolt-v2-binary\n",
                "",
                1,
            )
        },
    )
    assert_artifact_retention_error(
        "artifact retention policy upload .github/workflows/ci.yml::build::upload-bolt-v2-binary has no matching upload-artifact step",
        {
            ".github/workflows/ci.yml": BASE_WORKFLOW.replace(
                "        id: upload-bolt-v2-binary\n",
                "        id: upload-renamed-binary\n",
                1,
            )
        },
    )
    duplicate_binary_upload = BASE_WORKFLOW.replace(
        "      - name: Upload artifact\n        id: upload-bolt-v2-binary\n",
        f"""      - name: Duplicate binary upload id
        id: upload-bolt-v2-binary
        {upload_artifact_action}
        with:
          name: duplicate-binary
          path: duplicate.txt
          retention-days: 1

      - name: Upload artifact
        id: upload-bolt-v2-binary
""",
        1,
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-artifact step id upload-bolt-v2-binary is duplicated",
        {".github/workflows/ci.yml": duplicate_binary_upload},
    )
    verifier = load_verifier()
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    lowered_transient_config = config_text.replace(
        "[artifact_retention.classes.transient]\nmax_retention_days = 7",
        "[artifact_retention.classes.transient]\nmax_retention_days = 5",
        1,
    )
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        ci_dir = tmp_path / "ci"
        ci_dir.mkdir(parents=True, exist_ok=True)
        config_path = ci_dir / "github-actions-runners.toml"
        config_path.write_text(lowered_transient_config, encoding="utf-8")
        (ci_dir / "chainlink-reference-fixture-capture-provenance.toml").write_text(
            (REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml").read_text(
                encoding="utf-8"
            ),
            encoding="utf-8",
        )
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            ceiling_errors = verifier.verify_artifact_retention_policy(
                {
                    ".github/workflows/backtester-ci.yml": repo_workflow_text(
                        ".github/workflows/backtester-ci.yml"
                    ),
                },
                {},
            )
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    expected_ceiling = (
        ".github/workflows/backtester-ci.yml issue_789 upload-issue-789-first-pl artifact "
        "issue-789-first-pl-${{ github.run_id }}-${{ github.run_attempt }} retention-days 7 "
        "exceeds configured max 5"
    )
    if not any(expected_ceiling in error for error in ceiling_errors):
        raise AssertionError(f"expected class ceiling exceedance error, got: {ceiling_errors}")
    assert_artifact_retention_error(
        ".github/workflows/ci.yml capture upload-capture-artifact artifact ${{ steps.provenance.outputs.artifact_name }} "
        "retention-days must be a positive integer",
        {
            ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml").replace(
                "          name: ${{ steps.provenance.outputs.artifact_name }}\n          path: ${{ env.CAPTURE_OUTPUT_DIR }}\n          if-no-files-found: error\n          retention-days: 14",
                "          name: ${{ steps.provenance.outputs.artifact_name }}\n          path: ${{ env.CAPTURE_OUTPUT_DIR }}\n          if-no-files-found: error\n          retention-days: ${{ github.event.inputs.retention_days }}",
                1,
            ),
            ".github/workflows/backtester-ci.yml": repo_workflow_text(".github/workflows/backtester-ci.yml"),
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml capture upload-capture-artifact artifact chainlink-reference-fixture-capture-attempt-${{ github.run_attempt }} "
        "does not match configured name ${{ steps.provenance.outputs.artifact_name }}",
        {
            ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml").replace(
                "          name: ${{ steps.provenance.outputs.artifact_name }}\n",
                "          name: chainlink-reference-fixture-capture-attempt-${{ github.run_attempt }}\n",
                1,
            ),
            ".github/workflows/backtester-ci.yml": repo_workflow_text(".github/workflows/backtester-ci.yml"),
        },
    )

    extra_action = f"""
name: Uploading composite
runs:
  using: composite
  steps:
    - name: Upload from composite
      id: upload-composite-artifact
      {upload_artifact_action}
      with:
        name: composite-artifact
        path: composite.txt
        retention-days: 1
"""
    result, output = run_verifier_main_with_extra_action(textwrap.dedent(extra_action))
    expected = (
        ".github/actions/evade/action.yml::__composite__::upload-composite-artifact "
        "missing from artifact retention policy"
    )
    if result == 0 or expected not in output:
        raise AssertionError(f"composite upload-artifact drift was silent: exit={result}, output={output!r}")
    yaml_result, yaml_output = run_verifier_main_with_extra_action(
        textwrap.dedent(extra_action),
        "action.yaml",
    )
    yaml_expected = (
        ".github/actions/evade/action.yaml::__composite__::upload-composite-artifact "
        "missing from artifact retention policy"
    )
    if yaml_result == 0 or yaml_expected not in yaml_output:
        raise AssertionError(
            f"composite action.yaml upload-artifact drift was silent: exit={yaml_result}, output={yaml_output!r}"
        )
    nested_result, nested_output = run_verifier_main_with_extra_action(
        textwrap.dedent(extra_action),
        "nested/action.yml",
    )
    nested_expected = (
        ".github/actions/evade/nested/action.yml::__composite__::upload-composite-artifact "
        "missing from artifact retention policy"
    )
    if nested_result == 0 or nested_expected not in nested_output:
        raise AssertionError(
            f"nested composite upload-artifact drift was silent: exit={nested_result}, output={nested_output!r}"
        )

    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    noncanonical_config = config_text.replace(
        '[artifact_retention.uploads.".github/workflows/ci.yml::build::upload-bolt-v2-binary"]',
        '[artifact_retention.uploads."ci.yml::build::upload-bolt-v2-binary"]',
        1,
    )
    error = runner_config_load_error(noncanonical_config, verifier=verifier)
    if "artifact_retention.uploads source must use canonical repo path" not in error:
        raise AssertionError(f"non-canonical retention source must fail config load, got: {error!r}")
    nested_workflow_config = config_text.replace(
        '[artifact_retention.uploads.".github/workflows/ci.yml::build::upload-bolt-v2-binary"]',
        '[artifact_retention.uploads.".github/workflows/archive/ci.yml::build::upload-bolt-v2-binary"]',
        1,
    )
    error = runner_config_load_error(nested_workflow_config, verifier=verifier)
    if "artifact_retention.uploads source must use canonical repo path" not in error:
        raise AssertionError(f"nested workflow retention source must fail config load, got: {error!r}")


def assert_artifact_retention_config_ref_retention_is_exact() -> None:
    verifier = load_verifier()
    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    capture_config_text = (
        REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml"
    ).read_text(encoding="utf-8")
    over_ceiling_capture_config = capture_config_text.replace(
        "retention_days = 14",
        "retention_days = 13",
        1,
    ).replace(
        "max_lookback_age_seconds = 1209600",
        "max_lookback_age_seconds = 1123200",
        1,
    )
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        ci_dir = tmp_path / "ci"
        ci_dir.mkdir()
        runner_config = ci_dir / "github-actions-runners.toml"
        runner_config.write_text(config_text, encoding="utf-8")
        (ci_dir / "chainlink-reference-fixture-capture-provenance.toml").write_text(
            over_ceiling_capture_config,
            encoding="utf-8",
        )
        verifier.DEFAULT_RUNNERS_CONFIG = runner_config
        try:
            errors = verifier.verify_artifact_retention_policy(
                {
                    ".github/workflows/ci.yml": repo_workflow_text(".github/workflows/ci.yml"),
                    ".github/workflows/backtester-ci.yml": repo_workflow_text(".github/workflows/backtester-ci.yml"),
                },
                {},
            )
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    expected = (
        ".github/workflows/ci.yml capture upload-capture-artifact artifact "
        "${{ steps.provenance.outputs.artifact_name }} retention-days 14 "
        "does not match configured retention-days 13"
    )
    if not any(expected in error for error in errors):
        raise AssertionError(f"config-ref retention must be checked exactly, got: {errors}")


def assert_ci_provenance_upload_name_comes_from_config_template() -> None:
    verifier = load_verifier()
    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    configured_config = (
        config_text.replace(
            'artifact_name_template = "ci-provenance-attempt-{run_attempt}"',
            'artifact_name_template = "proof-bundle-{run_attempt}"',
            1,
        )
    )
    configured_workflow = BASE_WORKFLOW.replace(
        "          name: ci-provenance-attempt-${{ github.run_attempt }}",
        "          name: proof-bundle-${{ github.run_attempt }}",
        1,
    )
    emit_lines = verifier.parse_jobs(configured_workflow)["ci-provenance-emit"]
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_path = write_temp_runner_config(pathlib.Path(tmp), configured_config)
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            errors = verifier.ci_provenance_emit_upload_errors(emit_lines)
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if errors:
        raise AssertionError(f"ci provenance upload name must come from config template, got: {errors}")


def assert_artifact_retention_config_refs_are_validated() -> None:
    verifier = load_verifier()
    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(encoding="utf-8")
    capture_config_text = (
        REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml"
    ).read_text(encoding="utf-8")
    binding_coverage_error = "artifact_retention.lookback_bindings must exactly cover config-ref retention uploads"
    build_lookback_binding = """[artifact_retention.lookback_bindings.build_deploy]
upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"
config_file = "ci/github-actions-runners.toml"
retention_ref = "ci_provenance.deploy.artifact_retention_days"
lookback_ref = "ci_provenance.deploy.artifact_lookback_age_seconds"

"""
    cases = {
        "artifact_retention.classes.reuse-bound.max_retention_days must be a positive integer": config_text.replace(
            "max_retention_days = 14",
            "max_retention_days = true",
            1,
        ),
        "artifact_retention.classes.reuse-bound has unexpected keys: ['allowed_refs']": config_text.replace(
            "[artifact_retention.classes.reuse-bound]\nmax_retention_days = 14",
            '[artifact_retention.classes.reuse-bound]\nmax_retention_days = 14\nallowed_refs = ["refs/heads/main"]',
            1,
        ),
        "ci_provenance.artifacts.retention_days must be a positive integer": config_text.replace(
            "retention_days = 14",
            "retention_days = true",
            1,
        ),
        "ci_provenance.deploy.artifact_retention_days must be a positive integer": config_text.replace(
            "artifact_retention_days = 3",
            "artifact_retention_days = true",
            1,
        ),
        "ci_provenance.deploy.artifact_lookback_age_seconds must be a positive integer": config_text.replace(
            "artifact_lookback_age_seconds = 259200",
            "artifact_lookback_age_seconds = true",
            1,
        ),
        "ci_provenance.deploy.artifact_lookback_age_seconds must not exceed artifact retention": config_text.replace(
            "artifact_lookback_age_seconds = 259200",
            "artifact_lookback_age_seconds = 259201",
            1,
        ),
        "ci_provenance.deploy.artifact_upload_if must match push to main deploy source policy": config_text.replace(
            'artifact_upload_if = "${{ github.event_name == \'push\' && github.ref == \'refs/heads/main\' }}"',
            'artifact_upload_if = "${{ always() }}"',
            1,
        ),
        "ci_provenance.api_limits.max_lookback_age_seconds must not exceed artifact retention": config_text.replace(
            "max_lookback_age_seconds = 1209600",
            "max_lookback_age_seconds = 1209601",
            1,
        ),
        "artifact_retention.classes.reuse-bound.max_retention_days_config_ref references missing TOML key": config_text.replace(
            "[artifact_retention.classes.reuse-bound]\nmax_retention_days = 14",
            '[artifact_retention.classes.reuse-bound]\nmax_retention_days_config_file = "ci/github-actions-runners.toml"\nmax_retention_days_config_ref = "ci_provenance.artifacts.missing"',
            1,
        ),
        "artifact_retention.classes.reuse-bound must define exactly one max retention source": config_text.replace(
            "[artifact_retention.classes.reuse-bound]\nmax_retention_days = 14",
            '[artifact_retention.classes.reuse-bound]\nmax_retention_days = 14\nmax_retention_days_config_file = "ci/github-actions-runners.toml"\nmax_retention_days_config_ref = "ci_provenance.artifacts.retention_days"',
            1,
        ),
        "artifact_retention.classes has unused classes: ['unused']": config_text.replace(
            "[artifact_retention.classes.reuse-bound]\n",
            "[artifact_retention.classes.unused]\nmax_retention_days = 1\n\n[artifact_retention.classes.reuse-bound]\n",
            1,
        ),
        "artifact_name_template_vars_config_ref references missing TOML key": config_text.replace(
            'artifact_name_template_vars_config_ref = "ci_provenance.artifact_name_template_vars"',
            'artifact_name_template_vars_config_ref = "ci_provenance.missing_artifact_name_template_vars"',
        ),
        "ci_provenance.artifact_name_template missing template vars": config_text.replace(
            "run_attempt = \"${{ github.run_attempt }}\"",
            "run_number = \"${{ github.run_number }}\"",
        ),
        "ci_provenance.artifact_name_template has unused template vars": config_text.replace(
            "run_attempt = \"${{ github.run_attempt }}\"",
            "run_attempt = \"${{ github.run_attempt }}\"\nrun_number = \"${{ github.run_number }}\"",
        ),
        "ci_provenance.artifact_name_template_vars values must be non-empty strings": config_text.replace(
            "run_attempt = \"${{ github.run_attempt }}\"",
            "run_attempt = true",
        ),
        "has partial artifact name source; missing ['artifact_name_config_file']": config_text.replace(
            'artifact_name_config_file = "ci/github-actions-runners.toml"\n'
            'artifact_name_config_ref = "ci_provenance.deploy.artifact_name"',
            'artifact_name_config_ref = "ci_provenance.deploy.artifact_name"',
            1,
        ),
        "has partial artifact name source; missing ['artifact_name_template_config_file']": config_text.replace(
            'artifact_name_template_config_file = "ci/github-actions-runners.toml"\n'
            'artifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
            'artifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
            1,
        ),
        "must define exactly one artifact name source": config_text.replace(
            'artifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
            'artifact_name = "ci-provenance-attempt-${{ github.run_attempt }}"\nartifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
            1,
        ),
        "has unexpected keys: ['artifact_name_prefix']": config_text.replace(
            'artifact_name_template_vars_config_ref = "backtester.issue_789.artifact_name_template_vars"',
            'artifact_name_template_vars_config_ref = "backtester.issue_789.artifact_name_template_vars"\nartifact_name_prefix = "issue-789-first-pl-"',
            1,
        ),
        "has unexpected keys: ['retention_days_expression']": config_text.replace(
            'artifact_class = "capture-provenance"\nretention_days_config_file = "ci/chainlink-reference-fixture-capture-provenance.toml"',
            'artifact_class = "capture-provenance"\nretention_days_expression = "${{ steps.provenance.outputs.retention_days }}"\nretention_days_config_file = "ci/chainlink-reference-fixture-capture-provenance.toml"',
            1,
        ),
        "retention_days_config_file must be a non-empty string": config_text.replace(
            'retention_days_config_file = "ci/chainlink-reference-fixture-capture-provenance.toml"',
            'retention_days_config_file = "   "',
            1,
        ),
        "retention_days_config_ref must be a non-empty string": config_text.replace(
            '\nretention_days_config_ref = "ci_provenance.artifacts.retention_days"',
            '\nretention_days_config_ref = "   "',
            1,
        ),
        "retention_days_config_ref references missing TOML key": config_text.replace(
            '\nretention_days_config_ref = "ci_provenance.artifacts.retention_days"',
            '\nretention_days_config_ref = "ci_provenance.artifacts.missing"',
            1,
        ),
        "required_if_config_ref references missing TOML key": config_text.replace(
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
            'required_if_config_ref = "ci_provenance.deploy.missing_artifact_upload_if"',
            1,
        ),
        "has partial required if source; missing ['required_if_config_file']": config_text.replace(
            'required_if_config_file = "ci/github-actions-runners.toml"\n'
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
            1,
        ),
        "deployable uploads must define required_if": config_text.replace(
            'required_if_config_file = "ci/github-actions-runners.toml"\n'
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"\n',
            "",
            1,
        ),
        "artifact_class must be deployable": config_text.replace(
            'artifact_class = "deployable"',
            'artifact_class = "reuse-bound"',
            1,
        ),
        "required_if_config_ref must be ci_provenance.deploy.artifact_upload_if": config_text.replace(
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
            'required_if_config_ref = "storage_audit.cleanup_feasibility_alert.workflow.json_artifact_upload_if"',
            1,
        ),
        "artifact_retention.lookback_bindings.build_deploy.upload must reference a configured upload": config_text.replace(
            'upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"',
            'upload = ".github/workflows/ci.yml::build::missing-upload"',
            1,
        ),
        "artifact_retention.lookback_bindings.build_deploy must match the upload retention source": config_text.replace(
            'retention_ref = "ci_provenance.deploy.artifact_retention_days"',
            'retention_ref = "ci_provenance.api_limits.max_lookback_age_seconds"',
            1,
        ),
        "artifact_retention.lookback_bindings.build_deploy.lookback_ref must be ci_provenance.deploy.artifact_lookback_age_seconds": config_text.replace(
            'lookback_ref = "ci_provenance.deploy.artifact_lookback_age_seconds"',
            'lookback_ref = "ci_provenance.merge_readiness.poll_seconds"',
            1,
        ),
        "artifact_retention.lookback_bindings.renamed_deploy.lookback_ref must be ci_provenance.deploy.artifact_lookback_age_seconds": config_text.replace(
            build_lookback_binding,
            """[artifact_retention.lookback_bindings.renamed_deploy]
upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"
config_file = "ci/github-actions-runners.toml"
retention_ref = "ci_provenance.deploy.artifact_retention_days"
lookback_ref = "ci_provenance.merge_readiness.poll_seconds"

""",
            1,
        ),
        binding_coverage_error: config_text.replace(
            build_lookback_binding,
            "",
            1,
        ),
    }
    additional_cases = [
        (
            binding_coverage_error,
            config_text
        + """
[artifact_retention.lookback_bindings.duplicate_build]
upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"
config_file = "ci/github-actions-runners.toml"
retention_ref = "ci_provenance.deploy.artifact_retention_days"
lookback_ref = "ci_provenance.deploy.artifact_lookback_age_seconds"
""",
        ),
    ]
    with tempfile.TemporaryDirectory() as tmp:
        for expected, mutated_config in (*cases.items(), *additional_cases):
            config_path = write_temp_runner_config(pathlib.Path(tmp), mutated_config)
            try:
                verifier.load_github_actions_runners_config(config_path)
            except ValueError as exc:
                if expected not in str(exc):
                    raise AssertionError(f"expected {expected!r}, got {exc}") from exc
            else:
                raise AssertionError(f"config mutation did not fail: {expected}")
        capture_lookback_config = capture_config_text.replace(
            "max_lookback_age_seconds = 1209600",
            "max_lookback_age_seconds = 1209601",
            1,
        )
        config_path = write_temp_runner_config(pathlib.Path(tmp), config_text)
        config_path.with_name("chainlink-reference-fixture-capture-provenance.toml").write_text(
            capture_lookback_config,
            encoding="utf-8",
        )
        try:
            verifier.load_github_actions_runners_config(config_path)
        except ValueError as exc:
            expected = "artifact_retention.lookback_bindings.capture: max lookback age must not exceed artifact retention"
            if expected not in str(exc):
                raise AssertionError(
                    f"expected binding-level lookback error {expected!r}, got {exc!r}"
                ) from exc
        else:
            raise AssertionError(
                "artifact retention binding must reject lookback windows longer than external retention"
            )




def assert_v6_red_exact_head_governance_inputs_are_cache_keyed() -> None:
    governed_workflow = workflow_with_exact_head_governance_cache_inputs(BASE_WORKFLOW)
    assert_error("managed target cache keys must use Rust-relevant inputs only", governed_workflow)




def assert_v6_red_workflow_policy_gaps() -> None:
    checks = [
        assert_v6_red_exact_head_governance_inputs_are_cache_keyed,
        assert_backtester_bvs_target_cache_removal_and_artifact_identity_contract,
        assert_backtester_primary_sccache_contract,
        assert_v6_red_backtester_gate_fails_when_detect_fails,
        assert_v6_red_backtester_test_uses_nextest_archive,
        assert_cache_as_same_run_transport_is_banned,
        assert_v6_red_backtester_nextest_archive_recipes_absolutize_paths,
    ]
    failures: list[str] = []
    for check in checks:
        try:
            check()
        except AssertionError as exc:
            failures.append(f"{check.__name__}: {exc}")
    if failures:
        raise AssertionError("v6 RED workflow policy coverage failures: " + " | ".join(failures))


def assert_backtester_bvs_target_cache_removal_and_artifact_identity_contract() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")

    def errors(mutated: str) -> list[str]:
        return verifier.backtester_managed_target_cache_errors(
            ".github/workflows/backtester-ci.yml", mutated
        ) + verifier.backtester_test_shard_errors(
            ".github/workflows/backtester-ci.yml", mutated
        )

    clean = [error for error in errors(workflow) if "BVS_TEST_ARCHIVE_JOB_SHA256" not in error]
    if clean:
        raise AssertionError(f"real backtester workflow violates BVS cache-removal contract: {clean}")

    target_input_forms = (
        "          path: crates/backtesting-vertical-slice/target",
        "          path: ./crates/backtesting-vertical-slice/target/",
        "          path: ${{steps.crate_target.outputs.dir}}",
        "          path: [crates/backtesting-vertical-slice/target]",
        "          path: |\n            crates/backtesting-vertical-slice/target",
    )
    for rendered_input in target_input_forms:
        block = (
            "      - uses: buildjet/cache@example\n"
            "        with:\n"
            f"{rendered_input}\n"
        ).splitlines()
        if not verifier.block_references_bvs_target_input(block):
            raise AssertionError(f"BVS target input normalization missed {rendered_input!r}")
    allowed_subpath = (
        "      - uses: actions/upload-artifact@example\n"
        "        with:\n"
        "          path: crates/backtesting-vertical-slice/target/issue-789-first-pl/result.json\n"
    ).splitlines()
    if verifier.block_references_bvs_target_input(allowed_subpath):
        raise AssertionError("BVS target input normalization rejected the issue-789 artifact subpath")

    injected_target_key = workflow + "\n# managed-target-bvs-v99-forbidden\n"
    if not any("must not contain managed-target-bvs-" in error for error in errors(injected_target_key)):
        raise AssertionError("managed-target-bvs injection was not rejected")

    injected_bootstrap = workflow + "\n# bootstrap-${GITHUB_SHA}\n"
    if not any("must not contain bootstrap-${GITHUB_SHA}" in error for error in errors(injected_bootstrap)):
        raise AssertionError("bootstrap SHA identity injection was not rejected")

    missing_digest = replace_once(
        workflow,
        "python3 scripts/ci_input_sets.py hash backtester_cache",
        "python3 scripts/ci_input_sets.py hash backtester_detect",
    )
    if not any("must compute one full-source BVS artifact digest" in error for error in errors(missing_digest)):
        raise AssertionError("missing full-source artifact digest was not rejected")

    cross_wired = replace_once(
        workflow,
        "${{ steps.bvs_artifact_inputs.outputs.digest }}",
        "${{ github.sha }}",
    )
    if not any("must consistently use the BVS artifact digest" in error for error in errors(cross_wired)):
        raise AssertionError("cross-wired artifact identity was not rejected")

    injected_restore = replace_once(
        workflow,
        "      - name: Install cargo-nextest\n",
        "      - name: Restore forbidden BVS target\n"
        "        uses: actions/cache/restore@example\n"
        "        with:\n"
        "          path: ${{ steps.crate_target.outputs.dir }}\n"
        "          key: forbidden\n"
        "      - name: Install cargo-nextest\n",
    )
    if not any("must not use Actions cache restore/save in a BVS workflow" in error for error in errors(injected_restore)):
        raise AssertionError("BVS managed-target restore injection was not rejected")

    sneaky_clippy_restore = replace_once(
        workflow,
        "      - name: clippy\n",
        "      - name: Restore disguised BVS whole target\n"
        "        uses: actions/cache/restore@example\n"
        "        with:\n"
        "          path: crates/backtesting-vertical-slice/target\n"
        "          key: sneaky-bvs-whole-target-${{ github.sha }}\n"
        "          restore-keys: sneaky-bvs-whole-target-\n"
        "      - name: clippy\n",
    )
    if not any("must not use Actions cache restore/save in a BVS workflow" in error for error in errors(sneaky_clippy_restore)):
        raise AssertionError("disguised BVS clippy target restore was not rejected")

    combined_cache = replace_once(
        workflow,
        "      - name: clippy\n",
        "      - name: Combined forbidden BVS target cache\n"
        "        uses: actions/cache@example\n"
        "        with:\n"
        "          path: crates/backtesting-vertical-slice/target\n"
        "          key: sneaky-bvs-whole-target-${{ github.sha }}\n"
        "      - name: clippy\n",
    )
    if not any("must not use Actions cache restore/save in a BVS workflow" in error for error in errors(combined_cache)):
        raise AssertionError("combined BVS target cache action was not rejected")

    third_party_cache = replace_once(
        workflow,
        "      - name: clippy\n",
        "      - name: Third-party forbidden BVS target cache\n"
        "        uses: buildjet/cache@example\n"
        "        with:\n"
        "          path: crates/backtesting-vertical-slice/target\n"
        "          key: novel-bvs-target-${{ github.sha }}\n"
        "      - name: clippy\n",
    )
    if not any("must not pass the BVS target directory to an action" in error for error in errors(third_party_cache)):
        raise AssertionError("third-party BVS target cache action was not rejected")

    for label, rendered_inputs in (
        (
            "spaced with key",
            "        with :\n"
            "          path: |\n"
            "            crates/backtesting-vertical-slice/target\n",
        ),
        (
            "quoted with key",
            "        \"with\":\n"
            "          path: ./crates/backtesting-vertical-slice/target/\n",
        ),
        (
            "single-quoted with key",
            "        'with':\n"
            "          path: ${{steps.crate_target.outputs.dir}}\n",
        ),
        (
            "flow-list target path",
            "        with:\n"
            "          path: [allowed/path, 'crates/backtesting-vertical-slice/target']\n",
        ),
    ):
        normalized_workflow_cache = replace_once(
            workflow,
            "      - name: clippy\n",
            "      - name: Normalized forbidden BVS target cache\n"
            "        uses: buildjet/cache@example\n"
            f"{rendered_inputs}"
            "      - name: clippy\n",
        )
        if not any(
            "backtester clippy must not pass the BVS target directory to an action" in error
            for error in errors(normalized_workflow_cache)
        ):
            raise AssertionError(f"{label} did not emit the BVS target-directory diagnostic")

    issue_789_third_party_cache = replace_once_after(
        workflow,
        "  issue_789:\n    name: bvs-test issue-789\n",
        "      - name: Build issue #789 lib archive\n",
        "      - name: Third-party forbidden BVS target cache\n"
        "        uses: buildjet/cache@example\n"
        "        with:\n"
        "          path: ${{ steps.crate_target.outputs.dir }}\n"
        "          key: novel-bvs-target-${{ github.sha }}\n"
        "      - name: Build issue #789 lib archive\n",
    )
    if not any(
        "backtester issue_789 must not pass the BVS target directory to an action" in error
        for error in errors(issue_789_third_party_cache)
    ):
        raise AssertionError("issue-789 third-party BVS target cache action was not rejected")

    issue_789_block_scalar_cache = replace_once_after(
        workflow,
        "  issue_789:\n    name: bvs-test issue-789\n",
        "      - name: Build issue #789 lib archive\n",
        "      - name: Block-scalar third-party BVS target cache\n"
        "        uses: buildjet/cache@example\n"
        "        with:\n"
        "          path: |\n"
        "            ${{steps.crate_target.outputs.dir}}\n"
        "      - name: Build issue #789 lib archive\n",
    )
    if not any(
        "backtester issue_789 must not pass the BVS target directory to an action" in error
        for error in errors(issue_789_block_scalar_cache)
    ):
        raise AssertionError("issue-789 block-scalar BVS target cache action was not rejected")

    obsolete_clippy_digest = replace_once(
        workflow,
        "      - name: clippy\n",
        "      - name: Compute obsolete BVS cache digest\n"
        "        id: bvs_cache_inputs\n"
        "        run: python3 scripts/ci_input_sets.py hash backtester_cache\n"
        "      - name: clippy\n",
    )
    if not any("must not contain bvs_cache_inputs" in error for error in errors(obsolete_clippy_digest)):
        raise AssertionError("obsolete clippy BVS digest step was not rejected")

    for field, replacement, expected in (
        ("          cache-targets: false\n", "          cache-targets: true\n", "must set cache-targets: false"),
        ("          cache-bin: false\n", "          cache-bin: true\n", "must set cache-bin: false"),
        (
            "          cache-targets: false\n",
            '          cache-targets: false\n          "cache-directories": crates/backtesting-vertical-slice/target\n',
            "must not set cache-directories",
        ),
    ):
        mutated = replace_once(workflow, field, replacement)
        if not any(expected in error for error in errors(mutated)):
            raise AssertionError(f"BVS registry cache mutation was not rejected with {expected!r}")


def assert_backtester_primary_sccache_contract() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")

    def errors(mutated: str) -> list[str]:
        return [
            error
            for error in (
                verifier.backtester_managed_target_cache_errors(
                    ".github/workflows/backtester-ci.yml", mutated
                )
                + verifier.backtester_test_shard_errors(
                    ".github/workflows/backtester-ci.yml", mutated
                )
            )
            if "BVS_TEST_ARCHIVE_JOB_SHA256" not in error
        ]

    required = (
        "      - name: Setup governed sccache\n",
        "          active: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && 'true' || 'false' }}\n",
        "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
        "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
        "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}",
        "      - name: Print sccache stats\n",
    )
    missing = [fragment for fragment in required if fragment not in workflow]
    if missing:
        raise AssertionError(f"primary BVS sccache contract missing: {missing}")

    mutations = [
        (
            replace_once(workflow, "      - name: Setup governed sccache\n", "      - name: Omit governed sccache\n"),
            "governed sccache setup must include",
        ),
        (
            replace_once(
            workflow,
            "          active: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && 'true' || 'false' }}\n",
            '          active: "true"\n',
            ),
            "governed sccache setup must include",
        ),
        (
            replace_once(workflow, "          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n", ""),
            "governed sccache setup must include",
        ),
        (
            replace_once(workflow, "          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n", ""),
            "governed sccache setup must include",
        ),
        (
            replace_once(
            workflow,
            "BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}",
            'BOLT_RUST_VERIFICATION_SCCACHE: "1"',
            ),
            "compile must opt into managed sccache conditionally",
        ),
        (
            replace_once(workflow, "      - name: Print sccache stats\n", "      - name: Omit sccache stats\n"),
            "must print governed sccache stats after both compile opportunities",
        ),
    ]
    for step_name in ("Build BVS nextest archive", "Build BVS binary sidecars"):
        for profile_env in ("CARGO_PROFILE_TEST_DEBUG", "CARGO_PROFILE_DEV_DEBUG"):
            mutations.append(
                (
                    replace_once_after(
                        workflow,
                        f"      - name: {step_name}\n",
                        f'          {profile_env}: "0"\n',
                        "",
                    ),
                    f"compile must preserve {profile_env}",
                )
            )
        for forbidden_env in (
            "RUSTFLAGS",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_INCREMENTAL",
        ):
            mutations.append(
                (
                    replace_once_after(
                        workflow,
                        f"      - name: {step_name}\n",
                        "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n",
                        "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n"
                        f'          {forbidden_env}: "forbidden"\n',
                    ),
                    f"compile must not set {forbidden_env}",
                )
            )
            for rendered_key in (f'"{forbidden_env}"', f"{forbidden_env} "):
                mutations.append(
                    (
                        replace_once_after(
                            workflow,
                            f"      - name: {step_name}\n",
                            "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n",
                            "          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}\n"
                            f'          {rendered_key}: "forbidden"\n',
                        ),
                        f"compile must not set {forbidden_env}",
                    )
                )
    for forbidden_env in (
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_INCREMENTAL",
    ):
        for rendered_env in (
            "    env:\n" + f'      {forbidden_env}: "forbidden"\n',
            "    env:\n" + f'      "{forbidden_env}": "forbidden"\n',
            "    env:\n" + f'      {forbidden_env} : "forbidden"\n',
            "    env: {" + f'{forbidden_env}: "forbidden"' + "}\n",
        ):
            mutations.append(
                (
                    replace_once_after(
                        workflow,
                        "  clippy:\n",
                        "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n",
                        "    runs-on: ${{ vars.CI_RUNNER_MANAGED_HEAVY }}\n" + rendered_env,
                    ),
                    f"backtester clippy must not set {forbidden_env}",
                )
            )
        for rendered_key in (forbidden_env, f'"{forbidden_env}"', f"{forbidden_env} "):
            mutations.append(
                (
                    replace_once_after(
                        workflow,
                        "  test-archive:\n",
                        "    env:\n",
                        "    env:\n" + f'      {rendered_key}: "forbidden"\n',
                    ),
                    f"backtester test-archive must not set {forbidden_env}",
                )
            )
        mutations.append(
            (
                replace_once_after(
                    workflow,
                    "  issue_789:\n    name: bvs-test issue-789\n",
                    "    env:\n",
                    "    env:\n" + f'      {forbidden_env}: "forbidden"\n',
                ),
                f"backtester issue_789 must not set {forbidden_env}",
            )
        )
    for mutation_index, (mutated, expected_substring) in enumerate(mutations):
        mutation_errors = errors(mutated)
        if not any(expected_substring in error for error in mutation_errors):
            raise AssertionError(
                f"primary BVS sccache mutation {mutation_index} did not emit {expected_substring!r}: "
                f"{mutation_errors!r}"
            )


def assert_v6_red_backtester_gate_fails_when_detect_fails() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")
    bad = replace_once(
        workflow,
        "--job detect=${{ needs.detect.result }}",
        "--job detect=${{ needs.detect.outputs.bvs_changed }}",
    )
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    assert any("backtester-gate shared verdict call must include needs.detect.result" in error for error in errors), errors
    assert any(
        "backtester iteration policy shared gate call must include --job detect=${{ needs.detect.result }}" in error
        for error in errors
    ), errors


def assert_v6_red_backtester_test_uses_nextest_archive() -> None:
    verifier = load_verifier()
    real_backtester_workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")
    real_backtester_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": real_backtester_workflow}
    )
    if real_backtester_errors:
        raise AssertionError(f"real backtester-ci.yml must be clean, got: {real_backtester_errors}")
    bvs_just_version_bump = replace_once(
        real_backtester_workflow,
        '  JUST_VERSION: "1.49.0"\n',
        '  JUST_VERSION: "1.49.1"\n',
    )
    bvs_just_version_bump_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_just_version_bump}
    )
    if bvs_just_version_bump_errors:
        raise AssertionError(f"backtester JUST_VERSION bump must be boundary-clean, got: {bvs_just_version_bump_errors}")
    bvs_job_body_edit = replace_once_after(
        real_backtester_workflow,
        "  test-archive:",
        '          echo "BVS nextest archive S3 save outcome:',
        '          echo "BVS nextest archive S3 save outcome: probe ',
    )
    bvs_job_body_boundary_errors = verifier.partition_workflow_boundary_errors(
        bvs_job_body_edit,
        "backtester-ci.yml",
    )
    if bvs_job_body_boundary_errors:
        raise AssertionError(f"backtester job body edit must not trip top-level boundary, got: {bvs_job_body_boundary_errors}")
    bvs_duplicate_env_path = replace_once(
        real_backtester_workflow,
        "permissions:\n",
        "env:\n  PATH: /tmp/shim\npermissions:\n",
    )
    bvs_duplicate_env_path_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_duplicate_env_path}
    )
    assert any(
        "backtester-ci.yml duplicate top-level key 'env'" in error
        for error in bvs_duplicate_env_path_errors
    ), bvs_duplicate_env_path_errors
    bvs_duplicate_jobs = replace_once(
        real_backtester_workflow,
        "permissions:\n",
        "jobs: {}\npermissions:\n",
    )
    bvs_duplicate_jobs_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_duplicate_jobs}
    )
    assert any(
        "backtester-ci.yml duplicate top-level key 'jobs'" in error
        for error in bvs_duplicate_jobs_errors
    ), bvs_duplicate_jobs_errors
    bad = """jobs:
  test-archive:
    name: bvs-test archive
  test:
    name: bvs-test
    steps:
      - name: test
        run: just bte-test --partition "count:${{ matrix.shard }}/4"
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    assert any("backtester bvs-test must not run direct per-shard target builds" in error for error in errors), errors
    assert any("backtester bvs-test must run partitions in the archive producer" in error for error in errors), errors
    assert any("backtester bvs-test must define manual issue-789 diagnostic job" in error for error in errors), errors

    good = """jobs:
  test-archive:
    name: bvs-test archive
    needs: [ci-policy, detect, fmt]
    outputs:
      bvs_nextest_archive_cache_save_outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}
      bvs_bin_sidecars_cache_save_outcome: ${{ steps.bvs-bin-sidecars-cache-save.outputs.save-status || (steps.bvs-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}
    env:
      BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst
      BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz
      BVS_NEXTEST_SHARDS: "4"
      NEXTEST_ARTIFACT_CACHE_ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}
      NEXTEST_ARTIFACT_CACHE_BUCKET: ${{ vars.CI_SCCACHE_BUCKET }}
      NEXTEST_ARTIFACT_CACHE_REGION: ${{ vars.CI_SCCACHE_REGION }}
      NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}
    steps:
      - name: Compute BVS artifact input hash
        id: bvs_artifact_inputs
        run: |
          echo "digest=$(python3 scripts/ci_input_sets.py hash backtester_cache)" >> "$GITHUB_OUTPUT"
      - name: Resolve BVS nextest artifact cache eligibility
        id: bvs-nextest-artifact-cache
        continue-on-error: true
        env:
          ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}
          ROLE_ARN: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}
          PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}
          BUCKET: ${{ vars.CI_SCCACHE_BUCKET }}
          REGION: ${{ vars.CI_SCCACHE_REGION }}
          PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}
        run: |
          cache_mode=""
          role_arn=""
          if [[ "$GITHUB_EVENT_NAME" == "push" && "$GITHUB_REF" == "refs/heads/main" ]]; then
            cache_mode="read_write"
            role_arn="$ROLE_ARN"
          elif [[ "$GITHUB_EVENT_NAME" == "pull_request" || "$GITHUB_EVENT_NAME" == "merge_group" || "$GITHUB_EVENT_NAME" == "workflow_dispatch" ]]; then
            cache_mode="read_only"
            role_arn="$PR_READONLY_ROLE_ARN"
          fi
          vars_present=true
          [[ "$ENABLED" == "true" && -n "$role_arn" && -n "$BUCKET" && -n "$REGION" && -n "$PREFIX" ]] || vars_present=false
          if [[ -n "$cache_mode" && "$vars_present" == "true" ]]; then
            echo "eligible=true" >> "$GITHUB_OUTPUT"
          else
            echo "eligible=false" >> "$GITHUB_OUTPUT"
          fi
          echo "role_arn=$role_arn" >> "$GITHUB_OUTPUT"
          echo "cache_mode=$cache_mode" >> "$GITHUB_OUTPUT"
      - name: Configure AWS credentials for BVS nextest artifact cache
        id: bvs-nextest-artifact-cache-aws
        if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true'
        continue-on-error: true
        with:
          role-to-assume: ${{ steps.bvs-nextest-artifact-cache.outputs.role_arn }}
      - name: Restore BVS nextest archive from S3
        id: bvs-nextest-archive-cache
        if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success'
        env:
          CACHE_KEY: bvs-nextest-archive-v4-${{ runner.os }}-${{ runner.arch }}-test-profile-discovered-targets-shards-4-${{ steps.bvs_artifact_inputs.outputs.digest }}
          DIGEST: ${{ steps.bvs_artifact_inputs.outputs.digest }}
        run: |
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/bvs-nextest-archive/${CACHE_KEY}.tar.zst"
          metadata_digest="$(aws s3api head-object --bucket "$NEXTEST_ARTIFACT_CACHE_BUCKET" --key "$object_key" --query 'Metadata."nextest-digest"' --output text 2>/dev/null)"
          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::BVS nextest archive S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
          aws s3 cp "$uri" "$BVS_NEXTEST_ARCHIVE_PATH" --only-show-errors || true
          echo "cache-hit=false" >> "$GITHUB_OUTPUT"
          echo "restore-result=miss" >> "$GITHUB_OUTPUT"
          echo "restore-reason=fixture-miss" >> "$GITHUB_OUTPUT"
          exit 0
      - name: Restore BVS binary sidecars from S3
        id: bvs-bin-sidecars-cache
        if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success'
        env:
          CACHE_KEY: bvs-bin-sidecars-v4-${{ runner.os }}-${{ runner.arch }}-test-profile-discovered-cargo-bin-exe-${{ steps.bvs_artifact_inputs.outputs.digest }}
          DIGEST: ${{ steps.bvs_artifact_inputs.outputs.digest }}
        run: |
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/bvs-bin-sidecars/${CACHE_KEY}.tar.gz"
          metadata_digest="$(aws s3api head-object --bucket "$NEXTEST_ARTIFACT_CACHE_BUCKET" --key "$object_key" --query 'Metadata."nextest-digest"' --output text 2>/dev/null)"
          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::BVS binary sidecar S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
          aws s3 cp "$uri" "$BVS_BIN_SIDECARS_PATH" --only-show-errors || true
          echo "cache-hit=false" >> "$GITHUB_OUTPUT"
          echo "restore-result=miss" >> "$GITHUB_OUTPUT"
          echo "restore-reason=fixture-miss" >> "$GITHUB_OUTPUT"
          exit 0
      - name: Summarize BVS nextest archive S3 state
        if: always()
        shell: bash
        env:
          S3_ELIGIBLE: ${{ steps.bvs-nextest-artifact-cache.outputs.eligible || 'false' }}
          S3_CACHE_MODE: ${{ steps.bvs-nextest-artifact-cache.outputs.cache_mode || 'none' }}
          S3_AWS_OUTCOME: ${{ steps.bvs-nextest-artifact-cache-aws.outcome }}
          BVS_NEXTEST_RESTORE_OUTCOME: ${{ steps.bvs-nextest-archive-cache.outcome }}
          BVS_NEXTEST_RESTORE_HIT: ${{ steps.bvs-nextest-archive-cache.outputs.cache-hit || '' }}
          BVS_NEXTEST_RESTORE_RESULT: ${{ steps.bvs-nextest-archive-cache.outputs.restore-result || '' }}
          BVS_NEXTEST_RESTORE_REASON: ${{ steps.bvs-nextest-archive-cache.outputs.restore-reason || '' }}
          BVS_SIDECAR_RESTORE_OUTCOME: ${{ steps.bvs-bin-sidecars-cache.outcome }}
          BVS_SIDECAR_RESTORE_HIT: ${{ steps.bvs-bin-sidecars-cache.outputs.cache-hit || '' }}
          BVS_SIDECAR_RESTORE_RESULT: ${{ steps.bvs-bin-sidecars-cache.outputs.restore-result || '' }}
          BVS_SIDECAR_RESTORE_REASON: ${{ steps.bvs-bin-sidecars-cache.outputs.restore-reason || '' }}
        run: |
          restore_state() {
            local eligible="$1" aws="$2" outcome="$3" result="$4" hit="$5"
            if [[ "$eligible" != "true" ]]; then echo "ineligible"; return; fi
            if [[ "$aws" != "success" ]]; then echo "skipped"; return; fi
            if [[ "$outcome" == "failure" || "$result" == "error" ]]; then echo "error"; return; fi
            if [[ "$result" == "hit" || "$hit" == "true" ]]; then echo "hit"; return; fi
            if [[ "$result" == "miss" || "$hit" == "false" ]]; then echo "miss"; return; fi
            echo "skipped"
          }
          restore_reason() {
            local eligible="$1" aws="$2" outcome="$3" reason="$4"
            if [[ "$eligible" != "true" ]]; then echo "eligible=false"; return; fi
            if [[ "$aws" != "success" ]]; then echo "aws=${aws:-skipped}"; return; fi
            if [[ -n "$reason" ]]; then echo "$reason"; return; fi
            echo "outcome=${outcome:-skipped}"
          }
          bvs_nextest_restore="$(restore_state "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$BVS_NEXTEST_RESTORE_OUTCOME" "$BVS_NEXTEST_RESTORE_RESULT" "$BVS_NEXTEST_RESTORE_HIT")"
          bvs_nextest_reason="$(restore_reason "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$BVS_NEXTEST_RESTORE_OUTCOME" "$BVS_NEXTEST_RESTORE_REASON")"
          bvs_sidecar_restore="$(restore_state "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$BVS_SIDECAR_RESTORE_OUTCOME" "$BVS_SIDECAR_RESTORE_RESULT" "$BVS_SIDECAR_RESTORE_HIT")"
          bvs_sidecar_reason="$(restore_reason "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$BVS_SIDECAR_RESTORE_OUTCOME" "$BVS_SIDECAR_RESTORE_REASON")"
          {
            echo "BVS nextest archive S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_nextest_restore} reason=${bvs_nextest_reason}"
            echo "BVS binary sidecars S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_sidecar_restore} reason=${bvs_sidecar_reason}"
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Resolve crate managed target dir
        id: crate_target
      - uses: Swatinem/rust-cache@example
        with:
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Setup governed sccache
        id: sccache
        uses: ./.github/actions/sccache-setup
        with:
          active: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && 'true' || 'false' }}
          role-arn: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}
          write-role-arn: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}
      - name: Build BVS nextest archive
        if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_TEST_DEBUG: "0"
          CARGO_PROFILE_DEV_DEBUG: "0"
          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}
        run: |
          mapfile -t archive_args < <(python3 scripts/rust_test_targets.py archive-args --crate crates/backtesting-vertical-slice)
          printf 'BVS archive args: %s\\n' "${archive_args[*]}"
          just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH" "${archive_args[@]}"
      - name: Save BVS nextest archive
        id: bvs-nextest-archive-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.bvs-nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success' && steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' }}
        continue-on-error: true
        run: |
          if [[ ! -s "$BVS_NEXTEST_ARCHIVE_PATH" ]]; then
            echo "save-status=skipped" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if aws s3 cp "$BVS_NEXTEST_ARCHIVE_PATH" "$uri" --only-show-errors; then
            echo "save-status=success" >> "$GITHUB_OUTPUT"
          else
            echo "save-status=failed" >> "$GITHUB_OUTPUT"
            exit 1
          fi
      - name: Build BVS binary sidecars
        if: steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_TEST_DEBUG: "0"
          CARGO_PROFILE_DEV_DEBUG: "0"
          BOLT_RUST_VERIFICATION_SCCACHE: ${{ steps.sccache.outputs.enabled == 'true' && '1' || '0' }}
        run: |
          python3 scripts/rust_test_targets.py sidecars --crate crates/backtesting-vertical-slice
          python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo crates/backtesting-vertical-slice -- "${cargo_args[@]}"
          tar --null -czf "$GITHUB_WORKSPACE/$BVS_BIN_SIDECARS_PATH" --files-from -
      - name: Print sccache stats
        if: always()
        uses: ./.github/actions/sccache-stats
        with:
          enabled: ${{ steps.sccache.outputs.enabled || 'false' }}
      - name: Save BVS binary sidecars
        id: bvs-bin-sidecars-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.bvs-nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success' && steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true' }}
        continue-on-error: true
        run: |
          if [[ ! -s "$BVS_BIN_SIDECARS_PATH" ]]; then
            echo "save-status=skipped" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          if aws s3 cp "$BVS_BIN_SIDECARS_PATH" "$uri" --only-show-errors; then
            echo "save-status=success" >> "$GITHUB_OUTPUT"
          else
            echo "save-status=failed" >> "$GITHUB_OUTPUT"
            exit 1
          fi
      - name: Summarize BVS S3 save outcomes
        if: always()
        run: |
          echo "BVS nextest archive S3 save outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}" >> "$GITHUB_STEP_SUMMARY"
          echo "BVS binary sidecars S3 save outcome: ${{ steps.bvs-bin-sidecars-cache-save.outputs.save-status || (steps.bvs-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}" >> "$GITHUB_STEP_SUMMARY"
      - name: Require BVS local payload
        run: |
          test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty"; exit 1; }
          test -s "$BVS_BIN_SIDECARS_PATH" || { echo "BVS binary sidecars missing or empty"; exit 1; }
          stat -c 'bvs-payload-size %n %s' "$BVS_NEXTEST_ARCHIVE_PATH" "$BVS_BIN_SIDECARS_PATH"
      - name: Extract BVS binary sidecars
        run: tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"
      - name: List scoped BVS archive tests
        run: nextest list --archive-file "$GITHUB_WORKSPACE/$BVS_NEXTEST_ARCHIVE_PATH"
      - name: Setup BVS MinIO S3 smoke
        uses: ./.github/actions/setup-bvs-minio-s3-smoke
      - name: test
        shell: bash
        run: |
          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"
          failures=0
          for shard in $(seq 1 "$BVS_NEXTEST_SHARDS"); do
            echo "::group::bvs-test partition ${shard}/${BVS_NEXTEST_SHARDS}"
            partition_log="$RUNNER_TEMP/bvs-nextest-archive-partition-${shard}.log"
            set +e
            just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${shard}/${BVS_NEXTEST_SHARDS}" -- --skip issue_789_first_real_free_data_taker_pl --skip backtesting_vertical_slice_s3_catalog_smoke 2>&1 | tee "$partition_log"
            rc="${PIPESTATUS[0]}"
            set -e
            if [[ "$rc" -ne 0 ]]; then
              failures=$((failures + 1))
              echo "::error title=BVS nextest archive partition failed::shard=${shard}/${BVS_NEXTEST_SHARDS} exit=${rc}"
              echo "last relevant log lines for BVS nextest archive partition ${shard}/${BVS_NEXTEST_SHARDS}:"
              tail -80 "$partition_log"
            fi
            echo "::endgroup::"
          done
          if [[ "$failures" != "0" ]]; then
            echo "$failures BVS test partitions failed"
            exit 1
          fi
  issue_789:
    name: bvs-test issue-789
    needs: [ci-policy, detect, gate]
    if: ${{ always() && github.event_name == 'workflow_dispatch' && github.event.inputs.issue_789 == 'true' && needs.ci-policy.outputs.ci_policy_path == 'iteration' && needs.detect.outputs.bvs_changed == 'true' && needs.gate.result == 'success' }}
    env:
      BVS_ISSUE_789_ARCHIVE_PATH: .nextest-archive/bvs-issue-789-lib.tar.zst
      BOLT_ISSUE_789_RESULT_PATH: result.json
    steps:
      - name: Build issue #789 lib archive
        run: |
          just bte-test-archive "$BVS_ISSUE_789_ARCHIVE_PATH" --lib
          stat -c 'bvs-issue-789-archive-size %n %s' "$BVS_ISSUE_789_ARCHIVE_PATH"
      - name: test issue-789
        run: |
          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"
          just bte-test-archive-run "$BVS_ISSUE_789_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" issue_789_first_real_free_data_taker_pl
      - name: Upload issue #789 first-P/L artifact
        id: upload-issue-789-first-pl
        uses: actions/upload-artifact@example
        with:
          name: issue-789-first-pl-${{ github.run_id }}-${{ github.run_attempt }}
          if-no-files-found: error
"""
    def bvs_cache_errors(workflow: str, *, include_job_digest: bool = False) -> list[str]:
        errors = verifier.backtester_test_shard_errors(
            ".github/workflows/backtester-ci.yml", workflow
        ) + verifier.backtester_managed_target_cache_errors(
            ".github/workflows/backtester-ci.yml", workflow
        )
        if include_job_digest:
            return errors
        return [error for error in errors if "BVS_TEST_ARCHIVE_JOB_SHA256" not in error]

    good_errors = bvs_cache_errors(good)
    assert not [error for error in good_errors if "backtester bvs-test" in error], good_errors

    full_ci_gated_issue_789 = replace_once(
        good,
        "needs.ci-policy.outputs.ci_policy_path == 'iteration'",
        "needs.ci-policy.outputs.full_ci_required == 'true'",
    )
    full_ci_gated_issue_789_errors = bvs_cache_errors(full_ci_gated_issue_789)
    assert any(
        "backtester bvs-test issue-789 must not depend on full CI dispatch" in error
        for error in full_ci_gated_issue_789_errors
    ), full_ci_gated_issue_789_errors

    missing_bvs_eligibility_fail_open = replace_once(
        good,
        "        continue-on-error: true\n        env:\n          ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}\n",
        "        env:\n          ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}\n",
    )
    missing_bvs_eligibility_fail_open_errors = bvs_cache_errors(missing_bvs_eligibility_fail_open)
    assert any(
        "backtester bvs-test archive S3 artifact cache eligibility must be fail-open" in error
        for error in missing_bvs_eligibility_fail_open_errors
    ), missing_bvs_eligibility_fail_open_errors

    weakened_bvs_role_split = replace_once(
        good,
        '          elif [[ "$GITHUB_EVENT_NAME" == "pull_request" || "$GITHUB_EVENT_NAME" == "merge_group" || "$GITHUB_EVENT_NAME" == "workflow_dispatch" ]]; then\n',
        '          elif [[ "$GITHUB_EVENT_NAME" == "pull_request" ]]; then\n',
    )
    weakened_bvs_role_split_errors = bvs_cache_errors(weakened_bvs_role_split)
    assert any(
        "backtester bvs-test archive S3 role selection must split main writers from read-only consumers" in error
        for error in weakened_bvs_role_split_errors
    ), weakened_bvs_role_split_errors

    missing_bvs_aws_fail_open = replace_once(
        good,
        "        continue-on-error: true\n        with:\n          role-to-assume: ${{ steps.bvs-nextest-artifact-cache.outputs.role_arn }}\n",
        "        with:\n          role-to-assume: ${{ steps.bvs-nextest-artifact-cache.outputs.role_arn }}\n",
    )
    missing_bvs_aws_fail_open_errors = bvs_cache_errors(missing_bvs_aws_fail_open)
    assert any(
        "backtester bvs-test archive S3 AWS credential setup must be fail-open" in error
        for error in missing_bvs_aws_fail_open_errors
    ), missing_bvs_aws_fail_open_errors

    github_artifact_cache = replace_once(
        good,
        "      - name: Restore BVS nextest archive from S3\n",
        "      - name: Restore forbidden BVS nextest archive cache\n"
        "        uses: actions/cache/restore@example\n"
        "        with:\n"
        "          path: ${{ env.BVS_NEXTEST_ARCHIVE_PATH }}\n"
        "          key: bvs-nextest-archive-v4-${{ runner.os }}-${{ runner.arch }}-test-profile-discovered-targets-shards-4-${{ steps.bvs_artifact_inputs.outputs.digest }}\n"
        "      - name: Restore BVS nextest archive from S3\n",
    )
    github_artifact_cache_errors = bvs_cache_errors(github_artifact_cache)
    assert any(
        "backtester bvs-test archive payloads must use S3 artifact cache" in error
        for error in github_artifact_cache_errors
    ), github_artifact_cache_errors

    missing_bvs_nextest_restore_guard = replace_once(
        good,
        "        if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success'\n",
        "",
    )
    missing_bvs_nextest_restore_guard_errors = bvs_cache_errors(missing_bvs_nextest_restore_guard)
    assert any(
        "backtester bvs-test archive must gate S3 restores on eligibility and AWS credential success" in error
        for error in missing_bvs_nextest_restore_guard_errors
    ), missing_bvs_nextest_restore_guard_errors

    missing_bvs_sidecar_restore_guard = replace_once_after(
        good,
        "      - name: Restore BVS binary sidecars from S3\n",
        "        if: steps.bvs-nextest-artifact-cache.outputs.eligible == 'true' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success'\n",
        "",
    )
    missing_bvs_sidecar_restore_guard_errors = bvs_cache_errors(missing_bvs_sidecar_restore_guard)
    assert any(
        "backtester bvs-test archive must gate S3 restores on eligibility and AWS credential success" in error
        for error in missing_bvs_sidecar_restore_guard_errors
    ), missing_bvs_sidecar_restore_guard_errors

    missing_bvs_nextest_integrity = replace_once(
        good,
        """          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::BVS nextest archive S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
""",
        "",
    )
    missing_bvs_nextest_integrity_errors = bvs_cache_errors(missing_bvs_nextest_integrity)
    assert any(
        "backtester bvs-test archive must fail closed on nextest archive S3 digest mismatch"
        in error
        for error in missing_bvs_nextest_integrity_errors
    ), missing_bvs_nextest_integrity_errors

    missing_bvs_sidecar_integrity = replace_once(
        good,
        """          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::BVS binary sidecar S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
""",
        "",
    )
    missing_bvs_sidecar_integrity_errors = bvs_cache_errors(missing_bvs_sidecar_integrity)
    assert any(
        "backtester bvs-test archive must fail closed on binary sidecar S3 digest mismatch"
        in error
        for error in missing_bvs_sidecar_integrity_errors
    ), missing_bvs_sidecar_integrity_errors

    weakened_bvs_s3_save_credentials = replace_once(
        good,
        "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.bvs-nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.bvs-nextest-artifact-cache-aws.outcome == 'success' && steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' }}",
        "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.bvs-nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' }}",
    )
    weakened_bvs_s3_save_credentials_errors = bvs_cache_errors(weakened_bvs_s3_save_credentials)
    assert any(
        "backtester bvs-test archive must save nextest archive to S3 only from push-to-main with write credentials"
        in error
        for error in weakened_bvs_s3_save_credentials_errors
    ), weakened_bvs_s3_save_credentials_errors

    missing_bvs_save_outcome_output = replace_once(
        good,
        "      bvs_nextest_archive_cache_save_outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}\n",
        "",
    )
    missing_bvs_save_outcome_output_errors = bvs_cache_errors(missing_bvs_save_outcome_output)
    assert any(
        "backtester bvs-test archive must expose BVS S3 save outcomes" in error
        for error in missing_bvs_save_outcome_output_errors
    ), missing_bvs_save_outcome_output_errors

    missing_bvs_save_outcome_summary = replace_once(
        good,
        '          echo "BVS nextest archive S3 save outcome: ${{ steps.bvs-nextest-archive-cache-save.outputs.save-status || (steps.bvs-nextest-archive-cache-save.outcome == \'skipped\' && \'skipped\' || \'failed\') }}" >> "$GITHUB_STEP_SUMMARY"\n',
        "",
    )
    missing_bvs_save_outcome_summary_errors = bvs_cache_errors(missing_bvs_save_outcome_summary)
    assert any(
        "backtester bvs-test archive must summarize BVS S3 save outcomes" in error
        for error in missing_bvs_save_outcome_summary_errors
    ), missing_bvs_save_outcome_summary_errors

    missing_bvs_restore_output = replace_once_after(
        good,
        "      - name: Restore BVS nextest archive from S3",
        '          echo "restore-result=miss" >> "$GITHUB_OUTPUT"\n',
        "",
    )
    missing_bvs_restore_output_errors = bvs_cache_errors(missing_bvs_restore_output)
    assert any(
        "backtester bvs-test archive must emit restore result and reason outputs" in error
        for error in missing_bvs_restore_output_errors
    ), missing_bvs_restore_output_errors

    missing_bvs_nextest_restore_summary = replace_once(
        good,
        '            echo "BVS nextest archive S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_nextest_restore} reason=${bvs_nextest_reason}"\n',
        "",
    )
    missing_bvs_nextest_restore_summary_errors = bvs_cache_errors(missing_bvs_nextest_restore_summary)
    assert any(
        "backtester bvs-test archive must summarize BVS nextest archive S3 restore state" in error
        for error in missing_bvs_nextest_restore_summary_errors
    ), missing_bvs_nextest_restore_summary_errors

    missing_bvs_sidecar_restore_summary = replace_once(
        good,
        '            echo "BVS binary sidecars S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${bvs_sidecar_restore} reason=${bvs_sidecar_reason}"\n',
        "",
    )
    missing_bvs_sidecar_restore_summary_errors = bvs_cache_errors(missing_bvs_sidecar_restore_summary)
    assert any(
        "backtester bvs-test archive must summarize BVS binary sidecars S3 restore state" in error
        for error in missing_bvs_sidecar_restore_summary_errors
    ), missing_bvs_sidecar_restore_summary_errors

    missing_bvs_partition_annotation = replace_once(
        good,
        '              echo "::error title=BVS nextest archive partition failed::shard=${shard}/${BVS_NEXTEST_SHARDS} exit=${rc}"\n',
        "",
    )
    missing_bvs_partition_annotation_errors = bvs_cache_errors(missing_bvs_partition_annotation)
    assert any(
        "backtester bvs-test partition failures must emit shard error annotations" in error
        for error in missing_bvs_partition_annotation_errors
    ), missing_bvs_partition_annotation_errors

    missing_bvs_partition_set_e_disable = replace_once(
        good,
        """            set +e
            just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${shard}/${BVS_NEXTEST_SHARDS}" -- --skip issue_789_first_real_free_data_taker_pl --skip backtesting_vertical_slice_s3_catalog_smoke 2>&1 | tee "$partition_log"
""",
        '            just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${shard}/${BVS_NEXTEST_SHARDS}" -- --skip issue_789_first_real_free_data_taker_pl --skip backtesting_vertical_slice_s3_catalog_smoke 2>&1 | tee "$partition_log"\n',
    )
    missing_bvs_partition_set_e_disable_errors = bvs_cache_errors(missing_bvs_partition_set_e_disable)
    assert any(
        "backtester bvs-test partition failures must use contiguous failure wrapper" in error
        for error in missing_bvs_partition_set_e_disable_errors
    ), missing_bvs_partition_set_e_disable_errors

    missing_bvs_partition_rc_condition = replace_once(
        real_backtester_workflow,
        '            if [[ "$rc" -ne 0 ]]; then\n',
        '            if [[ "$rc" == 0 ]]; then\n',
    )
    missing_bvs_partition_rc_condition_errors = bvs_cache_errors(
        missing_bvs_partition_rc_condition,
        include_job_digest=True,
    )
    assert any(
        "BVS_TEST_ARCHIVE_JOB_SHA256" in error
        for error in missing_bvs_partition_rc_condition_errors
    ), missing_bvs_partition_rc_condition_errors

    bvs_partition_github_path_probe = insert_step_before_named_step_after(
        real_backtester_workflow,
        "  test-archive:",
        "test",
        """      - name: Install BVS partition shim
        shell: bash
        run: printf '%s\\n' "$GITHUB_WORKSPACE/.ci-shims" | tee -a "$GITHUB_PATH"
""",
    )
    bvs_partition_github_path_errors = bvs_cache_errors(
        bvs_partition_github_path_probe,
        include_job_digest=True,
    )
    assert any(
        "BVS_TEST_ARCHIVE_JOB_SHA256"
        in error
        for error in bvs_partition_github_path_errors
    ), bvs_partition_github_path_errors
    bvs_partition_job_continue_on_error_probe = replace_once_after(
        real_backtester_workflow,
        "  test-archive:",
        "    name: bvs-test archive\n",
        "    name: bvs-test archive\n    continue-on-error: true\n",
    )
    bvs_partition_job_continue_on_error_errors = bvs_cache_errors(
        bvs_partition_job_continue_on_error_probe,
        include_job_digest=True,
    )
    assert any(
        "BVS_TEST_ARCHIVE_JOB_SHA256"
        in error
        for error in bvs_partition_job_continue_on_error_errors
    ), bvs_partition_job_continue_on_error_errors
    bvs_partition_job_defaults_probe = replace_once_after(
        real_backtester_workflow,
        "  test-archive:",
        "    steps:\n",
        "    defaults:\n      run:\n        shell: sh\n    steps:\n",
    )
    bvs_partition_job_defaults_errors = bvs_cache_errors(
        bvs_partition_job_defaults_probe,
        include_job_digest=True,
    )
    assert any(
        "BVS_TEST_ARCHIVE_JOB_SHA256"
        in error
        for error in bvs_partition_job_defaults_errors
    ), bvs_partition_job_defaults_errors
    bvs_partition_run_body_probe = replace_once_after(
        real_backtester_workflow,
        "      - name: test",
        '            if [[ "$rc" -ne 0 ]]; then\n',
        '            if [[ "$rc" == 0 ]]; then\n',
    )
    bvs_partition_run_body_errors = bvs_cache_errors(
        bvs_partition_run_body_probe,
        include_job_digest=True,
    )
    assert any(
        "BVS_TEST_ARCHIVE_JOB_SHA256"
        in error
        for error in bvs_partition_run_body_errors
    ), bvs_partition_run_body_errors
    bvs_top_level_path_env = replace_once(
        real_backtester_workflow,
        "env:\n  JUST_VERSION: \"1.49.0\"\n",
        "env:\n  PATH: /tmp/partition-shim\n  JUST_VERSION: \"1.49.0\"\n",
    )
    bvs_top_level_path_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_top_level_path_env}
    )
    assert any(
        "top-level env.PATH must be classified as reuse-scoped or build-neutral" in error
        for error in bvs_top_level_path_errors
    ), bvs_top_level_path_errors
    bvs_top_level_ld_preload_env = replace_once(
        real_backtester_workflow,
        "env:\n  JUST_VERSION: \"1.49.0\"\n",
        "env:\n  LD_PRELOAD: /tmp/partition-shim.so\n  JUST_VERSION: \"1.49.0\"\n",
    )
    bvs_top_level_ld_preload_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_top_level_ld_preload_env}
    )
    assert any(
        "top-level env.LD_PRELOAD must be classified as reuse-scoped or build-neutral" in error
        for error in bvs_top_level_ld_preload_errors
    ), bvs_top_level_ld_preload_errors
    bvs_top_level_arbitrary_env = replace_once(
        real_backtester_workflow,
        "env:\n  JUST_VERSION: \"1.49.0\"\n",
        "env:\n  EVIL_INJECT: x\n  JUST_VERSION: \"1.49.0\"\n",
    )
    bvs_top_level_arbitrary_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_top_level_arbitrary_env}
    )
    assert any(
        "top-level env.EVIL_INJECT must be classified as reuse-scoped or build-neutral" in error
        for error in bvs_top_level_arbitrary_errors
    ), bvs_top_level_arbitrary_errors
    bvs_top_level_flow_env = replace_once(
        real_backtester_workflow,
        """env:
  JUST_VERSION: "1.49.0"
  CARGO_TERM_COLOR: always
  # Same single base as ci.yml — one base, two namespaces. The crate's namespace
  # (backtesting-vertical-slice) is selected by --repo, not by a second base.
  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification
permissions:
""",
        """env: {"PATH": "/tmp/partition-shim:${{ env.PATH }}", JUST_VERSION: "1.49.0", CARGO_TERM_COLOR: always, RUST_VERIFICATION_ROOT_BASE: "${{ github.workspace }}/.rust-verification"}
permissions:
""",
    )
    bvs_top_level_flow_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": bvs_top_level_flow_env}
    )
    assert any(
        "top-level env reuse scope could not parse backtester-ci.yml" in error
        for error in bvs_top_level_flow_errors
    ), bvs_top_level_flow_errors
    for defaults_probe in (
        "defaults:\n  run:\n    shell: sh\n",
        'defaults: {"run": {shell: bash}}\n',
        "? defaults\n: {run: {working-directory: scripts}}\n",
        "? defaults : {run: {shell: bash}}\n",
        "!!map defaults: {run: {shell: bash}}\n",
        "defaults: &defaults_probe {run: {shell: bash}}\n",
        '"defaults":\n  run:\n    shell: sh\n',
        "evil_top: {a: b}\n",
    ):
        bvs_top_level_defaults = replace_once(
            real_backtester_workflow,
            "permissions:\n",
            f"{defaults_probe}permissions:\n",
        )
        bvs_top_level_defaults_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": bvs_top_level_defaults}
        )
        assert any(
            "backtester-ci.yml top-level entry" in error
            for error in bvs_top_level_defaults_errors
        ), bvs_top_level_defaults_errors
    bvs_partition_step_rename_probe = replace_once_after(
        real_backtester_workflow,
        "  test-archive:",
        "      - name: test\n",
        "      - name: renamed test\n",
    )
    bvs_partition_step_rename_errors = bvs_cache_errors(
        bvs_partition_step_rename_probe,
        include_job_digest=True,
    )
    assert any(
        "backtester bvs-test archive must define test partition step" in error
        for error in bvs_partition_step_rename_errors
    ), bvs_partition_step_rename_errors

    missing_bvs_archive_save_status = replace_once(
        good,
        '            echo "save-status=failed" >> "$GITHUB_OUTPUT"\n',
        "",
    )
    missing_bvs_archive_save_status_errors = bvs_cache_errors(missing_bvs_archive_save_status)
    assert any(
        "backtester bvs-test archive must emit explicit nextest archive S3 save status" in error
        for error in missing_bvs_archive_save_status_errors
    ), missing_bvs_archive_save_status_errors

    missing_bvs_sidecar_save_status = replace_once_after(
        good,
        "      - name: Save BVS binary sidecars",
        '            echo "save-status=failed" >> "$GITHUB_OUTPUT"\n',
        "",
    )
    missing_bvs_sidecar_save_status_errors = bvs_cache_errors(missing_bvs_sidecar_save_status)
    assert any(
        "backtester bvs-test archive must emit explicit binary sidecars S3 save status" in error
        for error in missing_bvs_sidecar_save_status_errors
    ), missing_bvs_sidecar_save_status_errors

    fanout = good.replace(
        "      - name: Require BVS local payload\n",
        "      - name: Upload BVS test payload\n        with:\n          name: bvs-test-payload\n      - name: Require BVS local payload\n",
        1,
    )
    fanout_errors = bvs_cache_errors(fanout)
    assert any("legacy fan-out payload" in error for error in fanout_errors), fanout_errors

    download_in_archive = good.replace(
        "      - name: Require BVS local payload\n",
        "      - name: Download forbidden BVS test payload\n"
        "        uses: actions/download-artifact@example\n"
        "      - name: Require BVS local payload\n",
        1,
    )
    download_errors = bvs_cache_errors(download_in_archive)
    assert any(
        "backtester required bvs-test path must not download a test payload artifact" in error
        for error in download_errors
    ), download_errors

    consumer_managed_target = good.replace(
        "      - name: Build issue #789 lib archive\n",
        "      - name: Restore forbidden managed target cache\n"
        "        uses: actions/cache/restore@example\n"
        "        with:\n"
        "          key: managed-target-bvs-v4-${{ runner.os }}-${{ runner.arch }}-test-${{ steps.bvs_cache_inputs.outputs.digest }}\n"
        "      - name: Build issue #789 lib archive\n",
        1,
    )
    consumer_target_errors = bvs_cache_errors(consumer_managed_target)
    assert any(
        "backtester bvs-test consumers must not restore the managed target cache" in error
        for error in consumer_target_errors
    ), consumer_target_errors

    hardcoded_archive_targets = good.replace(
        "mapfile -t archive_args < <(python3 scripts/rust_test_targets.py archive-args --crate crates/backtesting-vertical-slice)\n"
        "          printf 'BVS archive args: %s\\n' \"${archive_args[*]}\"\n"
        '          just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH" "${archive_args[@]}"',
        'just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH" --lib --test backtesting_vertical_slice_tests --bin backtesting-vertical-slice',
        1,
    )
    hardcoded_archive_errors = bvs_cache_errors(hardcoded_archive_targets)
    assert any("archive targets must be discovered" in error for error in hardcoded_archive_errors), hardcoded_archive_errors

    weakened_archive_guard = good.replace(
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty"; exit 1; }',
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || true',
        1,
    )
    weakened_errors = bvs_cache_errors(weakened_archive_guard)
    assert any("backtester bvs-test archive must fail closed on missing local payload" in error for error in weakened_errors), weakened_errors

    exit_one_archive_guard = good.replace(
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty"; exit 1; }',
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || exit 1',
        1,
    )
    exit_one_errors = bvs_cache_errors(exit_one_archive_guard)
    assert any(
        "backtester bvs-test archive must fail closed on missing local payload" in error
        for error in exit_one_errors
    ), exit_one_errors


def assert_cache_as_same_run_transport_is_banned() -> None:
    verifier = load_verifier()
    fail_on_miss_message = verifier.CACHE_SAME_RUN_TRANSPORT_FAIL_ON_MISS_MESSAGE
    def has_fail_on_miss_message(errors: list[str]) -> bool:
        return any(fail_on_miss_message in error for error in errors)

    bad_hand_rolled = """jobs:
  test:
    steps:
      - name: Restore payload
        id: x-cache
        uses: actions/cache/restore@example
      - name: Require payload cache
        if: steps.x-cache.outputs.cache-hit != 'true'
        run: |
          echo "payload cache unavailable"
          exit 1
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/example-ci.yml": bad_hand_rolled})
    assert any("must not fail a job on a cache miss" in error for error in errors), errors

    bad_builtin = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: true
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/example-ci.yml": bad_builtin})
    assert any("fail-closed same-run transport" in error for error in errors), errors

    # Quoted/case variants are the same fail-closed directive; the old exact
    # substring check missed `'true'`, so the ban must catch these too.
    for variant in ("'true'", '"true"', "True"):
        bad_builtin_variant = f"""jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: {variant}
"""
        variant_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/example-ci.yml": bad_builtin_variant}
        )
        assert any(
            "fail-closed same-run transport" in error for error in variant_errors
        ), (variant, variant_errors)

    bad_builtin_flow = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with: { path: payload, key: payload-key, fail-on-cache-miss: true }
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_builtin_flow}
    )
    assert any("fail-closed same-run transport" in error for error in errors), errors

    bad_builtin_bool_tag = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: !!bool true
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_builtin_bool_tag}
    )
    assert has_fail_on_miss_message(errors), errors

    bad_folded_true = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: >-
            true
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_folded_true}
    )
    assert has_fail_on_miss_message(errors), errors

    bad_block_literal_true = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: |
            true
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_block_literal_true}
    )
    assert has_fail_on_miss_message(errors), errors

    ok_commented_block_scalar_key = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          # fail-on-cache-miss: >-
            true
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": ok_commented_block_scalar_key}
    )
    assert not has_fail_on_miss_message(errors), errors

    ok_block_multiline_string = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: |
            some line
            true
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": ok_block_multiline_string}
    )
    assert not has_fail_on_miss_message(errors), errors

    ok_folded_false = """jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: >-
            false
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": ok_folded_false}
    )
    assert not has_fail_on_miss_message(errors), errors

    for variant in ("yes", "on"):
        bad_builtin_truthy = f"""jobs:
  test:
    steps:
      - name: Restore payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: {variant}
"""
        truthy_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/example-ci.yml": bad_builtin_truthy}
        )
        assert any(
            "fail-closed same-run transport" in error for error in truthy_errors
        ), (variant, truthy_errors)

    for run_body in (
        "test -s payload || exit 1",
        "test -s payload && exit 1",
        "test -s payload || { echo m; exit 1; }",
    ):
        bad_guarded_chain = f"""jobs:
  test:
    steps:
      - name: Restore payload
        id: x-cache
        uses: actions/cache/restore@example
      - name: Require payload cache
        if: steps.x-cache.outputs.cache-hit != 'true'
        run: {run_body}
"""
        chain_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/example-ci.yml": bad_guarded_chain}
        )
        assert any(
            "must not fail a job on a cache miss" in error for error in chain_errors
        ), (run_body, chain_errors)

    good = """jobs:
  test:
    steps:
      - name: Restore payload
        id: x-cache
        uses: actions/cache/restore@example
      - name: Build payload on miss
        if: steps.x-cache.outputs.cache-hit != 'true'
        run: just build-payload
      - name: Validate unrelated invariant
        run: |
          echo "unrelated failure path"
          exit 1
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/example-ci.yml": good})
    assert not [
        error for error in errors if "cache" in error and "same-run" in error
    ], errors

    producer_nested_exit = """jobs:
  test:
    steps:
      - name: Build payload on miss
        if: steps.x-cache.outputs.cache-hit != 'true'
        run: |
          if [[ "$n" == "0" ]]; then
            echo none
            exit 1
          fi
          echo ok
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": producer_nested_exit}
    )
    assert not [
        error for error in errors if "cache" in error and "same-run" in error
    ], errors

    bad_both_arms = """jobs:
  test:
    steps:
      - name: Restore builtin payload
        uses: actions/cache/restore@example
        with:
          path: payload
          key: payload-key
          fail-on-cache-miss: true
      - name: Restore hand rolled payload
        id: x-cache
        uses: actions/cache/restore@example
      - name: Require hand rolled payload cache
        if: steps.x-cache.outputs.cache-hit != 'true'
        run: exit 1
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_both_arms}
    )
    assert any("fail-closed same-run transport" in error for error in errors), errors
    assert any("must not fail a job on a cache miss" in error for error in errors), errors

    bad_double_quoted_false_guard = """jobs:
  test:
    steps:
      - name: Restore payload
        id: x-cache
        uses: actions/cache/restore@example
      - name: Require payload cache
        if: steps.x-cache.outputs.cache-hit == "false"
        run: exit 1
"""
    errors = verifier.verify_repo_automation_texts(
        {".github/workflows/example-ci.yml": bad_double_quoted_false_guard}
    )
    assert any("must not fail a job on a cache miss" in error for error in errors), errors

    # The guard's if-matcher must tolerate zero leading whitespace; the old
    # anchor required at least one leading space and would miss a stripped or
    # pre-processed line.
    assert verifier.step_has_cache_miss_guard(
        ["if: steps.x.outputs.cache-hit != 'true'"]
    ), "zero-indent cache-miss guard must be detected"


def assert_v6_red_backtester_nextest_archive_recipes_absolutize_paths() -> None:
    verifier = load_verifier()
    bad = """bte-test-archive archive *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest archive --locked --archive-file "{{archive}}" {{args}}

bte-test-archive-run archive extract_root *args: check-workspace require-rust-verification-owner
    python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest run --archive-file "{{archive}}" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{repo_root}}/crates/backtesting-vertical-slice" {{args}}
"""
    errors = verifier.verify_repo_automation_texts({"justfile": bad})
    assert any("backtester nextest archive recipes must absolutize archive paths" in error for error in errors), errors
    assert any("backtester nextest archive recipes must not pass crate-relative archive paths" in error for error in errors), errors

    good = """bte-test-archive archive *args: check-workspace require-rust-verification-owner
    archive_path="{{archive}}"; \\
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \\
      mkdir -p "$(dirname "$archive_path")"; \\
      python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest archive --locked --archive-file "$archive_path" {{args}}

bte-test-archive-run archive extract_root *args: check-workspace require-rust-verification-owner
    archive_path="{{archive}}"; \\
      case "$archive_path" in /*) ;; *) archive_path="{{repo_root}}/$archive_path";; esac; \\
      python3 "{{rust_verification_owner}}" cargo --repo "{{repo_root}}/crates/backtesting-vertical-slice" -- nextest run --archive-file "$archive_path" --extract-to "{{extract_root}}" --extract-overwrite --workspace-remap "{{repo_root}}/crates/backtesting-vertical-slice" {{args}}
"""
    good_errors = verifier.verify_repo_automation_texts({"justfile": good})
    assert not [
        error for error in good_errors if "backtester nextest archive recipes" in error
    ], good_errors


def remove_fragment_if_present(text: str, fragment: str) -> str:
    return text.replace(fragment, "", 1) if fragment in text else text


def remove_all_fragments_if_present(text: str, fragment: str) -> str:
    return text.replace(fragment, "") if fragment in text else text


def assert_nextest_fingerprint_reuse_adversarial_gaps_are_reported() -> None:
    cache_key_line = (
        "nextest_archive_cache_key=${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
    )
    inline_hashfiles_key_line = (
        "nextest_archive_cache_key=nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock') }}"
    )
    assert_error(
        "nextest archive cache key must use nextest fingerprint output",
        replace_once(BASE_WORKFLOW, cache_key_line, inline_hashfiles_key_line),
    )
    assert_error(
        "nextest-fingerprint must run the canonical producer script",
        replace_once(
            BASE_WORKFLOW,
            "--config ci/nextest-fingerprint.toml",
            "--config ci/not-nextest-fingerprint.toml",
        ),
    )
    assert_error(
        "nextest-fingerprint must not inline nextest hashFiles",
        replace_once(
            BASE_WORKFLOW,
            "python3 scripts/nextest_fingerprint.py",
            "python3 scripts/nextest_fingerprint.py ${{ hashFiles('Cargo.lock') }}",
        ),
    )
    assert_error(
        "nextest-fingerprint artifact name must come from producer output",
        replace_once(
            BASE_WORKFLOW,
            "          name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}",
            "          name: nextest-archive-fingerprint-v2-Linux-X64-test-profile-shards-4-deadbeef",
        ),
    )
    assert_error(
        "test-archive must fail closed on invalid nextest shard count",
        replace_once(BASE_WORKFLOW, '          if [[ ! "$shards" =~ ^[1-9][0-9]*$ ]]; then\n', ""),
    )
    assert_error(
        "test-archive partition count must come from nextest fingerprint output",
        replace_once(BASE_WORKFLOW, '          for shard in $(seq 1 "$shards"); do', "          for shard in 1 2 3 4; do"),
    )
    assert_error(
        "test-archive partition count must come from nextest fingerprint output",
        replace_once(BASE_WORKFLOW, '          for shard in $(seq 1 "$shards"); do', "          for shard in {1..$shards}; do"),
    )

    assert_error(
        "nextest-fingerprint-reuse must admit PR, workflow_dispatch, and merge_group consumers",
        remove_fragment_if_present(
            BASE_WORKFLOW,
            " && contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name)",
        ),
    )
    assert_error(
        "detector must determine fingerprint_reuse_allowed",
        remove_fragment_if_present(
            BASE_WORKFLOW,
            """          elif [[ "${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
""",
        ),
    )
    pr_only_detector_refs = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Fetch detector base/head refs",
        "        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'",
        "        if: github.event_name == 'pull_request'",
    )
    assert_error("detector base/head refs step must match canonical envelope", pr_only_detector_refs)
    detector_refs_without_dispatch_branch = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Fetch detector base/head refs",
        """          elif [[ "$EVENT_NAME" == "workflow_dispatch" ]]; then
            base_branch="$DISPATCH_BASE_REF"
            if [[ "$base_branch" == refs/* ]]; then
              echo "unsupported workflow_dispatch default_branch: $base_branch" >&2
              exit 1
            fi
            base_ref="refs/remotes/origin/dispatch-base-${GITHUB_RUN_ID}"
            head_ref="HEAD"
            git check-ref-format "refs/heads/$base_branch"
            git fetch --no-tags origin "+refs/heads/${base_branch}:${base_ref}"
""",
        "",
    )
    assert_error(
        "detector base/head refs step must match canonical script",
        detector_refs_without_dispatch_branch,
    )
    detector_refs_without_merge_group_branch = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Fetch detector base/head refs",
        """          elif [[ "$EVENT_NAME" == "merge_group" ]]; then
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
""",
        "",
    )
    assert_error(
        "detector base/head refs step must match canonical script",
        detector_refs_without_merge_group_branch,
    )

    verifier = load_verifier()

    def detector_refs_result(merge_group_base_ref: str):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            output_path = tmp_path / "github-output"
            git_calls_path = tmp_path / "git-calls"
            script = (
                """set -euo pipefail
git() {
  if [[ "$1" == "check-ref-format" ]]; then
    command git check-ref-format "$2"
    return $?
  fi
  if [[ "$1" == "fetch" ]]; then
    printf '%s\\n' "$*" >> "$GIT_CALLS"
    return 0
  fi
  echo "unexpected git invocation: $*" >&2
  return 99
}
"""
                + verifier.DETECTOR_REFS_RUN
            )
            env = {
                **os.environ,
                "EVENT_NAME": "merge_group",
                "PR_BASE_REF": "main",
                "PR_NUMBER": "1182",
                "DISPATCH_BASE_REF": "main",
                "MERGE_GROUP_BASE_REF": merge_group_base_ref,
                "GITHUB_RUN_ID": "12345",
                "GITHUB_OUTPUT": str(output_path),
                "GIT_CALLS": str(git_calls_path),
            }
            script_path = tmp_path / "detector-refs.sh"
            script_path.write_text(script, encoding="utf-8")
            completed = subprocess.run(
                ["bash", str(script_path)],
                cwd=tmp_path,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            output_text = output_path.read_text(encoding="utf-8") if output_path.exists() else ""
            git_calls = git_calls_path.read_text(encoding="utf-8") if git_calls_path.exists() else ""
            return completed, output_text, git_calls

    for valid_base_ref in ("main", "refs/heads/main"):
        completed, output_text, git_calls = detector_refs_result(valid_base_ref)
        if completed.returncode != 0:
            raise AssertionError((valid_base_ref, completed.returncode, completed.stderr))
        if "base_ref=refs/remotes/origin/pr-base-merge-group-12345" not in output_text:
            raise AssertionError((valid_base_ref, output_text))
        if "head_ref=HEAD" not in output_text:
            raise AssertionError((valid_base_ref, output_text))
        if "+refs/heads/main:refs/remotes/origin/pr-base-merge-group-12345" not in git_calls:
            raise AssertionError((valid_base_ref, git_calls))

    for invalid_base_ref in ("", "refs/tags/main", "refs/pull/1182/merge", "main..evil", "feature branch"):
        completed, output_text, git_calls = detector_refs_result(invalid_base_ref)
        if completed.returncode == 0:
            raise AssertionError((invalid_base_ref, output_text, git_calls))

    def dispatch_refs_result(dispatch_base_ref: str):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = pathlib.Path(tmp)
            output_path = tmp_path / "github-output"
            git_calls_path = tmp_path / "git-calls"
            script = (
                """set -euo pipefail
git() {
  if [[ "$1" == "check-ref-format" ]]; then
    command git check-ref-format "$2"
    return $?
  fi
  if [[ "$1" == "fetch" ]]; then
    printf '%s\\n' "$*" >> "$GIT_CALLS"
    return 0
  fi
  echo "unexpected git invocation: $*" >&2
  return 99
}
"""
                + verifier.DETECTOR_REFS_RUN
            )
            env = {
                **os.environ,
                "EVENT_NAME": "workflow_dispatch",
                "PR_BASE_REF": "main",
                "PR_NUMBER": "1182",
                "DISPATCH_BASE_REF": dispatch_base_ref,
                "MERGE_GROUP_BASE_REF": "main",
                "GITHUB_RUN_ID": "12345",
                "GITHUB_OUTPUT": str(output_path),
                "GIT_CALLS": str(git_calls_path),
            }
            script_path = tmp_path / "dispatch-refs.sh"
            script_path.write_text(script, encoding="utf-8")
            completed = subprocess.run(
                ["bash", str(script_path)],
                cwd=tmp_path,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            output_text = output_path.read_text(encoding="utf-8") if output_path.exists() else ""
            git_calls = git_calls_path.read_text(encoding="utf-8") if git_calls_path.exists() else ""
            return completed, output_text, git_calls

    completed, output_text, git_calls = dispatch_refs_result("main")
    if completed.returncode != 0:
        raise AssertionError(("workflow_dispatch", completed.returncode, completed.stderr))
    if "base_ref=refs/remotes/origin/dispatch-base-12345" not in output_text:
        raise AssertionError(output_text)
    if "head_ref=HEAD" not in output_text:
        raise AssertionError(output_text)
    if "+refs/heads/main:refs/remotes/origin/dispatch-base-12345" not in git_calls:
        raise AssertionError(git_calls)

    for invalid_base_ref in ("", "refs/tags/main", "main..evil", "feature branch"):
        completed, output_text, git_calls = dispatch_refs_result(invalid_base_ref)
        if completed.returncode == 0:
            raise AssertionError((invalid_base_ref, output_text, git_calls))

    pr_only_governance_detector = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        "        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'",
        "        if: github.event_name == 'pull_request'",
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        pr_only_governance_detector,
    )

    assert_error(
        "detector must map fingerprint-reuse governance changes to any_changed=true",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            """          if [[ -n "$changed" ]]; then
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          fi""",
            """          if [[ -n "$changed" ]]; then
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          fi""",
        ),
    )
    assert_error(
        "detector must map fingerprint-reuse governance changes to any_changed=true",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            """          if [[ -n "$changed" ]]; then
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          fi""",
            """          if [[ -n "$changed" ]]; then
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          fi
          echo "any_changed=false" >> "$GITHUB_OUTPUT\"""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical script",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            """          if [[ -n "$changed" ]]; then""",
            """          changed=""
          if [[ -n "$changed" ]]; then""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical script",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            """          fi""",
            """          fi
          printf 'any_changed=false\\n' >> "$GITHUB_OUTPUT\"""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'",
            "        if: false",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        without_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'\n",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        shell: bash\n",
            """        shell: bash
        working-directory: /tmp
        continue-on-error: true
""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        shell: bash\n",
            """        shell: bash
        "working-directory" : /tmp
        "continue-on-error" : true
""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        shell: bash\n",
            """        shell: bash
        "working-directory": /tmp
        "continue-on-error": true
""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'\n",
            """        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'
        if: false
""",
        ),
    )

    narrowed_pathspec = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        """.github/actions/setup-environment/action.yml             ci/nextest-fingerprint.toml             ci/github-actions-runners.toml             scripts/nextest_fingerprint.py             scripts/test_nextest_fingerprint.py             scripts/root_bin_sidecars.py             scripts/test_root_bin_sidecars.py             scripts/config_validators.py             scripts/ci_provenance.py             scripts/test_ci_provenance.py             scripts/verify_ci_workflow_hygiene.py             scripts/test_verify_ci_workflow_hygiene.py)""",
        """.github/actions/setup-environment/action.yml)
          echo "decoy paths: .github/workflows/ci.yml ci/nextest-fingerprint.toml ci/github-actions-runners.toml scripts/nextest_fingerprint.py scripts/test_nextest_fingerprint.py scripts/root_bin_sidecars.py scripts/test_root_bin_sidecars.py scripts/config_validators.py scripts/ci_provenance.py scripts/test_ci_provenance.py scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py\"""",
    )
    assert_error(
        "detector must detect fingerprint-reuse governance changes",
        narrowed_pathspec,
    )
    git_diff_decoy_pathspec = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        """          changed="$(git diff --name-only "$diff_range" --             .github/actions/setup-environment/action.yml""",
        """          echo "$(git diff --name-only "$diff_range" -- .github/workflows/ci.yml .github/actions/setup-environment/action.yml ci/nextest-fingerprint.toml ci/github-actions-runners.toml scripts/nextest_fingerprint.py scripts/test_nextest_fingerprint.py scripts/root_bin_sidecars.py scripts/test_root_bin_sidecars.py scripts/config_validators.py scripts/ci_provenance.py scripts/test_ci_provenance.py scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py)"
          changed="$(git diff --name-only "$diff_range" --             .github/actions/setup-environment/action.yml""",
    )
    git_diff_decoy_pathspec = replace_once_after(
        git_diff_decoy_pathspec,
        "      - name: Detect fingerprint-reuse governance changes",
        """.github/actions/setup-environment/action.yml             ci/nextest-fingerprint.toml             ci/github-actions-runners.toml             scripts/nextest_fingerprint.py             scripts/test_nextest_fingerprint.py             scripts/root_bin_sidecars.py             scripts/test_root_bin_sidecars.py             scripts/config_validators.py             scripts/ci_provenance.py             scripts/test_ci_provenance.py             scripts/verify_ci_workflow_hygiene.py             scripts/test_verify_ci_workflow_hygiene.py)""",
        """.github/actions/setup-environment/action.yml)""",
    )
    assert_error("detector must detect fingerprint-reuse governance changes", git_diff_decoy_pathspec)

    relocated_job_if = replace_once(
        BASE_WORKFLOW,
        " && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main'",
        "",
    )
    relocated_job_if = replace_once_after(
        relocated_job_if,
        "  nextest-fingerprint-reuse:",
        "      - name: Resolve nextest fingerprint reuse",
        """      - name: decoy needs.detector.outputs.fingerprint_reuse_allowed == 'true' github.ref != 'refs/heads/main'
        run: echo "job-if decoy"

      - name: Resolve nextest fingerprint reuse""",
    )
    assert_error("nextest-fingerprint-reuse must skip main branch", relocated_job_if)
    assert_error("nextest-fingerprint-reuse must gate on fingerprint_reuse_allowed", relocated_job_if)
    assert_error(
        "top-level env.RUSTFLAGS must be classified as reuse-scoped or build-neutral",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            '  JUST_VERSION: "1.49.0"\n  RUSTFLAGS: "-D warnings"\n',
        ),
    )
    assert_error(
        "top-level env.JUST_VERSION must use a same-line scalar value",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            '  JUST_VERSION:\n    "1.49.0"\n',
        ),
    )
    assert_error(
        "top-level env entry must use canonical indentation",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            '  JUST_VERSION: "1.49.0"\n    RUSTFLAGS: "-D warnings"\n',
        ),
    )
    assert_error(
        "top-level env.JUST_VERSION is reuse-scoped but missing from ci.yml",
        replace_once(BASE_WORKFLOW, '  JUST_VERSION: "1.49.0"\n', ""),
    )
    assert_error(
        "top-level env.RUST_VERIFICATION_ROOT_BASE is reuse-scoped but missing from ci.yml",
        replace_once(
            BASE_WORKFLOW,
            "  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification\n",
            "",
        ),
    )
    for defaults_probe in (
        "defaults:\n  run:\n    shell: sh\n\n",
        'defaults: {"run": {shell: bash}}\n',
        "? defaults\n: {run: {working-directory: scripts}}\n",
        "? defaults : {run: {shell: bash}}\n",
        "!!map defaults: {run: {shell: bash}}\n",
        "defaults: &defaults_probe {run: {shell: bash}}\n",
        '"defaults":\n  run:\n    shell: sh\n',
        "evil_top: {a: b}\n",
    ):
        assert_error(
            "ci.yml top-level entry",
            replace_once(BASE_WORKFLOW, "permissions:\n", f"{defaults_probe}permissions:\n"),
        )
    assert_error(
        "YAML anchors and aliases must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification\n",
            "  RUST_VERIFICATION_ROOT_BASE: &reuse_root ${{ github.workspace }}/.rust-verification\n",
        ),
    )
    assert_error(
        "YAML anchors and aliases must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                '  JUST_VERSION: "1.49.0"\n',
                '  JUST_VERSION: &just_version "1.49.0"\n',
            ),
            "  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification\n",
            "  RUST_VERIFICATION_ROOT_BASE: *just_version\n",
        ),
    )
    assert_error(
        "YAML anchors and aliases must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification\n",
            "  RUST_VERIFICATION_ROOT_BASE: &reuse@root ${{ github.workspace }}/.rust-verification\n",
        ),
    )
    assert_error(
        "YAML anchors and aliases must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                '  JUST_VERSION: "1.49.0"\n',
                '  JUST_VERSION: &just+version "1.49.0"\n',
            ),
            "  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification\n",
            "  RUST_VERIFICATION_ROOT_BASE: *just+version\n",
        ),
    )
    assert_error(
        "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "        run: |\n          python3 scripts/nextest_fingerprint.py",
            "        run: !!str |\n          python3 scripts/nextest_fingerprint.py",
        ),
    )
    assert_error(
        "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "permissions:\n",
            "%YAML 1.2\npermissions:\n",
        ),
    )
    assert_error(
        "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "permissions:\n",
            "---\npermissions:\n",
        ),
    )
    assert_error(
        "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml while nextest reuse is enabled",
        BASE_WORKFLOW + "\n--- {jobs: {evil: {}}}\n",
    )
    assert_error(
        "YAML tags, explicit keys, directives, and document markers must not be used in ci.yml while nextest reuse is enabled",
        replace_once(
            BASE_WORKFLOW,
            "permissions:\n",
            "? |\n  explicit key probe\npermissions:\n",
        ),
    )
    assert_error(
        "top-level env.JUST_VERSION must use a single-line scalar value",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            "  JUST_VERSION: >-\n    1.49.0\n",
        ),
    )
    assert_error(
        "top-level env.JUST_VERSION must use a single-line scalar value",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            "  JUST_VERSION: >2\n    1.49.0\n",
        ),
    )
    assert_error(
        "top-level env.JUST_VERSION must use a single-line scalar value",
        replace_once(
            BASE_WORKFLOW,
            '  JUST_VERSION: "1.49.0"\n',
            "  JUST_VERSION:\n",
        ),
    )
    nested_env_decoy = replace_once(
        BASE_WORKFLOW,
        '  JUST_VERSION: "1.49.0"\n',
        '  S3_DEPLOY_PATH: |\n    JUST_VERSION: "1.49.0"\n',
    )
    assert_error("top-level env.JUST_VERSION is reuse-scoped but missing from ci.yml", nested_env_decoy)
    folded_job_if = replace_once(
        BASE_WORKFLOW,
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name) && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}",
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name) && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main'\n      || github.event_name == 'pull_request' }}",
    )
    assert_error("nextest-fingerprint-reuse must use the canonical job if", folded_job_if)
    folded_job_if_with_canonical_first_line = replace_once(
        BASE_WORKFLOW,
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name) && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}",
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && contains(fromJSON('[\"pull_request\",\"workflow_dispatch\",\"merge_group\"]'), github.event_name) && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}\n      || github.event_name == 'pull_request'",
    )
    assert_error("nextest-fingerprint-reuse must use the canonical job if", folded_job_if_with_canonical_first_line)

    decoy_step = replace_once_after(
        narrowed_pathspec,
        "      - name: Determine build requirement",
        "      - name: Determine build requirement",
        """      - name: Decoy fingerprint reuse inputs
        run: |
          echo "id: fingerprint_reuse_inputs_changed"
          echo ".github/workflows/ci.yml .github/actions/setup-environment/action.yml ci/github-actions-runners.toml scripts/ci_provenance.py scripts/test_ci_provenance.py scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py"

      - name: Determine build requirement""",
    )
    assert_error("detector must detect fingerprint-reuse governance changes", decoy_step)

    stale_fingerprint_with_decoy = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        '          --current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"',
        '          --current-fingerprint "stale-fingerprint"',
    )
    stale_fingerprint_with_decoy = replace_once_after(
        stale_fingerprint_with_decoy,
        "  nextest-fingerprint-reuse:",
        "      - name: Resolve nextest fingerprint reuse",
        """      - name: Decoy resolver command
        run: |
          echo 'python3 scripts/ci_provenance.py resolve-fingerprint --current-run-id "${{ github.run_id }}" --current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}" | tee -a "$GITHUB_OUTPUT"'

      - name: Resolve nextest fingerprint reuse""",
    )
    assert_error("nextest-fingerprint-reuse must use secure current nextest fingerprint output", stale_fingerprint_with_decoy)
    assert_error("nextest-fingerprint-reuse must run ci_provenance.py resolve-fingerprint", stale_fingerprint_with_decoy)
    missing_base_probe = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        """      - name: Prepare trusted base provenance emitter
        id: reuse_provenance_base
        if: github.event_name == 'pull_request' || github.event_name == 'merge_group'
        shell: bash
        env:
          EVENT_NAME: ${{ github.event_name }}
          PR_NUMBER: ${{ github.event.pull_request.number || github.run_id }}
          PR_BASE_REF: ${{ github.event.pull_request.base.ref || '' }}
          PR_BASE_SHA: ${{ github.event.pull_request.base.sha || '' }}
          MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}
          MERGE_GROUP_BASE_SHA: ${{ github.event.merge_group.base_sha || '' }}
        run: |
          if [[ "$EVENT_NAME" == "pull_request" ]]; then
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
          echo "script=$provenance_script" >> "$GITHUB_OUTPUT"

""",
        "",
    )
    assert_error(
        "nextest-fingerprint-reuse must probe trusted base inherited emitter support",
        missing_base_probe,
    )
    missing_emitter_arg = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        '--require-inherited-emitter "$required_emitter"',
        '--omitted-inherited-emitter "$required_emitter"',
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical script", missing_emitter_arg)
    resolver_pipe_scalar = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Resolve nextest fingerprint reuse",
        "        run: |",
        "        run: >",
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical envelope", resolver_pipe_scalar)
    resolver_extra_workdir = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Resolve nextest fingerprint reuse",
        "        shell: bash\n",
        """        shell: bash
        working-directory: /tmp
""",
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical envelope", resolver_extra_workdir)
    resolver_extra_quoted_env = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        "          GITHUB_TOKEN: ${{ github.token }}\n",
        """          GITHUB_TOKEN: ${{ github.token }}
          "EXTRA": injected
""",
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical envelope", resolver_extra_quoted_env)
    resolver_extra_quoted_env_spaced_colon = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        "          GITHUB_TOKEN: ${{ github.token }}\n",
        """          GITHUB_TOKEN: ${{ github.token }}
          "EXTRA" : injected
""",
    )
    assert_error(
        "nextest-fingerprint-reuse resolver step must match canonical envelope",
        resolver_extra_quoted_env_spaced_colon,
    )
    fabricated_reuse_outputs = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        """          | tee -a "$GITHUB_OUTPUT\"""",
        """          | tee -a "$GITHUB_OUTPUT"
          ; echo "reuse_found=true" >> "$GITHUB_OUTPUT"
          ; echo "source_run_id=1" >> "$GITHUB_OUTPUT"
          ; echo "source_sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" >> "$GITHUB_OUTPUT"
          ; echo "source_artifact_id=1" >> "$GITHUB_OUTPUT\"""",
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical script", fabricated_reuse_outputs)

    assert_error(
        "detector must determine fingerprint_reuse_allowed",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Determine fingerprint reuse allowance",
            """          elif [[ "${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
""",
            """          elif [[ "${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
            echo "value=true" >> "$GITHUB_OUTPUT"
""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse allowance step must match canonical script",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Determine fingerprint reuse allowance",
            """          fi""",
            """          fi
          printf 'value=true\\n' >> "$GITHUB_OUTPUT\"""",
        ),
    )
    assert_error(
        "detector fingerprint-reuse allowance step must match canonical envelope",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Determine fingerprint reuse allowance",
            "        shell: bash\n",
            """        shell: bash
        continue-on-error: true
""",
        ),
    )

    assert_error(
        "gate shared verdict call must include --job nextest-fingerprint-reuse=${{ needs.nextest-fingerprint-reuse.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job nextest-fingerprint-reuse=${{ needs.nextest-fingerprint-reuse.result }}",
            "--job nextest-fingerprint-reuse=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --reuse-found",
        replace_once(
            BASE_WORKFLOW,
            '--reuse-found "${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || \'false\' }}"',
            '--reuse-omitted "${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || \'false\' }}"',
        ),
    )
    assert_error(
        "gate shared verdict call must include needs.nextest-fingerprint-reuse.outputs.reuse_found",
        replace_once(
            BASE_WORKFLOW,
            '--reuse-found "${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || \'false\' }}"',
            '--reuse-found "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"',
        ),
    )






@functools.cache
def ci_lint_suite_commands() -> set[str]:
    import run_ci_lint_suites as module
    return {" ".join(suite.command) for suite in module.CI_LINT_SUITES}


def assert_ci_lint_suite_runs(command: str, message: str) -> None:
    if command not in ci_lint_suite_commands():
        raise AssertionError(message)


def assert_ci_lint_runs_rust_verification_cache_retention_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_rust_verification_cache_retention.py",
        "ci-lint-workflow must run rust verification cache retention self-tests",
    )


def assert_ci_lint_runs_verify_remote_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_verify_remote.py",
        "ci-lint-workflow must run remote verification watcher self-tests",
    )


def assert_ci_lint_runs_ci_provenance_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_ci_provenance.py",
        "ci-lint-workflow must run CI provenance self-tests",
    )


def assert_ci_lint_runs_command_understanding_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_command_understanding.py",
        "ci-lint-workflow must run command understanding self-tests",
    )


def assert_ci_lint_runs_rust_probe_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_run_rust_probe.py",
        "ci-lint-workflow must run Rust Probe runner self-tests",
    )


def assert_ci_lint_runs_cancel_obsolete_dispatch_tests() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_cancel_obsolete_dispatch_runs.py",
        "ci-lint-workflow must run dispatch cancellation self-tests",
    )


def test_ci_test_manifest_self_tests_are_gated() -> None:
    assert_ci_lint_suite_runs(
        "python3 scripts/test_ci_test_manifest.py",
        "ci-lint-workflow must invoke scripts/test_ci_test_manifest.py so the manifest parser's self-tests are gated",
    )


def assert_github_scripts_are_repo_automation_fenced() -> None:
    verifier = load_verifier()
    expected_glob = (verifier.REPO_ROOT / ".github" / "scripts", "*.sh")
    if expected_glob not in verifier.DEFAULT_REPO_AUTOMATION_GLOBS:
        raise AssertionError(".github/scripts/*.sh must be covered by repo automation globs")
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    if ".github/scripts/*.sh" not in justfile:
        raise AssertionError("ci-lint-workflow must scan .github/scripts/*.sh")
    if ".github/actions/sccache-setup/action.yml" not in justfile:
        raise AssertionError("ci-lint-workflow must scan the shared sccache setup action")
    if ".github/actions/sccache-stats/action.yml" not in justfile:
        raise AssertionError("ci-lint-workflow must scan the shared sccache stats action")

    raw_cargo_message = "repo automation raw Cargo must use managed rust_verification wrapper"
    probe_script = (REPO_ROOT / ".github" / "scripts" / "run-rust-probe.sh").read_text(encoding="utf-8")
    clean_errors = verifier.verify_repo_automation_texts({".github/scripts/run-rust-probe.sh": probe_script})
    if any(raw_cargo_message in error for error in clean_errors):
        raise AssertionError(f"Rust Probe wrapper argv arrays must not be treated as raw cargo: {clean_errors!r}")

    raw_errors = verifier.verify_repo_automation_texts(
        {".github/scripts/future-sibling.sh": "#!/usr/bin/env bash\ncargo build\n"}
    )
    if not any(raw_cargo_message in error for error in raw_errors):
        raise AssertionError(f".github/scripts raw cargo drift was silent: {raw_errors!r}")

    array_errors = verifier.verify_repo_automation_texts(
        {".github/scripts/argv-array.sh": "#!/usr/bin/env bash\nprobe_args=(nextest run --locked --test target)\n"}
    )
    if any(raw_cargo_message in error for error in array_errors):
        raise AssertionError(f"plain argv array data must not be treated as a launch: {array_errors!r}")

    wrapper_array_errors = verifier.verify_repo_automation_texts(
        {
            ".github/scripts/wrapper-array.sh": (
                "#!/usr/bin/env bash\n"
                "probe_args=(nextest run --locked --test target)\n"
                'python3 scripts/rust_verification.py cargo --repo . -- "${probe_args[@]}"\n'
            )
        }
    )
    if any(raw_cargo_message in error for error in wrapper_array_errors):
        raise AssertionError(f"wrapper-routed argv array data must stay allowed: {wrapper_array_errors!r}")

    wrapper_star_array_errors = verifier.verify_repo_automation_texts(
        {
            ".github/scripts/wrapper-star-array.sh": (
                "#!/usr/bin/env bash\n"
                "probe_args=(nextest run --locked --test target)\n"
                'python3 scripts/rust_verification.py cargo --repo . -- "${probe_args[*]}"\n'
            )
        }
    )
    if any(raw_cargo_message in error for error in wrapper_star_array_errors):
        raise AssertionError(f"wrapper-routed star argv array data must stay allowed: {wrapper_star_array_errors!r}")

    cargo_array_errors = verifier.verify_repo_automation_texts(
        {
            ".github/scripts/cargo-array.sh": (
                "#!/usr/bin/env bash\n"
                "probe_args=(cargo build --release)\n"
                '"${probe_args[@]}"\n'
            )
        }
    )
    if not any(raw_cargo_message in error for error in cargo_array_errors):
        raise AssertionError(f"cargo array execution raw cargo drift was silent: {cargo_array_errors!r}")

    nextest_array_errors = verifier.verify_repo_automation_texts(
        {
            ".github/scripts/nextest-array.sh": (
                "#!/usr/bin/env bash\n"
                "probe_args=(nextest run --locked --test target)\n"
                '"${probe_args[@]}"\n'
            )
        }
    )
    if not any(raw_cargo_message in error for error in nextest_array_errors):
        raise AssertionError(f"nextest array execution raw cargo drift was silent: {nextest_array_errors!r}")

    star_array_errors = verifier.verify_repo_automation_texts(
        {
            ".github/scripts/star-array.sh": (
                "#!/usr/bin/env bash\n"
                "probe_args=(nextest run --locked --test target)\n"
                '"${probe_args[*]}"\n'
            )
        }
    )
    if not any(raw_cargo_message in error for error in star_array_errors):
        raise AssertionError(f"star array execution raw cargo drift was silent: {star_array_errors!r}")

    cargo_array_data_errors = verifier.verify_repo_automation_texts(
        {".github/scripts/cargo-array-data.sh": "#!/usr/bin/env bash\nprobe_args=(cargo install --git https://example.invalid/tool.git)\n"}
    )
    if not any(raw_cargo_message in error for error in cargo_array_data_errors):
        raise AssertionError(f"cargo-led argv array data raw cargo drift was silent: {cargo_array_data_errors!r}")

    substitution_errors = verifier.verify_repo_automation_texts(
        {".github/scripts/array-substitution.sh": "#!/usr/bin/env bash\nprobe_args=($(cargo build))\n"}
    )
    if not any(raw_cargo_message in error for error in substitution_errors):
        raise AssertionError(f"array command substitution raw cargo drift was silent: {substitution_errors!r}")


def assert_cargo_zigbuild_probe_has_no_redundant_true() -> None:
    workflow = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    redundant = 'test -x "$HOME/.cargo/bin/cargo-zigbuild" && true'
    if redundant in workflow:
        raise AssertionError("cargo-zigbuild executable probe must not use redundant && true")


def main() -> int:
    assert_ci_lint_runs_rust_verification_cache_retention_tests()
    assert_ci_lint_runs_verify_remote_tests()
    assert_ci_lint_runs_ci_provenance_tests()
    assert_ci_lint_runs_command_understanding_tests()
    assert_ci_lint_runs_rust_probe_tests()
    assert_ci_lint_runs_cancel_obsolete_dispatch_tests()
    test_ci_test_manifest_self_tests_are_gated()
    assert_github_scripts_are_repo_automation_fenced()
    assert_cargo_zigbuild_probe_has_no_redundant_true()
    assert_binance_timestamp_archive_execution_proof_gaps_are_reported()
    assert_test_archive_filtered_run_recipe_preserves_filterset_argv()
    real_ci_workflow = repo_workflow_text(".github/workflows/ci.yml")
    assert_clean(workflow=real_ci_workflow)
    root_just_version_bump = replace_once(
        real_ci_workflow,
        '  JUST_VERSION: "1.49.0"\n',
        '  JUST_VERSION: "1.49.1"\n',
    )
    root_just_version_bump_errors = load_verifier().verify_text(
        root_just_version_bump,
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    if root_just_version_bump_errors:
        raise AssertionError(f"root JUST_VERSION bump must be boundary-clean, got: {root_just_version_bump_errors}")
    root_job_body_edit = replace_once_after(
        real_ci_workflow,
        "      - name: Run nextest archive partitions",
        '            echo "reproduce locally: just test-archive-run',
        '            echo "probe reproduce locally: just test-archive-run',
    )
    root_job_body_boundary_errors = load_verifier().partition_workflow_boundary_errors(
        root_job_body_edit,
        "ci.yml",
    )
    if root_job_body_boundary_errors:
        raise AssertionError(f"root job body edit must not trip top-level boundary, got: {root_job_body_boundary_errors}")
    root_duplicate_env_path = replace_once(
        real_ci_workflow,
        "permissions:\n",
        "env:\n  PATH: /tmp/shim\npermissions:\n",
    )
    assert_error("ci.yml duplicate top-level key 'env'", root_duplicate_env_path)
    root_duplicate_jobs = replace_once(
        real_ci_workflow,
        "permissions:\n",
        "jobs: {}\npermissions:\n",
    )
    assert_error("ci.yml duplicate top-level key 'jobs'", root_duplicate_jobs)
    if "  test-archive:\n" not in BASE_WORKFLOW or "      - name: Run nextest archive partitions\n" not in BASE_WORKFLOW:
        raise AssertionError("BASE_WORKFLOW must include the root test-archive partition fixture")
    real_ci_errors = load_verifier().verify_text(
        real_ci_workflow,
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    if real_ci_errors:
        raise AssertionError(f"real ci.yml must be clean, got: {real_ci_errors}")
    assert_workflows_clean({"ci.yml": real_ci_workflow, "advisory.yml": BASE_ADVISORY_WORKFLOW})
    assert_ci_workflow_run_name_matches_dispatch_config()
    assert_pin_consistency_cross_file_mismatch_errors()
    assert_pin_consistency_same_sha_no_error()
    assert_pin_consistency_includes_setup_action()
    assert_pin_consistency_rejects_mutable_tag()
    assert_pin_consistency_ignores_non_uses_mentions()
    assert_pin_consistency_accepts_uppercase_sha()
    assert_pin_consistency_intra_file_mismatch_uses_pin_drift_wording()
    assert_pin_consistency_rejects_multi_line_mutable_tag()
    assert_pin_consistency_rejects_block_scalar_mutable_tag()
    assert_pin_consistency_rejects_multi_line_valid_sha()
    assert_pin_consistency_accepts_double_quoted_sha()
    assert_pin_consistency_accepts_single_quoted_sha()
    assert_pin_consistency_rejects_mismatched_quotes()
    assert_prebuilt_tool_installs_accepts_uppercase_pinned_install_action()
    assert_v6_red_yaml_steps_aliases_are_rejected()
    assert_v6_red_local_composite_actions_are_scanned()
    assert_v6_red_additional_workflows_are_scanned()
    assert_artifact_retention_policy_spine_gaps_are_reported()
    assert_artifact_retention_config_ref_retention_is_exact()
    assert_ci_provenance_upload_name_comes_from_config_template()
    assert_artifact_retention_config_refs_are_validated()
    assert_workflow_hygiene_reviewer_regressions()
    assert_error("workflow must define PR-only concurrency", without_pr_concurrency(BASE_WORKFLOW))
    assert_error(
        "workflow permissions must include issues: read",
        replace_once(BASE_WORKFLOW, "  issues: read\n", ""),
    )
    assert_error(
        "concurrency group must use the canonical ready PR metadata iteration predicate",
        replace_once(
            BASE_WORKFLOW,
            "            || (github.event.pull_request.draft == false\n"
            "                && github.event.action == 'edited'\n"
            "                && !(github.event.changes.base.ref.from && true || false)))\n"
            "        && format('pr-{0}-iteration', github.event.number)",
            "            || (github.event.pull_request.draft == false\n"
            "                && github.event.action == 'edited')\n"
            "        && format('pr-{0}-iteration', github.event.number)",
        ),
    )
    assert_error(
        "gate name must come from ci-policy gate_name output",
        replace_once(BASE_WORKFLOW, GATE_NAME, "name: gate"),
    )
    assert_error(
        "gate shared verdict call must include --expected-event-class",
        replace_once(
            BASE_WORKFLOW,
            '--expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}"',
            '--expected-event-class "docs"',
        ),
    )
    for marker, replacement in (
        (
            "if: github.event_name == 'pull_request' || github.event_name == 'merge_group'",
            "if: github.event_name == 'pull_request'",
        ),
        (
            "MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}",
            "MERGE_GROUP_BASE_REF: ''",
        ),
        (
            "MERGE_GROUP_BASE_SHA: ${{ github.event.merge_group.base_sha || '' }}",
            "MERGE_GROUP_BASE_SHA: ''",
        ),
        (
            'git check-ref-format "refs/heads/$base_branch"',
            "echo skip-base-ref-format-check",
        ),
        (
            'git fetch --no-tags origin "+${base_sha}:${base_ref}"',
            'git fetch --no-tags origin "+refs/heads/${base_branch}:${base_ref}"',
        ),
        (
            'git archive "$base_ref" scripts/ ci/github-actions-runners.toml',
            'git archive "$base_ref" scripts/',
        ),
    ):
        assert_error(
            "gate must use pinned trusted base-tree ci_provenance.py verdict",
            replace_once_after(BASE_WORKFLOW, "  gate:\n", marker, replacement),
        )
    for marker, replacement in (
        (
            "steps.verdict_base.outputs.script",
            "steps.verdict_base.outputs.local_script",
        ),
        (
            'python3 "$verdict_script" check-ci-gate',
            'python3 "$verdict_script" unchecked-ci-gate',
        ),
    ):
        mutated_workflow = replace_once_after(BASE_WORKFLOW, "  gate:\n", marker, replacement)
        assert_error(
            f"gate must use trusted base-tree ci_provenance.py check-ci-gate verdict ({marker})",
            mutated_workflow,
        )
    assert_error(
        "concurrency group must split iteration PR runs from full CI runs",
        replace_once(BASE_WORKFLOW, "format('pr-{0}-iteration', github.event.number)", "github.ref_name"),
    )
    assert_error(
        "concurrency group must keep non-PR runs isolated by ref and SHA",
        replace_once(BASE_WORKFLOW, "format('{0}-{1}', github.ref_name, github.sha)", "github.ref_name"),
    )
    assert_error(
        "cancel-in-progress must not cancel merge_group queue validations",
        replace_once(
            BASE_WORKFLOW,
            """cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        || github.event_name == 'workflow_dispatch' }}""",
            "cancel-in-progress: true",
        ),
    )
    assert_error(
        "concurrency group must branch on pull_request event",
        replace_once(
            BASE_WORKFLOW,
            """  group: >-
    ${{ github.event_name == 'pull_request'
        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !((github.event.action == 'edited'
              && !(github.event.changes.base.ref.from && true || false))
             || (github.event.pull_request.draft == true
                 && github.event.action == 'converted_to_draft'))
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)
        || github.event_name == 'pull_request'
        && ((github.event.pull_request.draft == true
             && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action))
            || (github.event.pull_request.draft == false
                && github.event.action == 'edited'
                && !(github.event.changes.base.ref.from && true || false)))
        && format('pr-{0}-iteration', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'workflow_dispatch'
        && format('{0}-dispatch-iteration', github.ref_name)
        || github.event_name == 'merge_group'
        && format('mq-{0}', github.ref)
        || format('{0}-{1}', github.ref_name, github.sha) }}""",
            "  group: format('pr-{0}', github.event.number)",
        ),
    )
    assert_error(
        "concurrency group must branch on pull_request event",
        BASE_WORKFLOW.replace("github.event_name == 'pull_request'", "github.event_name != 'pull_request'"),
    )
    assert_error(
        "workflow_dispatch runs must use the iteration concurrency group",
        replace_once(
            BASE_WORKFLOW,
            "        || github.event_name == 'workflow_dispatch'\n"
            "        && format('{0}-dispatch-iteration', github.ref_name)\n",
            "",
        ),
    )
    assert_parse_jobs_strips_comments()
    assert_strip_comment_handles_single_quoted_backslash()
    assert_command_parse_cache_is_transparent()
    assert_decorator_spacing_guard_covers_reviewed_evasions()
    assert_no_decorators_separated_from_targets()
    assert_relocated_symbols_keep_legacy_exports()
    assert_giant_twin_import_guard_ignores_fixture_strings()
    assert_relocated_tests_do_not_import_giant_twin()
    assert_required_job_indentation_is_actionable()
    assert_body_exits_requires_top_level_exit()
    assert_nextest_live_node_group_required()
    assert_nextest_live_node_group_covers_bolt_v3_builders()
    assert_nextest_live_node_group_uses_manifest_harness_scope()
    assert_nextest_live_node_group_accepts_manifest_standalone_member()
    test_harness_manifest_requires_autotests_false()
    test_harness_manifest_rejects_orphan_test_members()
    test_harness_manifest_rejects_double_modded_members()
    test_harness_manifest_rejects_unreferenced_top_level_files()
    test_harness_manifest_enforces_expected_harness_count()
    test_harness_manifest_rejects_harness_roots_as_members()
    test_harness_manifest_masks_inner_attrs_and_rejects_crate_attrs()
    test_harness_manifest_rejects_retired_member_test_filters()
    test_harness_manifest_rejects_typo_positional_test_filter()
    test_harness_manifest_rejects_quoted_retired_member_test_flag()
    test_nextest_config_rejects_surprise_binary_overrides()
    test_nextest_config_rejects_regex_form_binary_overrides()
    test_nextest_config_rejects_regex_binary_smuggled_into_live_node_override()
    test_nextest_config_rejects_foreign_test_prefix_in_live_node_override()
    for job in (
        "detector",
        "deny",
        "clippy",
        "check-aarch64",
        "source-fence",
        "nextest-fingerprint",
        "test-archive",
        "test",
        "build",
        "same-sha-main-evidence",
        "gate",
        "deploy",
    ):
        assert_error(f"missing required job {job}", without_job(BASE_WORKFLOW, job))
    for job in ("detector", "deny", "clippy", "check-aarch64", "source-fence", "test", "build"):
        assert_error("gate needs " + job, replace_once(BASE_WORKFLOW, GATE_NEEDS, without_inline_need(GATE_NEEDS, job)))
        assert_error(
            f"gate shared verdict call must include --job {job}=${{{{ needs.{job}.result }}}}",
            replace_once(
                BASE_WORKFLOW,
                f"--job {job}=${{{{ needs.{job}.result }}}}",
                f"--job {job}=${{{{ omitted.{job}.result }}}}",
            ),
        )
    for job in (
        "gate",
        "same-sha-main-evidence",
        "build",
        "detector",
        "deny",
        "clippy",
        "check-aarch64",
        "source-fence",
        "test",
    ):
        assert_error("deploy needs " + job, replace_once(BASE_WORKFLOW, DEPLOY_NEEDS, without_inline_need(DEPLOY_NEEDS, job)))
    assert_error(
        "check-aarch64 needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  check-aarch64:\n    name: check-aarch64\n    needs: [ci-policy, detector]",
            "  check-aarch64:\n    name: check-aarch64\n    needs: ci-policy",
        ),
    )
    assert_error(
        "check-aarch64 must run just check-aarch64",
        replace_once(
            BASE_WORKFLOW,
            "      - if: needs.detector.outputs.build_required != 'true'\n        run: just check-aarch64",
            "      - if: needs.detector.outputs.build_required != 'true'\n        run: echo skip check-aarch64",
        ),
    )
    assert_error(
        "check-aarch64 must install aarch64 cross compiler packages",
        replace_once(
            BASE_WORKFLOW,
            "        run: sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross",
            "        run: sudo apt-get install -y gcc-aarch64-linux-gnu",
        ),
    )
    assert_error(
        "check-aarch64 must document build-lane aarch64 coverage delegation",
        BASE_WORKFLOW.replace(
            """      - name: Resolve aarch64 coverage owner
        run: |
          if [[ "${{ needs.detector.outputs.build_required }}" == "true" ]]; then
            echo "build_required=true; aarch64 coverage is provided by build"
          else
            echo "build_required=false; running standalone aarch64 check"
          fi
""",
            "",
        ),
    )
    assert_error(
        "check-aarch64 setup must run only when build_required is not true",
        replace_once(
            BASE_WORKFLOW,
            "      - uses: ./.github/actions/setup-environment\n        if: needs.detector.outputs.build_required != 'true'",
            "      - uses: ./.github/actions/setup-environment",
        ),
    )
    assert_error(
        "check-aarch64 compiler install must run only when build_required is not true",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Install aarch64 cross compiler\n        if: needs.detector.outputs.build_required != 'true'",
            "      - name: Install aarch64 cross compiler",
        ),
    )
    assert_error(
        "check-aarch64 cache must run only when build_required is not true",
        replace_once(
            BASE_WORKFLOW,
            "      - uses: Swatinem/rust-cache@example\n        if: needs.detector.outputs.build_required != 'true'",
            "      - uses: Swatinem/rust-cache@example",
        ),
    )
    assert_error(
        "check-aarch64 managed target cache must run only when build_required is not true",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore check-aarch64 managed target cache\n        id: check-aarch64-managed-target-cache\n        uses: actions/cache/restore@example\n        if: needs.detector.outputs.build_required != 'true'",
            "      - name: Restore check-aarch64 managed target cache\n        id: check-aarch64-managed-target-cache\n        uses: actions/cache/restore@example",
        ),
    )
    assert_error(
        "check-aarch64 command must run only when build_required is not true",
        replace_once(
            BASE_WORKFLOW,
            "      - if: needs.detector.outputs.build_required != 'true'\n        run: just check-aarch64",
            "      - run: just check-aarch64",
        ),
    )
    assert_error(
        "ci.yml check-aarch64 must include build values",
        replace_once(
            BASE_WORKFLOW,
            '          just-version: ${{ env.JUST_VERSION }}\n'
            '          include-build-values: "true"\n'
            '          use-default-target: "true"',
            '          just-version: ${{ env.JUST_VERSION }}\n'
            '          # include-build-values: "true"\n'
            '          use-default-target: "true"',
        ),
    )
    assert_error(
        "ci.yml check-aarch64 must use default target",
        replace_once(BASE_WORKFLOW, '          use-default-target: "true"', '          # use-default-target: "true"'),
    )
    assert_error(
        "check-aarch64 must use isolated managed target cache",
        replace_once(
            BASE_WORKFLOW,
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}",
        ),
    )
    assert_error(
        "deny must use shared Cargo registry/git cache key",
        replace_once(BASE_WORKFLOW, "          shared-key: cargo-registry-git-v1", "          key: deny"),
    )
    assert_error(
        "deny shared Cargo registry/git cache must disable cargo bin caching",
        replace_once(BASE_WORKFLOW, "          cache-bin: false", "          cache-bin: true"),
    )
    assert_error(
        "deny shared Cargo registry/git cache must not include target directories",
        replace_once(
            BASE_WORKFLOW,
            "          cache-targets: false\n          shared-key: cargo-registry-git-v1",
            "          cache-targets: false\n          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n          shared-key: cargo-registry-git-v1",
        ),
    )
    assert_error(
        "deny shared Cargo registry/git cache must not include target directories",
        replace_once(
            BASE_WORKFLOW,
            "          cache-targets: false\n          shared-key: cargo-registry-git-v1",
            "          cache-targets: false\n          cache-directories:\n            - ${{ steps.setup.outputs.managed_target_dir }}\n          shared-key: cargo-registry-git-v1",
        ),
    )
    assert_error(
        "deny must use only shared Cargo registry/git rust-cache blocks",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Install cargo-deny\n",
            "      - uses: Swatinem/rust-cache@example\n        with:\n          cache-on-failure: true\n          cache-bin: true\n          cache-targets: true\n          key: deny-targets\n      - name: Install cargo-deny\n",
        ),
    )
    assert_error(
        "clippy must use isolated managed target cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore clippy managed target cache\n        id: clippy-managed-target-cache\n        uses: actions/cache/restore@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n",
            "",
        ),
    )
    assert_error(
        "build managed target cache key must isolate build-aarch64-release",
        replace_once(
            BASE_WORKFLOW,
            "managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-",
            "managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        ),
    )
    assert_error(
        "deny shared Cargo registry/git cache save must be main-only",
        replace_once(
            BASE_WORKFLOW,
            "          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}",
            "          save-if: true",
        ),
    )
    assert_error(
        "deny shared Cargo registry/git cache save must be main-only",
        replace_once(
            BASE_WORKFLOW,
            "          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}",
            "          cache-comment: |\n            save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}",
        ),
    )
    assert_clean(
        replace_once(
            BASE_WORKFLOW,
            "          cache-bin: false",
            '          cache-bin: "false"',
        )
    )
    assert_error(
        "test-shards job must not reintroduce nextest archive artifact fan-out",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test",
            """  test-shards:
    name: nextest shard ${{ matrix.shard }} of 4
    runs-on: ubuntu-latest
    steps:
      - name: Download nextest archive
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1

  test:
    name: test""",
        ),
    )
    assert_error(
        "test-archive must run all nextest archive partitions",
        replace_once(BASE_WORKFLOW, '          for shard in $(seq 1 "$shards"); do', "          for shard in $(seq 1 3); do"),
    )
    assert_error(
        "test-archive must run partitioned nextest from local archive",
        replace_once(
            BASE_WORKFLOW,
            'just test-archive-run "$NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/nextest-archive-extract" --partition "count:${shard}/${shards}"',
            "just test",
        ),
    )
    assert_error(
        "test-archive must create nextest archive extract root",
        replace_once(BASE_WORKFLOW, '          mkdir -p "$RUNNER_TEMP/nextest-archive-extract"\n', ""),
    )
    assert_error(
        "ci-policy must summarize CI classification",
        replace_once(
            BASE_WORKFLOW,
            '          echo "CI classification: class=${class} policy=${CI_POLICY_PATH:-unknown} full_ci_required=${FULL_CI_REQUIRED:-false} event_class=${EXPECTED_EVENT_CLASS:-unknown} reason=${POLICY_REASON:-missing}" >> "$GITHUB_STEP_SUMMARY"\n',
            "",
        ),
    )
    assert_error(
        "test must summarize nextest fingerprint reuse decision",
        replace_once(
            BASE_WORKFLOW,
            '          echo "Nextest reuse: decision=${decision} detector_allowed=${detector_allowed} reuse_found=${reuse_found} source_run=${source_run:-none} source_sha=${source_sha:-none} artifact=${artifact:-none} reason=${reason:-none}" >> "$GITHUB_STEP_SUMMARY"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must emit restore result and reason outputs",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive from S3",
            '          echo "restore-result=miss" >> "$GITHUB_OUTPUT"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must summarize nextest archive S3 restore state",
        replace_once(
            BASE_WORKFLOW,
            '            echo "Root nextest archive S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${archive_restore} reason=${archive_reason}"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must summarize root binary sidecars S3 restore state",
        replace_once(
            BASE_WORKFLOW,
            '            echo "Root binary sidecars S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${sidecar_restore} reason=${sidecar_reason}"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must log partition diagnostics",
        replace_once(
            BASE_WORKFLOW,
            '            echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <extract-root> --partition count:${shard}/${shards}"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must aggregate partition failures",
        replace_once(BASE_WORKFLOW, "              status=1\n", ""),
    )
    assert_error(
        "test-archive must aggregate partition failures",
        replace_once(
            BASE_WORKFLOW,
            '            if [[ "$rc" -ne 0 ]]; then\n',
            '            if [[ "$rc" == 0 ]]; then\n',
        ),
    )
    root_partition_rc_condition_probe = replace_once_after(
        real_ci_workflow,
        "      - name: Run nextest archive partitions",
        '            if [[ "$rc" -ne 0 ]]; then\n',
        '            if [[ "$rc" == 0 ]]; then\n',
    )
    assert_error("ROOT_TEST_ARCHIVE_JOB_SHA256", root_partition_rc_condition_probe)
    root_partition_comment_probe = replace_once_after(
        real_ci_workflow,
        "      - name: Run nextest archive partitions",
        '            set +e\n',
        '            #\n            set +e\n',
    )
    assert_error("ROOT_TEST_ARCHIVE_JOB_SHA256", root_partition_comment_probe)
    root_partition_github_path_probe = insert_step_before_named_step_after(
        real_ci_workflow,
        "  test-archive:",
        "Run nextest archive partitions",
        """      - name: Install partition shim
        shell: bash
        run: printf '%s\\n' "$GITHUB_WORKSPACE/.ci-shims" | tee -a "$GITHUB_PATH"
""",
    )
    assert_error("ROOT_TEST_ARCHIVE_JOB_SHA256", root_partition_github_path_probe)
    root_partition_job_continue_on_error_probe = replace_once_after(
        real_ci_workflow,
        "  test-archive:",
        "    name: nextest archive\n",
        "    name: nextest archive\n    continue-on-error: true\n",
    )
    assert_error("ROOT_TEST_ARCHIVE_JOB_SHA256", root_partition_job_continue_on_error_probe)
    root_partition_job_defaults_probe = replace_once_after(
        real_ci_workflow,
        "  test-archive:",
        "    steps:\n",
        "    defaults:\n      run:\n        shell: sh\n    steps:\n",
    )
    assert_error("ROOT_TEST_ARCHIVE_JOB_SHA256", root_partition_job_defaults_probe)
    root_top_level_path_env = replace_once(
        real_ci_workflow,
        "env:\n  JUST_VERSION: \"1.49.0\"\n",
        "env:\n  PATH: /tmp/partition-shim\n  JUST_VERSION: \"1.49.0\"\n",
    )
    assert_error("top-level env.PATH must be classified as reuse-scoped or build-neutral", root_top_level_path_env)
    for defaults_probe in (
        "defaults:\n  run:\n    shell: sh\n",
        'defaults: {"run": {shell: bash}}\n',
        "? defaults\n: {run: {working-directory: scripts}}\n",
        "? defaults : {run: {shell: bash}}\n",
        "!!map defaults: {run: {shell: bash}}\n",
        "defaults: &defaults_probe {run: {shell: bash}}\n",
        '"defaults":\n  run:\n    shell: sh\n',
        "evil_top: {a: b}\n",
    ):
        root_top_level_defaults = replace_once(
            real_ci_workflow,
            "permissions:\n",
            f"{defaults_probe}permissions:\n",
        )
        assert_error("ci.yml top-level entry", root_top_level_defaults)
    root_partition_step_rename_probe = replace_once_after(
        real_ci_workflow,
        "  test-archive:",
        "      - name: Run nextest archive partitions\n",
        "      - name: Run renamed nextest archive partitions\n",
    )
    root_partition_step_rename_errors = load_verifier().verify_workflow(root_partition_step_rename_probe)
    assert (
        "test-archive must define Run nextest archive partitions step"
        in root_partition_step_rename_errors
    ), root_partition_step_rename_errors
    assert_error(
        "test-archive partition failures must preserve shard exit codes",
        replace_once(
            BASE_WORKFLOW,
            '            rc="${PIPESTATUS[0]}"\n',
            '            rc="0"\n',
        ),
    )
    assert_error(
        "test-archive partition failures must emit shard error annotations",
        replace_once(
            BASE_WORKFLOW,
            '              echo "::error title=nextest archive partition failed::shard=${shard}/${shards} exit=${rc}"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must use only shared Cargo registry/git rust-cache blocks",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive from S3",
            "      - uses: Swatinem/rust-cache@example\n"
            "        with:\n"
            "          cache-targets: true\n"
            "          workspaces: . -> ${{ steps.setup.outputs.managed_target_dir_relative }}\n"
            "          key: nextest-archive-build-v1\n"
            "      - name: Restore nextest archive from S3",
        ),
    )
    assert_error(
        "test-archive must opt into managed target dir",
        replace_once(
            BASE_WORKFLOW,
            """      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          include-managed-target-dir: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.test-archive
      - name: Resolve root nextest cache keys""",
            """      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.test-archive
      - name: Resolve root nextest cache keys""",
        ),
    )
    assert_error(
        "test-archive must restore archive build target cache",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Restore archive build target cache
        id: test-target-cache
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-
""",
            "",
        ),
    )
    assert_error(
        "test-archive must save archive build target cache",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Save archive build target cache
        id: test-target-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}
""",
            "",
        ),
    )
    assert_error(
        "test-archive must save target cache only on target cache miss",
        replace_once(
            BASE_WORKFLOW,
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}\n",
            "",
        ),
    )
    assert_error(
        "test-archive cache persistence keys must come from single-source cache key outputs",
        replace_once(
            BASE_WORKFLOW,
            "archive_build_target_cache_key=managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
            "archive_build_target_cache_key=managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock') }}",
        ),
    )
    assert_error(
        "test-archive must not save a second archive-build cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive from S3",
            "      - name: Archive build cache key marker\n"
            "        run: echo nextest-archive-build-v1\n"
            "      - name: Restore nextest archive from S3",
        ),
    )
    assert_error(
        "test-archive must restore nextest archive from S3 fail-open",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive from S3\n        id: nextest-archive-cache\n",
            "",
        ),
    )
    assert_error(
        "test-archive must fail closed on nextest archive S3 digest mismatch",
        replace_once(
            BASE_WORKFLOW,
            '          if [[ "$metadata_digest" != "$DIGEST" ]]; then\n',
            '          if [[ "$metadata_digest" == "$DIGEST" ]]; then\n',
        ),
    )
    assert_error(
        "test-archive must fail closed on root binary sidecar S3 digest mismatch",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Restore root binary sidecars from S3",
            '          if [[ "$metadata_digest" != "$DIGEST" ]]; then\n',
            '          if [[ "$metadata_digest" == "$DIGEST" ]]; then\n',
        ),
    )
    assert_error(
        "test-archive must save nextest archive to S3 only from push-to-main",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Save nextest archive to S3\n        id: nextest-archive-cache-save\n",
            "",
        ),
    )
    assert_error(
        "test-archive must save nextest archive to S3 only from push-to-main with write credentials",
        replace_once(
            BASE_WORKFLOW,
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.nextest-artifact-cache-aws.outcome == 'success' && steps.nextest-archive-cache.outputs.cache-hit != 'true' }}\n",
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.nextest-archive-cache.outputs.cache-hit != 'true' }}\n",
        ),
    )
    assert_error(
        "test-archive must save root binary sidecar to S3 only from push-to-main with write credentials",
        replace_once(
            BASE_WORKFLOW,
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.nextest-artifact-cache-aws.outcome == 'success' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true' }}\n",
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true' }}\n",
        ),
    )
    assert_error(
        "test-archive must emit explicit nextest archive S3 save status",
        replace_once(
            BASE_WORKFLOW,
            '            echo "save-status=failed" >> "$GITHUB_OUTPUT"\n',
            "",
        ),
    )
    assert_error(
        "test-archive must emit explicit root binary sidecar S3 save status",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Save root binary sidecars to S3",
            '            echo "save-status=failed" >> "$GITHUB_OUTPUT"\n',
            "",
        ),
    )
    assert_error(
        "nextest archive cache key must use nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            "nextest_archive_cache_key=${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
            "nextest_archive_cache_key=nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock') }}",
        ),
    )
    assert_error(
        "managed target cache keys must use Rust-relevant inputs only",
        replace_once(
            BASE_WORKFLOW,
            "hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml')",
            "hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'justfile')",
        ),
    )
    assert_error(
        "test-archive payloads must use S3 artifact cache, not GitHub Actions cache",
        replace_once(
            BASE_WORKFLOW,
            "      NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}",
            "      # NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}",
        ),
    )
    assert_error(
        "managed target cache saves must be push-to-main only",
        replace_once(
            BASE_WORKFLOW,
            "        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.clippy-managed-target-cache.outputs.cache-hit != 'true' }}",
            "        if: ${{ steps.clippy-managed-target-cache.outputs.cache-hit != 'true' }}",
        ),
    )
    # #400: every managed-target cache must declare a restore-keys prefix fallback.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    assert_error(
        "test-archive managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-",
        replace_once(
            BASE_WORKFLOW,
            "          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-\n      - name: Install cargo-nextest",
            "          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}\n      - name: Install cargo-nextest",
        ),
    )
    assert_error(
        "check-aarch64 managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-\n      - if: needs.detector.outputs.build_required != 'true'",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n      - if: needs.detector.outputs.build_required != 'true'",
        ),
    )
    assert_error(
        "source-fence managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-\n      - run: |",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n      - run: |",
        ),
    )
    assert_error(
        "build managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-\n      - name: Install zig",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n      - name: Install zig",
        ),
    )
    # #400 parser tightness: the inline-scalar form of restore-keys is a valid
    # YAML alternative to the block-scalar (`|`) form. The verifier must accept
    # both. Uses clippy's block-scalar declaration as the conversion source.
    assert_clean(
        workflow=replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          restore-keys: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: a restore-keys block-scalar declaring an unrelated
    # cache family prefix must fail the per-job prefix check.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          restore-keys: |\n            nextest-archive-v1-\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: an empty block-scalar body (no prefix line under
    # `restore-keys: |`) must not be treated as a satisfied restore-keys.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          restore-keys: |\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: YAML 1.2 §8.1.1 allows a block-scalar header to
    # carry an explicit indentation indicator (e.g., `|2`, `|-3`, `>+1`) in
    # addition to the bare/chomping forms. Currently the verifier only
    # recognises six fixed forms (`|`, `>`, `|-`, `>-`, `|+`, `>+`); any
    # block-scalar header containing an explicit indentation digit is
    # silently skipped by the body-scan and the prefix check spuriously
    # fails on an otherwise-valid restore-keys declaration. The fixture
    # below switches clippy's `|` marker to `|2` (content indent 12 = 10 + 2
    # relative to the `restore-keys:` line at indent 10, matching the YAML
    # 1.2 spec).
    assert_clean(
        workflow=replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          restore-keys: |2\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: the body-scan that locates the `restore-keys:`
    # marker line uses an unscoped substring match (`"restore-keys:" in
    # text`) and walks the entire step block from the top. A step-level
    # `name:` carrying the literal substring `restore-keys:` (which survives
    # `strip_comment` because the substring is inside a double-quoted
    # scalar) appears before the real `restore-keys:` input line, so the
    # body-scan anchors on the wrong line; with `marker_indent` set to the
    # step-level indent (8), the next line (`with:` at indent 8) ends the
    # sub-scan immediately, the real block-scalar body (indent 12) is never
    # consulted, and the prefix check spuriously fails. After the fix, the
    # body-scan must anchor on the actual `restore-keys:` input line (the
    # one whose `block_input_items` entry produced the block-scalar marker).
    assert_clean(
        workflow=replace_once(
            BASE_WORKFLOW,
            "      - name: Restore clippy managed target cache\n        id: clippy-managed-target-cache\n        uses: actions/cache/restore@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "      - name: \"Cache with restore-keys: probe\"\n        id: clippy-managed-target-cache\n        uses: actions/cache/restore@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    assert_error(
        "test-archive build must be skipped on archive cache hit",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Build nextest archive\n        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'",
            "      - name: Build nextest archive",
        ),
    )
    assert_error(
        "test-archive must extract cached root binary sidecars",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Extract root binary sidecars
        if: steps.root-bin-sidecars-cache.outputs.cache-hit == 'true'
        run: |
          mkdir -p "${{ steps.setup.outputs.managed_target_dir }}"
          tar -xzf "$ROOT_BIN_SIDECARS_PATH" -C "${{ steps.setup.outputs.managed_target_dir }}"
""",
            "",
        ),
    )
    assert_error(
        "test-archive must build CARGO_BIN_EXE sidecars on sidecar cache miss",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Build root binary sidecars
        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_DEV_DEBUG: "0"
        run: |
          python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo "$GITHUB_WORKSPACE" -- build --locked --bins
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          python3 scripts/root_bin_sidecars.py pack \
            --repo-root "$GITHUB_WORKSPACE" \
            --target-dir "$target_dir" \
            --output "$GITHUB_WORKSPACE/$ROOT_BIN_SIDECARS_PATH"
""",
            "",
        ),
    )
    assert_error(
        "test-archive must pack root binary sidecars from archive builds on archive-cache miss",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Pack root binary sidecars from archive build
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        run: |
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          python3 scripts/root_bin_sidecars.py pack \
            --repo-root "$GITHUB_WORKSPACE" \
            --target-dir "$target_dir" \
            --output "$GITHUB_WORKSPACE/$ROOT_BIN_SIDECARS_PATH"
""",
            "",
        ),
    )
    assert_error(
        "test-archive archive-miss sidecar pack must use tracked root binary sidecar helper",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Pack root binary sidecars from archive build
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        run: |
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          python3 scripts/root_bin_sidecars.py pack \
            --repo-root "$GITHUB_WORKSPACE" \
            --target-dir "$target_dir" \
            --output "$GITHUB_WORKSPACE/$ROOT_BIN_SIDECARS_PATH"
""",
            """      - name: Pack root binary sidecars from archive build
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        run: |
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          find "$target_dir/debug" -maxdepth 1 -type f -perm -111 -print0
""",
        ),
    )
    sidecar_build_guard_regression_workflow = BASE_WORKFLOW
    if (
        "        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'\n"
        in BASE_WORKFLOW
    ):
        sidecar_build_guard_regression_workflow = replace_once(
            BASE_WORKFLOW,
            "        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'\n",
            "        if: steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'\n",
        )
    assert_error(
        "test-archive sidecar cargo build must run only on archive-cache hit and sidecar-cache miss",
        sidecar_build_guard_regression_workflow,
    )
    assert_error(
        "test-archive sidecar build must use dev profile debug knob",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Build root binary sidecars
        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_DEV_DEBUG: "0"
""",
            """      - name: Build root binary sidecars
        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_TEST_DEBUG: "0"
""",
        ),
    )
    assert_error(
        "test-archive sidecar build must use tracked root binary sidecar helper",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Build root binary sidecars
        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_DEV_DEBUG: "0"
        run: |
          python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo "$GITHUB_WORKSPACE" -- build --locked --bins
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          python3 scripts/root_bin_sidecars.py pack \
            --repo-root "$GITHUB_WORKSPACE" \
            --target-dir "$target_dir" \
            --output "$GITHUB_WORKSPACE/$ROOT_BIN_SIDECARS_PATH"
""",
            """      - name: Build root binary sidecars
        if: steps.nextest-archive-cache.outputs.cache-hit == 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_DEV_DEBUG: "0"
        run: |
          python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo "$GITHUB_WORKSPACE" -- build --locked --bins
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          find "$target_dir/debug" -maxdepth 1 -type f -perm -111 -print0
""",
        ),
    )
    assert_error(
        "test-archive must not upload nextest archive artifact",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Run nextest archive partitions",
            "      - name: Upload nextest archive\n        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1\n        with:\n          name: nextest-archive\n          path: ${{ env.NEXTEST_ARCHIVE_PATH }}\n      - name: Run nextest archive partitions",
        ),
    )
    assert_error(
        "actions/upload-artifact must be pinned to a 40-character SHA",
        replace_once(
            BASE_WORKFLOW,
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            "uses: actions/upload-artifact@v7",
        ),
    )
    assert_error(
        "actions/upload-artifact must be pinned to a 40-character SHA",
        replace_once(
            BASE_WORKFLOW,
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            "uses: actions/upload-artifact@v7",
        )
        .replace(
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            "PINNED_UPLOAD_ARTIFACT_PLACEHOLDER",
            1,
        )
        .replace(
            "uses: actions/upload-artifact@v7",
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            1,
        )
        .replace(
            "PINNED_UPLOAD_ARTIFACT_PLACEHOLDER",
            "uses: actions/upload-artifact@v7",
            1,
        ),
    )
    assert_error(
        "actions/upload-artifact must be pinned to a 40-character SHA",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Upload artifact\n"
            "        id: upload-bolt-v2-binary\n"
            f"{BOLT_V2_BINARY_UPLOAD_IF_LINE}"
            f"        {base_upload_artifact_action_line()}",
            "      - name: Upload artifact\n"
            "        id: upload-bolt-v2-binary\n"
            f"{BOLT_V2_BINARY_UPLOAD_IF_LINE}"
            "        uses: actions/upload-artifact@v7",
        ),
    )
    assert_error(
        "nextest-fingerprint must publish nextest archive fingerprint",
        replace_once(
            BASE_WORKFLOW,
            "          path: .nextest-archive-fingerprint/cache-key.txt",
            "          path: missing-nextest-fingerprint.txt",
        ),
    )
    assert_error(
        "nextest-fingerprint must expose secure nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            "      nextest_fingerprint: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint }}\n",
            "",
        ),
    )
    assert_error(
        "nextest-fingerprint must run the canonical producer script",
        replace_once(
            BASE_WORKFLOW,
            "            --runners-config ci/github-actions-runners.toml",
            "            --runners-config ci/not-github-actions-runners.toml",
        ),
    )
    assert_error(
        "nextest-fingerprint must publish nextest fingerprint before repo-controlled steps",
        replace_once(
            BASE_WORKFLOW,
            "    steps:\n      - name: Publish nextest archive fingerprint",
            "    steps:\n      - uses: ./.github/actions/setup-environment\n      - name: Publish nextest archive fingerprint",
        ),
    )
    assert_error(
        "nextest archive cache key must use nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            "nextest_archive_cache_key=${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
            "nextest_archive_cache_key=nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('extra-input.txt', 'Cargo.lock') }}",
        ),
    )
    assert_error(
        "test-archive must not download nextest archive artifact",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Run nextest archive partitions",
            "      - name: Download nextest archive\n        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1\n      - name: Run nextest archive partitions",
        ),
    )
    assert_error(
        "test-archive needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]",
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse]",
        ),
    )
    assert_error(
        "test-archive must not need source-fence",
        replace_once(
            BASE_WORKFLOW,
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]",
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, source-fence]",
        ),
    )
    assert_error(
        "detector must expose fingerprint_reuse_allowed",
        replace_once(
            BASE_WORKFLOW,
            "      fingerprint_reuse_allowed: ${{ steps.fingerprint_reuse_allowed.outputs.value }}\n",
            "",
        ),
    )
    assert_error(
        "detector must detect fingerprint-reuse governance changes",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "scripts/ci_provenance.py",
            "scripts/not_ci_provenance.py",
        ),
    )
    for governed_path, replacement in (
        (".github/actions/setup-environment/action.yml", ".github/actions/not-setup-environment/action.yml"),
        ("ci/nextest-fingerprint.toml", "ci/not-nextest-fingerprint.toml"),
        ("ci/github-actions-runners.toml", "ci/not-github-actions-runners.toml"),
        ("scripts/nextest_fingerprint.py", "scripts/not_nextest_fingerprint.py"),
        ("scripts/test_nextest_fingerprint.py", "scripts/not_test_nextest_fingerprint.py"),
        ("scripts/root_bin_sidecars.py", "scripts/not_root_bin_sidecars.py"),
        ("scripts/test_root_bin_sidecars.py", "scripts/not_test_root_bin_sidecars.py"),
        ("scripts/config_validators.py", "scripts/not_config_validators.py"),
    ):
        assert_error(
            "detector must detect fingerprint-reuse governance changes",
            replace_once_after(
                BASE_WORKFLOW,
                "      - name: Detect fingerprint-reuse governance changes",
                governed_path,
                replacement,
            ),
        )
    assert_error(
        "detector must determine fingerprint_reuse_allowed",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Determine fingerprint reuse allowance",
            '            echo "value=true" >> "$GITHUB_OUTPUT"\n',
            "",
        ),
    )
    assert_error(
        "detector must expose fingerprint_reuse_reason",
        replace_once(
            BASE_WORKFLOW,
            "      fingerprint_reuse_reason: ${{ steps.fingerprint_reuse_allowed.outputs.reason }}\n",
            "",
        ),
    )
    assert_error(
        "detector must explain fingerprint_reuse_allowed decisions",
        replace_once_after(
            BASE_WORKFLOW,
            "      - name: Determine fingerprint reuse allowance",
            '            echo "reason=governance-changed" >> "$GITHUB_OUTPUT"\n',
            "",
        ),
    )
    assert_error(
        "nextest-fingerprint-reuse needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  nextest-fingerprint-reuse:\n    name: nextest fingerprint reuse\n    needs: [ci-policy, detector, nextest-fingerprint]",
            "  nextest-fingerprint-reuse:\n    name: nextest fingerprint reuse\n    needs: [ci-policy, nextest-fingerprint]",
        ),
    )
    assert_error(
        "nextest-fingerprint-reuse must gate on fingerprint_reuse_allowed",
        replace_once(BASE_WORKFLOW, " && needs.detector.outputs.fingerprint_reuse_allowed == 'true'", ""),
    )
    assert_error(
        "test-archive needs nextest-fingerprint",
        replace_once(
            BASE_WORKFLOW,
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]",
            "  test-archive:\n    name: nextest archive\n    needs: [ci-policy, detector, nextest-fingerprint-reuse]",
        ),
    )
    assert_error(
        "test-archive must require detector success",
        replace_once(BASE_WORKFLOW, " && needs.detector.result == 'success'", ""),
    )
    assert_error(
        "test-archive must require nextest-fingerprint success",
        replace_once(BASE_WORKFLOW, " && needs.nextest-fingerprint.result == 'success'", ""),
    )
    assert_error(
        "test-archive must skip on validated nextest fingerprint reuse",
        replace_once(BASE_WORKFLOW, " && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true'", ""),
    )
    assert_error(
        "test needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
            "  test:\n    name: test\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
        ),
    )
    assert_error(
        "test needs nextest-fingerprint",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
            "  test:\n    name: test\n    needs: ci-policy",
        ),
    )
    assert_error(
        "nextest-fingerprint-reuse resolver must use bash",
        without_once_after(
            BASE_WORKFLOW,
            "      - name: Resolve nextest fingerprint reuse",
            "        shell: bash\n",
        ),
    )
    assert_error(
        "test must check needs.test-archive.result",
        replace_once(
            BASE_WORKFLOW,
            'if [[ "${{ needs.test-archive.result }}" != "success" ]]; then',
            'if [[ "${{ omitted.test-archive.result }}" != "success" ]]; then',
        ),
    )
    assert_error(
        "test must use always()",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]\n    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' }}",
            "  test:\n    name: test\n    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
        ),
    )
    assert_error(
        "clippy must not run check-aarch64",
        replace_once(BASE_WORKFLOW, "      - run: just clippy", "      - run: just check-aarch64\n      - run: just clippy"),
    )
    assert_error(
        "clippy must not install aarch64 cross compiler",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just clippy",
            "      - name: Install aarch64 cross compiler\n        run: sudo apt-get install -y gcc-aarch64-linux-gnu\n      - run: just clippy",
        ),
    )
    assert_error(
        "source-fence needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]",
            "  source-fence:\n    name: source-fence\n    needs: ci-policy",
        ),
    )
    assert_error(
        "source-fence checkout must use pull_request head SHA for docs policy and github.sha otherwise",
        replace_once(
            BASE_WORKFLOW,
            "        with:\n          ref: ${{ needs.ci-policy.outputs.ci_policy_path == 'docs' && github.event.pull_request.head.sha || github.sha }}\n",
            "",
        ),
    )
    assert_error(
        "source-fence must run just source-fence",
        replace_once(BASE_WORKFLOW, "            just source-fence", "            echo source-fence"),
    )
    assert_error(
        "source-fence must branch to just source-fence for full CI and just source-fence-static for docs policy",
        replace_once(BASE_WORKFLOW, "            just source-fence-static", "            echo source-fence-static"),
    )
    assert_error(
        "source-fence must branch to just source-fence for full CI and just source-fence-static for docs policy",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
            just source-fence
          else
            just source-fence-static
          fi""",
            """          if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
            just source-fence
            just source-fence-static
          else
            echo docs policy skipped
          fi""",
        ),
    )
    assert_error(
        "source-fence must branch to just source-fence for full CI and just source-fence-static for docs policy",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
            just source-fence
          else
            just source-fence-static
          fi""",
            """          if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
            just source-fence-static
          else
            just source-fence
          fi""",
        ),
    )
    for job in ("deny", "clippy", "source-fence", "nextest-fingerprint", "test-archive", "nextest-fingerprint-reuse", "test"):
        assert_error(f"{job} must skip on tag reuse", without_job_if(BASE_WORKFLOW, job))
    assert_error(
        "nextest-fingerprint-reuse must skip main branch",
        replace_once(BASE_WORKFLOW, " && github.ref != 'refs/heads/main'", ""),
    )
    assert_error(
        "nextest-fingerprint-reuse must use secure current nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            '--current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"',
            "--current-fingerprint-path .ci-provenance/fingerprint/cache-key.txt",
        ),
    )
    assert_error(
        "clippy must run just fmt-check",
        replace_once(BASE_WORKFLOW, "- run: just fmt-check", "- run: echo skip fmt-check"),
    )
    assert_error(
        ".github/workflows/ci.yml clippy must enable workflow contract lint",
        replace_once(BASE_WORKFLOW, '          lint-workflow-contract: "true"\n', ""),
    )
    assert_error(
        ".github/workflows/ci.yml clippy must install rustfmt component",
        replace_once(BASE_WORKFLOW, "          toolchain-components: clippy, rustfmt", "          toolchain-components: clippy"),
    )
    assert_error(
        "deny must run just deny",
        replace_once(BASE_WORKFLOW, "- run: just deny", "- run: echo skip deny"),
    )
    assert_error(
        "clippy must run just clippy",
        replace_once(BASE_WORKFLOW, "- run: just clippy", "- run: echo skip clippy"),
    )
    assert_error(
        "build must run just build",
        replace_once(BASE_WORKFLOW, "- run: just build", "- run: echo skip build"),
    )
    assert_error(
        "on.pull_request must have no paths-ignore",
        replace_once(
            BASE_WORKFLOW,
            "    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]\n",
            "    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]\n"
            "    paths-ignore:\n"
            "      - 'AGENTS.md'\n",
        ),
    )
    assert_error(
        "on.push must have no paths-ignore",
        replace_once(
            BASE_WORKFLOW,
            '  push:\n    branches: [main]\n    tags: ["v*"]\n',
            '  push:\n    branches: [main]\n    tags: ["v*"]\n    paths-ignore:\n      - \'docs/**\'\n',
        ),
    )
    assert_error(
        "build needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  build:\n    name: build\n    needs: [ci-policy, detector]",
            "  build:\n    name: build\n    needs: ci-policy",
        ),
    )
    assert_error(
        "build must gate on needs.detector.outputs.build_required",
        replace_once(
            BASE_WORKFLOW,
            "if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && needs.detector.outputs.build_required == 'true' }}",
            "if: ${{ needs.detector.outputs.build_required != 'true' }}",
        ),
    )
    assert_error(
        "build must gate on needs.detector.outputs.build_required",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && needs.detector.outputs.build_required == 'true' }}\n",
                "",
            ),
            "      - uses: ./.github/actions/setup-environment",
            "      - if: needs.detector.outputs.build_required == 'true'\n        uses: ./.github/actions/setup-environment",
        ),
    )
    assert_error(
        "ci-provenance-emit needs source-fence",
        replace_once(
            BASE_WORKFLOW,
            "    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]",
            "    needs: [ci-policy, detector, deny, clippy, check-aarch64, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]",
        ),
    )
    assert_error(
        "ci-provenance-emit must use always()",
        replace_once(
            BASE_WORKFLOW,
            "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') }}",
            "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}",
        ),
    )
    assert_error(
        "ci-provenance-emit must run provenance emitter",
        replace_once(
            BASE_WORKFLOW,
            'python3 "$provenance_script" emit-full-ci',
            "python3 scripts/ci_provenance.py emit-full-ci",
        ),
    )
    assert_error(
        "ci-provenance-emit must run provenance emitter",
        replace_once_after(
            BASE_WORKFLOW,
            "  ci-provenance-emit:",
            'git fetch --no-tags origin "+${base_sha}:${base_ref}"',
            'git fetch --no-tags origin "+refs/heads/${base_branch}:${base_ref}"',
        ),
    )
    assert_error(
        "ci-provenance-emit must pass detector result from needs.detector.result",
        replace_once(
            BASE_WORKFLOW,
            "--required-job detector=${{ needs.detector.result }}",
            "--required-job detector=success\n          printf '%s\\n' '${{ needs.detector.result }}'",
        ),
    )
    assert_error(
        "ci-provenance-emit must pass build.required from needs.detector.outputs.build_required",
        replace_once(
            BASE_WORKFLOW,
            "--conditional-job build.required=${{ needs.detector.outputs.build_required }}",
            "--conditional-job build.required=true\n          printf '%s\\n' '${{ needs.detector.outputs.build_required }}'",
        ),
    )
    assert_error(
        "ci-provenance-emit must pass build.result from needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            "--conditional-job build.result=${{ needs.build.result }}",
            "--conditional-job build.result=success\n          printf '%s\\n' '${{ needs.build.result }}'",
        ),
    )
    assert_error(
        "ci-provenance-emit must record nextest fingerprint when present",
        replace_once(
            BASE_WORKFLOW,
            '--nextest-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"',
            '--nextest-fingerprint ""',
        ),
    )
    assert_error(
        "ci-provenance-emit must use secure nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            '--nextest-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"',
            "--nextest-fingerprint-path .ci-provenance/fingerprint/cache-key.txt",
        ),
    )
    assert_error(
        "actions/upload-artifact must be pinned to a 40-character SHA",
        replace_once(
            BASE_WORKFLOW,
            "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            "uses: actions/upload-artifact@v7",
        ),
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml ci-provenance-emit upload-ci-provenance artifact ci-provenance-attempt-${{ github.run_attempt }} retention-days 13 does not match configured retention-days 14",
        {
            ".github/workflows/ci.yml": replace_once(
                BASE_WORKFLOW,
                "          name: ci-provenance-attempt-${{ github.run_attempt }}\n          path: ci-provenance.json\n          if-no-files-found: error\n          retention-days: 14",
                "          name: ci-provenance-attempt-${{ github.run_attempt }}\n          path: ci-provenance.json\n          if-no-files-found: error\n          retention-days: 13",
            )
        },
    )
    assert_error(
        "gate must not read nextest_fingerprint",
        replace_once(
            BASE_WORKFLOW,
            "--job ci-provenance-emit=${{ needs.ci-provenance-emit.result }}",
            "--job ci-provenance-emit=${{ needs.ci-provenance-emit.outputs.nextest_fingerprint }}",
        ),
    )
    assert_error("same-sha-main-evidence needs detector", replace_once(BASE_WORKFLOW, "    needs: detector\n    if: startsWith(github.ref, 'refs/tags/v')", "    if: startsWith(github.ref, 'refs/tags/v')"))
    assert_error("same-sha-main-evidence must be tag-gated", without_job_if(BASE_WORKFLOW, "same-sha-main-evidence"))
    assert_error(
        "same-sha-main-evidence must expose source run",
        replace_once(BASE_WORKFLOW, "      artifact_id: ${{ steps.evidence.outputs.artifact_id }}\n", ""),
    )
    assert_error(
        "same-sha-main-evidence must run resolver script",
        replace_once(BASE_WORKFLOW, "python3 scripts/find_same_sha_main_evidence.py", "python3 scripts/other.py"),
    )
    assert_error(
        "gate needs same-sha-main-evidence",
        replace_once(BASE_WORKFLOW, GATE_NEEDS, without_inline_need(GATE_NEEDS, "same-sha-main-evidence")),
    )
    assert_error(
        "gate shared verdict call must include --job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
            "--job same-sha-main-evidence=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --job build=${{ needs.build.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job build=${{ needs.build.result }}",
            "--job build=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --job deny=${{ needs.deny.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job deny=${{ needs.deny.result }}",
            "--job deny=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --job check-aarch64=${{ needs.check-aarch64.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job check-aarch64=${{ needs.check-aarch64.result }}",
            "--job check-aarch64=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --job ci-provenance-emit=${{ needs.ci-provenance-emit.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job ci-provenance-emit=${{ needs.ci-provenance-emit.result }}",
            "--job ci-provenance-emit=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "ci.yml build must resolve artifact through rust_verification_owner binary-path",
        replace_once(
            BASE_WORKFLOW,
            'binary_path="$(python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2)"',
            'binary_path="target/aarch64-unknown-linux-gnu/release/bolt-v2"',
        ),
    )
    assert_error(
        "ci.yml must not reference repo-local target release artifacts",
        replace_once(
            BASE_WORKFLOW,
            "${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2",
            "target/aarch64-unknown-linux-gnu/release/bolt-v2",
        ),
    )
    assert_error(
        "ci.yml build upload must use the staged artifact directory",
        BASE_WORKFLOW.replace("${{ steps.managed_artifact.outputs.stage_dir }}", "$RUNNER_TEMP/bolt-v2-binary"),
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary retention-days 31 does not match configured retention-days 3",
        {
            ".github/workflows/ci.yml": replace_once(
                BASE_WORKFLOW,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK.replace("retention-days: 3", "retention-days: 31"),
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary must set retention-days",
        {
            ".github/workflows/ci.yml": replace_once(
                BASE_WORKFLOW,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK.replace("\n          retention-days: 3", ""),
            )
        },
    )
    assert_artifact_retention_error(
        ".github/workflows/ci.yml build upload-bolt-v2-binary artifact bolt-v2-binary must set exactly one retention-days",
        {
            ".github/workflows/ci.yml": replace_once(
                BASE_WORKFLOW,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK,
                BOLT_V2_BINARY_UPLOAD_WITH_BLOCK + "\n          retention-days: 31",
            )
        },
    )
    assert_workflows_error(
        "advisory.yml advisories must include deny version",
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": replace_once(BASE_ADVISORY_WORKFLOW, '          include-deny-version: "true"\n', "")},
    )
    assert_workflows_error(
        "advisory.yml advisories must use setup.outputs.deny_version",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                "tool: cargo-deny@${{ steps.setup.outputs.deny_version }}",
                "tool: cargo-deny@0.18.3",
            ),
        },
    )
    assert_workflows_error(
        "advisory.yml advisories must install cargo-deny with pinned taiki-e/install-action",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                """      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none""",
                """      - name: Install cargo-deny
        run: |
          cargo install cargo-deny --version "${{ steps.setup.outputs.deny_version }}" --locked""",
            ),
        },
    )
    assert_workflows_error(
        "advisory.yml advisories install-action fallback must be none",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                "          fallback: none\n      - name: Check advisories",
                "          fallback: cargo-install\n      - name: Check advisories",
            ),
        },
    )
    assert_error(
        "ci.yml deny must install cargo-deny with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none""",
            """      - name: Install cargo-deny
        run: |
          cargo install cargo-deny --version "${{ steps.setup.outputs.deny_version }}" --locked""",
        ),
    )
    assert_error(
        "ci.yml deny install-action fallback must be none",
        replace_once(
            BASE_WORKFLOW,
            "          fallback: none\n      - run: just deny",
            "          fallback: cargo-install\n      - run: just deny",
        ),
    )
    assert_error(
        "ci.yml deny must install cargo-deny with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            "uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538",
            "uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538-suffix",
        ),
    )
    assert_error(
        "ci.yml deny must install cargo-deny before just deny",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - run: just deny""",
            """      - run: just deny
      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo install --locked cargo-deny
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo --config net.git-fetch-with-cli=true install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo --manifest-path Cargo.toml install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo --target x86_64-unknown-linux-gnu install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo --ignore-rust-version install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo -Zunstable-options install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo +stable install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo install cargo-deny@${{ steps.setup.outputs.deny_version }} --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo install --git https://github.com/EmbarkStudios/cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          cargo install --path vendor/cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          if cargo install --git https://github.com/EmbarkStudios/cargo-deny --locked; then
            just deny
          fi""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          env cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          rustup run stable cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          /tmp/builder install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          docker run --rm rust:latest cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          RUSTFLAGS= cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo -E cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo -EH cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo -A cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo --askpass cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo -b cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo --background cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          flock -o /tmp/bolt.lock cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          flock -c 'cargo install cargo-deny --locked' /tmp/bolt.lock
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          su user -c 'cargo install cargo-deny --locked'
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          runuser -u user -- cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sg group -c 'cargo install cargo-deny --locked'
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo flock -o /tmp/bolt.lock cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sudo --preserve-env=PATH cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          env -u RUSTFLAGS cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          env -iu RUSTFLAGS cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          { cargo install cargo-deny --locked; }
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          while cargo install cargo-deny --locked; do
            break
          done
          just deny""",
        ),
    )
    assert_error(
        "ci.yml deny must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          sleep 1 & cargo install cargo-deny --locked
          just deny""",
        ),
    )
    assert_clean(
        replace_once(
            BASE_WORKFLOW,
            "      - run: just deny",
            """      - run: |
          echo cargo install cargo-deny
          just deny""",
        )
    )
    assert_error(
        "ci.yml docs-tool-smoke must not compile cargo-deny from source",
        replace_once(
            BASE_WORKFLOW,
            "  gate:\n",
            """  docs-tool-smoke:
    name: docs-tool-smoke
    runs-on: ubuntu-latest
    steps:
      - run: |
          cargo install cargo-deny --locked

  gate:
""",
        ),
    )
    assert_error(
        "ci.yml source-fence must not compile cargo-nextest from source",
        replace_once(
            BASE_WORKFLOW,
            "            just source-fence",
            """            cargo install --git https://github.com/nextest-rs/nextest --package cargo-nextest --locked
            just source-fence""",
        ),
    )
    assert_error(
        "ci.yml test-archive must install cargo-nextest with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-nextest
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none""",
            """      - name: Install cargo-nextest
        run: |
          cargo install cargo-nextest --version "${{ steps.setup.outputs.nextest_version }}" --locked""",
        ),
    )
    assert_error(
        "ci.yml test-archive must not compile cargo-nextest from source",
        replace_once(
            BASE_WORKFLOW,
            '          just test-archive "$NEXTEST_ARCHIVE_PATH"',
            '''          cargo install --git https://github.com/nextest-rs/nextest --package cargo-nextest --locked
          just test-archive "$NEXTEST_ARCHIVE_PATH"''',
        ),
    )
    assert_error(
        "ci.yml test-archive install-action fallback must be none",
        replace_once(
            BASE_WORKFLOW,
            '          fallback: none\n      - name: Build nextest archive',
            '          fallback: cargo-install\n      - name: Build nextest archive',
        ),
    )
    assert_error(
        'ci.yml test-archive must install cargo-nextest before just test-archive "$NEXTEST_ARCHIVE_PATH"',
        replace_once(
            BASE_WORKFLOW,
            "      - name: Install cargo-nextest",
            """      - name: Premature nextest archive build
        run: |
          just test-archive "$NEXTEST_ARCHIVE_PATH"
      - name: Install cargo-nextest""",
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-zigbuild
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}
          fallback: none""",
            """      - name: Install cargo-zigbuild
        run: |
          cargo install cargo-zigbuild --version "${{ steps.setup.outputs.zigbuild_version }}" --locked""",
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            "        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538\n        with:\n          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}",
            "        uses: taiki-e/install-action@v2\n        with:\n          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}",
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            "          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}",
            "          tool: cargo-zigbuild@${{ env.ZIGBUILD_VERSION }}",
        ),
    )
    assert_error(
        "ci.yml build install-action fallback must be none",
        replace_once(
            BASE_WORKFLOW,
            "          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}\n          fallback: none",
            "          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}\n          fallback: cargo-install",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just build",
            """      - run: |
          cargo install --version "${{ steps.setup.outputs.zigbuild_version }}" cargo-zigbuild
          just build""",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just build",
            """      - run: |
          cargo +stable install cargo-zigbuild --version "${{ steps.setup.outputs.zigbuild_version }}"
          just build""",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just build",
            """      - run: |
          cargo install cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }} --locked
          just build""",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just build",
            """      - run: |
          cargo install --path tools/cargo-zigbuild --locked
          just build""",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just build",
            """      - run: |
          cargo install --git https://github.com/rust-cross/cargo-zigbuild --locked
          just build""",
        ),
    )
    assert_error(
        "ci.yml clippy must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just fmt-check",
            """      - run: |
          cargo install --path vendor/cargo-zigbuild --locked
          just fmt-check""",
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild before just build",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-zigbuild
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}
          fallback: none
      - run: just build""",
            """      - run: just build
      - name: Install cargo-zigbuild
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}
          fallback: none""",
        ),
    )
    assert_workflows_error(
        "advisory.yml advisories must install cargo-deny before just deny-advisories",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                """      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - name: Check advisories
        run: just deny-advisories""",
                """      - name: Check advisories
        run: just deny-advisories
      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none""",
            ),
        },
    )
    assert_error(
        "gate must use always()",
        replace_once(
            BASE_WORKFLOW,
            f"  gate:\n    {GATE_NAME}\n    {GATE_NEEDS}\n    if: ${{{{ always() }}}}",
            f"  gate:\n    {GATE_NAME}\n    {GATE_NEEDS}\n    if: ${{{{ always() && false }}}}",
        ),
    )
    assert_error(
        "gate must use always()",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                f"  gate:\n    {GATE_NAME}\n    {GATE_NEEDS}\n    if: ${{{{ always() }}}}\n",
                f"  gate:\n    {GATE_NAME}\n    {GATE_NEEDS}\n",
            ),
            "    runs-on: ubuntu-latest\n    steps:\n      - name: Prepare trusted base verdict tree",
            "    runs-on: ubuntu-latest\n    steps:\n      - if: ${{ always() }}\n      - name: Prepare trusted base verdict tree",
        ),
    )
    assert_error(
        "gate shared verdict call must include --job detector=${{ needs.detector.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job detector=${{ needs.detector.result }}",
            "--job detector=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "gate shared verdict call must include --build-required",
        replace_once(
            BASE_WORKFLOW,
            '--build-required "${{ needs.detector.outputs.build_required || \'false\' }}"',
            '--build-required "false"',
        ),
    )
    assert_error(
        "gate shared verdict call must include --job build=${{ needs.build.result }}",
        replace_once(
            BASE_WORKFLOW,
            "--job build=${{ needs.build.result }}",
            "--job build=${{ needs.omitted.result }}",
        ),
    )
    assert_error(
        "deploy must be tag-gated",
        replace_once(
            BASE_WORKFLOW,
            "if: ${{ always() && startsWith(github.ref, 'refs/tags/v') && needs.gate.result == 'success' && needs.same-sha-main-evidence.result == 'success' }}",
            "if: ${{ always() }}",
        ),
    )
    assert_error(
        "deploy must be tag-gated",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "    if: ${{ always() && startsWith(github.ref, 'refs/tags/v') && needs.gate.result == 'success' && needs.same-sha-main-evidence.result == 'success' }}\n",
                "",
            ),
            "      - run: echo deploy",
            "      - if: startsWith(github.ref, 'refs/tags/v')\n        run: echo deploy",
        ),
    )
    assert_error(
        "deploy permissions must include actions: read",
        replace_once(BASE_WORKFLOW, "      actions: read\n      id-token: write", "      id-token: write"),
    )
    assert_error(
        "deploy must download same-SHA main artifact by artifact ID",
        replace_once(
            BASE_WORKFLOW,
            "          artifact-ids: ${{ needs.same-sha-main-evidence.outputs.artifact_id }}",
            "          name: bolt-v2-binary",
        ),
    )
    assert_error(
        "deploy must log reused source run",
        replace_once(BASE_WORKFLOW, '          echo "check_suite_id=${{ needs.same-sha-main-evidence.outputs.check_suite_id }}"\n', ""),
    )
    assert_error(
        "deploy must verify downloaded artifact checksum",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Verify downloaded artifact checksum
        run: |
          cd artifact
          sha256sum -c bolt-v2.sha256
""",
            "",
        ),
    )
    assert_error(
        "clippy uses managed target dir but setup does not opt in",
        replace_once(
            BASE_WORKFLOW,
            '          include-managed-target-dir: "true"',
            '          # include-managed-target-dir: "true"',
        ),
    )
    assert_error(
        "deny opts into managed target dir but does not use it",
        replace_once(
            BASE_WORKFLOW,
            '          include-deny-version: "true"',
            '          include-deny-version: "true"\n          include-managed-target-dir: "true"',
        ),
    )
    assert_error(
        "clippy must use isolated managed target cache",
        replace_once(
            BASE_WORKFLOW,
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}",
        ),
    )
    assert_error(
        "ci.yml clippy must install configured Rust linker",
        replace_once(
            BASE_WORKFLOW,
            '          install-rust-linker: "true"\n          build-jobs-key: ci.clippy',
            '          build-jobs-key: ci.clippy',
        ),
    )
    assert_error(
        "ci.yml source-fence must install configured Rust linker",
        replace_once(
            BASE_WORKFLOW,
            '          install-rust-linker: "true"\n          build-jobs-key: ci.source-fence',
            '          build-jobs-key: ci.source-fence',
        ),
    )
    assert_error(
        "ci.yml test-archive must install configured Rust linker",
        replace_once(
            BASE_WORKFLOW,
            '          install-rust-linker: "true"\n          build-jobs-key: ci.test-archive',
            '          build-jobs-key: ci.test-archive',
        ),
    )
    assert_error(
        "ci.yml build must install configured Rust linker",
        replace_once(
            BASE_WORKFLOW,
            '          include-managed-target-dir: "true"\n          install-rust-linker: "true"\n      - uses: Swatinem/rust-cache@example',
            '          include-managed-target-dir: "true"\n      - uses: Swatinem/rust-cache@example',
        ),
    )
    assert_error(
        "setup action missing exported output 'nextest_version'",
        action=BASE_ACTION.replace(
            """  nextest_version:
    value: ${{ steps.shared.outputs.nextest_version }}
""",
            "",
        ),
    )
    assert_error(
        "setup action missing output mapping for 'rust_verification_owner'",
        action=replace_once(
            BASE_ACTION,
            "    value: ${{ steps.shared.outputs.rust_verification_owner }}",
            '    value: ""',
        ),
    )
    assert_error(
        "setup action missing expected literal 'just --evaluate nextest_version'",
        action=replace_once(BASE_ACTION, "just --evaluate nextest_version", "just --evaluate cargo_nextest_version"),
    )
    assert_error(
        "setup action must install just with pinned taiki-e/install-action",
        action=replace_once(
            BASE_ACTION,
            """    - name: Install just
      uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538 # v2.81.1
      with:
        tool: just@${{ inputs.just-version }}
        fallback: none
""",
            """    - name: Install just
      shell: bash
      run: echo "${{ inputs.just-version }}"
""",
        ),
    )
    assert_error(
        "setup action just install-action fallback must be none",
        action=replace_once(BASE_ACTION, "        fallback: none\n    - name: Lint workflow contract", "        fallback: cargo-install\n    - name: Lint workflow contract"),
    )
    assert_error(
        "setup action step order drifted",
        action=replace_once(
            replace_once(
                BASE_ACTION,
                """    - name: Lint workflow contract
      if: ${{ inputs.lint-workflow-contract == 'true' }}
      shell: bash
      run: just ci-lint-workflow
""",
                "",
            ),
            "    - name: Resolve managed target dir",
            """    - name: Lint workflow contract
      if: ${{ inputs.lint-workflow-contract == 'true' }}
      shell: bash
      run: just ci-lint-workflow
    - name: Resolve managed target dir""",
        ),
    )
    assert_error(
        "setup action missing include-managed-target-dir input",
        action=BASE_ACTION.replace(
            """  include-managed-target-dir:
    description: Whether to resolve the managed target dir.
    required: false
    default: "false"
""",
            "",
        ),
    )
    assert_error(
        "setup action include-managed-target-dir default must be false",
        action=replace_once(
            BASE_ACTION,
            """  include-managed-target-dir:
    description: Whether to resolve the managed target dir.
    required: false
    default: "false"
""",
            """  include-managed-target-dir:
    description: Whether to resolve the managed target dir.
    required: false
    default: "true"
""",
        ),
    )
    assert_error(
        "setup action missing install-rust-linker input",
        action=BASE_ACTION.replace(
            """  install-rust-linker:
    description: Whether to install the configured Rust fast linker.
    required: false
    default: "false"
""",
            "",
        ),
    )
    assert_error(
        "setup action install-rust-linker default must be false",
        action=replace_once(
            BASE_ACTION,
            """  install-rust-linker:
    description: Whether to install the configured Rust fast linker.
    required: false
    default: "false"
""",
            """  install-rust-linker:
    description: Whether to install the configured Rust fast linker.
    required: false
    default: "true"
""",
        ),
    )
    assert_error(
        "setup action missing expected literal 'python3.12 \"${{ steps.shared.outputs.rust_verification_owner }}\" fast-linker-programs --repo \"$GITHUB_WORKSPACE\"'",
        action=replace_once(
            BASE_ACTION,
            'python3.12 "${{ steps.shared.outputs.rust_verification_owner }}" fast-linker-programs --repo "$GITHUB_WORKSPACE"',
            'python3 "${{ steps.shared.outputs.rust_verification_owner }}" fast-linker-programs --repo "$GITHUB_WORKSPACE"',
        ),
    )
    assert_error(
        "setup action missing expected literal 'command -v \"$rust_linker_program\" >/dev/null'",
        action=replace_once(
            BASE_ACTION,
            'command -v "$rust_linker_program" >/dev/null',
            'printf "%s\\n" "$rust_linker_program" >/dev/null',
        ),
    )
    assert_error(
        "setup action missing expected literal 'BOLT_RUST_FAST_LINKER=$rust_linker_program'",
        action=BASE_ACTION.replace("BOLT_RUST_FAST_LINKER=$rust_linker_program", "BOLT_RUST_FAST_LINKER=hardcoded"),
    )
    assert_error(
        "setup action Rust linker install failures must fail open",
        action=replace_once(
            BASE_ACTION,
            'echo "::warning::failed to install any configured Rust linker; continuing without fast linker"',
            'echo "::error::failed to install any configured Rust linker"',
        ),
    )
    assert_error(
        "setup action must export managed_target_dir from target_dir step",
        action=replace_once(
            BASE_ACTION,
            "    value: ${{ steps.target_dir.outputs.managed_target_dir }}",
            '    value: ""',
        ),
    )
    assert_error(
        "setup action must export managed_target_dir from target_dir step",
        action=replace_once(
            BASE_ACTION,
            "    value: ${{ steps.target_dir.outputs.managed_target_dir }}",
            '    value: "" # ${{ steps.target_dir.outputs.managed_target_dir }}',
        ),
    )
    assert_error(
        "setup action must export managed_target_dir_relative from target_dir step",
        action=replace_once(
            BASE_ACTION,
            "    value: ${{ steps.target_dir.outputs.managed_target_dir_relative }}",
            '    value: "" # ${{ steps.target_dir.outputs.managed_target_dir_relative }}',
        ),
    )
    assert_error(
        "setup action target_dir step must write managed_target_dir_relative",
        action=replace_once(
            BASE_ACTION,
            '        echo "managed_target_dir_relative=$managed_target_dir_relative" >> "$GITHUB_OUTPUT"',
            '        echo "managed_target_dir=$managed_target_dir" >> "$GITHUB_OUTPUT"',
        ),
    )
    assert_error(
        "setup action target_dir step must compute managed_target_dir_relative from workspace to target dir",
        action=replace_once(
            BASE_ACTION,
            """        managed_target_dir_relative="$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[2], sys.argv[1]))' "$GITHUB_WORKSPACE" "$managed_target_dir")\"""",
            """        managed_target_dir_relative="$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[2], sys.argv[1]))' "$managed_target_dir" "$GITHUB_WORKSPACE")\"""",
        ),
    )
    assert_error(
        "setup action target dir step must be conditional",
        action=BASE_ACTION.replace("      if: ${{ inputs.include-managed-target-dir == 'true' }}\n", ""),
    )
    assert_error(
        "setup action target dir step must be conditional",
        action=replace_once(
            BASE_ACTION,
            "      if: ${{ inputs.include-managed-target-dir == 'true' }}",
            "      # if: ${{ inputs.include-managed-target-dir == 'true' }}",
        ),
    )
    assert_v6_red_workflow_policy_gaps()
    assert_nextest_fingerprint_reuse_adversarial_gaps_are_reported()
    assert_ci_provenance_config_contract()
    assert_runner_contract_rejects_missing_and_extra_jobs()
    assert_runner_contract_rejects_unmapped_workflow_jobs()
    assert_deleted_ai_review_model_freshness_workflow_is_unmapped()
    assert_runner_contract_accepts_flaky_detection_workflow_mapping()
    assert_flaky_detection_workflow_uses_supported_mergify_contract()
    assert_flaky_backtester_jobs_require_shared_minio_action()
    assert_runner_config_floor_handles_missing_and_empty_inputs()
    assert_runner_contract_requires_meter_workflows_for_managed_workflows()
    assert_runner_contract_requires_meter_api_limits()
    assert_runner_contract_requires_fingerprint_archive_tier_coupling()
    assert_runner_contract_requires_configured_cargo_build_jobs()
    assert_ci_workflow_rejects_spike_probe_markers()
    assert_jules_advisory_workflow_contracts()
    assert_jules_advisory_config_carries_repo_variable_values()
    assert_debug_workflow_rejects_non_manual_trigger()
    assert_debug_workflow_checks_each_ssh_runner_step()
    assert_debug_lane_compile_cache_parity_contract()
    assert_cache_docs_cover_debug_schedule_consumers()
    assert_bootstrap_uses_onepassword_key_generation()
    assert_sync_errors_redact_command_arguments()
    assert_sync_public_key_uses_stdin()
    assert_security_key_public_prefix_is_validated()
    assert_backtester_detect_uses_ci_input_set()
    assert_backtester_ci_input_set_config_covers_systematic_inputs()
    assert_backtester_ci_requires_pr_event_types()
    assert_backtester_ci_uses_iteration_for_feedback_paths()
    assert_actionlint_rejects_stale_config_variables()
    assert_actionlint_requires_pr_event_types()
    assert_actionlint_runs_ci_storage_audit_tests()
    assert_storage_cleanup_alert_workflow_contract()
    assert_ci_docs_pass_stub_is_absent()
    assert_source_fence_static_ignores_comments()
    assert_local_verification_gate_recipes_are_enforced()
    assert_nextest_fingerprint_reuse_governance_covers_sidecar_helper()
    assert_nextest_fingerprint_reuse_governance_pathspec_order_is_pinned()
    assert_nextest_fingerprint_reuse_governance_excludes_whole_ci_workflow()
    assert_reuse_neutral_env_comment_states_fail_closed_classification_rule()
    assert_rust_verification_policy_parse_errors_are_domain_specific()
    assert_ci_policy_matrix()
    assert_ci_policy_resolvers_agree()
    assert_ci_policy_rejects_literal_event_sender_id_argument()
    assert_ci_policy_rejects_inline_event_sender_id_override()
    assert_ci_policy_rejects_backslash_split_event_sender_id_argument()
    assert_ci_policy_rejects_env_command_event_sender_id_override()
    assert_ci_policy_rejects_prior_event_sender_id_exports()
    assert_ci_policy_rejects_event_sender_id_append_assignment()
    assert_ci_policy_rejects_alternate_python_event_sender_id_argument()
    assert_ci_policy_rejects_split_and_boundary_event_sender_id_arguments()
    assert_ci_policy_counts_structural_event_sender_id_env_keys()
    assert_ci_policy_real_workflows_keep_event_sender_binding_clean()
    assert_pull_request_type_parser_accepts_block_list_indentation()
    assert_ci_workflow_requires_policy_trigger_and_dispatch_input()
    assert_ci_workflow_dispatch_config_errors_are_reported()
    assert_test_archive_sccache_fail_open_contract()
    assert_test_archive_build_script_delegates_sccache_retry_to_owner()
    assert_ci_detector_forces_build_on_workflow_dispatch()
    assert_capture_artifact_metadata_is_config_derived()
    assert_ci_base_ref_archives_use_scripts_directory()
    assert_ci_detector_docs_only_archive_includes_lane_policy()
    assert_ci_policy_heavy_lane_gaps_are_reported()
    assert_cache_persistence_audit_gaps_are_reported()
    assert_ci_concurrency_split_gaps_are_reported()
    assert_mergify_proof_pr_concurrency_gaps_are_reported()
    assert_mergify_proof_prefix_alignment_holds()
    assert_base_change_predicate_avoids_loose_empty_string_equality()
    assert_mergify_proof_prefix_alignment_detects_drift()
    assert_dispatch_cancel_watchdog_gaps_are_reported()
    assert_merge_readiness_progress_gaps_are_reported()
    assert_merge_readiness_finalizer_gaps_are_reported()
    assert_coverage_enforcer_workflow_gaps_are_reported()
    assert_coverage_enforcer_bootstrap_marker_matches_script()

    verifier = load_verifier()
    runner_config = REPO_ROOT / "ci" / "github-actions-runners.toml"
    assert runner_config.exists(), "ci/github-actions-runners.toml must exist"
    real_workflows = verifier.repo_workflow_texts()
    runner_errors = verifier.verify_github_actions_runner_contract(real_workflows)
    assert not runner_errors, runner_errors
    storage_policy_path = verifier.ci_storage_tripwire.discover_policy_path(REPO_ROOT)
    storage_policy = storage_policy_path.read_text(encoding="utf-8")
    storage_errors = verifier.verify_storage_tripwire_workflow(real_workflows, storage_policy)
    assert not storage_errors, storage_errors
    actionlint_errors = verifier.verify_actionlint_runner_contract(real_workflows)
    assert not actionlint_errors, actionlint_errors
    dispatch_cancel_errors = verifier.verify_dispatch_ci_cancel_workflow(real_workflows)
    assert not dispatch_cancel_errors, dispatch_cancel_errors
    progress_errors = verifier.verify_merge_readiness_ci_job(
        real_workflows[".github/workflows/ci.yml"]
    )
    assert not progress_errors, progress_errors
    finalizer_errors = verifier.verify_merge_readiness_finalizer_workflow(real_workflows)
    assert not finalizer_errors, finalizer_errors
    print("OK: CI workflow hygiene verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
