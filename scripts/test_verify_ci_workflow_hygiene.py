#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import contextlib
import dataclasses
import io
import importlib.util
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap

from ci_test_manifest import CiTestManifest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
SYNC_CI_DEBUG_SSH_PATH = REPO_ROOT / "scripts" / "sync_ci_debug_ssh_secret.py"
DEBUG_WORKFLOW_PATH = ".github/workflows/ci-runner-debug.yml"
SSH_RUNNER_ACTION = "ubicloud/ssh-runner@b6ccad69f047c476b84a54a990f89b1ea5f2a828"
GATE_NEEDS = "needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build, ci-provenance-emit, same-sha-main-evidence]"
GATE_NAME = "name: ${{ needs.ci-policy.outputs.gate_name }}"
GATE_EXPECTED_EVENT_CLASS_ASSIGNMENT = 'expected_event_class="${{ needs.ci-policy.outputs.expected_event_class }}"'
GATE_DEFER_CONTEXT_GUARD = """            if [[ "$expected_event_class" != "defer" ]]; then
              echo "deferred CI policy outside resolver-permitted event class '$expected_event_class'"
              exit 1
            fi
"""
GATE_DEFER_BLOCK = f"""          if [[ "$policy_path" == "defer" || "$full_ci_deferred" == "true" ]]; then
{GATE_DEFER_CONTEXT_GUARD}            echo "full CI deferred for draft PR; use just rust-probe suggest for debugging; run just verify-remote for full feedback or mark ready for merge proof"
            exit 0
          fi
"""
GATE_NOOP_CONTEXT_GUARD = """            if [[ "$expected_event_class" != "noop" ]]; then
              echo "noop CI policy outside resolver-permitted event class '$expected_event_class'"
              exit 1
            fi
"""
GATE_NOOP_BLOCK = f"""          if [[ "$policy_path" == "noop" ]]; then
{GATE_NOOP_CONTEXT_GUARD}            echo "no code-change CI event; preserving prior required same-SHA gate conclusion"
            exit 0
          fi
"""
DEPLOY_NEEDS = "needs: [gate, same-sha-main-evidence, build, detector, deny, clippy, check-aarch64, source-fence, test]"
EXACT_HEAD_GOVERNANCE_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
    "'scripts/command_understanding.py'",
)

CI_PROVENANCE_TOML = """
[ci_provenance]
schema_version = 1
artifact_name_template = "ci-provenance-attempt-{run_attempt}"
workflow_key = "ci"
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"
fingerprint_source = "meter"

[ci_provenance.full_ci]
required_jobs = [
  "detector",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "nextest-fingerprint",
  "test-archive",
  "test",
]
conditional_jobs = ["build"]
conditional_job_outputs = { build = "detector.build_required" }

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.nextest-fingerprint]
check_name = "nextest fingerprint"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.test]
check_name = "test"

[ci_provenance.full_ci.jobs.build]
check_name = "build"
conditional = "detector.build_required"

[ci_provenance.deploy]
artifact_name = "bolt-v2-binary"
require_source_event = "push"
require_source_branch = "main"
require_gate_check = true

[ci_provenance.dispatch]
workflow_input = "full_ci"
run_name_default = "CI"
run_name_full = "CI [dispatch:full]"
run_name_iteration = "CI [dispatch:iteration]"
proof_gate_job = "gate"

[ci_provenance.gate_names]
gate_required = "gate"
gate_iteration = "gate-iteration"
gate_dispatch_full = "gate-dispatch"
backtester_required = "backtester-gate"
backtester_iteration = "backtester-gate-iteration"
backtester_dispatch_full = "backtester-gate-dispatch"

[ci_provenance.docs]
safe_paths = [
  "AGENTS.md",
  "CLAUDE.md",
  "GEMINI.md",
  "REASONIX.md",
  "LICENSE",
  "SECURITY.md",
  ".github/ISSUE_TEMPLATE/**",
  ".claude/**",
  ".codex/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".specify/**",
]
forbidden_ignored_build_paths = [
  ".claude/rust-verification.toml",
]
non_heavy_required_jobs = ["detector"]

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000

[ci_provenance.artifacts]
retention_days = 30

[ci_provenance.policy]
draft_pr_synchronize = "iteration"
draft_pr_opened = "iteration"
draft_pr_reopened = "iteration"
draft_pr_edited = "iteration"
converted_to_draft = "iteration"
ready_pr = "iteration"
ready_pr_edited_no_base = "iteration"
ready_pr_reopened = "iteration"
ready_for_review = "iteration"
docs = "docs"
workflow_dispatch = "iteration"
workflow_dispatch_full_ci = "full"
main_push = "full"
merge_group = "full"
mergify_temp_pr = "full"
tag = "tag_reuse"
unknown_event = "full"

[ci_provenance.mergify]
temp_pr_head_ref_prefix = "mergify/merge-queue/"
mergify_temp_pr_actor_id = 37929162

[ci_provenance.policy.override]
force_full_ci = false
ignore_emit_failure = false
"""


def load_verifier(
    path: pathlib.Path = VERIFIER_PATH, module_name: str = "verify_ci_workflow_hygiene"
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load verify_ci_workflow_hygiene.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if hasattr(module, "build_test_manifest"):
        module.build_test_manifest = lambda _manifest_path, _tests_root: all_standalone_live_node_manifest(module)
    return module


def load_provenance(
    path: pathlib.Path = REPO_ROOT / "scripts" / "ci_provenance.py",
    module_name: str = "ci_provenance",
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load ci_provenance.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_sync_ci_debug_ssh_script(
    path: pathlib.Path = SYNC_CI_DEBUG_SSH_PATH, module_name: str = "sync_ci_debug_ssh_secret"
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load sync_ci_debug_ssh_secret.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BASE_WORKFLOW = """
name: CI
run-name: >-
  ${{ github.event_name == 'workflow_dispatch'
      && github.event.inputs.full_ci == 'true'
      && 'CI [dispatch:full]'
      || github.event_name == 'workflow_dispatch'
      && 'CI [dispatch:iteration]'
      || 'CI' }}

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]
  push:
    branches: [main]
    tags: ["v*"]
  workflow_dispatch:
    inputs:
      full_ci:
        description: "Run full CI for the selected ref"
        required: false
        default: "false"
  merge_group:
    types: [checks_requested]

concurrency:
  group: >-
    ${{ github.event_name == 'pull_request'
        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        && format('pr-{0}-deferred', github.event.number)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == false
        && (github.event.action == 'reopened'
            || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != '')))
        && format('pr-{0}-noop', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'workflow_dispatch'
        && github.event.inputs.full_ci == 'true'
        && format('{0}-dispatch-full', github.ref_name)
        || github.event_name == 'workflow_dispatch'
        && format('{0}-dispatch-iteration', github.ref_name)
        || github.event_name == 'merge_group'
        && format('mq-{0}', github.ref)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))))
        || github.event_name == 'workflow_dispatch' }}

permissions:
  contents: read
  actions: read

jobs:
  merge-readiness-progress:
    name: merge-readiness-progress
    if: >-
      ${{ github.event_name == 'pull_request'
          && github.event.pull_request.draft == false
          && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
              || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
          && !(github.event.action == 'edited'
               && !(github.event.changes.base.ref.from != '')) }}
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    permissions:
      contents: read
      checks: read
      pull-requests: write
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
        with:
          ref: ${{ github.event.pull_request.base.sha }}
          persist-credentials: false
      - uses: actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0
        with:
          python-version: "3.12"
      - name: Watch merge-readiness progress
        env:
          GITHUB_TOKEN: ${{ github.token }}
          GITHUB_REPOSITORY: ${{ github.repository }}
          PR_NUMBER: ${{ github.event.pull_request.number }}
          PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}
        run: |
          if [[ ! -f scripts/merge_readiness.py ]]; then
            echo "merge_readiness.py is not present on the PR base; skipping"
            exit 0
          fi
          python3 scripts/merge_readiness.py comment "$PR_NUMBER" \
            --head-sha "$PR_HEAD_SHA" \
            --run-id "$GITHUB_RUN_ID" \
            --run-attempt "$GITHUB_RUN_ATTEMPT" \
            --watch

  ci-policy:
    name: ci-policy
    needs: detector
    outputs:
      ci_policy_path: ${{ steps.policy.outputs.ci_policy_path }}
      full_ci_required: ${{ steps.policy.outputs.full_ci_required }}
      full_ci_deferred: ${{ steps.policy.outputs.full_ci_deferred }}
      gate_name: ${{ steps.policy.outputs.gate_name }}
      backtester_gate_name: ${{ steps.policy.outputs.backtester_gate_name }}
      expected_event_class: ${{ steps.policy.outputs.expected_event_class }}
      reason: ${{ steps.policy.outputs.reason }}
      ignore_emit_failure: ${{ steps.policy.outputs.ignore_emit_failure }}
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - uses: actions/setup-python@a309ff8b426b58ec0e2a45f0f869d46889d02405 # v6.2.0
        with:
          python-version: "3.12"
      - name: Prepare trusted base policy tree
        id: policy_base
        if: github.event_name == 'pull_request' || github.event_name == 'merge_group'
        shell: bash
        env:
          MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}
        run: |
          base_ref="refs/remotes/origin/ci-policy-base-${{ github.event.pull_request.number }}"
          git fetch --no-tags origin "+refs/heads/${{ github.event.pull_request.base.ref }}:${base_ref}"
          git check-ref-format "refs/heads/$base_branch"
          base_tree="$RUNNER_TEMP/ci-policy-base-tree"
          mkdir -p "$base_tree"
          git archive "$base_ref" scripts/ ci/github-actions-runners.toml | tar -x -C "$base_tree"
          echo "script=$base_tree/scripts/ci_provenance.py" >> "$GITHUB_OUTPUT"
          echo "config=$base_tree/ci/github-actions-runners.toml" >> "$GITHUB_OUTPUT"
      - name: Compute CI policy
        id: policy
        shell: bash
        env:
          PR_HEAD_REF: ${{ github.event.pull_request.head.ref || '' }}
          EVENT_SENDER_ID: ${{ github.event.sender.id }}
        run: |
          policy_script="${{ steps.policy_base.outputs.script }}"
          if [[ -z "$policy_script" ]]; then
            policy_script="scripts/ci_provenance.py"
          fi
          policy_config="${{ steps.policy_base.outputs.config }}"
          if [[ -z "$policy_config" ]]; then
            policy_config="ci/github-actions-runners.toml"
          fi
          python3 "$policy_script" ci-policy \
            --config "$policy_config" \
            --event-name "${{ github.event_name }}" \
            --event-action "${{ github.event.action || '' }}" \
            --pull-request-draft "${{ github.event.pull_request.draft || false }}" \
            --pull-request-head-ref "$PR_HEAD_REF" \
            --pull-request-base-changed "${{ github.event.changes.base.ref.from != '' }}" \
            --workflow-dispatch-full-ci "${{ github.event.inputs.full_ci || '' }}" \
            --docs-only "${{ needs.detector.outputs.docs_only || 'false' }}" \
            --ref "${{ github.ref }}" \
            | tee -a "$GITHUB_OUTPUT"

  detector:
    name: detector
    outputs:
      build_required: ${{ steps.build_required.outputs.value }}
      fingerprint_reuse_allowed: ${{ steps.fingerprint_reuse_allowed.outputs.value }}
      docs_only: ${{ steps.docs_only.outputs.docs_only }}
    runs-on: ubuntu-latest
    steps:
      # detector probe insertion point
      - name: Fetch PR base/head refs
        id: pr_refs
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          base_ref="refs/remotes/origin/pr-base-${{ github.event.pull_request.number }}"
          head_ref="refs/remotes/origin/pr-head-${{ github.event.pull_request.number }}"
          git fetch --no-tags origin \
            "+refs/heads/${{ github.event.pull_request.base.ref }}:${base_ref}" \
            "+refs/pull/${{ github.event.pull_request.number }}/head:${head_ref}"
          echo "base_ref=${base_ref}" >> "$GITHUB_OUTPUT"
          echo "head_ref=${head_ref}" >> "$GITHUB_OUTPUT"

      - name: Detect docs-only safe changes
        id: docs_only
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          base_ref="${{ steps.pr_refs.outputs.base_ref }}"
          head_ref="${{ steps.pr_refs.outputs.head_ref }}"
          changed_files="$RUNNER_TEMP/docs-safe-changed-files.txt"
          git diff --name-only "${base_ref}...${head_ref}" > "$changed_files"
          base_tree="$RUNNER_TEMP/ci-policy-base-tree"
          mkdir -p "$base_tree"
          git archive "$base_ref" scripts/ ci/rust-verification.toml ci/github-actions-runners.toml .github/workflows/ci.yml | tar -x -C "$base_tree"
          python3 "$base_tree/scripts/verify_ci_path_filters.py" \
            --changed-files "$changed_files" \
            --github-output "$GITHUB_OUTPUT"

      - name: Detect build-affecting changes
        id: build_inputs_changed
        if: github.event_name == 'pull_request'
        shell: bash
        run: echo "any_changed=false" >> "$GITHUB_OUTPUT"

      - name: Detect fingerprint-reuse governance changes
        id: fingerprint_reuse_inputs_changed
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          base_ref="${{ steps.pr_refs.outputs.base_ref }}"
          head_ref="${{ steps.pr_refs.outputs.head_ref }}"
          changed="$(git diff --name-only "${base_ref}...${head_ref}" -- \
            .github/workflows/ci.yml \
            .github/actions/setup-environment/action.yml \
            ci/nextest-fingerprint.toml \
            ci/github-actions-runners.toml \
            scripts/nextest_fingerprint.py \
            scripts/test_nextest_fingerprint.py \
            scripts/root_bin_sidecars.py \
            scripts/test_root_bin_sidecars.py \
            scripts/ci_provenance.py \
            scripts/test_ci_provenance.py \
            scripts/verify_ci_workflow_hygiene.py \
            scripts/test_verify_ci_workflow_hygiene.py)"
          if [[ -n "$changed" ]]; then
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          fi

      - name: Determine build requirement
        id: build_required
        shell: bash
        run: |
          if [[ "${{ github.event_name }}" == "push" || "${{ github.event_name }}" == "workflow_dispatch" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
          elif [[ "${{ github.event_name }}" == "merge_group" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
          elif [[ "${{ steps.build_inputs_changed.outputs.any_changed }}" == "true" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
          else
            echo "value=false" >> "$GITHUB_OUTPUT"
          fi

      - name: Determine fingerprint reuse allowance
        id: fingerprint_reuse_allowed
        shell: bash
        run: |
          if [[ "${{ github.event_name }}" != "pull_request" ]]; then
            echo "value=false" >> "$GITHUB_OUTPUT"
          elif [[ "${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true" ]]; then
            echo "value=false" >> "$GITHUB_OUTPUT"
          else
            echo "value=true" >> "$GITHUB_OUTPUT"
          fi

  deny:
    name: deny
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-deny-version: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - run: just deny

  clippy:
    name: clippy
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
          toolchain-components: clippy, rustfmt
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - uses: actions/cache@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-
      - run: just fmt-check
      - run: just clippy

  check-aarch64:
    name: check-aarch64
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'tag_reuse' }}
    runs-on: ubuntu-latest
    steps:
      - name: Resolve aarch64 coverage owner
        run: |
          if [[ "${{ needs.detector.outputs.build_required }}" == "true" ]]; then
            echo "build_required=true; aarch64 coverage is provided by build"
          else
            echo "build_required=false; running standalone aarch64 check"
          fi
      - uses: ./.github/actions/setup-environment
        if: needs.detector.outputs.build_required != 'true'
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-build-values: "true"
          use-default-target: "true"
          include-managed-target-dir: "true"
      - name: Install aarch64 cross compiler
        if: needs.detector.outputs.build_required != 'true'
        run: sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross
      - uses: Swatinem/rust-cache@example
        if: needs.detector.outputs.build_required != 'true'
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - uses: actions/cache@example
        if: needs.detector.outputs.build_required != 'true'
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-
      - if: needs.detector.outputs.build_required != 'true'
        run: just check-aarch64

  source-fence:
    name: source-fence
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - uses: actions/cache@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-
      - run: just source-fence

  nextest-fingerprint:
    name: nextest fingerprint
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ubuntu-latest
    outputs:
      nextest_digest: ${{ steps.nextest-fingerprint.outputs.nextest_digest }}
      nextest_fingerprint: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint }}
      nextest_fingerprint_artifact_name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}
      nextest_archive_prefix: ${{ steps.nextest-fingerprint.outputs.nextest_archive_prefix }}
      nextest_schema: ${{ steps.nextest-fingerprint.outputs.nextest_schema }}
      nextest_profile: ${{ steps.nextest-fingerprint.outputs.nextest_profile }}
      nextest_shards: ${{ steps.nextest-fingerprint.outputs.nextest_shards }}
    steps:
      - name: Publish nextest archive fingerprint
        id: nextest-fingerprint
        shell: bash
        run: |
          python3 scripts/nextest_fingerprint.py \
            --repo-root "$GITHUB_WORKSPACE" \
            --config ci/nextest-fingerprint.toml \
            --runners-config ci/github-actions-runners.toml \
            --runner-os "${{ runner.os }}" \
            --runner-arch "${{ runner.arch }}" \
            --output-path .nextest-archive-fingerprint/cache-key.txt
      - name: Upload nextest archive fingerprint
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}
          path: .nextest-archive-fingerprint/cache-key.txt
          if-no-files-found: error
          retention-days: 30

  nextest-fingerprint-reuse:
    name: nextest fingerprint reuse
    needs: [ci-policy, detector, nextest-fingerprint]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && github.event_name == 'pull_request' && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}
    runs-on: ubuntu-latest
    outputs:
      reuse_found: ${{ steps.reuse.outputs.reuse_found }}
      source_run_id: ${{ steps.reuse.outputs.source_run_id }}
      source_sha: ${{ steps.reuse.outputs.source_sha }}
      source_artifact_id: ${{ steps.reuse.outputs.source_artifact_id }}
      reason: ${{ steps.reuse.outputs.reason }}
    steps:
      - name: Resolve nextest fingerprint reuse
        id: reuse
        shell: bash
        env:
          GITHUB_TOKEN: ${{ github.token }}
        run: >
          python3 scripts/ci_provenance.py resolve-fingerprint
          --current-run-id "${{ github.run_id }}"
          --current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"
          | tee -a "$GITHUB_OUTPUT"

  test-archive:
    name: nextest archive
    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && needs.detector.result == 'success' && needs.nextest-fingerprint.result == 'success' && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}
    runs-on: ubuntu-latest
    env:
      NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst
      ROOT_BIN_SIDECARS_PATH: .nextest-archive/root-bin-sidecars.tar.gz
    steps:
      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - name: Restore nextest archive
        id: nextest-archive-cache
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5
        with:
          path: ${{ env.NEXTEST_ARCHIVE_PATH }}
          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}
      - name: Restore root binary sidecars
        id: root-bin-sidecars-cache
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ env.ROOT_BIN_SIDECARS_PATH }}
          key: root-bin-sidecars-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-${{ needs.nextest-fingerprint.outputs.nextest_digest }}
      - name: Restore archive build target cache
        id: test-target-cache
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-
      - name: Install cargo-nextest
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - name: Build nextest archive
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        env:
          CARGO_PROFILE_TEST_DEBUG: "0"
          CARGO_PROFILE_DEV_DEBUG: "0"
        run: |
          mkdir -p "$(dirname "$NEXTEST_ARCHIVE_PATH")"
          just test-archive "$NEXTEST_ARCHIVE_PATH"
      - name: Save nextest archive
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5
        with:
          path: ${{ env.NEXTEST_ARCHIVE_PATH }}
          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}
      - name: Extract root binary sidecars
        if: steps.root-bin-sidecars-cache.outputs.cache-hit == 'true'
        run: |
          mkdir -p "${{ steps.setup.outputs.managed_target_dir }}"
          tar -xzf "$ROOT_BIN_SIDECARS_PATH" -C "${{ steps.setup.outputs.managed_target_dir }}"
      - name: Pack root binary sidecars from archive build
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        run: |
          target_dir="${{ steps.setup.outputs.managed_target_dir }}"
          python3 scripts/root_bin_sidecars.py pack \
            --repo-root "$GITHUB_WORKSPACE" \
            --target-dir "$target_dir" \
            --output "$GITHUB_WORKSPACE/$ROOT_BIN_SIDECARS_PATH"
      - name: Build root binary sidecars
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
      - name: Save root binary sidecars
        if: steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ env.ROOT_BIN_SIDECARS_PATH }}
          key: root-bin-sidecars-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-${{ needs.nextest-fingerprint.outputs.nextest_digest }}
      - name: Save archive build target cache
        if: ${{ (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}
      - name: Run nextest archive partitions
        shell: bash
        run: |
          shards="${{ needs.nextest-fingerprint.outputs.nextest_shards }}"
          if [[ ! "$shards" =~ ^[1-9][0-9]*$ ]]; then
            echo "invalid nextest shard count: ${shards}"
            exit 1
          fi
          mkdir -p "$RUNNER_TEMP/nextest-archive-extract"
          status=0
          for shard in $(seq 1 "$shards"); do
            echo "::group::nextest archive partition ${shard}/${shards}"
            echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <extract-root> --partition count:${shard}/${shards}"
            if ! just test-archive-run "$NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/nextest-archive-extract" --partition "count:${shard}/${shards}"; then
              status=1
            fi
            echo "::endgroup::"
          done
          exit "$status"

  test:
    name: test
    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          if [[ "${{ needs.nextest-fingerprint.result }}" != "success" ]]; then
            exit 1
          fi
          reuse_found="${{ needs.nextest-fingerprint-reuse.outputs.reuse_found }}"
          if [[ "$reuse_found" == "true" ]]; then
            if [[ "${{ needs.nextest-fingerprint-reuse.result }}" != "success" ]]; then
              exit 1
            fi
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.source_run_id }}" ]]; then
              echo "nextest fingerprint reuse did not expose source_run_id"
              exit 1
            fi
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.source_sha }}" ]]; then
              echo "nextest fingerprint reuse did not expose source_sha"
              exit 1
            fi
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.source_artifact_id }}" ]]; then
              echo "nextest fingerprint reuse did not expose source_artifact_id"
              exit 1
            fi
            echo "nextest archive reused from run ${{ needs.nextest-fingerprint-reuse.outputs.source_run_id }}"
            exit 0
          fi
          if [[ "${{ needs.test-archive.result }}" != "success" ]]; then
            exit 1
          fi

  build:
    name: build
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' && needs.detector.outputs.build_required == 'true' }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-build-values: "true"
          use-default-target: "true"
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.job == 'test-archive' }}
      - uses: actions/cache@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-
      - name: Install zig
        run: |
          python -m pip install ziglang=="${{ steps.setup.outputs.zig_version }}"
      - name: Install cargo-zigbuild
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-zigbuild@${{ steps.setup.outputs.zigbuild_version }}
          fallback: none
      - run: just build
      - name: Stage managed build artifact
        id: managed_artifact
        run: |
          binary_path="$(python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2)"
          stage_dir="$RUNNER_TEMP/bolt-v2-binary"
          rm -rf "$stage_dir"
          mkdir -p "$stage_dir"
          cp "$binary_path" "$stage_dir/bolt-v2"
          (
            cd "$stage_dir"
            sha256sum bolt-v2 > bolt-v2.sha256
          )
          echo "stage_dir=$stage_dir" >> "$GITHUB_OUTPUT"
      - name: Upload artifact
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: bolt-v2-binary
          path: |
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256
          retention-days: 3

  ci-provenance-emit:
    name: ci-provenance-emit
    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]
    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}
    runs-on: ubuntu-latest
    steps:
      - name: Prepare trusted base provenance tree
        id: provenance_base
        if: github.event_name == 'pull_request' || github.event_name == 'merge_group'
        env:
          MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}
        run: |
          git check-ref-format "refs/heads/$base_branch"
          git archive "$base_ref" scripts/ ci/github-actions-runners.toml
          tested_workflow="$GITHUB_WORKSPACE/.github/workflows/ci.yml"
          echo "tested workflow file is missing or not a regular file"
          cp "$tested_workflow" "$base_tree/.github/workflows/ci.yml"
          {
            echo "script=$base_tree/scripts/ci_provenance.py"
            echo "config=$base_tree/ci/github-actions-runners.toml"
            echo "workflow=$base_tree/.github/workflows/ci.yml"
          } >> "$GITHUB_OUTPUT"
      - name: Emit CI provenance
        run: |
          provenance_script="${{ steps.provenance_base.outputs.script }}"
          provenance_config="${{ steps.provenance_base.outputs.config }}"
          provenance_workflow="${{ steps.provenance_base.outputs.workflow }}"
          ci_policy_path="${{ needs.ci-policy.outputs.ci_policy_path }}"
          policy_args=()
          if python3 "$provenance_script" emit-full-ci --help | grep -q -- "--ci-policy-path"; then
            policy_args+=(--ci-policy-path "$ci_policy_path")
          elif [[ "$ci_policy_path" != "full" ]]; then
            echo "trusted base provenance emitter does not support ci_policy_path=$ci_policy_path" >&2
            exit 1
          fi
          workflow_args=()
          if python3 "$provenance_script" emit-full-ci --help | grep -q -- "--workflow-file"; then
            workflow_args+=(--workflow-file "$provenance_workflow")
          fi
          python3 "$provenance_script" emit-full-ci \
            --config "$provenance_config" \
            "${policy_args[@]}" \
            "${workflow_args[@]}" \
            --output ci-provenance.json \
            --required-job detector=${{ needs.detector.result }} \
            --required-job deny=${{ needs.deny.result }} \
            --required-job clippy=${{ needs.clippy.result }} \
            --required-job check-aarch64=${{ needs.check-aarch64.result }} \
            --required-job source-fence=${{ needs.source-fence.result }} \
            --required-job nextest-fingerprint=${{ needs.nextest-fingerprint.result }} \
            --required-job test-archive=${{ needs.test-archive.result }} \
            --required-job test=${{ needs.test.result }} \
            --conditional-job build.required=${{ needs.detector.outputs.build_required }} \
            --conditional-job build.result=${{ needs.build.result }} \
            --nextest-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}"
      - name: Upload CI provenance
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ci-provenance-attempt-${{ github.run_attempt }}
          path: ci-provenance.json
          if-no-files-found: error
          retention-days: 30

  same-sha-main-evidence:
    name: same-sha-main-evidence
    needs: detector
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    outputs:
      source_run_id: ${{ steps.evidence.outputs.source_run_id }}
      check_suite_id: ${{ steps.evidence.outputs.check_suite_id }}
      artifact_id: ${{ steps.evidence.outputs.artifact_id }}
      source_sha: ${{ steps.evidence.outputs.source_sha }}
    steps:
      - name: Resolve same-SHA main evidence
        id: evidence
        run: python3 scripts/find_same_sha_main_evidence.py

  gate:
    name: ${{ needs.ci-policy.outputs.gate_name }}
    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build, ci-provenance-emit, same-sha-main-evidence]
    if: ${{ always() }}
    runs-on: ubuntu-latest
    steps:
      - name: Prepare trusted base verdict tree
        id: verdict_base
        if: github.event_name == 'pull_request' || github.event_name == 'merge_group'
        shell: bash
        env:
          MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}
        run: |
          base_ref="refs/remotes/origin/ci-gate-base-${{ github.event.pull_request.number }}"
          git fetch --no-tags origin "+refs/heads/${{ github.event.pull_request.base.ref }}:${base_ref}"
          git check-ref-format "refs/heads/$base_branch"
          base_tree="$RUNNER_TEMP/ci-gate-base-tree"
          mkdir -p "$base_tree"
          git archive "$base_ref" scripts/ ci/github-actions-runners.toml | tar -x -C "$base_tree"
          echo "script=$base_tree/scripts/ci_provenance.py" >> "$GITHUB_OUTPUT"
      - name: Resolve gate carry-forward
        id: carry_forward
        if: ${{ needs.ci-policy.outputs.ci_policy_path == 'noop' || needs.ci-policy.outputs.full_ci_deferred == 'true' }}
        shell: bash
        run: |
          verdict_script="${{ steps.verdict_base.outputs.script }}"
          if [[ -z "$verdict_script" ]]; then
            verdict_script="scripts/ci_provenance.py"
          fi
          python3 "$verdict_script" resolve-gate-carry-forward \
            --sha "${{ github.event.pull_request.head.sha || github.sha }}" \
            --base-sha "${{ github.event.pull_request.base.sha || '' }}" \
            --current-run-id "${{ github.run_id }}" \
            --gate-name "${{ needs.ci-policy.outputs.gate_name }}" \
            --workflow-path ".github/workflows/ci.yml" \
            --require-provenance-base true \
            | tee -a "$GITHUB_OUTPUT"
      - name: Check required lanes
        shell: bash
        run: |
          verdict_script="${{ steps.verdict_base.outputs.script }}"
          if [[ -z "$verdict_script" ]]; then
            verdict_script="scripts/ci_provenance.py"
          fi
          carry_forward_args=()
          carry_forward_verified="${{ steps.carry_forward.outputs.carry_forward_verified }}"
          if [[ -n "$carry_forward_verified" ]]; then
            carry_forward_args+=(--carry-forward-verified "$carry_forward_verified")
          fi
          python3 "$verdict_script" check-ci-gate \
            --policy-path "${{ needs.ci-policy.outputs.ci_policy_path }}" \
            --expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}" \
            --full-ci-deferred "${{ needs.ci-policy.outputs.full_ci_deferred }}" \
            --ignore-emit-failure "${{ needs.ci-policy.outputs.ignore_emit_failure }}" \
            --reuse-found "${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || 'false' }}" \
            "${carry_forward_args[@]}" \
            --build-required "${{ needs.detector.outputs.build_required || 'false' }}" \
            --job ci-policy=${{ needs.ci-policy.result }} \
            --job detector=${{ needs.detector.result }} \
            --job deny=${{ needs.deny.result }} \
            --job clippy=${{ needs.clippy.result }} \
            --job check-aarch64=${{ needs.check-aarch64.result }} \
            --job source-fence=${{ needs.source-fence.result }} \
            --job nextest-fingerprint=${{ needs.nextest-fingerprint.result }} \
            --job test-archive=${{ needs.test-archive.result }} \
            --job nextest-fingerprint-reuse=${{ needs.nextest-fingerprint-reuse.result }} \
            --job test=${{ needs.test.result }} \
            --job build=${{ needs.build.result }} \
            --job ci-provenance-emit=${{ needs.ci-provenance-emit.result }} \
            --job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}

  deploy:
    name: deploy
    needs: [gate, same-sha-main-evidence, build, detector, deny, clippy, check-aarch64, source-fence, test]
    if: ${{ always() && startsWith(github.ref, 'refs/tags/v') && needs.gate.result == 'success' && needs.same-sha-main-evidence.result == 'success' }}
    runs-on: ubuntu-latest
    permissions:
      contents: read
      actions: read
      id-token: write
    steps:
      - run: |
          echo "source_run_id=${{ needs.same-sha-main-evidence.outputs.source_run_id }}"
          echo "check_suite_id=${{ needs.same-sha-main-evidence.outputs.check_suite_id }}"
          echo "artifact_id=${{ needs.same-sha-main-evidence.outputs.artifact_id }}"
          echo "source_sha=${{ needs.same-sha-main-evidence.outputs.source_sha }}"
      - uses: actions/download-artifact@example
        with:
          artifact-ids: ${{ needs.same-sha-main-evidence.outputs.artifact_id }}
          github-token: ${{ github.token }}
          repository: ${{ github.repository }}
          run-id: ${{ needs.same-sha-main-evidence.outputs.source_run_id }}
          path: artifact/
      - name: Verify downloaded artifact checksum
        run: |
          cd artifact
          sha256sum -c bolt-v2.sha256
      - run: echo deploy
"""


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

permissions:
  checks: write
  contents: read
  pull-requests: read

jobs:
  coverage-enforcer:
    name: coverage-enforcer
    if: >-
      ${{ github.event_name == 'merge_group'
          || (github.event_name == 'pull_request'
              && github.event.pull_request.draft == false
              && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
                  || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
              && !(github.event.action == 'edited'
                   && !(github.event.changes.base.ref.from != ''))) }}
    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}
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
            echo "coverage-enforcer bootstrap: trusted base tree lacks scripts/coverage_enforcer.py"
            exit 0
          fi
          python3 scripts/coverage_enforcer.py
"""


BASE_ADVISORY_WORKFLOW = """
name: Advisory Check

on:
  workflow_dispatch: {}

env:
  JUST_VERSION: "1.49.0"

jobs:
  advisories:
    name: advisories
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@example
      - name: Setup environment
        id: setup
        uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-deny-version: "true"
      - name: Install cargo-deny
        uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - name: Check advisories
        run: just deny-advisories
"""

BASE_ACTION = """
name: Setup Environment
inputs:
  just-version:
    required: true
  include-deny-version:
    required: false
    default: "false"
  include-nextest-version:
    required: false
    default: "false"
  include-build-values:
    required: false
    default: "false"
  lint-workflow-contract:
    required: false
    default: "false"
  include-managed-target-dir:
    description: Whether to resolve the managed target dir.
    required: false
    default: "false"
outputs:
  rust_toolchain:
    value: ${{ steps.shared.outputs.rust_toolchain }}
  deny_version:
    value: ${{ steps.shared.outputs.deny_version }}
  nextest_version:
    value: ${{ steps.shared.outputs.nextest_version }}
  target:
    value: ${{ steps.shared.outputs.target }}
  zig_version:
    value: ${{ steps.shared.outputs.zig_version }}
  zigbuild_version:
    value: ${{ steps.shared.outputs.zigbuild_version }}
  rust_verification_owner:
    value: ${{ steps.shared.outputs.rust_verification_owner }}
  managed_target_dir:
    value: ${{ steps.target_dir.outputs.managed_target_dir }}
  managed_target_dir_relative:
    value: ${{ steps.target_dir.outputs.managed_target_dir_relative }}
runs:
  using: composite
  steps:
    - name: Install just
      uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538 # v2.81.1
      with:
        tool: just@${{ inputs.just-version }}
        fallback: none
    - name: Lint workflow contract
      if: ${{ inputs.lint-workflow-contract == 'true' }}
      shell: bash
      run: just ci-lint-workflow
    - name: Read shared values
      id: shared
      shell: bash
      run: |
        echo "rust_toolchain=$(awk -F'\\\"' '/^channel = / {print $2}' rust-toolchain.toml)" >> "$GITHUB_OUTPUT"
        echo "rust_verification_owner=$(just --evaluate rust_verification_owner)" >> "$GITHUB_OUTPUT"
        if [ "${{ inputs.include-deny-version }}" = "true" ]; then
          echo "deny_version=$(just --evaluate deny_version)" >> "$GITHUB_OUTPUT"
        fi
        if [ "${{ inputs.include-nextest-version }}" = "true" ]; then
          echo "nextest_version=$(just --evaluate nextest_version)" >> "$GITHUB_OUTPUT"
        fi
        if [ "${{ inputs.include-build-values }}" = "true" ]; then
          echo "target=$(just --evaluate target)" >> "$GITHUB_OUTPUT"
          echo "zig_version=$(just --evaluate zig_version)" >> "$GITHUB_OUTPUT"
          echo "zigbuild_version=$(just --evaluate zigbuild_version)" >> "$GITHUB_OUTPUT"
        fi
    - name: Resolve managed target dir
      if: ${{ inputs.include-managed-target-dir == 'true' }}
      id: target_dir
      shell: bash
      run: |
        managed_target_dir="$(python3 "${{ steps.shared.outputs.rust_verification_owner }}" target-dir --repo "$GITHUB_WORKSPACE")"
        managed_target_dir_relative="$(python3 -c 'import os, sys; print(os.path.relpath(sys.argv[2], sys.argv[1]))' "$GITHUB_WORKSPACE" "$managed_target_dir")"
        echo "managed_target_dir=$managed_target_dir" >> "$GITHUB_OUTPUT"
        echo "managed_target_dir_relative=$managed_target_dir_relative" >> "$GITHUB_OUTPUT"
    - name: Setup Rust toolchain
      shell: bash
      run: echo setup
"""


BASE_NEXTEST_CONFIG = """
[test-groups]
live-node = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'binary(=bolt_v2) & (test(~bolt_v3_client_registration::tests::) | test(~bolt_v3_live_node::tests::))'
test-group = 'live-node'

[[profile.default.overrides]]
filter = 'binary(=bolt_v3_adapter_mapping) | binary(=bolt_v3_client_registration) | binary(=bolt_v3_controlled_connect) | binary(=bolt_v3_credential_log_suppression) | binary(=bolt_v3_readiness) | binary(=bolt_v3_strategy_registration) | binary(=bolt_v3_submit_admission) | binary(=config_parsing) | binary(=lake_batch) | binary(=nt_runtime_capture) | binary(=venue_contract)'
test-group = 'live-node'
"""


def all_standalone_live_node_manifest(verifier=None) -> CiTestManifest:
    if verifier is None:
        verifier = load_verifier()
    member_to_harness = {member: member for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    harness_to_members = {member: (member,) for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)


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


TEST_HARNESS_NAMES = (
    "iv",
    "outcome_groups",
    "maker_taker",
    "kill_switch_loss",
    "pricing",
    "admission_orders",
    "platform_config",
    "runtime_capture_io",
    "wiring_registration",
    "bolt_v3_risk_reservation_substrate",
    "bolt_v3_risk_reservation_epoch_manager",
)
TEST_HARNESS_MEMBER = "bolt_v3_fixture_member"


def base_test_harness_manifest(
    harness_to_members: dict[str, tuple[str, ...]] | None = None,
) -> CiTestManifest:
    if harness_to_members is None:
        harness_to_members = {
            harness: ((harness, TEST_HARNESS_MEMBER) if harness == "iv" else (harness,))
            for harness in TEST_HARNESS_NAMES
        }
    member_to_harness: dict[str, str] = {}
    for harness, members in harness_to_members.items():
        for member in members:
            member_to_harness.setdefault(member, harness)
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)


def write_test_harness_fixture(
    root: pathlib.Path,
    *,
    manifest: CiTestManifest | None = None,
    cargo_autotests: str = "false",
    test_files: dict[str, str] | None = None,
    workflow_text: str = "jobs:\n  test:\n    steps:\n      - run: cargo test --test pricing\n",
    justfile_text: str = "ci-test:\n    cargo test --test iv\n",
    write_workflow: bool = True,
    write_justfile: bool = True,
) -> None:
    cargo_lines = [
        "[package]",
        'name = "bolt-v2-fixture"',
        'version = "0.0.0"',
        'edition = "2021"',
        f"autotests = {cargo_autotests}",
        "",
    ]
    for harness in TEST_HARNESS_NAMES:
        cargo_lines.extend(
            [
                "[[test]]",
                f'name = "{harness}"',
                f'path = "tests/{harness}.rs"',
                "",
            ]
        )
    (root / "Cargo.toml").write_text("\n".join(cargo_lines), encoding="utf-8")
    tests_root = root / "tests"
    tests_root.mkdir()
    fixture_files = {harness: "" for harness in TEST_HARNESS_NAMES}
    manifest_members = manifest.harness_to_members if manifest is not None else base_test_harness_manifest().harness_to_members
    for harness, members in manifest_members.items():
        for member in members:
            if member != harness:
                fixture_files[member] = "#[test]\nfn fixture_member_runs() {}\n"
    if test_files:
        fixture_files.update(test_files)
    for stem, text in fixture_files.items():
        path = tests_root / f"{stem}.rs"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    if write_workflow:
        workflow_path = root / ".github" / "workflows" / "ci.yml"
        workflow_path.parent.mkdir(parents=True)
        workflow_path.write_text(workflow_text, encoding="utf-8")
    if write_justfile:
        (root / "justfile").write_text(justfile_text, encoding="utf-8")


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


LOCAL_COMPILE_POLICY_TOML = """
[local_compile_policy]
enabled = true
allowed_ci_env = "GITHUB_ACTIONS"
break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
refused_managed_commands = ["test", "clippy", "build"]
refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]
"""

LOCAL_LANE_POLICY_TOML = """
[local_lane_policy]
enabled = true
allowed_ci_env = "GITHUB_ACTIONS"
lock_dir = "/tmp/rust-verification-lanes"
acquire_timeout_seconds = 1800
heartbeat_seconds = 15
poll_interval_seconds = 1
"""

BASE_RUST_VERIFICATION_POLICY = f"""
schema_version = 2
project_id = "bolt-v2"
target_namespace = "bolt-v2"

{LOCAL_COMPILE_POLICY_TOML}
{LOCAL_LANE_POLICY_TOML}

[remote_verification]
poll_interval_seconds = 15
checks_appear_timeout_seconds = 300
overall_timeout_seconds = 3600
diagnostic_log_max_lines = 160
diagnostic_log_max_bytes = 20000
diagnostic_unavailable_notice_interval_polls = 4
"""

BASE_BVS_RUST_VERIFICATION_POLICY = f"""
schema_version = 2
project_id = "backtesting-vertical-slice"
target_namespace = "backtesting-vertical-slice"

{LOCAL_COMPILE_POLICY_TOML}
{LOCAL_LANE_POLICY_TOML}
"""


def write_rust_verification_policy_fixtures(root: pathlib.Path) -> None:
    root_policy = root / "ci" / "rust-verification.toml"
    root_policy.parent.mkdir(parents=True, exist_ok=True)
    root_policy.write_text(BASE_RUST_VERIFICATION_POLICY, encoding="utf-8")
    bvs_policy = root / "crates" / "backtesting-vertical-slice" / "ci" / "rust-verification.toml"
    bvs_policy.parent.mkdir(parents=True, exist_ok=True)
    bvs_policy.write_text(BASE_BVS_RUST_VERIFICATION_POLICY, encoding="utf-8")


def assert_clean(
    workflow: str = BASE_WORKFLOW,
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action, nextest_config)
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


def assert_error(
    fragment: str,
    workflow: str = BASE_WORKFLOW,
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action, nextest_config)
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected error containing {fragment!r}, got: {errors}")


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


def without_job(workflow: str, job: str) -> str:
    lines = workflow.splitlines()
    start = next(i for i, line in enumerate(lines) if line == f"  {job}:")
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("  ") and not lines[i].startswith("    ") and lines[i].strip().endswith(":"):
            end = i
            break
    return "\n".join(lines[:start] + lines[end:]) + "\n"


def replace_once(text: str, old: str, new: str) -> str:
    if old not in text:
        raise AssertionError(f"fixture fragment not found: {old!r}")
    return text.replace(old, new, 1)


def replace_once_after(text: str, anchor: str, old: str, new: str) -> str:
    index = text.find(anchor)
    if index == -1:
        raise AssertionError(f"fixture anchor not found: {anchor!r}")
    before = text[:index]
    after = text[index:]
    return before + replace_once(after, old, new)


def without_once_after(text: str, anchor: str, old: str) -> str:
    index = text.find(anchor)
    if index == -1:
        raise AssertionError(f"fixture anchor not found: {anchor!r}")
    before = text[:index]
    after = text[index:]
    return before + after.replace(old, "", 1)


def repo_workflow_text(path: str) -> str:
    return (REPO_ROOT / path).read_text().replace("\r\n", "\n")


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


def ci_provenance_config_fixture() -> str:
    config_text = (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text()
    return strip_ci_provenance_config(config_text) + "\n" + CI_PROVENANCE_TOML


def runner_config_load_error(config_text: str, verifier=None) -> str:
    if verifier is None:
        verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        config_path = pathlib.Path(tmp) / "github-actions-runners.toml"
        config_path.write_text(config_text, encoding="utf-8")
        try:
            verifier.load_github_actions_runners_config(config_path)
        except Exception as exc:  # noqa: BLE001 - loader raises domain errors.
            return str(exc)
    return ""


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
            "ci_provenance.gate_names.gate_dispatch_full must not equal gate_required",
            valid.replace('gate_dispatch_full = "gate-dispatch"', 'gate_dispatch_full = "gate"'),
        ),
        (
            "ci_provenance.gate_names.backtester_dispatch_full must not equal backtester_required",
            valid.replace('backtester_dispatch_full = "backtester-gate-dispatch"', 'backtester_dispatch_full = "backtester-gate"'),
        ),
        (
            "ci_provenance.gate_names.gate_dispatch_full must be a GitHub Actions output-safe check name",
            valid.replace('gate_dispatch_full = "gate-dispatch"', 'gate_dispatch_full = "gate\\nignored=1"'),
        ),
        (
            "ci_provenance.gate_names.gate_dispatch_full must be a GitHub Actions output-safe check name",
            valid.replace('gate_dispatch_full = "gate-dispatch"', 'gate_dispatch_full = "gate-dispatch "'),
        ),
        (
            "ci_provenance.gate_names.backtester_dispatch_full must be a GitHub Actions output-safe check name",
            valid.replace(
                'backtester_dispatch_full = "backtester-gate-dispatch"',
                'backtester_dispatch_full = " backtester-gate-dispatch"',
            ),
        ),
        (
            "ci_provenance.gate_names.gate_iteration must be a GitHub Actions output-safe check name",
            valid.replace('gate_iteration = "gate-iteration"', 'gate_iteration = "${{ github.ref }}"'),
        ),
        (
            "ci_provenance.policy.ready_pr is proof-affecting",
            valid.replace('ready_pr = "iteration"', 'ready_pr = "defer"'),
        ),
        (
            "ci_provenance.policy.ready_for_review is proof-affecting",
            valid.replace('ready_for_review = "iteration"', 'ready_for_review = "defer"'),
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
            valid.replace('merge_group = "full"', 'merge_group = "defer"'),
        ),
        (
            "ci_provenance.policy.merge_group is proof-affecting and must be full",
            valid.replace('merge_group = "full"', 'merge_group = "tag_reuse"'),
        ),
        (
            "ci_provenance.policy.mergify_temp_pr is proof-affecting",
            valid.replace('mergify_temp_pr = "full"', 'mergify_temp_pr = "defer"'),
        ),
        (
            "ci_provenance.policy.unknown_event is proof-affecting",
            valid.replace('unknown_event = "full"', 'unknown_event = "defer"'),
        ),
        (
            "ci_provenance.policy.unknown_event is proof-affecting and must be full",
            valid.replace('unknown_event = "full"', 'unknown_event = "tag_reuse"'),
        ),
        (
            "ci_provenance.policy.workflow_dispatch_full_ci must remain full",
            valid.replace('workflow_dispatch_full_ci = "full"', 'workflow_dispatch_full_ci = "iteration"'),
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
            "ci_provenance.policy.ready_pr must be iteration",
            valid.replace('ready_pr = "iteration"', 'ready_pr = "full"'),
        ),
        (
            "ci_provenance.policy.ready_pr_edited_no_base must be iteration",
            valid.replace('ready_pr_edited_no_base = "iteration"', 'ready_pr_edited_no_base = "full"'),
        ),
        (
            "ci_provenance.policy.ready_pr_reopened must be iteration",
            valid.replace('ready_pr_reopened = "iteration"', 'ready_pr_reopened = "full"'),
        ),
        (
            "ci_provenance.policy has unexpected keys",
            valid.replace(
                "[ci_provenance.mergify]",
                'unexpected_policy_row = "defer"\n\n[ci_provenance.mergify]',
            ),
        ),
    ]
    for fragment, config_text in cases:
        error = runner_config_load_error(config_text)
        if fragment not in error:
            raise AssertionError(f"expected {fragment!r}, got {error!r}")

    verifier = load_verifier()

    queue_covered_policy = {
        "draft_pr_synchronize": "defer",
        "draft_pr_opened": "defer",
        "draft_pr_reopened": "defer",
        "draft_pr_edited": "defer",
        "converted_to_draft": "defer",
        "ready_pr": "iteration",
        "ready_pr_edited_no_base": "noop",
        "ready_pr_reopened": "noop",
        "ready_for_review": "iteration",
        "docs": "docs",
        "workflow_dispatch": "iteration",
        "workflow_dispatch_full_ci": "full",
        "main_push": "full",
        "merge_group": "full",
        "mergify_temp_pr": "full",
        "tag": "tag_reuse",
        "unknown_event": "full",
    }
    queue_covered_errors = verifier.policy_proof_invariant_errors(queue_covered_policy)
    if queue_covered_errors:
        raise AssertionError(
            "proof invariant must keep the future queue-covered iteration carve-out "
            f"isolated from the active Design 0 contract, got: {queue_covered_errors}"
        )

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
    # Queue-only rework (#981): every ordinary pull_request defers heavy lanes to
    # ci_policy_path == "iteration"; only the merge boundary (merge_group, push to
    # main, tag) stays "full". The actor-bound mergify temp PR is covered separately
    # below because it depends on the event sender id.
    cases = [
        ("push", "", False, False, "", "refs/heads/main", "full"),
        ("push", "", False, False, "true", "refs/heads/main", "full"),
        ("push", "", False, False, "", "refs/tags/v1.2.3", "tag_reuse"),
        ("pull_request", "opened", True, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "synchronize", True, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "reopened", True, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "edited", True, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "converted_to_draft", True, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "opened", False, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "edited", False, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "edited", False, True, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "reopened", False, False, "", "refs/pull/1/merge", "iteration"),
        ("pull_request", "ready_for_review", False, False, "", "refs/pull/1/merge", "iteration"),
        ("workflow_dispatch", "", True, False, "true", "refs/heads/codex/branch", "full"),
        ("workflow_dispatch", "", True, False, "false", "refs/heads/codex/branch", "iteration"),
        ("workflow_dispatch", "", True, False, "", "refs/heads/codex/branch", "iteration"),
        ("workflow_dispatch", "", True, False, "TRUE", "refs/heads/codex/branch", "iteration"),
        ("workflow_dispatch", "", True, False, " true ", "refs/heads/codex/branch", "iteration"),
        ("workflow_dispatch", "", True, False, "1", "refs/heads/codex/branch", "iteration"),
        ("merge_group", "checks_requested", False, False, "", "refs/heads/gh-readonly-queue/main/pr-1-deadbeef", "full"),
        ("unknown_event", "", True, False, "", "refs/heads/codex/branch", "full"),
    ]
    # Queue-only rework (#981): the policy table pins every row to a non-"defer" value,
    # so no matrix row can exercise full_ci_deferred's True branch. Assert that invariant
    # explicitly; the deferred branch is exercised by dedicated VALIDATOR tests that feed
    # policy_path="defer" directly (verify_ci_provenance / backtester), not via this table.
    if any(expected == "defer" for *_, expected in cases):
        raise AssertionError("policy matrix must not expect 'defer' — defer is unreachable via the table")
    for event_name, action, draft, base_changed, workflow_dispatch_full_ci, ref, expected in cases:
        result = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci=workflow_dispatch_full_ci,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        if result.ci_policy_path != expected:
            raise AssertionError((event_name, action, draft, ref, expected, result))
        if result.full_ci_required != (expected == "full"):
            raise AssertionError(f"full_ci_required must derive from {expected}: {result}")
        if result.full_ci_deferred is not False:
            raise AssertionError(f"every policy-table row must resolve full_ci_deferred False: {result}")
        if event_name == "workflow_dispatch" and workflow_dispatch_full_ci == "true":
            if result.gate_name != "gate-dispatch" or result.backtester_gate_name != "backtester-gate-dispatch":
                raise AssertionError(f"workflow_dispatch full CI must publish non-required gate names: {result}")

    try:
        verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name="pull_request",
            action="ready_for_review",
            pull_request_draft=True,
            pull_request_base_changed=False,
            workflow_dispatch_full_ci="",
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref="refs/pull/1/merge",
        )
    except ValueError as exc:
        if "ready_for_review cannot be on a draft PR" not in str(exc):
            raise AssertionError(f"unexpected ready_for_review draft error: {exc}") from exc
    else:
        raise AssertionError("ready_for_review draft event must fail closed")

    # The actor-bound mergify temp PR (draft head ref + sender == bound actor) is the
    # SOLE pull_request that earns the required gate, and only for full-CI actions.
    mergify_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
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
        workflow_dispatch_full_ci="",
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

    # A metadata edit (no base change) is not a full-CI action, so even the bound
    # actor's temp PR falls through to the ordinary draft path -> iteration.
    mergify_edited_result = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
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
            f"Mergify temp PR metadata edits must defer to iteration: {mergify_edited_result}"
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
        workflow_dispatch_full_ci="",
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
        workflow_dispatch_full_ci="",
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
        ("push", "", False, False, "", "refs/heads/main"),
        ("push", "", False, False, "true", "refs/heads/main"),
        ("push", "", False, False, "", "refs/tags/v1.2.3"),
        ("pull_request", "opened", True, False, "", "refs/pull/1/merge"),
        ("pull_request", "synchronize", True, False, "", "refs/pull/1/merge"),
        ("pull_request", "reopened", True, False, "", "refs/pull/1/merge"),
        ("pull_request", "edited", True, False, "", "refs/pull/1/merge"),
        ("pull_request", "converted_to_draft", True, False, "", "refs/pull/1/merge"),
        ("pull_request", "opened", False, False, "", "refs/pull/1/merge"),
        ("pull_request", "edited", False, False, "", "refs/pull/1/merge"),
        ("pull_request", "edited", False, True, "", "refs/pull/1/merge"),
        ("pull_request", "reopened", False, False, "", "refs/pull/1/merge"),
        ("pull_request", "ready_for_review", False, False, "", "refs/pull/1/merge"),
        ("workflow_dispatch", "", True, False, "true", "refs/heads/codex/branch"),
        ("workflow_dispatch", "", True, False, "false", "refs/heads/codex/branch"),
        ("workflow_dispatch", "", True, False, "", "refs/heads/codex/branch"),
        ("workflow_dispatch", "", True, False, "TRUE", "refs/heads/codex/branch"),
        ("workflow_dispatch", "", True, False, " true ", "refs/heads/codex/branch"),
        ("merge_group", "checks_requested", False, False, "", "refs/heads/gh-readonly-queue/main/pr-1-deadbeef"),
        ("unknown_event", "", True, False, "", "refs/heads/codex/branch"),
    ]
    saw_full = saw_iteration = False
    for event_name, action, draft, base_changed, workflow_dispatch_full_ci, ref in cases:
        ver = verifier.evaluate_ci_policy(
            policy,
            gate_names,
            event_name=event_name,
            action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci=workflow_dispatch_full_ci,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        prov = provenance.evaluate_ci_policy(
            prov_config,
            event_name=event_name,
            event_action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci=workflow_dispatch_full_ci,
            ref=ref,
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.full_ci_deferred,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.full_ci_deferred,
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
    # Non-vacuous: the matrix must exercise both a full (merge boundary) and an
    # iteration (ordinary PR defer) resolution so the parity assertion compares real
    # divergent branches, not a constant. Under the #981 queue-only rework no event
    # resolves to defer/noop any more.
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
            workflow_dispatch_full_ci=workflow_dispatch_full_ci,
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        ).ci_policy_path
        == "full"
        for event_name, action, draft, base_changed, workflow_dispatch_full_ci, ref in cases
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
            workflow_dispatch_full_ci="",
            mergify_temp_pr_head_ref_prefix=mergify_prefix,
            ref=ref,
        )
        prov = provenance.evaluate_ci_policy(
            forced_prov,
            event_name=event_name,
            event_action=action,
            pull_request_draft=draft,
            pull_request_base_changed=base_changed,
            workflow_dispatch_full_ci="",
            ref=ref,
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.full_ci_deferred,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.full_ci_deferred,
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
        # Queue-only rework (#981): force_full_ci keeps ci_policy_path == "full" (heavy
        # lanes still run) but a pull_request head run is never proof of the merged
        # commit, so the gate name demotes to the non-required gate-iteration.
        if ver_tuple != ("full", True, False, "gate-iteration", "backtester-gate-iteration", "full", "force_full_ci"):
            raise AssertionError(
                f"force_full_ci must keep {event_name}/{action!r} full but demote the gate; got {ver_tuple}"
            )

    ver = verifier.evaluate_ci_policy(
        policy,
        gate_names,
        event_name="pull_request",
        action="opened",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
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
        workflow_dispatch_full_ci="",
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.full_ci_deferred,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.full_ci_deferred,
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
        action="edited",
        pull_request_draft=True,
        pull_request_head_ref="mergify/merge-queue/83d4b0be7e",
        pull_request_base_changed=False,
        workflow_dispatch_full_ci="",
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
        workflow_dispatch_full_ci="",
        event_sender_id=actor_id,
        ref="refs/pull/965/merge",
    )
    ver_tuple = (
        ver.ci_policy_path,
        ver.full_ci_required,
        ver.full_ci_deferred,
        ver.gate_name,
        ver.backtester_gate_name,
        ver.expected_event_class,
        ver.reason,
    )
    prov_tuple = (
        prov.ci_policy_path,
        prov.full_ci_required,
        prov.full_ci_deferred,
        prov.gate_name,
        prov.backtester_gate_name,
        prov.expected_event_class,
        prov.reason,
    )
    if ver_tuple != prov_tuple:
        raise AssertionError(
            f"ci_policy resolver drift for Mergify temp PR metadata edit: verifier={ver_tuple} provenance={prov_tuple}"
        )
    # A metadata edit (no base change) is not a full-CI action, so even the bound
    # actor's temp PR falls through to the ordinary draft path -> iteration.
    if ver_tuple != (
        "iteration",
        False,
        False,
        "gate-iteration",
        "backtester-gate-iteration",
        "iteration",
        "draft_pr_edited",
    ):
        raise AssertionError(f"Mergify temp PR metadata edits must defer to iteration: {ver_tuple}")
    for string_base_changed, expected in [
        (
            "false",
            (
                "iteration",
                False,
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
                False,
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
            workflow_dispatch_full_ci="",
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
            workflow_dispatch_full_ci="",
            event_sender_id=actor_id,
            ref="refs/pull/965/merge",
        )
        ver_tuple = (
            ver.ci_policy_path,
            ver.full_ci_required,
            ver.full_ci_deferred,
            ver.gate_name,
            ver.backtester_gate_name,
            ver.expected_event_class,
            ver.reason,
        )
        prov_tuple = (
            prov.ci_policy_path,
            prov.full_ci_required,
            prov.full_ci_deferred,
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
            "workflow_dispatch must define configured full CI input",
            replace_once(workflow, "      full_ci:\n", "      not_full_ci:\n"),
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
        '[[ "$BUCKET" == "bolt-v2-ci-cache-675819144420-us-east-2" && "$REGION" == "us-east-2" && "$PREFIX" == "sccache/bolt-v2/arm64/root-nextest/" ]]',
        "PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}",
        'if [[ "$GITHUB_EVENT_NAME" == "pull_request" ]]; then',
        'cache_mode="read_only"',
        'role_arn="$PR_READONLY_ROLE_ARN"',
        "echo \"cache_mode=$cache_mode\" >> \"$GITHUB_OUTPUT\"",
        "role-to-assume: ${{ steps.sccache-eligible.outputs.role_arn }}",
    ]
    for fragment in required_pr_read_fragments:
        if fragment not in workflow:
            raise AssertionError(f"real ci.yml must configure PR read-only sccache role path: missing {fragment!r}")
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
            "Resolve sccache eligibility' must be continue-on-error",
            replace_once(
                workflow,
                "        id: sccache-eligible\n"
                "        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'\n"
                "        continue-on-error: true\n",
                "        id: sccache-eligible\n"
                "        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'\n",
            ),
        ),
        (
            "Configure AWS credentials for sccache' must be continue-on-error",
            replace_once(
                workflow,
                "        id: sccache-aws\n"
                "        if: steps.sccache-eligible.outputs.eligible == 'true'\n"
                "        continue-on-error: true\n",
                "        id: sccache-aws\n"
                "        if: steps.sccache-eligible.outputs.eligible == 'true'\n",
            ),
        ),
        (
            "Resolve sccache enablement' must be continue-on-error",
            replace_once(
                workflow,
                "        id: sccache\n"
                "        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'\n"
                "        continue-on-error: true\n",
                "        id: sccache\n"
                "        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'\n",
            ),
        ),
        (
            "must set SCCACHE_IGNORE_SERVER_IO_ERROR",
            replace_once(workflow, '      SCCACHE_IGNORE_SERVER_IO_ERROR: "1"\n', ""),
        ),
        (
            # Value spoof: flipping the flag to "0" disables in-flight fail-open.
            "must set SCCACHE_IGNORE_SERVER_IO_ERROR",
            replace_once(
                workflow,
                '      SCCACHE_IGNORE_SERVER_IO_ERROR: "1"\n',
                '      SCCACHE_IGNORE_SERVER_IO_ERROR: "0"\n',
            ),
        ),
        (
            # Dropping the without-sccache retry removes the only cover for a
            # mid-build sccache server crash.
            "must retry the build without sccache",
            replace_once(
                workflow,
                'BOLT_RUST_VERIFICATION_SCCACHE=0 just test-archive "$NEXTEST_ARCHIVE_PATH"',
                "true",
            ),
        ),
        (
            "must require CI_SCCACHE_S3_KEY_PREFIX",
            replace_once(
                workflow,
                ' && -n "$PREFIX"',
                "",
            ),
        ),
        (
            "must gate write-cache use exactly to main push/dispatch refs",
            replace_once(
                workflow,
                '          if [[ "$GITHUB_EVENT_NAME" == "workflow_dispatch" && "$GITHUB_REF" == "refs/heads/main" ]]; then trusted=true; fi\n',
                '          if [[ "$GITHUB_EVENT_NAME" == "workflow_dispatch" ]]; then trusted=true; fi\n',
            ),
        ),
        (
            # Adding a fourth trusted arm must fail even if the required arms
            # remain present.
            "must gate write-cache use exactly to main push/dispatch refs",
            replace_once(
                workflow,
                "          trusted=false\n",
                '          trusted=false\n'
                '          if [[ "$GITHUB_EVENT_NAME" == "pull_request_target" ]];'
                ' then trusted=true; fi\n',
            ),
        ),
        (
            "must pin bucket/region/prefix to the bolt-v2 CI cache",
            replace_once(
                workflow,
                '"bolt-v2-ci-cache-675819144420-us-east-2"',
                '"some-other-cache"',
            ),
        ),
        (
            "must configure PR read-only sccache role path",
            replace_once(
                workflow,
                "          PR_READONLY_ROLE_ARN: ${{ vars.AWS_CI_CACHE_PR_READONLY_ROLE_ARN }}\n",
                "",
            ),
        ),
        (
            "must configure PR read-only sccache role path",
            replace_once(
                workflow,
                '          if [[ "$GITHUB_EVENT_NAME" == "pull_request" ]]; then\n'
                '            cache_mode="read_only"\n'
                '            role_arn="$PR_READONLY_ROLE_ARN"\n'
                "          fi\n",
                "",
            ),
        ),
        (
            "must assume the resolved sccache role",
            replace_once(
                workflow,
                "          role-to-assume: ${{ steps.sccache-eligible.outputs.role_arn }}\n",
                "          role-to-assume: ${{ vars.AWS_CI_CACHE_ROLE_ARN }}\n",
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
            "NEXTEST_ARCHIVE_PATH": "out/nextest-archive.tar.zst",
            "BOLT_RUST_VERIFICATION_SCCACHE": sccache,
        }
        result = subprocess.run(
            ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", script],
            cwd=root,
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        count = int(counter.read_text(encoding="utf-8")) if counter.exists() else 0
        return result.returncode, count


def assert_test_archive_sccache_retry_preserves_compile_failures() -> None:
    verifier = load_verifier()
    script = _test_archive_build_script(verifier)
    rc, count = _run_test_archive_build_script(
        script,
        sccache="1",
        fake_just_mode="transient-cache-failure",
    )
    if (rc, count) != (0, 2):
        raise AssertionError(f"sccache transient failure must retry once and pass, got rc={rc} count={count}")

    rc, count = _run_test_archive_build_script(
        script,
        sccache="1",
        fake_just_mode="compile-error",
    )
    if (rc, count) != (42, 2):
        raise AssertionError(f"compile failure with sccache must fail after retry, got rc={rc} count={count}")

    rc, count = _run_test_archive_build_script(
        script,
        sccache="0",
        fake_just_mode="no-cache-failure",
    )
    if (rc, count) != (43, 1):
        raise AssertionError(f"without sccache, build failure must not retry, got rc={rc} count={count}")


def assert_ci_workflow_run_name_matches_dispatch_config() -> None:
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    assert_error(
        "workflow run-name must publish configured dispatch full marker",
        replace_once(workflow, "&& 'CI [dispatch:full]'", "&& 'CI [manual:full]'"),
    )
    assert_error(
        "workflow run-name must publish configured dispatch full marker",
        replace_once(workflow, "&& 'CI [dispatch:full]'", "&& 'CI [manual:full]'")
        + "\n# && 'CI [dispatch:full]'\n",
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
    if not any("detector must force build_required=true for workflow_dispatch full CI" in error for error in errors):
        raise AssertionError(f"expected workflow_dispatch detector guard error, got: {errors}")


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


def assert_merge_group_support_gaps_are_reported() -> None:
    # Non-vacuous mutation tests for the merge queue (merge_group) lane:
    # the real workflows/config must be clean, and each mutation must surface
    # its own specific error. A skipped required check counts as passing in
    # GitHub, so every gap here would silently let an unvalidated commit merge.
    verifier = load_verifier()
    ci_workflow = repo_workflow_text(".github/workflows/ci.yml")
    actionlint_workflow = repo_workflow_text(".github/workflows/actionlint.yml")
    backtester_workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")

    # Baseline: real workflows declare merge_group and resolve clean.
    if verifier.verify_workflow(ci_workflow):
        raise AssertionError(
            f"real ci.yml must be merge_group-clean, got: {verifier.verify_workflow(ci_workflow)}"
        )
    actionlint_baseline = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_workflow}
    )
    if any("merge_group" in error for error in actionlint_baseline):
        raise AssertionError(
            f"real actionlint.yml must be merge_group-clean, got: {actionlint_baseline}"
        )
    backtester_baseline = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_workflow}
    )
    if any("merge_group" in error for error in backtester_baseline):
        raise AssertionError(
            f"real backtester-ci.yml must be merge_group-clean, got: {backtester_baseline}"
        )

    # (i) merge_group policy value flipped away from required proof → config contract error.
    flipped_config = ci_provenance_config_fixture().replace(
        'merge_group = "full"', 'merge_group = "defer"'
    )
    if flipped_config == ci_provenance_config_fixture():
        raise AssertionError("merge_group policy fixture fragment not found")
    error = runner_config_load_error(flipped_config)
    if "ci_provenance.policy.merge_group is proof-affecting" not in error:
        raise AssertionError(f"expected merge_group policy contract error, got: {error!r}")

    # (ii-a) merge_group trigger removed from ci.yml → CI workflow error.
    ci_without_merge_group = replace_once(
        ci_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    ci_errors = verifier.verify_workflow(ci_without_merge_group)
    if not any("on must define merge_group for merge queue full CI" in error for error in ci_errors):
        raise AssertionError(f"expected ci.yml merge_group trigger error, got: {ci_errors}")

    # (ii-b) merge_group trigger removed from actionlint.yml → actionlint error.
    actionlint_without_merge_group = replace_once(
        actionlint_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    actionlint_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_without_merge_group}
    )
    if not any(
        "on must define merge_group for merge queue" in error for error in actionlint_errors
    ):
        raise AssertionError(
            f"expected actionlint.yml merge_group trigger error, got: {actionlint_errors}"
        )

    # (ii-c) merge_group trigger removed from backtester-ci.yml → Backtester CI error.
    backtester_without_merge_group = replace_once(
        backtester_workflow,
        "  merge_group:\n    types: [checks_requested]\n",
        "",
    )
    backtester_trigger_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_merge_group}
    )
    if not any(
        "on must define merge_group for merge queue" in error for error in backtester_trigger_errors
    ):
        raise AssertionError(
            f"expected backtester-ci.yml merge_group trigger error, got: {backtester_trigger_errors}"
        )

    # Backtester detect must force proof lanes on merge_group. A no-op required
    # gate counts as passing and would poison the live queue evidence.
    backtester_without_detector_arm = replace_once(
        backtester_workflow,
        '          elif [[ "${{ github.event_name }}" == "merge_group" ]]; then\n'
        "            # A skipped required gate counts as passing, so queue validation must run proof lanes.\n"
        '            echo "merge_group event; treating crate as changed"\n'
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        '            exit 0\n',
        "",
    )
    backtester_detector_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_detector_arm}
    )
    if not any(
        "backtester detect must force bvs_changed=true for merge_group" in error
        for error in backtester_detector_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group detector error, got: {backtester_detector_errors}"
        )

    backtester_detector_without_exit = replace_once(
        backtester_workflow,
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        "            exit 0\n"
        "          fi\n",
        '            echo "bvs_changed=true" >> "$GITHUB_OUTPUT"\n'
        "          fi\n",
    )
    backtester_detector_exit_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_detector_without_exit}
    )
    if not any(
        "backtester detect must force bvs_changed=true for merge_group" in error
        for error in backtester_detector_exit_errors
    ):
        raise AssertionError(
            "expected backtester merge_group detector short-circuit error, "
            f"got: {backtester_detector_exit_errors}"
        )

    # Detector must force build on merge_group (a skipped required build is a hole).
    ci_without_detector_arm = replace_once(
        ci_workflow,
        '          elif [[ "${{ github.event_name }}" == "merge_group" ]]; then\n',
        "",
    )
    detector_errors = verifier.verify_workflow(ci_without_detector_arm)
    if not any(
        "detector must force build_required=true for merge_group full CI" in error
        for error in detector_errors
    ):
        raise AssertionError(f"expected merge_group detector guard error, got: {detector_errors}")

    # Concurrency group must match an approved merge_group-safe form and must
    # not cancel merge_group runs.
    ci_without_concurrency_arm = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n        && format('mq-{0}', github.ref)\n",
        "",
    )
    concurrency_errors = verifier.verify_workflow(ci_without_concurrency_arm)
    if not any(
        "approved merge_group-safe form" in error
        for error in concurrency_errors
    ):
        raise AssertionError(f"expected merge_group concurrency error, got: {concurrency_errors}")

    ci_cancelling_merge_group = replace_once(
        ci_workflow,
        "        || github.event_name == 'workflow_dispatch' }}",
        "        || github.event_name == 'workflow_dispatch'\n        || github.event_name == 'merge_group' }}",
    )
    cancel_errors = verifier.verify_workflow(ci_cancelling_merge_group)
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_errors
    ):
        raise AssertionError(f"expected merge_group cancel-scope error, got: {cancel_errors}")

    backtester_without_concurrency_arm = replace_once(
        backtester_workflow,
        "        || github.event_name == 'merge_group'\n        && format('bvs-mq-{0}', github.ref)\n",
        "",
    )
    backtester_concurrency_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_without_concurrency_arm}
    )
    if not any(
        "approved merge_group-safe form" in error
        for error in backtester_concurrency_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group concurrency error, got: {backtester_concurrency_errors}"
        )

    backtester_cancelling_merge_group = replace_once(
        backtester_workflow,
        "        || github.event_name == 'workflow_dispatch' }}",
        "        || github.event_name == 'workflow_dispatch'\n        || github.event_name == 'merge_group' }}",
    )
    backtester_cancel_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": backtester_cancelling_merge_group}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in backtester_cancel_errors
    ):
        raise AssertionError(
            f"expected backtester merge_group cancel-scope error, got: {backtester_cancel_errors}"
        )

    # Decoupled merge_group arm (ci.yml): a merge_group arm must be caught even
    # when 'mq-{0}'/'github.ref' still appear elsewhere. Swap the merge_group and
    # workflow_dispatch format strings so both substrings remain present but the
    # merge_group arm no longer keys on format('mq-{0}', github.ref). The allowlist
    # rejects it because the resulting group is not an approved form. (Regression
    # coverage: the prior expression-analysis verifier rejected this too — NOT a
    # gap the allowlist uniquely closes.)
    ci_fail_open = replace_once(
        ci_workflow,
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && github.event.inputs.full_ci == 'true'\n"
        "        && format('{0}-dispatch-full', github.ref_name)\n"
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && format('{0}-dispatch-iteration', github.ref_name)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n",
        "        || github.event_name == 'workflow_dispatch'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('{0}-dispatch-full', github.ref_name)\n",
    )
    if ci_fail_open == ci_workflow:
        raise AssertionError("merge_group fail-open fixture fragment not found in ci.yml")
    fail_open_errors = verifier.verify_workflow(ci_fail_open)
    if not any(
        "approved merge_group-safe form" in error
        for error in fail_open_errors
    ):
        raise AssertionError(
            f"merge_group concurrency allowlist must reject a decoupled arm, got: {fail_open_errors}"
        )

    # actionlint concurrency must also isolate merge_group (the reviewer-flagged
    # class gap: only ci.yml's concurrency was contract-checked). Removing
    # actionlint's merge_group concurrency arm must be reported.
    actionlint_no_concurrency_arm = replace_once(
        actionlint_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n",
        "",
    )
    if actionlint_no_concurrency_arm == actionlint_workflow:
        raise AssertionError("actionlint merge_group concurrency fixture fragment not found")
    actionlint_concurrency_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_no_concurrency_arm}
    )
    if not any(
        "approved merge_group-safe form" in error
        for error in actionlint_concurrency_errors
    ):
        raise AssertionError(
            f"expected actionlint merge_group concurrency error, got: {actionlint_concurrency_errors}"
        )

    # actionlint cancel-in-progress must never cancel merge_group queue runs.
    actionlint_cancel_merge_group = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "      || github.event_name == 'merge_group' }}",
    )
    if actionlint_cancel_merge_group == actionlint_workflow:
        raise AssertionError("actionlint cancel-in-progress fixture fragment not found")
    actionlint_cancel_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_merge_group}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in actionlint_cancel_errors
    ):
        raise AssertionError(
            f"expected actionlint merge_group cancel-scope error, got: {actionlint_cancel_errors}"
        )

    # cancel-in-progress: true cancels merge_group queue runs while naming no
    # event literally — the old bare-substring check missed it. (Reviewer-flagged
    # fail-open class: GPT/GLM.) The positive allowlist must reject it.
    actionlint_cancel_true = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: true",
    )
    if actionlint_cancel_true == actionlint_workflow:
        raise AssertionError("actionlint cancel-in-progress: true fixture fragment not found")
    cancel_true_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_true}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_true_errors
    ):
        raise AssertionError(
            f"cancel-in-progress: true must be rejected for merge_group, got: {cancel_true_errors}"
        )

    # A negation true for the queue ref (!= 'push') cancels the run while naming
    # no event literally — also fail-open under a substring deny-list.
    actionlint_cancel_negation = replace_once(
        actionlint_workflow,
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: ${{ github.event_name != 'push' }}",
    )
    if actionlint_cancel_negation == actionlint_workflow:
        raise AssertionError("actionlint cancel negation fixture fragment not found")
    cancel_negation_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_cancel_negation}
    )
    if not any(
        "cancel-in-progress must not cancel merge_group queue validations" in error
        for error in cancel_negation_errors
    ):
        raise AssertionError(
            f"cancel negation true for merge_group must be rejected, got: {cancel_negation_errors}"
        )

    # Decoy-after-fallback (ci.yml): the real merge_group arm is decoupled to a
    # shared key, but a dead keyed arm sits after the always-true fallback (which
    # GitHub's `||` never reaches). The allowlist rejects it because the decoupled
    # group expression is not an approved form. (Regression coverage: the prior
    # expression-analysis verifier rejected this too — a single .search() for the
    # keyed arm would have passed, but the count-based check did not — NOT a gap
    # the allowlist uniquely closes.)
    ci_decoy_after_fallback = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('shared-key')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_decoy_after_fallback == ci_workflow:
        raise AssertionError("ci.yml decoy-after-fallback fixture fragment not found")
    decoy_errors = verifier.verify_workflow(ci_decoy_after_fallback)
    if not any(
        "approved merge_group-safe form" in error
        for error in decoy_errors
    ):
        raise AssertionError(
            f"a decoupled merge_group arm hidden behind a keyed decoy must be rejected, got: {decoy_errors}"
        )

    # Index-syntax escape (ci.yml): the real merge_group arm selects the event
    # via github['event_name'] and uses a shared key, with a canonical keyed
    # decoy after the fallback. A counter keyed on the literal `github.event_name
    # == 'merge_group'` token never counts the index arm, so the count stays
    # balanced and it slips through — the allowlist rejects it. (Differential: the
    # prior expression-analysis verifier leaked this; the allowlist uniquely
    # closes it.)
    ci_index_syntax_escape = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github['event_name'] == 'merge_group'\n"
        "        && format('shared-key')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_index_syntax_escape == ci_workflow:
        raise AssertionError("ci.yml index-syntax escape fixture fragment not found")
    index_errors = verifier.verify_workflow(ci_index_syntax_escape)
    if not any(
        "approved merge_group-safe form" in error
        for error in index_errors
    ):
        raise AssertionError(
            f"an unkeyed merge_group arm using github['event_name'] must be rejected, got: {index_errors}"
        )

    # Ref-shape escape (ci.yml): an arm true for the queue ref
    # (startsWith(github.ref, 'refs/heads/gh-readonly-queue')) with a shared key
    # is placed before the canonical arm, so it wins under merge_group. It names
    # no event literally, so a token counter never sees it; the allowlist rejects
    # it. (Differential: the prior expression-analysis verifier leaked this; the
    # allowlist uniquely closes it.)
    ci_ref_shape_escape = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || startsWith(github.ref, 'refs/heads/gh-readonly-queue')\n"
        "        && format('shared-key')\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_ref_shape_escape == ci_workflow:
        raise AssertionError("ci.yml ref-shape escape fixture fragment not found")
    ref_shape_errors = verifier.verify_workflow(ci_ref_shape_escape)
    if not any(
        "approved merge_group-safe form" in error
        for error in ref_shape_errors
    ):
        raise AssertionError(
            f"an unkeyed arm true for the queue ref must be rejected, got: {ref_shape_errors}"
        )

    # Literal-string spoof (ci.yml): the merge_group arm's value is a constant
    # key that merely contains the text 'github.ref', so every queue entry gets
    # the same group. A naive ref-isolation check matching the bare token would be
    # fooled; the allowlist rejects it because the constant group is not an
    # approved form. (Regression coverage: the prior expression-analysis verifier
    # also rejected this form — it required github.ref as a format() placeholder
    # arg — so this is NOT a gap the allowlist uniquely closes; see the
    # load-bearing allowlist guard below for what is proven.)
    ci_literal_spoof = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-github.ref-static')\n"
        "        || format('{0}-{1}', github.ref_name, github.sha)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref) }}",
    )
    if ci_literal_spoof == ci_workflow:
        raise AssertionError("ci.yml literal-spoof fixture fragment not found")
    literal_errors = verifier.verify_workflow(ci_literal_spoof)
    if not any(
        "approved merge_group-safe form" in error
        for error in literal_errors
    ):
        raise AssertionError(
            f"a merge_group arm keyed on a constant string containing 'github.ref' must be "
            f"rejected, got: {literal_errors}"
        )

    # github.ref wrapped in a constant-collapsing function
    # (startsWith/endsWith/contains) yields the same key for every queue ref. The
    # allowlist rejects it because the normalized group is not an approved form.
    # (Regression coverage: the prior expression-analysis verifier also rejected
    # this — the merge_group arm's format() arg was startsWith(...), not the bare
    # github.ref it required — so, like literal_spoof, it is NOT a gap the
    # allowlist uniquely closes. The forms the allowlist DOES uniquely close
    # against expression analysis are index_syntax/ref_shape/amp_literal/
    # gate_literal; the guard below proves the allowlist is the sole gate for all
    # of them without depending on which historical check caught which.)
    ci_collapse = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', startsWith(github.ref, 'refs/heads/gh-readonly-queue'))\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_collapse == ci_workflow:
        raise AssertionError("ci.yml collapse fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_collapse)
    ):
        raise AssertionError("a github.ref wrapped in startsWith() must be rejected")

    # `&&` inside a string literal mis-splits a naive value/condition parse; the
    # whole literal is one constant key to GitHub. (Differential: the prior
    # expression-analysis verifier leaked this; the allowlist uniquely closes it.)
    ci_amp_literal = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || format('mq-static && github.ref ', 'x')\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_amp_literal == ci_workflow:
        raise AssertionError("ci.yml amp-in-literal fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_amp_literal)
    ):
        raise AssertionError("an && hidden inside a string literal must be rejected")

    # Event-gate text inside a string literal is not a real conjunct; the arm
    # still wins under merge_group with a shared static key. (Differential: the
    # prior expression-analysis verifier leaked this; the allowlist uniquely
    # closes it.)
    ci_gate_literal = replace_once(
        ci_workflow,
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
        "        || format(\"skip github.event_name == 'pull_request'\", github.ref) && 'mq-shared-static-group'\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if ci_gate_literal == ci_workflow:
        raise AssertionError("ci.yml gate-in-literal fixture fragment not found")
    if not any(
        "approved merge_group-safe form" in error
        for error in verifier.verify_workflow(ci_gate_literal)
    ):
        raise AssertionError("a gate hidden inside a string literal must be rejected")

    # --- Load-bearing proof for the group allowlist (differential) ---
    # Every merge_group group-expression mutation above is a NON-APPROVED group
    # form, and the positive allowlist rejects each one. Most resolve to a shared
    # or constant group that is genuinely unsafe under merge_group; fail_open_swap
    # is the exception — its merge_group arm keys on github.ref_name, which is
    # unique per queue entry (gh-readonly-queue/<base>/pr-N-<sha>), so it would in
    # fact isolate, yet it is still rejected because it is not the exact approved
    # form. That is the allowlist's whole point: it is fail-closed on any
    # non-approved form and never tries to decide whether a novel form happens to
    # be safe. Stub the allowlist branch back out (pre-rework behavior: cancel
    # check only) and every one must stop being rejected — proving the allowlist
    # is the sole load-bearing gate, not a vacuous assertion. (Some of these forms
    # were ALSO caught by the prior expression-analysis verifier and are kept as
    # regression coverage; the allowlist's value is that it rejects all of them
    # without depending on which historical check caught which.) load_verifier()
    # returns a fresh module, but restore anyway so the patch cannot leak.
    allowlist_gated_group_mutations = [
        ("fail_open_swap", ci_fail_open),
        ("decoy_after_fallback", ci_decoy_after_fallback),
        ("index_syntax_escape", ci_index_syntax_escape),
        ("ref_shape_escape", ci_ref_shape_escape),
        ("literal_spoof", ci_literal_spoof),
        ("collapse", ci_collapse),
        ("amp_literal", ci_amp_literal),
        ("gate_literal", ci_gate_literal),
    ]
    original_group_check = verifier.merge_group_concurrency_errors
    try:
        verifier.merge_group_concurrency_errors = (
            lambda group_text, cancel_text: (
                []
                if verifier.cancel_in_progress_is_merge_group_safe(cancel_text)
                else ["cancel-in-progress must not cancel merge_group queue validations"]
            )
        )
        for label, mutated in allowlist_gated_group_mutations:
            if any(
                "approved merge_group-safe form" in error
                for error in verifier.verify_workflow(mutated)
            ):
                raise AssertionError(
                    f"differential: {label} must no longer draw the allowlist error "
                    "once the group allowlist is stubbed out (else the allowlist "
                    "guard proves nothing)"
                )
    finally:
        verifier.merge_group_concurrency_errors = original_group_check

    # Duplicate top-level group: key — GitHub takes the last (a constant). The
    # extractor joins both group: lines, so the normalized text is not approved.
    dup_block = (
        "concurrency:\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request' && format('pr-{0}', github.event.number) "
        "|| github.event_name == 'merge_group' && format('mq-{0}', github.ref) "
        "|| format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  group: ci-shared-merge-queue\n"
        "  cancel-in-progress: false\n"
    )
    dup_split = verifier.concurrency_group_and_cancel(dup_block)
    if dup_split is None:
        raise AssertionError("duplicate group: block did not parse")
    dup_errors = verifier.merge_group_concurrency_errors(*dup_split)
    if not any("approved merge_group-safe form" in error for error in dup_errors):
        raise AssertionError(f"a duplicate group: key must be rejected, got: {dup_errors}")

    # Reversed key order (actionlint.yml): cancel-in-progress written before
    # group. The split must bucket by key, not by first cancel occurrence;
    # otherwise the whole group expression is misread as cancel text and a valid
    # block draws a spurious "must key merge_group" error.
    actionlint_reversed = replace_once(
        actionlint_workflow,
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request'\n"
        "        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)\n"
        "        || github.event_name == 'pull_request'\n"
        "        && format('pr-{0}', github.event.number)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  # cancel-in-progress is true only for ordinary PR runs; merge_group and Mergify\n"
        "  # proof PR validations must never be cancelled.\n"
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "  cancel-in-progress: >-\n"
        "    ${{ github.event_name == 'pull_request'\n"
        "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request'\n"
        "        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
        "            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
        "        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)\n"
        "        || github.event_name == 'pull_request'\n"
        "        && format('pr-{0}', github.event.number)\n"
        "        || github.event_name == 'merge_group'\n"
        "        && format('mq-{0}', github.ref)\n"
        "        || format('{0}-{1}', github.ref_name, github.sha) }}",
    )
    if actionlint_reversed == actionlint_workflow:
        raise AssertionError("actionlint reversed key-order fixture fragment not found")
    reversed_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_reversed}
    )
    if any(
        "merge_group" in error and "actionlint.yml" in error
        for error in reversed_errors
    ):
        raise AssertionError(
            f"a valid block with cancel-in-progress before group must not draw a spurious "
            f"merge_group concurrency error, got: {reversed_errors}"
        )

    # --- Job-level concurrency fail-open (round-3 adversarial pass) ---
    # GitHub evaluates job-level `concurrency:` in addition to the workflow-level
    # block, so a shared/cancelling job-level group on a required merge_group job
    # collapses queue entries even when the workflow-level group is allowlist-safe.
    # actionlint does NOT catch this (verified: exit 0), so the verifier must own
    # it. (Duplicate top-level `concurrency:` keys are deliberately NOT re-detected
    # here: actionlint — a required merge_group check this verifier already
    # enforces — rejects them in every form, block/flow/quoted, verified exit 1;
    # see merge_group_concurrency_workflow_errors for the single-source rationale
    # and the liveness-only residual.)

    # (a) Job-level concurrency on real actionlint.yml — a shared/cancelling
    #     job-level group collapses queue entries even with a safe workflow block.
    #     Exercises the verify_merge_group_concurrency entry point.
    actionlint_job_level = replace_once(
        actionlint_workflow,
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n    steps:",
        "    runs-on: ${{ vars.CI_RUNNER_GITHUB_HOSTED }}\n"
        "    concurrency:\n      group: actionlint-shared\n      cancel-in-progress: true\n"
        "    steps:",
    )
    if actionlint_job_level == actionlint_workflow:
        raise AssertionError("actionlint job-level concurrency fixture fragment not found")
    job_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/actionlint.yml": actionlint_job_level}
    )
    if not any("must not define job-level concurrency" in error for error in job_errors):
        raise AssertionError(
            f"job-level concurrency in a merge_group workflow must be rejected, got: {job_errors}"
        )

    # (b) Job-level concurrency on real ci.yml — exercises the verify_pr_concurrency
    #     entry point, confirming both merge_group concurrency entry points are
    #     wired to the job-level check.
    ci_job_level = replace_once(
        ci_workflow,
        "  build:\n    name: build\n    needs: [ci-policy, detector]\n    if:",
        "  build:\n    name: build\n    needs: [ci-policy, detector]\n"
        "    concurrency:\n      group: ci-build-shared\n      cancel-in-progress: true\n"
        "    if:",
    )
    if ci_job_level == ci_workflow:
        raise AssertionError("ci.yml job-level concurrency fixture fragment not found")
    if not any(
        "must not define job-level concurrency" in error
        for error in verifier.verify_workflow(ci_job_level)
    ):
        raise AssertionError("job-level concurrency in ci.yml must be rejected")

    # (c) False-positive guard: `concurrency:` appearing as run-block text (deeper
    #     than the job-key indentation) must NOT be flagged — only a real
    #     job-level key counts. Proves the indentation discrimination is
    #     load-bearing (a naive substring scan would wrongly reject this).
    job_run_block_text = (
        "name: actionlint\non:\n  merge_group:\n  pull_request:\n"
        "concurrency:\n"
        "  group: >-\n"
        "    actionlint-${{ github.event_name == 'pull_request' && format('pr-{0}', github.event.number)\n"
        "    || github.event_name == 'merge_group' && format('mq-{0}', github.ref)\n"
        "    || format('{0}-{1}', github.ref_name, github.sha) }}\n"
        "  cancel-in-progress: false\n"
        "jobs:\n  lint:\n    runs-on: ubuntu-latest\n"
        "    steps:\n      - run: |\n          echo 'concurrency: not-a-real-key'\n"
    )
    if verifier.jobs_with_job_level_concurrency(job_run_block_text):
        raise AssertionError(
            "run-block text 'concurrency:' must not be misread as a job-level concurrency key"
        )

    # (d) Differential proof: stub the whole-workflow check back out (the pre-fix
    #     behavior) and the bypass passes — proving the job-level check is
    #     load-bearing, not vacuous. load_verifier() returns a fresh module, but
    #     restore anyway so the patch cannot leak.
    original_whole_workflow = verifier.merge_group_concurrency_workflow_errors
    try:
        verifier.merge_group_concurrency_workflow_errors = lambda _text: []
        job_stubbed = verifier.verify_repo_automation_texts(
            {".github/workflows/actionlint.yml": actionlint_job_level}
        )
        if any("must not define job-level concurrency" in error for error in job_stubbed):
            raise AssertionError(
                "differential sanity: the job-level error must vanish once the "
                "job-level check is stubbed out (else the test proves nothing)"
            )
    finally:
        verifier.merge_group_concurrency_workflow_errors = original_whole_workflow


def assert_mergify_config_gaps_are_reported() -> None:
    verifier = load_verifier()
    mergify_config = (REPO_ROOT / ".mergify.yml").read_text()
    baseline_errors = verifier.verify_mergify_config(mergify_config)
    if baseline_errors:
        raise AssertionError(f"real .mergify.yml must be clean, got: {baseline_errors}")

    result, output = run_verifier_main_with_no_mistakes(
        "commands:\n  test: just source-fence-static\n",
        write_mergify_config=False,
    )
    if result == 0 or ".mergify.yml is required for Mergify queue governance" not in output:
        raise AssertionError(f"verifier main must reject a missing .mergify.yml, got: {result}, {output!r}")

    mutations = [
        (
            "missing max_parallel_checks",
            replace_once(mergify_config, "  max_parallel_checks: 1\n", ""),
            "merge_queue.max_parallel_checks must be 1",
        ),
        (
            "reset disabled",
            replace_once(
                mergify_config,
                "  reset_on_external_merge: always\n",
                "  reset_on_external_merge: never\n",
            ),
            "merge_queue.reset_on_external_merge must be always",
        ),
        (
            "autoqueue enabled",
            replace_once(
                mergify_config,
                "    batch_size: 1\n",
                "    autoqueue: true\n    batch_size: 1\n",
            ),
            "manual queueing only",
        ),
        (
            "pull request rules enabled",
            mergify_config + "\npull_request_rules:\n  - name: autoqueue\n",
            "manual queueing only",
        ),
        (
            "merge protections enabled",
            mergify_config + "\nmerge_protections:\n  - name: autoqueue\n",
            "manual queueing only",
        ),
        (
            "queue conditions require gate",
            replace_once(
                mergify_config,
                "    queue_conditions: []\n",
                "    queue_conditions:\n      - check-success = gate\n",
            ),
            "default queue_conditions must be empty",
        ),
        (
            "missing gate merge condition",
            replace_once(mergify_config, "      - check-success = gate\n", ""),
            "default merge_conditions must require sp-reviewer and all four gates",
        ),
        (
            "extra merge condition",
            replace_once(
                mergify_config,
                "      - check-success = host-health\n",
                "      - check-success = host-health\n      - label = queue-proof\n",
            ),
            "default merge_conditions must require sp-reviewer and all four gates",
        ),
        (
            "queue-time injection",
            replace_once(
                mergify_config,
                "    branch_protection_injection_mode: merge\n",
                "    branch_protection_injection_mode: queue\n",
            ),
            "default branch_protection_injection_mode must be merge",
        ),
        (
            "batch size widened",
            replace_once(mergify_config, "    batch_size: 1\n", "    batch_size: 2\n"),
            "default batch_size must be 1",
        ),
        (
            "unbounded timeout",
            replace_once(
                mergify_config,
                "    checks_timeout: 60 minutes\n",
                "    checks_timeout: auto\n",
            ),
            "default checks_timeout must be 60 minutes",
        ),
        (
            "zero timeout",
            replace_once(
                mergify_config,
                "    checks_timeout: 60 minutes\n",
                "    checks_timeout: 0 minutes\n",
            ),
            "default checks_timeout must be 60 minutes",
        ),
        (
            "draft impersonation",
            replace_once(
                mergify_config,
                "    draft_bot_account: null\n",
                '    draft_bot_account: "{{ author }}"\n',
            ),
            "default draft_bot_account must be null",
        ),
        (
            "non-squash merge",
            replace_once(mergify_config, "    merge_method: squash\n", "    merge_method: merge\n"),
            "default merge_method must be squash",
        ),
    ]
    for label, mutated, expected in mutations:
        errors = verifier.verify_mergify_config(mutated)
        if not any(expected in error for error in errors):
            raise AssertionError(
                f"expected .mergify.yml {label} error containing {expected!r}, got: {errors}"
            )


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
            "source-fence must gate on full_ci_required",
            replace_once(
                workflow,
                "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]\n    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' }}",
                "  source-fence:\n    name: source-fence\n    needs: [ci-policy, detector]\n    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}",
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
                "  test:\n    name: test\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
                "  test:\n    name: test\n    needs: [nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
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
                "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}",
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


def assert_gate_policy_truth_table_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cases = [
        (
            "gate needs ci-policy",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "ci-policy")),
        ),
        (
            "gate needs nextest-fingerprint",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "nextest-fingerprint")),
        ),
        (
            "gate needs test-archive",
            replace_once(workflow, GATE_NEEDS, without_inline_need(GATE_NEEDS, "test-archive")),
        ),
        (
            "gate name must come from ci-policy gate_name output",
            replace_once(workflow, GATE_NAME, "name: gate"),
        ),
        (
            "gate shared verdict call must include --job ci-policy=${{ needs.ci-policy.result }}",
            replace_once(
                workflow,
                "--job ci-policy=${{ needs.ci-policy.result }}",
                "--job ci-policy=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --policy-path",
            replace_once(workflow, '--policy-path "${{ needs.ci-policy.outputs.ci_policy_path }}"', '--policy-path "full"'),
        ),
        (
            "gate shared verdict call must include --expected-event-class",
            replace_once(
                workflow,
                '--expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}"',
                '--expected-event-class "iteration"',
            ),
        ),
        (
            "gate shared verdict call must include --full-ci-deferred",
            replace_once(
                workflow,
                '--full-ci-deferred "${{ needs.ci-policy.outputs.full_ci_deferred }}"',
                '--full-ci-deferred "false"',
            ),
        ),
        (
            "gate shared verdict call must include carry_forward_args=()",
            replace_once(
                workflow,
                "carry_forward_args=()",
                "carry_forward_args=(--carry-forward-verified false)",
            ),
        ),
        (
            "gate shared verdict call must include --job nextest-fingerprint=${{ needs.nextest-fingerprint.result }}",
            replace_once(
                workflow,
                "--job nextest-fingerprint=${{ needs.nextest-fingerprint.result }}",
                "--job nextest-fingerprint=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --job test-archive=${{ needs.test-archive.result }}",
            replace_once(
                workflow,
                "--job test-archive=${{ needs.test-archive.result }}",
                "--job test-archive=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
            replace_once(
                workflow,
                "--job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
                "--job same-sha-main-evidence=${{ needs.omitted.result }}",
            ),
        ),
        (
            "gate shared verdict call must include --ignore-emit-failure",
            replace_once(
                workflow,
                '--ignore-emit-failure "${{ needs.ci-policy.outputs.ignore_emit_failure }}"',
                '--ignore-emit-failure "false"',
            ),
        ),
    ]
    for fragment, mutated_workflow in cases:
        errors = verifier.verify_workflow(mutated_workflow)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


def assert_ci_concurrency_split_gaps_are_reported() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/ci.yml")
    cancel_in_progress_for_pr_and_dispatch = """  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))))
        || github.event_name == 'workflow_dispatch' }}
"""
    cancel_in_progress_for_draft_pr_and_dispatch = """  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        || github.event_name == 'workflow_dispatch' }}
"""
    cases = [
        (
            "concurrency group must split deferred PR runs from full CI runs",
            replace_once(workflow, "pr-{0}-deferred", "pr-{0}"),
        ),
        (
            "workflow_dispatch runs must split full and iteration concurrency groups",
            replace_once(
                workflow,
                "        || github.event_name == 'workflow_dispatch'\n"
                "        && github.event.inputs.full_ci == 'true'\n"
                "        && format('{0}-dispatch-full', github.ref_name)\n"
                "        || github.event_name == 'workflow_dispatch'\n"
                "        && format('{0}-dispatch-iteration', github.ref_name)\n",
                "",
            ),
        ),
        (
            "cancel-in-progress must apply to all pull_request and workflow_dispatch full CI runs only",
            replace_once(
                workflow,
                cancel_in_progress_for_pr_and_dispatch,
                "  cancel-in-progress: ${{ github.event_name == 'pull_request' }}\n",
            ),
        ),
        (
            "cancel-in-progress must not cancel noop PR runs",
            replace_once(
                workflow,
                cancel_in_progress_for_pr_and_dispatch,
                cancel_in_progress_for_draft_pr_and_dispatch,
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
    cases = [
        (
            ".github/workflows/ci.yml",
            "format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)",
            "format('pr-{0}-deferred', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
            "",
            "concurrency group must isolate Mergify proof PR runs",
        ),
        (
            ".github/workflows/backtester-ci.yml",
            "format('bvs-pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)",
            "format('bvs-pr-{0}-deferred', github.event.number)",
            "        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
            "             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
            "",
            "concurrency group must isolate Mergify proof PR runs",
        ),
        (
            ".github/workflows/actionlint.yml",
            "format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)",
            "format('pr-{0}-deferred', github.event.number)",
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
        group_errors = (
            verifier.verify_workflow(missing_group)
            if workflow_name.endswith("/ci.yml")
            else verifier.verify_repo_automation_texts({workflow_name: missing_group})
        )
        if not any(expected_error in error for error in group_errors):
            raise AssertionError(f"{workflow_name} must reject missing Mergify proof PR group, got: {group_errors}")

        missing_cancel_guard = replace_once_after(
            workflow,
            "cancel-in-progress:",
            cancel_guard,
            cancel_replacement,
        )
        cancel_errors = (
            verifier.verify_workflow(missing_cancel_guard)
            if workflow_name.endswith("/ci.yml")
            else verifier.verify_repo_automation_texts({workflow_name: missing_cancel_guard})
        )
        if not any("cancel-in-progress must not cancel Mergify proof PR validations" in error for error in cancel_errors):
            raise AssertionError(f"{workflow_name} must reject cancelling Mergify proof PRs, got: {cancel_errors}")


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
                "               && !(github.event.changes.base.ref.from != '')) }}\n",
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
            "permissions must include checks: write",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  checks: write\n", "  checks: read\n")},
        ),
        (
            "permissions must include pull-requests: read",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  pull-requests: read\n", "")},
        ),
        (
            "permissions must not include contents: write",
            {workflow_name: replace_once(BASE_COVERAGE_ENFORCER_WORKFLOW, "  contents: read\n", "  contents: write\n")},
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
                    "            echo \"coverage-enforcer bootstrap: trusted base tree lacks scripts/coverage_enforcer.py\"\n"
                    "            exit 0\n"
                    "          fi\n",
                    "",
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
            "coverage-enforcer job if-condition must run only on merge_group",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "      ${{ github.event_name == 'merge_group'\n"
                    "          || (github.event_name == 'pull_request'\n",
                    "      ${{ github.event_name == 'pull_request'\n",
                )
            },
        ),
        (
            "coverage-enforcer job if-condition must run only on merge_group",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "              && github.event.pull_request.draft == false\n",
                    "",
                )
            },
        ),
        (
            "coverage-enforcer job if-condition must run only on merge_group",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "              && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
                    "                  || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n",
                    "",
                )
            },
        ),
        (
            "coverage-enforcer job if-condition must run only on merge_group",
            {
                workflow_name: replace_once(
                    BASE_COVERAGE_ENFORCER_WORKFLOW,
                    "    if: >-\n"
                    "      ${{ github.event_name == 'merge_group'\n"
                    "          || (github.event_name == 'pull_request'\n"
                    "              && github.event.pull_request.draft == false\n"
                    "              && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')\n"
                    "                  || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))\n"
                    "              && !(github.event.action == 'edited'\n"
                    "                   && !(github.event.changes.base.ref.from != ''))) }}\n",
                    "",
                )
            },
        ),
    ]
    for fragment, workflows in cases:
        errors = verifier.verify_coverage_enforcer_workflow(workflows)
        if not any(fragment in error for error in errors):
            raise AssertionError(f"expected verifier error containing {fragment!r}, got: {errors}")


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


def assert_runner_contract_requires_meter_workflows_for_managed_workflows() -> None:
    verifier = load_verifier()
    original_config = verifier.DEFAULT_RUNNERS_CONFIG
    with tempfile.TemporaryDirectory() as tmp:
        config_path = pathlib.Path(tmp) / "github-actions-runners.toml"
        config_text = original_config.read_text()
        config_path.write_text(
            config_text.replace(
                'included_workflows = ["ci", "backtester_ci", "ci_runner_debug", "rust_probe"]',
                'included_workflows = ["ci", "ci_runner_debug", "rust_probe"]',
            ),
            encoding="utf-8",
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
        config_path = pathlib.Path(tmp) / "github-actions-runners.toml"
        config_text = original_config.read_text()
        config_path.write_text(
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
            encoding="utf-8",
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
        config_path = pathlib.Path(tmp) / "github-actions-runners.toml"
        config_text = original_config.read_text()
        config_path.write_text(
            replace_once(
                config_text,
                'nextest-fingerprint = "managed_heavy"',
                'nextest-fingerprint = "managed_light"',
            ),
            encoding="utf-8",
        )
        verifier.DEFAULT_RUNNERS_CONFIG = config_path
        try:
            errors = verifier.verify_github_actions_runner_contract({workflow_name: recoupled_workflow})
        finally:
            verifier.DEFAULT_RUNNERS_CONFIG = original_config
    if not any("nextest-fingerprint and test-archive must use the same runner tier" in error for error in errors):
        raise AssertionError(f"runner contract must reject nextest fingerprint/archive tier split, got: {errors}")


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


def assert_backtester_detect_includes_runner_config() -> None:
    verifier = load_verifier()
    workflow = repo_workflow_text(".github/workflows/backtester-ci.yml")
    if "            ci/github-actions-runners.toml \\\n" not in workflow:
        workflow = replace_once(
            workflow,
            "            rust-toolchain.toml \\\n",
            "            rust-toolchain.toml \\\n            ci/github-actions-runners.toml \\\n",
        )
    bad = workflow.replace("            ci/github-actions-runners.toml \\\n", "")
    bad_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    if not any("backtester detect paths must include ci/github-actions-runners.toml" in error for error in bad_errors):
        raise AssertionError(f"backtester detector must reject missing runner config path, got: {bad_errors}")
    good_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": workflow})
    if any("backtester detect paths must include ci/github-actions-runners.toml" in error for error in good_errors):
        raise AssertionError(f"backtester detector path check must pass when present, got: {good_errors}")
    missing_policy_script = replace_once_after(
        workflow,
        "scripts/command_understanding.py",
        "scripts/ci_provenance.py",
        "",
    )
    policy_script_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": missing_policy_script})
    if not any("backtester detect paths must include scripts/ci_provenance.py" in error for error in policy_script_errors):
        raise AssertionError(f"backtester detector must reject missing ci_provenance.py path, got: {policy_script_errors}")


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


def assert_backtester_ci_defers_managed_heavy_on_draft_prs() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/backtester-ci.yml"
    workflow = repo_workflow_text(workflow_name)
    errors = verifier.verify_repo_automation_texts({workflow_name: workflow})
    if any("backtester draft deferral" in error for error in errors):
        raise AssertionError(f"backtester-ci workflow must satisfy draft deferral policy, got: {errors}")

    missing_required_gate_note = replace_once(
        workflow,
        verifier.BACKTESTER_REQUIRED_GATE_COMMENT,
        "",
    )
    missing_required_gate_note_errors = verifier.verify_repo_automation_texts({workflow_name: missing_required_gate_note})
    if not any("backtester draft deferral must document that only backtester-gate should be required" in error for error in missing_required_gate_note_errors):
        raise AssertionError(
            f"backtester-ci workflow must document the required-capable gate context, got: {missing_required_gate_note_errors}"
        )

    required_gate_note_decoy = (
        missing_required_gate_note + "\n# " + verifier.BACKTESTER_REQUIRED_GATE_COMMENT + "\n"
    )
    required_gate_note_decoy_errors = verifier.verify_repo_automation_texts({workflow_name: required_gate_note_decoy})
    if not any(
        "backtester draft deferral must document that only backtester-gate should be required" in error
        for error in required_gate_note_decoy_errors
    ):
        raise AssertionError(
            "backtester-ci workflow must reject required-gate documentation decoys outside the header, "
            f"got: {required_gate_note_decoy_errors}"
        )

    missing_policy_gate = replace_once(
        workflow,
        "if: ${{ needs.detect.outputs.bvs_changed == 'true' && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'noop' || needs.ci-policy.outputs.full_ci_deferred == 'true') }}",
        "if: ${{ needs.detect.outputs.bvs_changed == 'true' }}",
    )
    missing_policy_errors = verifier.verify_repo_automation_texts({workflow_name: missing_policy_gate})
    if not any("backtester draft deferral managed-heavy jobs must require full CI policy" in error for error in missing_policy_errors):
        raise AssertionError(f"backtester-ci workflow must reject unmanaged heavy policy gates, got: {missing_policy_errors}")

    static_gate_name = replace_once(
        workflow,
        "name: ${{ needs.ci-policy.outputs.backtester_gate_name }}",
        "name: backtester-gate",
    )
    static_gate_errors = verifier.verify_repo_automation_texts({workflow_name: static_gate_name})
    if not any("backtester draft deferral gate name must come from ci-policy backtester_gate_name output" in error for error in static_gate_errors):
        raise AssertionError(f"backtester-ci workflow must reject static gate names, got: {static_gate_errors}")

    missing_expected_event_class = replace_once(
        workflow,
        '--expected-event-class "${{ needs.ci-policy.outputs.expected_event_class }}"',
        '--expected-event-class "iteration"',
    )
    missing_expected_event_class_errors = verifier.verify_repo_automation_texts({workflow_name: missing_expected_event_class})
    if not any(
        "backtester draft deferral shared gate call must include --expected-event-class" in error
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
    if not any("backtester draft deferral gate must use trusted base-tree check-backtester-gate verdict" in error for error in missing_shared_gate_errors):
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
        "backtester draft deferral gate must recompute instead of carrying forward unavailable provenance" in error
        for error in missing_carry_forward_errors
    ):
        raise AssertionError(
            f"backtester-ci workflow must reject carry-forward resolver, got: {missing_carry_forward_errors}"
        )

    issue_gate_workflow = workflow
    if "needs: [ci-policy, detect, fmt, clippy, test-archive, test, issue_789]" not in issue_gate_workflow:
        issue_gate_workflow = replace_once(
            issue_gate_workflow,
            "needs: [ci-policy, detect, fmt, clippy, test-archive, test]",
            "needs: [ci-policy, detect, fmt, clippy, test-archive, test, issue_789]",
        )
    issue_gate_workflow = replace_once(
        issue_gate_workflow,
        "--job test=${{ needs.test.result }}",
        "--job test=${{ needs.test.result }} \\\n            --job issue_789=${{ needs.issue_789.result }}",
    )
    issue_gate_errors = verifier.verify_repo_automation_texts({workflow_name: issue_gate_workflow})
    if not any("backtester diagnostic issue-789 lane must not gate merge proof" in error for error in issue_gate_errors):
        raise AssertionError(
            f"backtester-ci workflow must reject issue-789 as a merge-gating lane, got: {issue_gate_errors}"
        )

    broken_concurrency = replace_once(
        replace_once(
            workflow,
            "format('bvs-pr-{0}-deferred', github.event.number)",
            "format('bvs-pr-{0}', github.event.number)",
        ),
        "format('bvs-pr-{0}-full', github.event.number)",
        "format('bvs-pr-{0}', github.event.number)",
    )
    broken_concurrency += "\n# format('bvs-pr-{0}-deferred', github.event.number)\n# format('bvs-pr-{0}-full', github.event.number)\n"
    broken_concurrency_errors = verifier.verify_repo_automation_texts({workflow_name: broken_concurrency})
    if not any("backtester draft deferral concurrency must split deferred PR runs" in error for error in broken_concurrency_errors):
        raise AssertionError(f"backtester-ci workflow must reject broken concurrency even with comment decoys, got: {broken_concurrency_errors}")

    missing_concurrency_action_filter = replace_once(
        workflow,
        '        && contains(fromJSON(\'["opened","synchronize","reopened","converted_to_draft","edited"]\'), github.event.action)\n'
        "        && format('bvs-pr-{0}-deferred', github.event.number)",
        "        && format('bvs-pr-{0}-deferred', github.event.number)",
    )
    missing_concurrency_action_filter_errors = verifier.verify_repo_automation_texts({workflow_name: missing_concurrency_action_filter})
    if not any("backtester draft deferral concurrency must use the deferred draft action filter" in error for error in missing_concurrency_action_filter_errors):
        raise AssertionError(
            "backtester-ci workflow must reject concurrency groups that drift from the deferred draft action filter, got: "
            f"{missing_concurrency_action_filter_errors}"
        )

    missing_deferred_trigger_type = replace_once(
        workflow,
        "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]",
        "types: [synchronize, reopened, ready_for_review, converted_to_draft, edited]",
    )
    missing_deferred_trigger_type_errors = verifier.verify_repo_automation_texts({workflow_name: missing_deferred_trigger_type})
    if not any("backtester draft deferral pull_request types must include deferred actions: opened" in error for error in missing_deferred_trigger_type_errors):
        raise AssertionError(
            "backtester-ci workflow must reject deferred draft actions missing from pull_request types, got: "
            f"{missing_deferred_trigger_type_errors}"
        )

    policy_drift_defer_actions = workflow.replace(
        '["opened","synchronize","reopened","converted_to_draft","edited"]',
        '["opened","synchronize","reopened","converted_to_draft","edited","assigned"]',
    ).replace(
        "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited]",
        "types: [opened, synchronize, reopened, ready_for_review, converted_to_draft, edited, assigned]",
    )
    policy_drift_defer_action_errors = verifier.verify_repo_automation_texts({workflow_name: policy_drift_defer_actions})
    if not any("backtester draft deferral action list must match ci_provenance defer policy actions" in error for error in policy_drift_defer_action_errors):
        raise AssertionError(
            "backtester-ci workflow must reject deferred draft actions not backed by ci_provenance policy, got: "
            f"{policy_drift_defer_action_errors}"
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
        bad = replace_once(workflow, f"types: [opened, synchronize, reopened, ready_for_review, edited]", fragment)
        bad_errors = verifier.verify_repo_automation_texts({workflow_name: bad})
        if not any(f"pull_request types must include {missing_type}" in error for error in bad_errors):
            raise AssertionError(
                f"actionlint workflow must require {missing_type} in pull_request types, got: {bad_errors}"
            )


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
    python3 scripts/test_verify_runtime_capture_yaml.py
    python3 scripts/test_local_verification_gate.py
    python3 scripts/test_lane_governor.py
    python3 scripts/test_verify_lane_governance.py
    python3 scripts/verify_lane_governance.py

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
        "    python3 scripts/test_local_verification_gate.py",
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

    missing_lane_check = justfile_text.replace("    python3 scripts/verify_lane_governance.py\n", "")
    missing_errors = verifier.verify_source_fence_static_recipe(missing_lane_check)
    if not any("must run python3 scripts/verify_lane_governance.py" in error for error in missing_errors):
        raise AssertionError(f"source-fence-static must require lane governance meta-check, got: {missing_errors}")

    commented_lane_test = justfile_text.replace(
        "    python3 scripts/test_lane_governor.py",
        "    # python3 scripts/test_lane_governor.py",
    )
    commented_errors = verifier.verify_source_fence_static_recipe(commented_lane_test)
    if not any("must run python3 scripts/test_lane_governor.py" in error for error in commented_errors):
        raise AssertionError(f"source-fence-static comments must not satisfy lane test wiring, got: {commented_errors}")


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
    python3 scripts/test_verify_ci_workflow_hygiene.py
    python3 scripts/test_ci_storage_audit.py
    python3 scripts/test_root_bin_sidecars.py
"""
    errors = verifier.verify_local_verification_gate_recipes(justfile_text)
    if errors:
        raise AssertionError(f"local gate recipe wiring should pass, got: {errors}")

    missing_storage_audit_test = justfile_text.replace("    python3 scripts/test_ci_storage_audit.py\n", "")
    missing_storage_audit_test_errors = verifier.verify_local_verification_gate_recipes(missing_storage_audit_test)
    if not any(
        "justfile ci-lint-workflow-inner must run python3 scripts/test_ci_storage_audit.py" in error
        for error in missing_storage_audit_test_errors
    ):
        raise AssertionError(
            f"ci storage audit test wiring drift was silent, got: {missing_storage_audit_test_errors}"
        )

    missing_sidecar_test = justfile_text.replace("    python3 scripts/test_root_bin_sidecars.py\n", "")
    missing_sidecar_test_errors = verifier.verify_local_verification_gate_recipes(missing_sidecar_test)
    if not any("justfile ci-lint-workflow-inner must run python3 scripts/test_root_bin_sidecars.py" in error for error in missing_sidecar_test_errors):
        raise AssertionError(f"root bin sidecar test wiring drift was silent, got: {missing_sidecar_test_errors}")

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

    nested_public_gate = justfile_text.replace(
        "    python3 scripts/test_verify_ci_workflow_hygiene.py",
        "    just ci-lint-workflow",
    )
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
        "scripts/root_bin_sidecars.py",
        "scripts/test_root_bin_sidecars.py",
    )
    missing = [path for path in required_paths if path not in verifier.FINGERPRINT_REUSE_GOVERNANCE_PATHS]
    if missing:
        raise AssertionError(f"fingerprint-reuse governance pathspec must include root sidecar helper files: {missing}")


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
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        && format('pr-{0}-deferred', github.event.number)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == false
        && (github.event.action == 'reopened'
            || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != '')))
        && format('pr-{0}-noop', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'workflow_dispatch'
        && github.event.inputs.full_ci == 'true'
        && format('{0}-dispatch-full', github.ref_name)
        || github.event_name == 'workflow_dispatch'
        && format('{0}-dispatch-iteration', github.ref_name)
        || github.event_name == 'merge_group'
        && format('mq-{0}', github.ref)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))))
        || github.event_name == 'workflow_dispatch' }}

""",
        "",
    )


def without_inline_need(line: str, job: str) -> str:
    return line.replace(f"{job}, ", "").replace(f", {job}", "")


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
    findings_live = verifier.verify_text(probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
    if not findings_live:
        raise AssertionError("verifier-level differential probe must produce a finding to be meaningful")
    hits_before = cached_strip_fn.cache_info().hits + cached_token_fn.cache_info().hits
    verifier.strip_comment = cached_strip_fn.__wrapped__
    verifier._command_tokens_cached = cached_token_fn.__wrapped__
    findings_bypassed = verifier.verify_text(probe, BASE_ACTION, BASE_NEXTEST_CONFIG)
    hits_after = cached_strip_fn.cache_info().hits + cached_token_fn.cache_info().hits
    if findings_live != findings_bypassed:
        raise AssertionError(
            f"verify_text findings changed when the parse cache was bypassed: "
            f"{findings_live!r} != {findings_bypassed!r}"
        )
    if hits_after != hits_before:
        raise AssertionError(
            "parse-cache bypass did not take effect; the verifier-level differential is vacuous"
        )


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
        for harness in TEST_HARNESS_NAMES
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
        for harness in TEST_HARNESS_NAMES
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


def workflow_with_detector_probe(script: str) -> str:
    return replace_once(
        BASE_WORKFLOW,
        "      # detector probe insertion point",
        "      - name: V6 raw Rust storage policy probe\n        run: |\n"
        + textwrap.indent(script.strip(), "          "),
    )


def assert_v6_deploy_artifact_s3_stays_allowed() -> None:
    verifier = load_verifier()
    workflow = workflow_with_detector_probe(
        """
        mkdir -p dist
        aws s3 cp dist/bolt-v2.tar.zst s3://bolt-v2-deploy-artifacts/bolt-v2.tar.zst
        aws s3 cp "$PWD/dist/bolt-v2.sha256" s3://bolt-v2-deploy-artifacts/bolt-v2.sha256
        """
    )
    s3_errors = [error for error in verifier.verify_text(workflow, BASE_ACTION, BASE_NEXTEST_CONFIG) if "s3" in error.lower()]
    if s3_errors:
        raise AssertionError(f"deploy artifact S3 publication must stay allowed, got: {s3_errors!r}")


def assert_v6_red_s3_storage_transfer_policy_is_semantic() -> None:
    verifier = load_verifier()
    expected = "S3 active mutable target cache must be rejected"
    workflows = {
        "s3 destination hidden behind an env value": """
            DEST=s3://bolt-v2-active-cache/workspace
            aws s3 sync "$GITHUB_WORKSPACE" "$DEST"
        """,
        "workspace source hidden behind an env value": """
            SRC=$PWD
            aws s3 sync "$SRC" s3://bolt-v2-active-cache/workspace
        """,
        "managed target hidden behind command substitution": """
            TARGET=`python3 scripts/rust_verification.py target-dir --repo .`
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "inline backtick target operand": """
            aws s3 sync `echo target` s3://bolt-v2-active-cache/target
        """,
        "aws global option before active target endpoints": """
            aws s3 sync --endpoint-url https://example.com target s3://bolt-v2-active-cache/target
        """,
        "workspace root dot path": """
            aws s3 sync "$PWD/." s3://bolt-v2-active-cache/workspace
        """,
        "github workspace root dot path": """
            aws s3 sync "$GITHUB_WORKSPACE/." s3://bolt-v2-active-cache/workspace
        """,
        "github env unquoted assignment": """
            echo TARGET=target >> $GITHUB_ENV
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "github env quoted value assignment": """
            echo 'TARGET="target"' >> "$GITHUB_ENV"
            aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
        """,
        "s3 destination hidden behind printf github env": """
            printf 'DEST=%s\\n' "s3://bolt-v2-active-cache/target" >> "$GITHUB_ENV"
            aws s3 sync target "$DEST"
        """,
        "active target hidden behind second printf github env assignment": """
            printf 'BENIGN=dist\\nSRC=target\\n' >> "$GITHUB_ENV"
            aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
        """,
        "active target copied through neutral staging path": """
            mkdir /tmp/deploy
            cp -r target/debug /tmp/deploy/
            aws s3 sync /tmp/deploy s3://bolt-v2-active-cache/cache
        """,
        "active target cwd hidden behind env chdir wrapper": """
            env -C target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind sudo chdir wrapper": """
            sudo --chdir target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind cd separator": """
            cd -- target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind cd option and separator": """
            cd -L -- target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target cwd hidden behind combined cd options": """
            cd -LP target && aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "active target streamed through s3 stdin": """
            tar -czf - target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through cat to s3 stdin": """
            cat target/debug/libbolt_v2.rmeta | aws s3 cp - s3://bolt-v2-active-cache/cache
        """,
        "active target streamed through clustered tar stdout flag": """
            tar -czf- target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through traditional tar stdout flag": """
            tar cf - target | aws s3 cp - s3://bolt-v2-active-cache/target.tar
        """,
        "active target streamed through tar long file stdout flag": """
            tar -c --file=- target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
        """,
        "active target streamed through tar default stdout": """
            tar c target | aws s3 cp - s3://bolt-v2-active-cache/target.tar
        """,
        "active target streamed through fused input redirection": """
            cat <target/debug/libbolt_v2.rmeta | aws s3 cp - s3://bolt-v2-active-cache/cache
        """,
        "s3 stdout written to active target through shell group redirection": """
            { aws s3 cp s3://bolt-v2-active-cache/target.tar - ; } > target/cache.tar
        """,
        "s3 stdout written to active target through subshell redirection": """
            ( aws s3 cp s3://bolt-v2-active-cache/target.tar - ) > target/cache.tar
        """,
        "active target moved through local staging before S3 upload": """
            mv target my_cache
            aws s3 cp my_cache s3://bolt-v2-active-cache/target.tar
        """,
        "s3 download moved from local staging into active target": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar my_cache
            mv my_cache target
        """,
        "active target archived locally before S3 upload": """
            tar -cf cache.tar target
            aws s3 cp cache.tar s3://bolt-v2-active-cache/target.tar
        """,
        "active target zipped locally before S3 upload": """
            zip -r cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with option argument before S3 upload": """
            zip -b /tmp cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with clustered option argument before S3 upload": """
            zip -qr0b /tmp cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "active target zipped with exclude option argument before S3 upload": """
            zip -x ignored cache.zip target
            aws s3 cp cache.zip s3://bolt-v2-active-cache/target.zip
        """,
        "s3 archive downloaded locally before active target extraction": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar -xf cache.tar -C target
        """,
        "s3 archive extraction names active target as operand": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar xf cache.tar target
        """,
        "s3 zip downloaded locally before active target extraction": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip cache.zip -d target
        """,
        "s3 zip extraction names active target as operand": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip cache.zip target/*
        """,
        "s3 zip extraction skips option argument before archive": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip -x ignored cache.zip -d target
        """,
        "s3 zip extraction handles clustered directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.zip cache.zip
            unzip -qd target cache.zip
        """,
        "s3 download moved to active target through mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -t target s3_cache
        """,
        "s3 download moved to active target through clustered mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -vt target s3_cache
        """,
        "s3 download moved to active target through concatenated mv target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            mv -ttarget s3_cache
        """,
        "s3 download copied to active target through cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp --target-directory=target s3_cache
        """,
        "s3 download copied to active target through clustered cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp -vt target s3_cache
        """,
        "s3 download copied to active target through concatenated cp target-directory option": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar s3_cache
            cp -ttarget s3_cache
        """,
        "s3 tar extraction handles ordered traditional options": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar xCf target cache.tar
        """,
        "s3 tar extraction handles ordered clustered options": """
            aws s3 cp s3://bolt-v2-active-cache/target.tar cache.tar
            tar -xCf target cache.tar
        """,
        "s3 transfer hidden behind su command string": """
            su -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind su long command string": """
            su --command="aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind su clustered command string": """
            su -mc "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind sg command string": """
            sg docker -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind sg concatenated command string": """
            sg docker -c"aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind runuser long command string": """
            runuser --command="aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind runuser user command string": """
            runuser user -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "s3 transfer hidden behind flock clustered command string": """
            flock /tmp/lock -c"aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
        "env chdir ignores C inside unset option argument": """
            env -uC -C target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "sudo chdir ignores D inside user option argument": """
            sudo -uD -D target aws s3 sync debug s3://bolt-v2-active-cache/target/debug
        """,
        "s3 transfer hidden behind rustup shell command string": """
            rustup run nightly sh -c "aws s3 cp s3://bolt-v2-active-cache/target.tar target"
        """,
    }
    misses: list[str] = []
    for name, script in workflows.items():
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(script))
        if expected not in errors:
            misses.append(f"{name}: {errors!r}")
    if misses:
        raise AssertionError("storage-transfer policy did not classify semantic active-cache flows: " + "; ".join(misses))


def assert_v6_workflow_run_steps_reset_shell_state() -> None:
    verifier = load_verifier()
    s3_expected = "S3 active mutable target cache must be rejected"
    target_expected = "CARGO_TARGET_DIR raw target override must be classified"
    cases = [
        (
            "same run step cwd must classify active-target S3",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            True,
        ),
        (
            "same run step cd without target must clear active-target cwd",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      cd
                      aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "same run step cd separator without target must clear active-target cwd",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                      cd --
                      aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "separate run step cwd must not leak into later S3 step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cd target
                  - run: |
                      aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            False,
        ),
        (
            "flexibly indented same run step cwd must classify active-target S3",
            """
            jobs:
              test:
                steps:
                    - run: |
                        cd target
                        aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            True,
        ),
        (
            "flexibly indented separate run step cwd must not leak into later S3 step",
            """
            jobs:
              test:
                steps:
                    - run: |
                        cd target
                    - run: |
                        aws s3 cp s3://bolt-v2-cache/file artifact.bin
            """,
            s3_expected,
            False,
        ),
        (
            "same run step target env alias must classify raw target override",
            """
            jobs:
              test:
                steps:
                  - run: |
                      E=CARGO_TARGET_DIR
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "separate run step target env alias must not leak into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      E=CARGO_TARGET_DIR
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            False,
        ),
        (
            "github env target must persist into later S3 step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo TARGET=target >> $GITHUB_ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env target must persist after earlier command in same step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      mkdir -p out && echo TARGET=target >> $GITHUB_ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env continued active target must persist into later step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "TARGET=target" \\
                        >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
        (
            "github env target must not persist across jobs",
            """
            jobs:
              producer:
                steps:
                  - run: |
                      echo TARGET=target >> $GITHUB_ENV
              consumer:
                steps:
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            False,
        ),
        (
            "github env target key alias must persist into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo E=CARGO_TARGET_DIR >> $GITHUB_ENV
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env echo flags must persist target key alias into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo -e "E=CARGO_TARGET_DIR" >> $GITHUB_ENV
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env printf must persist target key alias into later cargo step",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'E=%s\\n' CARGO_TARGET_DIR >> "$GITHUB_ENV"
                  - run: |
                      env $E=/tmp/raw cargo check
            """,
            target_expected,
            True,
        ),
        (
            "github env printf multiple assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'SRC=target\\nDEST=s3://bolt-v2-active-cache/cache\\n' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf repeated format assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf '%s\\n' BENIGN=1 SRC=target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf missing argument still persists prior assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=%s\\nEXTRA=%s\\n' target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf missing argument clears stale assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "SRC=target" >> "$GITHUB_ENV"
                  - run: |
                      printf 'SRC=%s\\n' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env printf literal format persists despite extra argument",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=target\\n' benign >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf b conversion assignment must persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'TARGET=%b\\n' target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf b conversion decodes argument newlines",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf '%b' 'SRC=target\\nDEST=s3://bolt-v2-active-cache/cache' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env printf escaped percent must not consume argument",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'A=%%s\\n' SRC=target >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env printf arguments must not decode escaped newlines",
            """
            jobs:
              test:
                steps:
                  - run: |
                      printf 'A=%s\\n' 'B=1\\nSRC=target' >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "github env echo e multiple assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo -e "SRC=target\\nDEST=s3://bolt-v2-active-cache/cache" >> "$GITHUB_ENV"
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc assignments must all persist",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<'ENV'
                      SRC<<EOF
                      target
                      EOF
                      DEST=s3://bolt-v2-active-cache/cache
                      ENV
                  - run: |
                      aws s3 sync "$SRC" "$DEST"
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc must overwrite earlier inline assignment",
            """
            jobs:
              test:
                steps:
                  - run: |
                      echo "SRC=benign" >> "$GITHUB_ENV"
                      cat >> "$GITHUB_ENV" <<ENV
                      SRC=target
                      ENV
                  - run: |
                      aws s3 sync "$SRC" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "github env heredoc delimiter must be exact and preserve continuation",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<ENV
                      BENIGN=1
                       ENV
                      TARGET=targ\\
                      et
                      ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            True,
        ),
        (
            "quoted shell heredoc delimiter must not fold escaped newline payload",
            """
            jobs:
              test:
                steps:
                  - run: |
                      cat >> "$GITHUB_ENV" <<'ENV'
                      TARGET=targ\\
                      et
                      ENV
                  - run: |
                      aws s3 sync "$TARGET" s3://bolt-v2-active-cache/cache
            """,
            s3_expected,
            False,
        ),
        (
            "separate composite action run step cwd must not leak into later S3 step",
            """
            runs:
              using: composite
              steps:
                - shell: bash
                  run: cd target
                - shell: bash
                  run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "quoted composite action using value must still isolate step cwd",
            """
            runs:
              using: "composite"
              steps:
                - shell: bash
                  run: cd target
                - shell: bash
                  run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "indentless composite action run step cwd must not leak into later S3 step",
            """
            runs:
              using: composite
              steps:
              - shell: bash
                run: cd target
              - shell: bash
                run: aws s3 cp dist/app s3://bolt-v2-release/app
            """,
            s3_expected,
            False,
        ),
        (
            "indentless github env target must persist into later S3 step",
            """
            jobs:
              test:
                steps:
                - run: |
                    echo TARGET=target >> $GITHUB_ENV
                - run: |
                    aws s3 sync "$TARGET" s3://bolt-v2-active-cache/target
            """,
            s3_expected,
            True,
        ),
    ]
    failures: list[str] = []
    for name, workflow_text, expected, should_find in cases:
        errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow_text))
        found = expected in errors
        if found != should_find:
            failures.append(f"{name}: expected found={should_find}, got {errors!r}")
    env_assignment = verifier.github_env_assignment_line('mkdir -p out && echo "TARGET=target dir" >> "$GITHUB_ENV"')
    if env_assignment != "TARGET='target dir'":
        failures.append(f"GITHUB_ENV assignment with spaces was not shell-safe: {env_assignment!r}")
    if failures:
        raise AssertionError("workflow run step shell state handling failed: " + "; ".join(failures))


def assert_v6_red_raw_rust_storage_overrides_are_reported() -> None:
    cases = [
        (
            "CARGO_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; env $E=/tmp/raw-target cargo test",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'E=CARGO_TARGET_DIR; C=cargo; env -S "env $E=/tmp/raw-target $C check"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'env -iS "CARGO_TARGET_DIR=/tmp/raw-target cargo test"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            'E=CARGO_TARGET_DIR; env -iS "$E=/tmp/raw-target cargo test"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR\nexport $E=/tmp/raw-target\ncargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; eval \"export $E=/tmp/raw-target\"; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; $E=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO; ${E}_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "alias c='V=CARGO; eval \"${V}_TARGET_DIR=/tmp/raw cargo\"'; c build",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; bash -c \"${V}_TARGET_DIR=/tmp/raw cargo test\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; sudo ${V}_TARGET_DIR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; time ${V}_TARGET_DIR=/tmp/raw cargo test",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; CMD=\"${V}_TARGET_DIR=/tmp/raw cargo check\"; eval \"$CMD\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; export CMD=\"${V}_TARGET_DIR=/tmp/raw cargo check\"; bash -c \"$CMD\"",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "V=CARGO; alias c='${V}_TARGET_DIR=/tmp/raw cargo'; c build",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "CMD=\"cargo check --target-dir /tmp/raw\"; eval \"$CMD\"",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo>out check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "> /dev/null cargo check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "ARGS=\"--target-dir /tmp/raw\"; cargo check $ARGS",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "echo 'benign'\nE=CARGO_TARGET_DIR\nenv $E=/tmp/raw-target cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "run: |\n  C=cargo\n  $C check --target-dir /tmp/raw",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'VAR=CARGO; eval "${VAR}_TARGET_DIR=/tmp/raw cargo check"; VAR=BENIGN',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=$(echo CARGO_TARGET_DIR); export $E=/tmp/raw-target; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "export E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "declare -x E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_TARGET_DIR: /tmp/raw",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_TARGET_DIR\": /tmp/raw",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "CARGO_BUILD_TARGET_DIR=/tmp/raw-target cargo check",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_BUILD_TARGET_DIR: /tmp/raw",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_BUILD_TARGET_DIR\": /tmp/raw",
            "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        ),
        (
            "mkdir -p .cargo && printf '[build]\\ntarget-dir = \"/tmp/raw-target\"\\n' > .cargo/config.toml && cargo check",
            ".cargo/config.toml build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build.target-dir=/tmp/raw-target' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { target-dir = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { \"target\\u002Ddir\" = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build = { \"target\\U0000002Ddir\" = \"/tmp/raw-target\" }' check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config 'build.rustflags = [\"--out-dir\", \"/tmp/raw-out\"]' check",
            "cargo --config build.rustflags raw output override must be classified",
        ),
        (
            "cargo --config 'build = { rustflags = [\"--artifact-dir\", \"/tmp/raw-artifacts\"] }' check",
            "cargo --config build.rustflags raw output override must be classified",
        ),
        (
            "run: |\n  cargo check \\\n    --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "run: >\n  cargo check\n  --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo check --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "python -c \"import os; os.system('c' + 'argo build --target-dir /tmp/raw')\"",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'python -c "import subprocess; subprocess.run([\'cargo\', \'build\', \'--target-dir\', \'/tmp/raw\'])"',
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'python -c "import subprocess; subprocess.run(args=[\'cargo\', \'build\', \'--target-dir\', \'/tmp/raw\'])"',
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "echo 'cargo \"$@\"' | bash -s -- build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "CARGO_TARGET_TMPDIR=/tmp/raw-tmp cargo test",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "env:\n  CARGO_TARGET_TMPDIR: /tmp/raw-tmp",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "env:\n  \"CARGO_TARGET_TMPDIR\": /tmp/raw-tmp",
            "CARGO_TARGET_TMPDIR raw target override must be classified",
        ),
        (
            "CARGO_INCREMENTAL=1 cargo check",
            "CARGO_INCREMENTAL raw cache override must be classified",
        ),
        (
            "CARGO_ENCODED_RUSTFLAGS='--out-dir\\x1f/tmp/raw-out' cargo check",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "CARGO_BUILD_RUSTFLAGS='--out-dir /tmp/raw-out' cargo check",
            "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  CARGO_BUILD_RUSTFLAGS: '--artifact-dir /tmp/raw-artifacts'",
            "CARGO_BUILD_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  CARGO_ENCODED_RUSTFLAGS: '--out-dir\\x1f/tmp/raw-out'",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  \"CARGO_ENCODED_RUSTFLAGS\": '--out-dir\\x1f/tmp/raw-out'",
            "CARGO_ENCODED_RUSTFLAGS raw output override must be classified",
        ),
        (
            "CARGO_INSTALL_ROOT=/tmp/cargo-install cargo install ripgrep --locked",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "env:\n  CARGO_INSTALL_ROOT: /tmp/cargo-install",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "env:\n  \"CARGO_INSTALL_ROOT\": /tmp/cargo-install",
            "CARGO_INSTALL_ROOT install output override must be classified",
        ),
        (
            "CARGO_HOME=/tmp/cargo-home cargo check",
            "CARGO_HOME raw cache override must be classified",
        ),
        (
            "RUSTUP_HOME=/tmp/rustup-home cargo check",
            "RUSTUP_HOME raw toolchain override must be classified",
        ),
        (
            "RUSTFLAGS='--out-dir /tmp/raw-out' cargo check",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "RUSTFLAGS='--artifact-dir /tmp/raw-artifacts' cargo check",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  RUSTFLAGS: '--out-dir /tmp/raw-out'",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "env:\n  \"RUSTFLAGS\": '--out-dir /tmp/raw-out'",
            "RUSTFLAGS raw output override must be classified",
        ),
        (
            "RUSTC_WRAPPER=/tmp/wrapper cargo check",
            "RUSTC_WRAPPER raw compiler wrapper must be classified",
        ),
        (
            "RUSTC_WORKSPACE_WRAPPER=/tmp/workspace-wrapper cargo check",
            "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified",
        ),
        (
            "cargo rustc -- --out-dir /tmp/raw-out",
            "cargo rustc --out-dir raw output override must be classified",
        ),
        (
            "/tmp/myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "cargo rustc -- --artifact-dir /tmp/raw-artifacts",
            "cargo rustc --artifact-dir raw output override must be classified",
        ),
        (
            "cargo install ripgrep --target x86_64-unknown-linux-gnu --target-dir /tmp/install-build --root /tmp/install-root",
            "cargo install build target and install root ownership must be classified separately",
        ),
        (
            "cargo install ripgrep --root s3://bolt-v2-active-cache/install-root",
            "cargo install S3 install root must be classified",
        ),
        (
            "/tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "timeout 30 /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/builder build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "exec -a name cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "exec -cla name cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --rm -v $PWD:/workspace -w /workspace rust:latest cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --label my-label rust cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --unknown-opt=rust mycargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "podman run --unknown-opt=rust myrustc --out-dir /tmp/raw-out",
            "rustc --out-dir raw output override must be classified",
        ),
        (
            "env >output.log /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "env 1=2 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "timeout -- 30 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "taskset -- 0 cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "runuser -u user /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "sudo sudo sudo sudo sudo sudo sudo /tmp/c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "/tmp/c install cargo-deny --root s3://bolt-v2-active-cache/install-root",
            "cargo install S3 install root must be classified",
        ),
        (
            "/tmp/c --config build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "alias c=cargo; c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "alias c=cargo\nc build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "alias mybuild=cargo; alias c=mybuild; c build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "cargo --config=build.target-dir=/tmp/raw-target check",
            "cargo --config build.target-dir raw target override must be classified",
        ),
        (
            "cargo --config /tmp/cargo-config.toml check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo --config myconfig.txt check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo --config=myconfig.txt check",
            "cargo --config file raw target override must be classified",
        ),
        (
            "cargo install ripgrep --root /tmp/install-root --target-dir /tmp/install-build",
            "cargo install build target and install root ownership must be classified separately",
        ),
        (
            "BOLT_MANAGED_JUST=1 just managed-build",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "VAR=BOLT_MANAGED_JUST; export $VAR=1; just managed-build",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "echo \"BOLT_MANAGED_JUST<<EOF\" >> \"$GITHUB_ENV\"",
            "BOLT_MANAGED_JUST private just recipe bypass must be classified",
        ),
        (
            "no-mistakes run -- cargo check",
            "no-mistakes raw Cargo drift must be classified",
        ),
        (
            "run: >\n  no-mistakes run --\n  cargo check",
            "no-mistakes raw Cargo drift must be classified",
        ),
        (
            "no-mistakes run --worktree . -- cargo check --target-dir target",
            "no-mistakes worktree-local target path evidence must be reported",
        ),
        (
            "aws s3 sync target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync $(echo target) s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'bash -c "aws s3 sync target s3://bolt-v2-active-cache/target"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp ./$(echo target)/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp ./`echo target`/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp `echo target`/debug/lib.a s3://bolt-v2-active-cache/",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp $(echo file) target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp `echo`target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(echo target)" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'export E=CARGO_TARGET_DIR; env FOO=bar bash -c "$E=/tmp/raw cargo check"',
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "E=CARGO_TARGET_DIR; declare -x $E=/tmp/raw; cargo check",
            "CARGO_TARGET_DIR raw target override must be classified",
        ),
        (
            "cargo check $(echo ; echo --target-dir /tmp/raw)",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            'SRC_DIR=target/debug\naws s3 sync "$SRC_DIR" s3://bolt-v2-active-cache/target/debug',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'env:\n  DEST: s3://bucket/cache\nsteps:\n  - run: aws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'env:\n  DEST: s3://bucket/cache\nsteps:\n  - run: aws s3 sync target "${{ env.DEST }}"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'echo "DEST=s3://bucket/cache" >> "$GITHUB_ENV"\naws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'DEST="s3://bolt-v2-active-cache/target"\naws s3 sync target "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'SRC="s3://bolt-v2-active-cache/target"\naws s3 sync "$SRC" target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "run: |\n  aws s3 sync \\\n    target \\\n    s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "run: >\n  aws s3 sync\n  target\n  s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync ./target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(pwd)/target" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$CARGO_TARGET_DIR\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR}\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${CARGO_TARGET_DIR%/}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir }}\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ steps.setup.outputs.managed_target_dir_relative }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'aws s3 sync "$(python3 scripts/rust_verification.py target-dir --repo .)" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 mv --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws --profile prod s3 sync target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws --region us-east-1 s3 cp --recursive target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE\" s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync . s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$GITHUB_WORKSPACE\"/target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            'TARGET_DIR="$GITHUB_WORKSPACE"/target\nDEST=s3://bucket/cache\naws s3 sync "$TARGET_DIR" "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'TARGET_DIR="$PWD"/target\nDEST=s3://bucket/cache\naws s3 sync "$TARGET_DIR" "$DEST"',
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"$GITHUB_WORKSPACE/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp --recursive s3://bolt-v2-active-cache/target \"${PWD}/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 mv --recursive s3://bolt-v2-active-cache/target \"$PWD/./target\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ github.workspace }}/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ github.workspace }}\" s3://bolt-v2-active-cache/workspace",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"${{ env.CARGO_TARGET_DIR }}/debug\" s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$PWD/target\" s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync \"$PWD\"/target s3://some-bucket/linux-cache",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync $PWD/ s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync /home/runner/work/bolt-v2/bolt-v2/target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target && aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target/debug && aws s3 sync * s3://bolt-v2-active-cache/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "pushd target/debug\naws s3 sync * s3://bolt-v2-active-cache/target/debug",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target ; aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cd target || aws s3 sync * s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"$CARGO_TARGET_DIR\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 sync s3://bolt-v2-active-cache/target \"${{ steps.setup.outputs.managed_target_dir }}\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api put-object --bucket bolt-v2-active-cache --key target/debug/lib.rmeta --body target/debug/lib.rmeta",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api put-object --bucket bolt-v2-active-cache --key target/debug/lib.rmeta --body \"$CARGO_TARGET_DIR/debug/lib.rmeta\"",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3api get-object --bucket bolt-v2-active-cache --key cache target/debug/lib.rmeta",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "cat target/debug/lib.rmeta | base64 | aws s3 cp - s3://bolt-v2-active-cache/target.rmeta.b64",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "head -c 999 target/debug/lib.rmeta | aws s3 cp - s3://bolt-v2-active-cache/file",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "tar c target |\naws s3 cp - s3://bolt-v2-active-cache/target.tar",
            "S3 active mutable target cache must be rejected",
        ),
        (
            "aws s3 cp $(echo ;) target s3://bolt-v2-active-cache/target",
            "S3 active mutable target cache must be rejected",
        ),
    ]
    verifier = load_verifier()
    misses: list[str] = []
    for script, fragment in cases:
        errors = verifier.verify_text(workflow_with_detector_probe(script), BASE_ACTION, BASE_NEXTEST_CONFIG)
        if not any(fragment in error for error in errors):
            misses.append(f"{fragment!r}: errors={errors!r}")
    false_positive = "alias c=cargo; echo c build --target-dir /tmp/raw-target"
    errors = verifier.raw_rust_storage_errors(false_positive)
    if "cargo --target-dir raw target override must be classified" in errors:
        misses.append(f"non-executed alias text was classified: errors={errors!r}")
    false_positive = "aws s3 cp dist/app s3://bolt-v2-release/app\necho target"
    errors = verifier.raw_rust_storage_errors(false_positive)
    if "S3 active mutable target cache must be rejected" in errors:
        misses.append(f"separate-line non-target S3 upload was classified: errors={errors!r}")
    for false_positive in (
        "cargo test -- --target-dir /tmp/test-binary-arg",
        "cargo nextest run -- --target-dir /tmp/test-binary-arg",
        "python3 scripts/rust_verification.py cargo --repo . -- test -- --target-dir /tmp/test-binary-arg",
        "python3 scripts/rust_verification.py run --repo . test -- --target-dir /tmp/test-binary-arg",
    ):
        errors = verifier.raw_rust_storage_errors(false_positive)
        if (
            "cargo --target-dir raw target override must be classified" in errors
            or "S3 active mutable target cache must be rejected" in errors
        ):
            misses.append(f"benign command was classified: {false_positive!r} errors={errors!r}")
    for false_positive in ("/usr/bin/make build", "/tmp/build-tool test", "cargo -C /tmp/repo build"):
        errors = verifier.raw_rust_storage_errors(false_positive)
        if any("raw target override" in error or "raw Cargo drift" in error for error in errors):
            misses.append(f"path command was classified as raw cargo: {false_positive!r} errors={errors!r}")
    if misses:
        raise AssertionError("raw/unmanaged Rust storage policy gaps were silent: " + "; ".join(misses))


def assert_v6_red_renamed_path_cargo_source_builds_are_reported() -> None:
    verifier = load_verifier()
    cases = [
        (
            "/tmp/c install cargo-deny --root s3://bolt-v2-active-cache/install-root",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "timeout 30 /tmp/c install cargo-nextest --version 0.9.132 --locked",
            "repo automation must not compile cargo-nextest from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny /tmp/my-deny\ncargo install --path /tmp/my-deny",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny /tmp/my-deny\ncd /tmp/my-deny && cargo install --path .",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "git clone https://github.com/EmbarkStudios/cargo-deny.git\ncd cargo-deny && cargo install --path .",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "cargo install --git https://github.com/EmbarkStudios/Cargo-Deny --locked",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "cargo install --git https://github.com/nextest-rs/cargo-NeXtEsT --package cargo-nextest --locked",
            "repo automation must not compile cargo-nextest from source",
        ),
        (
            "cargo install $(echo ; echo cargo-deny) --locked",
            "repo automation must not compile cargo-deny from source",
        ),
        (
            "sudo sudo sudo sudo sudo sudo sudo cargo install cargo-deny --locked",
            "repo automation must not compile cargo-deny from source",
        ),
    ]
    for text, expected in cases:
        errors = verifier.repo_automation_source_build_errors(text)
        if expected not in errors:
            raise AssertionError(f"{text!r}: expected {expected!r}, got {errors!r}")


def workflow_with_exact_head_governance_cache_inputs(workflow: str) -> str:
    if all(cache_input in workflow for cache_input in EXACT_HEAD_GOVERNANCE_CACHE_INPUTS):
        return workflow
    governance_inputs = ", " + ", ".join(EXACT_HEAD_GOVERNANCE_CACHE_INPUTS)
    return workflow.replace("'justfile') }}", f"'justfile'{governance_inputs}) }}").replace(
        "'specs/**/*.md') }}",
        f"'specs/**/*.md'{governance_inputs}) }}",
    )


def write_base_workflows(workflow_dir: pathlib.Path) -> None:
    workflow_dir.mkdir(parents=True)
    (workflow_dir / "ci.yml").write_text(BASE_WORKFLOW)
    (workflow_dir / "dispatch-ci-cancel.yml").write_text(BASE_DISPATCH_CI_CANCEL_WORKFLOW)
    (workflow_dir / "merge-readiness-finalizer.yml").write_text(BASE_MERGE_READINESS_FINALIZER_WORKFLOW)
    (workflow_dir / "coverage-enforcer.yml").write_text(BASE_COVERAGE_ENFORCER_WORKFLOW)


def run_verifier_main_with_no_mistakes(
    no_mistakes_text: str,
    *,
    write_mergify_config: bool = True,
) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(VERIFIER_PATH.read_text())

        workflow_dir = tmp_path / ".github" / "workflows"
        write_base_workflows(workflow_dir)
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

        (tmp_path / ".no-mistakes.yaml").write_text(no_mistakes_text)
        if write_mergify_config:
            (tmp_path / ".mergify.yml").write_text((REPO_ROOT / ".mergify.yml").read_text())
        write_rust_verification_policy_fixtures(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_no_mistakes_entrypoint")
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = temp_verifier.main()
        return result, stdout.getvalue() + stderr.getvalue()


def run_verifier_main_with_extra_action(extra_action_text: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(VERIFIER_PATH.read_text())

        workflow_dir = tmp_path / ".github" / "workflows"
        write_base_workflows(workflow_dir)
        write_test_harness_fixture(
            tmp_path,
            manifest=base_test_harness_manifest(),
            write_workflow=False,
            write_justfile=False,
        )

        action_path = tmp_path / ".github" / "actions" / "setup-environment" / "action.yml"
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)

        extra_action_path = tmp_path / ".github" / "actions" / "evade" / "action.yml"
        extra_action_path.parent.mkdir(parents=True)
        extra_action_path.write_text(extra_action_text)

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)
        write_rust_verification_policy_fixtures(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_extra_action_entrypoint")
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = temp_verifier.main()
        return result, stdout.getvalue() + stderr.getvalue()


def run_verifier_main_with_extra_workflow(workflow_name: str, workflow_text: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(VERIFIER_PATH.read_text())

        workflow_dir = tmp_path / ".github" / "workflows"
        write_base_workflows(workflow_dir)
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

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_extra_workflow_entrypoint")
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = temp_verifier.main()
        return result, stdout.getvalue() + stderr.getvalue()


def assert_v6_red_yaml_anchor_jobs_do_not_hide_raw_storage() -> None:
    verifier = load_verifier()
    workflow = """
name: Probe
on: [push]
jobs:
  hidden: &hidden
    runs-on: ubuntu-latest
    steps:
      - run: cargo build --target-dir /tmp/raw
"""
    expected = "cargo --target-dir raw target override must be classified"
    errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow))
    if expected not in errors:
        raise AssertionError(f"anchored workflow job raw-storage drift was silent: {errors!r}")
    if "hidden" not in verifier.parse_jobs(textwrap.dedent(workflow)):
        raise AssertionError("anchored workflow job was not parsed")


def assert_v6_red_yaml_anchor_steps_do_not_hide_raw_storage() -> None:
    verifier = load_verifier()
    workflow = """
name: Probe
on: [push]
jobs:
  hidden:
    runs-on: ubuntu-latest
    steps:
      - run: echo ok
      - &raw run: cargo build --target-dir /tmp/raw
      - &s3 run: aws s3 sync target s3://bolt-v2-active-cache/target
"""
    errors = verifier.raw_rust_storage_errors(textwrap.dedent(workflow))
    expected_target = "cargo --target-dir raw target override must be classified"
    expected_s3 = "S3 active mutable target cache must be rejected"
    if expected_target not in errors or expected_s3 not in errors:
        raise AssertionError(f"anchored workflow step raw-storage drift was silent: {errors!r}")


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


def assert_v6_red_static_path_classifier_ignores_host_filesystem_resolution() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        cargo_target = tmp_path / "cargo"
        cargo_target.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        cargo_link = tmp_path / "builder"
        cargo_link.symlink_to(cargo_target)
        rustc_target = tmp_path / "rustc"
        rustc_target.write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        rustc_link = tmp_path / "compiler"
        rustc_link.symlink_to(rustc_target)
        if verifier.path_executable_looks_like_cargo(str(cargo_link)):
            raise AssertionError("static cargo classifier must not inspect host filesystem symlink targets")
        if verifier.path_executable_looks_like_rustc(str(rustc_link)):
            raise AssertionError("static rustc classifier must not inspect host filesystem symlink targets")
        if not verifier.path_executable_looks_like_cargo(str(tmp_path / "mycargo")):
            raise AssertionError("static cargo classifier must still classify path names that look like cargo")


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


def assert_v6_red_no_mistakes_raw_cargo_is_reported() -> None:
    raw_fixture = """
aliases:
  - &raw_top "cargo build --target-dir /tmp/raw"
  - &raw_top_block |
      cargo build --target-dir /tmp/raw
commands:
  test: cargo test
  lint: >
    cargo clippy --all-targets -- -D warnings
  format: "cargo fmt --check"
  review: 'cargo test --all-targets'
  ci: |
    cargo clippy --workspace
  envcheck: env CARGO_TARGET_DIR=target cargo test
  envsplit: env -S 'cargo test'
  envsplitunquoted: env -S timeout 30 cargo test
  envinvalidassignment: env 1=2 cargo build
  envblocksignal: env --block-signal cargo test
  timeoutdashdash: timeout -- 30 cargo build
  tasksetdashdash: taskset -- 0 cargo build
  anchored: &raw "cargo build --target-dir /tmp/raw"
  anchoralias: *raw
  topanchoralias: *raw_top
  topanchoraliascomment: *raw_top # inline comment
  topblockanchoralias: *raw_top_block
  shellcheck: bash -lc 'cargo test --all'
  evalraw: eval "cargo test"
  evaldashdash: eval -- cargo test
  evalprefix: A=B eval cargo test
  evalprefixquoted: A=B eval "cargo test"
  evalvar: 'CMD="cargo build --target-dir /tmp/raw"; eval "$CMD"'
  evalexportvar: 'export CMD="cargo build"; eval "$CMD"'
  evalparam: 'export CMD="cargo build"; eval "${CMD:-}"'
  evaldynamicenv: 'VAR=CARGO_TARGET_DIR; $VAR=/tmp/raw cargo check'
  evalcomposedenv: 'VAR=CARGO; ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  evalinlinecomposedenv: 'VAR=CARGO ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  evalinlinecomposedeval: 'VAR=CARGO eval "${VAR}_TARGET_DIR=/tmp/raw cargo check"'
  shellccomposedenv: 'VAR=CARGO; bash -c "${VAR}_TARGET_DIR=/tmp/raw cargo test"'
  aliaspayload: 'alias c='\''V=CARGO; eval "${V}_TARGET_DIR=/tmp/raw cargo"'\''; c build'
  sudocomposedenv: 'VAR=CARGO; sudo ${VAR}_TARGET_DIR=/tmp/raw cargo check'
  timecomposedenv: 'VAR=CARGO; time ${VAR}_TARGET_DIR=/tmp/raw cargo test'
  evalcmdpayload: 'VAR=CARGO; CMD="${VAR}_TARGET_DIR=/tmp/raw cargo check"; eval "$CMD"'
  shellcmdpayload: 'VAR=CARGO; export CMD="${VAR}_TARGET_DIR=/tmp/raw cargo check"; bash -c "$CMD"'
  aliasouterpayload: 'VAR=CARGO; alias c='\''${VAR}_TARGET_DIR=/tmp/raw cargo'\''; c build'
  redirectprefix: '> /dev/null cargo test'
  redirectcompact: 'cargo>out test'
  foldedplain: eval
    cargo test
  foldeddouble: "eval
    cargo test"
  shellprefix: A=B bash -c "cargo test"
  shellevalraw: bash -lc 'eval "cargo test"'
  shellalias: bash -lc 'alias c=cargo; c test'
  shellaliasquoted: bash -lc 'alias c="command cargo"; c test'
  shellaliasclippy: bash -lc 'alias c=clippy; c --all-targets'
  shellaliasnextest: bash -lc 'alias c=nextest; c run'
  shellaliasrustc: bash -lc 'alias c=rustc; c --crate-name bolt_v2'
  shellaliastime: bash -lc 'alias c=cargo; time c test'
  shellaliasnice: bash -lc 'alias c=clippy; nice c --all-targets'
  renamedcargo: /tmp/c build
  timerenamedcargo: time /tmp/c build
  xargsrenamedcargo: xargs /tmp/c build
  wrapped: command cargo fmt --check
  stdbufwrap: stdbuf -oL cargo build
  catchsegvwrap: catchsegv cargo test
  chrtbatchwrap: chrt -b cargo build
  chrtidlewrap: chrt -i cargo build
  nicewrap: nice cargo test
  timeniceadjust: time nice --adjustment 10 cargo test
  timeverbose: A=B time -v cargo test
  timeoutput: A=B time -o /tmp/bolt-time cargo test
  doaswrap: doas cargo test
  flockwrap: flock "$TMPDIR/bolt.lock" cargo test
  flockfilec: flock "$TMPDIR/bolt.lock" -c 'cargo test'
  flockshortc: flock -xc 'cargo test' "$TMPDIR/bolt.lock"
  timeflocktimeout: time flock --timeout 5 "$TMPDIR/bolt.lock" cargo test
  flockclose: flock -o "$TMPDIR/bolt.lock" cargo test
  sudoflock: sudo flock -o "$TMPDIR/bolt.lock" cargo test
  sudousercommand: sudo -u bash cargo build
  sudoshell: sudo bash -lc 'cargo test --all'
  envargv0: env --argv0 cargo cargo build
  envshortargv0: env -a cargo cargo build
  envshell: env -i bash -lc 'cargo test --all'
  hyphenated: cargo-clippy --workspace
  zigbuild: cargo zigbuild --release
  rustup: rustup run stable cargo test
  pyinline: python -c 'import os; os.system("cargo test")'
  timeout: timeout 30 cargo test
  managedjustenv: BOLT_MANAGED_JUST=1 just managed-build
  managedjustdynamic: VAR=BOLT_MANAGED_JUST; export $VAR=1; just managed-build
  declaretarget: E=CARGO_TARGET_DIR; declare -x $E=/tmp/raw; cargo check
  substitutiontarget: cargo check $(echo ; echo --target-dir /tmp/raw)
  dockeruncertainrenamed: docker run --unknown-opt=rust mycargo build
  podmanuncertainrenamedrustc: podman run --unknown-opt=rust myrustc --out-dir /tmp/raw
  chained: python3 scripts/rust_verification.py cargo --repo . -- test && cargo test
  compact_and: python3 scripts/rust_verification.py cargo --repo . -- test&&cargo test
  compact_semicolon: python3 scripts/rust_verification.py cargo --repo . -- test;cargo test
  compact_pipe: python3 scripts/rust_verification.py cargo --repo . -- test|cargo test
  compact_or: python3 scripts/rust_verification.py cargo --repo . -- test||cargo test
  blockmanagedhidden: |
    python3 scripts/rust_verification.py cargo --repo . -- test
    cargo test
  managedtarget: python3 scripts/rust_verification.py cargo --repo . -- test --target-dir /tmp/raw
  managedconfig: python3 scripts/rust_verification.py cargo --repo . -- --config=build.target-dir=/tmp/raw test
  managedencodedrustflags: CARGO_ENCODED_RUSTFLAGS='--out-dir\\x1f/tmp/raw-out' python3 scripts/rust_verification.py cargo --repo . -- check
  managedinstallroot: python3 scripts/rust_verification.py cargo --repo . -- install ripgrep --root /tmp/install-root
  managedrustcwrapper: RUSTC_WRAPPER=/tmp/wrapper python3 scripts/rust_verification.py cargo --repo . -- test
  managedtimeout: timeout 30 python3 scripts/rust_verification.py cargo --repo . -- test
  managedenvci: GITHUB_ACTIONS=true python3 scripts/rust_verification.py cargo --repo . -- test
  managedenvcmdci: env GITHUB_ACTIONS=true python3 scripts/rust_verification.py cargo --repo . -- test
  managedpythonflag: python3 -W ignore scripts/rust_verification.py cargo --repo . -- test
  no-mistakes-clippy-command: no-mistakes run -- clippy
  no-mistakes-nextest-command: no-mistakes run -- nextest run
  s3cache: aws s3 sync target s3://bolt-v2-active-cache/target
  docs: just docs
"""
    allowed_fixture = """
commands:
  test: just source-fence-static
  lint: just fmt-check
  format: python3 scripts/rust_verification.py cargo --repo . -- fmt --check
  exact-head-ci: gh run view --repo seungpyoson/bolt-v2 --commit "$GITHUB_SHA" --json conclusion
  sudouserarg: timeout 30 sudo -u cargo echo hello
"""
    commented_commands_fixture = """
commands: # repo review commands
  test: cargo test
"""
    inline_commands_fixture = """
commands: { test: "cargo test" }
"""
    fixture_expected_raw_keys = [
        "test",
        "lint",
        "format",
        "review",
        "ci",
        "envcheck",
        "envsplit",
        "envsplitunquoted",
        "envinvalidassignment",
        "envblocksignal",
        "timeoutdashdash",
        "tasksetdashdash",
        "anchored",
        "anchoralias",
        "topanchoralias",
        "topanchoraliascomment",
        "topblockanchoralias",
        "shellcheck",
        "evalraw",
        "evaldashdash",
        "evalprefix",
        "evalprefixquoted",
        "evalvar",
        "evalexportvar",
        "evalparam",
        "evaldynamicenv",
        "evalcomposedenv",
        "evalinlinecomposedenv",
        "evalinlinecomposedeval",
        "shellccomposedenv",
        "aliaspayload",
        "sudocomposedenv",
        "timecomposedenv",
        "evalcmdpayload",
        "shellcmdpayload",
        "aliasouterpayload",
        "redirectprefix",
        "redirectcompact",
        "foldedplain",
        "foldeddouble",
        "shellprefix",
        "shellevalraw",
        "shellalias",
        "shellaliasquoted",
        "shellaliasclippy",
        "shellaliasnextest",
        "shellaliasrustc",
        "shellaliastime",
        "shellaliasnice",
        "renamedcargo",
        "timerenamedcargo",
        "xargsrenamedcargo",
        "wrapped",
        "stdbufwrap",
        "catchsegvwrap",
        "chrtbatchwrap",
        "chrtidlewrap",
        "nicewrap",
        "timeniceadjust",
        "timeverbose",
        "timeoutput",
        "doaswrap",
        "flockwrap",
        "flockfilec",
        "flockshortc",
        "timeflocktimeout",
        "flockclose",
        "sudoflock",
        "sudousercommand",
        "sudoshell",
        "envargv0",
        "envshortargv0",
        "envshell",
        "hyphenated",
        "zigbuild",
        "rustup",
        "pyinline",
        "timeout",
        "managedjustenv",
        "managedjustdynamic",
        "declaretarget",
        "substitutiontarget",
        "dockeruncertainrenamed",
        "podmanuncertainrenamedrustc",
        "chained",
        "compact_and",
        "compact_semicolon",
        "compact_pipe",
        "compact_or",
        "blockmanagedhidden",
        "managedtarget",
        "managedconfig",
        "managedencodedrustflags",
        "managedinstallroot",
        "no-mistakes-clippy-command",
        "no-mistakes-nextest-command",
    ]
    expected = [
        f".no-mistakes.yaml commands.{command_name} raw Cargo drift must be classified"
        for command_name in fixture_expected_raw_keys
    ]
    expected_s3 = ".no-mistakes.yaml commands.s3cache S3 active mutable target cache must be rejected"
    expected_storage = [
        ".no-mistakes.yaml commands.managedrustcwrapper RUSTC_WRAPPER raw compiler wrapper must be classified",
        ".no-mistakes.yaml commands.managedenvci GITHUB_ACTIONS local CI spoof must not be checked in",
        ".no-mistakes.yaml commands.managedenvcmdci GITHUB_ACTIONS local CI spoof must not be checked in",
    ]
    expected_wrapper = [
        ".no-mistakes.yaml commands.managedtimeout wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedenvci wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedenvcmdci wrapper-routed local compile-heavy Rust must be remote-first",
        ".no-mistakes.yaml commands.managedpythonflag wrapper-routed local compile-heavy Rust must be remote-first",
    ]
    fixture_result, fixture_errors = run_verifier_main_with_no_mistakes(raw_fixture)
    missing_fixture = [fragment for fragment in expected if fragment not in fixture_errors]
    missing_storage = [fragment for fragment in expected_storage if fragment not in fixture_errors]
    missing_wrapper = [fragment for fragment in expected_wrapper if fragment not in fixture_errors]
    false_fixture = ".no-mistakes.yaml commands.docs raw Cargo drift must be classified" in fixture_errors
    allowed_result, allowed_errors = run_verifier_main_with_no_mistakes(allowed_fixture)
    false_allowed = [
        error for error in allowed_errors.splitlines()
        if ".no-mistakes.yaml" in error and "raw Cargo drift" in error
    ]
    commented_result, commented_errors = run_verifier_main_with_no_mistakes(commented_commands_fixture)
    commented_expected = ".no-mistakes.yaml commands.test raw Cargo drift must be classified"
    inline_result, inline_errors = run_verifier_main_with_no_mistakes(inline_commands_fixture)
    inline_expected = ".no-mistakes.yaml commands section must use block mapping"

    if (
        fixture_result == 0
        or missing_fixture
        or missing_storage
        or missing_wrapper
        or expected_s3 not in fixture_errors
        or false_fixture
        or allowed_result != 0
        or false_allowed
        or commented_result == 0
        or commented_expected not in commented_errors
        or inline_result == 0
        or inline_expected not in inline_errors
    ):
        raise AssertionError(
            "no-mistakes raw-Cargo drift must fail through verifier main() while managed-wrapper "
            "and exact-head CI evidence commands stay allowed: "
            f"fixture_result={fixture_result} missing_fixture={missing_fixture!r} "
            f"missing_storage={missing_storage!r} missing_wrapper={missing_wrapper!r} "
            f"expected_s3={expected_s3!r} false_fixture={false_fixture} fixture_errors={fixture_errors!r} "
            f"fixture_expected_raw_keys={fixture_expected_raw_keys!r} "
            f"allowed_result={allowed_result} false_allowed={false_allowed!r} "
            f"allowed_errors={allowed_errors!r} "
            f"commented_result={commented_result} commented_errors={commented_errors!r} "
            f"inline_result={inline_result} inline_errors={inline_errors!r}"
        )


def assert_v6_red_exact_head_governance_inputs_are_cache_keyed() -> None:
    governed_workflow = workflow_with_exact_head_governance_cache_inputs(BASE_WORKFLOW)
    assert_clean(governed_workflow)
    for cache_input in EXACT_HEAD_GOVERNANCE_CACHE_INPUTS:
        assert_error(
            "cache keys must include exact-head CI/no-mistakes governance inputs",
            governed_workflow.replace(f", {cache_input}", ""),
        )


def assert_shell_logical_lines_handles_crlf_continuations() -> None:
    verifier = load_verifier()
    logical_lines = verifier.shell_logical_lines("cargo check \\\r\n  --target-dir /tmp/raw\r\n")
    if logical_lines != ["cargo check    --target-dir /tmp/raw"]:
        raise AssertionError(f"CRLF shell continuation was not folded: {logical_lines!r}")


def assert_v6_red_workflow_policy_gaps() -> None:
    checks = [
        assert_v6_red_s3_storage_transfer_policy_is_semantic,
        assert_v6_workflow_run_steps_reset_shell_state,
        assert_v6_red_raw_rust_storage_overrides_are_reported,
        assert_v6_red_renamed_path_cargo_source_builds_are_reported,
        assert_v6_red_no_mistakes_raw_cargo_is_reported,
        assert_v6_red_exact_head_governance_inputs_are_cache_keyed,
        assert_v6_red_backtester_cache_keys_include_crate_sources,
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


def assert_v6_red_backtester_cache_keys_include_crate_sources() -> None:
    verifier = load_verifier()
    bad = """jobs:
  clippy:
    steps:
      - uses: actions/cache@example
        with:
          key: managed-target-bvs-v1-${{ runner.os }}-${{ runner.arch }}-clippy-${{ hashFiles('crates/backtesting-vertical-slice/Cargo.lock', 'crates/backtesting-vertical-slice/Cargo.toml') }}
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    assert any("backtester managed-target cache key must include crates/backtesting-vertical-slice/src/**" in error for error in errors), errors
    assert any("backtester managed-target cache key must include crates/backtesting-vertical-slice/tests/**" in error for error in errors), errors
    good = bad.replace(
        "'crates/backtesting-vertical-slice/Cargo.toml'",
        "'crates/backtesting-vertical-slice/Cargo.toml', 'crates/backtesting-vertical-slice/src/**', 'crates/backtesting-vertical-slice/tests/**'",
    )
    good_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": good})
    assert not [
        error for error in good_errors if "backtester managed-target cache key must include" in error
    ], good_errors


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
        "backtester draft deferral shared gate call must include --job detect=${{ needs.detect.result }}" in error
        for error in errors
    ), errors


def assert_v6_red_backtester_test_uses_nextest_archive() -> None:
    verifier = load_verifier()
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
    assert any("backtester bvs-test shards must name matrix shards" in error for error in errors), errors
    assert any("backtester bvs-test must define dedicated issue-789 job" in error for error in errors), errors

    good = """jobs:
  test-archive:
    name: bvs-test archive
    needs: [ci-policy, detect, fmt]
    env:
      BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst
      BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz
    steps:
      - name: Restore BVS nextest archive
        id: bvs-nextest-archive-cache
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          key: bvs-nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('crates/backtesting-vertical-slice/Cargo.lock', 'crates/backtesting-vertical-slice/Cargo.toml', 'crates/backtesting-vertical-slice/src/**', 'crates/backtesting-vertical-slice/tests/**') }}
      - name: Restore BVS binary sidecars
        id: bvs-bin-sidecars-cache
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          key: bvs-bin-sidecars-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('crates/backtesting-vertical-slice/Cargo.lock', 'crates/backtesting-vertical-slice/Cargo.toml', 'crates/backtesting-vertical-slice/src/**', 'crates/backtesting-vertical-slice/tests/**') }}
      - name: Resolve crate managed target dir
        if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'
      - uses: Swatinem/rust-cache@example
        with:
          save-if: ${{ github.job == 'test-archive' }}
      - name: Restore archive build target cache
        id: test-target-cache
        if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
      - name: Build BVS nextest archive
        if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true'
        run: just bte-test-archive "$BVS_NEXTEST_ARCHIVE_PATH"
      - name: Save BVS nextest archive
        if: steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true'
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
      - name: Build BVS binary sidecars
        if: steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true'
        run: |
          python3 "${{ steps.setup.outputs.rust_verification_owner }}" cargo --repo crates/backtesting-vertical-slice -- build --locked --bins
          find debug -maxdepth 1 -type f -perm -111 -print0
      - name: Save BVS binary sidecars
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
      - name: Save archive build target cache
        if: ${{ (steps.bvs-nextest-archive-cache.outputs.cache-hit != 'true' || steps.bvs-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
      - name: Upload BVS test payload
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a
        with:
          name: bvs-test-payload
          path: .nextest-archive
          include-hidden-files: true
          if-no-files-found: error
  test:
    name: bvs-test ${{ matrix.shard }} of 4
    needs: [ci-policy, detect, fmt, test-archive]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && needs.detect.outputs.bvs_changed == 'true' && needs.test-archive.result == 'success' }}
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3, 4]
    env:
      BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst
      BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz
      BVS_NEXTEST_SHARDS: "4"
    steps:
      - name: Download BVS test payload
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c
        with:
          name: bvs-test-payload
          path: .nextest-archive
      - name: Require BVS test payload
        run: |
          test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }
          test -s "$BVS_BIN_SIDECARS_PATH" || { echo "BVS binary sidecars missing or empty after artifact download"; exit 1; }
      - name: Extract BVS binary sidecars
        run: tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"
      - name: test
        run: |
          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"
          just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" --partition "count:${{ matrix.shard }}/${{ env.BVS_NEXTEST_SHARDS }}" -- --skip issue_789_first_real_free_data_taker_pl
  issue_789:
    name: bvs-test issue-789
    needs: [ci-policy, detect, test-archive, gate]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && needs.detect.outputs.bvs_changed == 'true' && needs.test-archive.result == 'success' && needs.gate.result == 'success' }}
    env:
      BVS_NEXTEST_ARCHIVE_PATH: .nextest-archive/bvs-nextest-archive.tar.zst
      BVS_BIN_SIDECARS_PATH: .nextest-archive/bvs-bin-sidecars.tar.gz
      BOLT_ISSUE_789_RESULT_PATH: result.json
    steps:
      - name: Download BVS test payload
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c
        with:
          name: bvs-test-payload
          path: .nextest-archive
      - name: Require BVS test payload
        run: |
          test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }
          test -s "$BVS_BIN_SIDECARS_PATH" || { echo "BVS binary sidecars missing or empty after artifact download"; exit 1; }
      - name: Extract BVS binary sidecars
        run: tar -xzf "$BVS_BIN_SIDECARS_PATH" -C "${{ steps.crate_target.outputs.dir }}"
      - name: test issue-789
        run: |
          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"
          just bte-test-archive-run "$BVS_NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/bvs-nextest-archive-extract" issue_789_first_real_free_data_taker_pl
      - name: Upload issue #789 first-P/L artifact
        uses: actions/upload-artifact@example
        with:
          name: issue-789-first-pl-${{ github.run_id }}-${{ github.run_attempt }}
          if-no-files-found: error
"""
    good_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": good})
    assert not [error for error in good_errors if "backtester bvs-test" in error], good_errors

    weakened_archive_guard = good.replace(
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }',
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || true',
        1,
    )
    weakened_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": weakened_archive_guard}
    )
    assert any("not fail-closed" in error and 'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || true' in error for error in weakened_errors), weakened_errors

    missing_archive_guard = good.replace(
        '          test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }\n',
        "",
        1,
    )
    def assert_missing_consumer_guard(workflow: str, payload_name: str, scope_name: str) -> None:
        expected_error = (
            f"backtester consumer must fail closed if the downloaded {payload_name} "
            f"is missing or empty ({scope_name})"
        )
        workflow_errors = verifier.verify_repo_automation_texts(
            {".github/workflows/backtester-ci.yml": workflow}
        )
        assert any(expected_error in error for error in workflow_errors), workflow_errors

    def assert_missing_shards_archive_guard(workflow: str) -> None:
        assert_missing_consumer_guard(workflow, "archive", "bvs-test shards")

    missing_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": missing_archive_guard}
    )
    assert any(
        "backtester consumer must fail closed if the downloaded archive is missing or empty" in error
        for error in missing_errors
    ), missing_errors
    assert_missing_shards_archive_guard(
        missing_archive_guard.replace(
            '      BVS_NEXTEST_SHARDS: "4"\n',
            '      BVS_NEXTEST_SHARDS: "4"\n'
            '      ARCHIVE_DECOY: \'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || exit 1\'\n',
            1,
        )
    )
    assert_missing_shards_archive_guard(
        missing_archive_guard.replace(
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n',
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n'
            '          echo \'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || exit 1\'\n',
            1,
        )
    )
    assert_missing_shards_archive_guard(
        missing_archive_guard.replace(
            '      BVS_NEXTEST_SHARDS: "4"\n',
            '      BVS_NEXTEST_SHARDS: "4"\n'
            "      DECOY: |\n"
            "        ignored\n"
            '        test -s "$BVS_NEXTEST_ARCHIVE_PATH" || exit 1\n',
            1,
        )
    )
    assert_missing_consumer_guard(
        replace_once(
            missing_archive_guard,
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n',
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n'
            '          test -s "$BVS_NEXTEST_ARCHIVE_PATH".decoy || exit 1\n',
        ),
        "archive",
        "bvs-test shards",
    )

    missing_sidecars_guard = good.replace(
        '          test -s "$BVS_BIN_SIDECARS_PATH" || { echo "BVS binary sidecars missing or empty after artifact download"; exit 1; }\n',
        "",
        1,
    )
    assert_missing_consumer_guard(
        replace_once(
            missing_sidecars_guard,
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n',
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n'
            '          test -s "$BVS_BIN_SIDECARS_PATH".decoy || exit 1\n',
        ),
        "sidecars",
        "bvs-test shards",
    )

    missing_issue_archive_guard = without_once_after(
        good,
        "  issue_789:\n",
        '          test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }\n',
    )
    assert_missing_consumer_guard(
        replace_once_after(
            missing_issue_archive_guard,
            "      - name: test issue-789\n",
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n',
            '          mkdir -p "$RUNNER_TEMP/bvs-nextest-archive-extract"\n'
            '          test -s "$BVS_NEXTEST_ARCHIVE_PATH".decoy || exit 1\n',
        ),
        "archive",
        "bvs-test issue-789",
    )

    exit_one_archive_guard = good.replace(
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || { echo "BVS nextest archive missing or empty after artifact download"; exit 1; }',
        'test -s "$BVS_NEXTEST_ARCHIVE_PATH" || exit 1',
        1,
    )
    exit_one_errors = verifier.verify_repo_automation_texts(
        {".github/workflows/backtester-ci.yml": exit_one_archive_guard}
    )
    assert not [error for error in exit_one_errors if "backtester consumer" in error], exit_one_errors


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
        "          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
    )
    inline_hashfiles_key_line = (
        "          key: nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock') }}"
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
        "nextest-fingerprint-reuse must be PR-only",
        remove_fragment_if_present(BASE_WORKFLOW, " && github.event_name == 'pull_request'"),
    )
    assert_error(
        "detector must deny fingerprint reuse outside pull_request",
        remove_fragment_if_present(
            BASE_WORKFLOW,
            """          if [[ "${{ github.event_name }}" != "pull_request" ]]; then
            echo "value=false" >> "$GITHUB_OUTPUT"
          elif """,
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
            "        if: github.event_name == 'pull_request'",
            "        if: false",
        ),
    )
    assert_error(
        "detector fingerprint-reuse governance step must match canonical envelope",
        without_once_after(
            BASE_WORKFLOW,
            "      - name: Detect fingerprint-reuse governance changes",
            "        if: github.event_name == 'pull_request'\n",
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
            "        if: github.event_name == 'pull_request'\n",
            """        if: github.event_name == 'pull_request'
        if: false
""",
        ),
    )

    narrowed_pathspec = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        """.github/workflows/ci.yml             .github/actions/setup-environment/action.yml             ci/nextest-fingerprint.toml             ci/github-actions-runners.toml             scripts/nextest_fingerprint.py             scripts/test_nextest_fingerprint.py             scripts/root_bin_sidecars.py             scripts/test_root_bin_sidecars.py             scripts/ci_provenance.py             scripts/test_ci_provenance.py             scripts/verify_ci_workflow_hygiene.py             scripts/test_verify_ci_workflow_hygiene.py)""",
        """.github/workflows/ci.yml)
          echo "decoy paths: .github/actions/setup-environment/action.yml ci/nextest-fingerprint.toml ci/github-actions-runners.toml scripts/nextest_fingerprint.py scripts/test_nextest_fingerprint.py scripts/root_bin_sidecars.py scripts/test_root_bin_sidecars.py scripts/ci_provenance.py scripts/test_ci_provenance.py scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py\"""",
    )
    assert_error(
        "detector must detect fingerprint-reuse governance changes",
        narrowed_pathspec,
    )
    git_diff_decoy_pathspec = replace_once_after(
        BASE_WORKFLOW,
        "      - name: Detect fingerprint-reuse governance changes",
        """          changed="$(git diff --name-only "${base_ref}...${head_ref}" --             .github/workflows/ci.yml""",
        """          echo "$(git diff --name-only "${base_ref}...${head_ref}" -- .github/workflows/ci.yml .github/actions/setup-environment/action.yml ci/nextest-fingerprint.toml ci/github-actions-runners.toml scripts/nextest_fingerprint.py scripts/test_nextest_fingerprint.py scripts/root_bin_sidecars.py scripts/test_root_bin_sidecars.py scripts/ci_provenance.py scripts/test_ci_provenance.py scripts/verify_ci_workflow_hygiene.py scripts/test_verify_ci_workflow_hygiene.py)"
          changed="$(git diff --name-only "${base_ref}...${head_ref}" --             .github/workflows/ci.yml""",
    )
    git_diff_decoy_pathspec = replace_once_after(
        git_diff_decoy_pathspec,
        "      - name: Detect fingerprint-reuse governance changes",
        """.github/workflows/ci.yml             .github/actions/setup-environment/action.yml             ci/nextest-fingerprint.toml             ci/github-actions-runners.toml             scripts/nextest_fingerprint.py             scripts/test_nextest_fingerprint.py             scripts/root_bin_sidecars.py             scripts/test_root_bin_sidecars.py             scripts/ci_provenance.py             scripts/test_ci_provenance.py             scripts/verify_ci_workflow_hygiene.py             scripts/test_verify_ci_workflow_hygiene.py)""",
        """.github/workflows/ci.yml)""",
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
    folded_job_if = replace_once(
        BASE_WORKFLOW,
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && github.event_name == 'pull_request' && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}",
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && github.event_name == 'pull_request' && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main'\n      || github.event_name == 'pull_request' }}",
    )
    assert_error("nextest-fingerprint-reuse must use the canonical job if", folded_job_if)
    folded_job_if_with_canonical_first_line = replace_once(
        BASE_WORKFLOW,
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && github.event_name == 'pull_request' && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}",
        "    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && github.event_name == 'pull_request' && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}\n      || github.event_name == 'pull_request'",
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
        f"""      - name: Decoy resolver command
        run: |
          echo 'python3 scripts/ci_provenance.py resolve-fingerprint --current-run-id "${{{{ github.run_id }}}}" --current-fingerprint "${{{{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}}}" | tee -a "$GITHUB_OUTPUT"'

      - name: Resolve nextest fingerprint reuse""",
    )
    assert_error("nextest-fingerprint-reuse must use secure current nextest fingerprint output", stale_fingerprint_with_decoy)
    assert_error("nextest-fingerprint-reuse must run ci_provenance.py resolve-fingerprint", stale_fingerprint_with_decoy)
    resolver_pipe_scalar = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
        "        run: >",
        "        run: |",
    )
    assert_error("nextest-fingerprint-reuse resolver step must match canonical envelope", resolver_pipe_scalar)
    resolver_extra_workdir = replace_once_after(
        BASE_WORKFLOW,
        "  nextest-fingerprint-reuse:",
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
            """          else
            echo "value=true" >> "$GITHUB_OUTPUT"
          fi""",
            """          else
            echo "value=true" >> "$GITHUB_OUTPUT"
          fi
          echo "value=true" >> "$GITHUB_OUTPUT\"""",
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


def assert_v6_red_raw_storage_checks_all_ci_automation() -> None:
    verifier = load_verifier()
    advisory = BASE_ADVISORY_WORKFLOW.replace(
        "        run: just deny-advisories",
        "        run: |\n          aws s3 sync target s3://some-bucket/linux-cache",
    )
    advisory_errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": advisory},
        BASE_ACTION,
        BASE_NEXTEST_CONFIG,
    )
    action = BASE_ACTION.replace(
        "      run: echo setup",
        "      run: |\n        CARGO_TARGET_DIR=/tmp/raw cargo check",
    )
    action_errors = verifier.verify_workflows(
        {"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW},
        action,
        BASE_NEXTEST_CONFIG,
    )
    repo_errors = verifier.verify_repo_automation_texts(
        {
            "justfile": "check:\n    CARGO_TARGET_DIR=/tmp/raw cargo check\n",
            "justfile.raw": "test:\n    cargo test\n",
            "justfile.spoof": 'bad:\n    echo "BOLT_MANAGED_JUST exit"\n    cargo build\n',
            "justfile.managed-spoof": 'managed-build:\n    echo BOLT_MANAGED_JUST rust_verification.py run exit 2\n    cargo build\n',
            "justfile.managed-exact-guard-raw": 'managed-build:\n    if [ "${BOLT_MANAGED_JUST:-}" != "1" ]; then echo "ERROR: managed-build must run through scripts/rust_verification.py run"; exit 2; fi\n    cargo build --release\n',
            "scripts/raw.sh": "#!/usr/bin/env bash\ncargo build\n",
            "scripts/raw-substitution-dollar.sh": "#!/usr/bin/env bash\nx=$(cargo build)\n",
            "scripts/raw-substitution-quoted.sh": "#!/usr/bin/env bash\nx=\"$(cargo build)\"\n",
            "scripts/raw-substitution-backtick.sh": "#!/usr/bin/env bash\nx=`cargo build`\n",
            "scripts/raw-find-exec.sh": "#!/usr/bin/env bash\nfind . -name Cargo.toml -exec cargo build \\;\n",
            "scripts/raw-su.sh": "#!/usr/bin/env bash\nsu user -c 'cargo build'\n",
            "scripts/raw-su-shell-arg.sh": "#!/usr/bin/env bash\nsu -s -c user -c 'cargo build'\n",
            "scripts/raw-runuser.sh": "#!/usr/bin/env bash\nrunuser -u user -- cargo build\n",
            "scripts/raw-flock.sh": "#!/usr/bin/env bash\nflock /tmp/lock -ccargo\\ build\n",
            "scripts/raw-flock-option-arg.sh": "#!/usr/bin/env bash\nflock -w -c /tmp/lock -c 'cargo build'\n",
            "scripts/raw-chrt-batch.sh": "#!/usr/bin/env bash\nchrt -b cargo build\n",
            "scripts/raw-env-argv0.sh": "#!/usr/bin/env bash\nenv --argv0 cargo cargo build\n",
            "scripts/multiline-eval.sh": "#!/usr/bin/env bash\nCMD=\"cargo build\"\nbash -c \"$CMD\"\n",
            "scripts/multiline-quoted-eval.sh": "#!/usr/bin/env bash\nCMD=\"cargo\nbuild --target-dir /tmp/raw\"\nbash -c \"$CMD\"\n",
            "scripts/comment-blind.sh": "# comment with unbalanced quote '\ncargo build\necho 'closing quote'\n",
            "scripts/nested-var-eval.sh": "CMD=\"cargo build\"\nbash -c \"echo benign; eval $CMD\"\n",
            "scripts/raw-guard-text.sh": '#!/usr/bin/env bash\necho "Missing BOLT_MANAGED_JUST, exit 1"\ncargo build\n',
            "scripts/raw-redirection.sh": "#!/usr/bin/env bash\n> /dev/null cargo build\n",
            "scripts/symlink-cargo.sh": "ln -s $(which cargo) /tmp/mycargo\n/tmp/mycargo build --target-dir /tmp/raw\n",
            "scripts/copy-cargo.sh": "cp $(which cargo) /tmp/mycargo\n/tmp/mycargo build\n",
            "scripts/non-rust-make.sh": "/usr/bin/make test\n",
            "scripts/non-rust-gradle.sh": "./gradlew build\n",
            "scripts/non-rust-cargo-build-script.sh": "/tmp/cargo-build.sh test\n",
            "scripts/non-rust-cargo-build-uppercase-py.sh": "/tmp/cargo-build.PY test\n",
            "scripts/non-rust-cargo-tests-py.sh": "tests/cargo-tests.py build\n",
            "justfile.setup": "setup:\n    cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.absolute": "setup:\n    /usr/bin/cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.timeout": "setup:\n    timeout 30 cargo install cargo-deny --version 0.18.2\n",
            "justfile.setup.xargs": "setup:\n    xargs cargo install cargo-nextest\n",
            "scripts/local.sh": "aws s3 sync \"$PWD\"/target s3://some-bucket/linux-cache\n",
            "scripts/workspace.sh": "aws s3 sync \"$GITHUB_WORKSPACE\" s3://some-bucket/workspace\n",
            "scripts/nested-s3-shell.sh": 'bash -c "aws s3 sync target s3://bolt-v2-active-cache/target"\n',
            "scripts/export-name-word.sh": "export E VAR=CARGO_TARGET_DIR\n$VAR=/tmp/raw cargo check\n",
            "scripts/s3api.sh": "aws s3api put-object --bucket b --key target/debug/lib --body target/debug/lib\n",
            "scripts/s3api-get.sh": "aws s3api get-object --bucket b --key cache target/debug/lib\n",
        }
    )
    expected = "S3 active mutable target cache must be rejected"
    if not any(expected in error for error in advisory_errors):
        raise AssertionError(f"advisory workflow raw-storage drift was silent: {advisory_errors!r}")
    expected = "CARGO_TARGET_DIR raw target override must be classified"
    if not any(expected in error for error in action_errors):
        raise AssertionError(f"setup action raw-storage drift was silent: {action_errors!r}")
    if not any("justfile" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile raw-storage drift was silent: {repo_errors!r}")
    expected = "repo automation raw Cargo must use managed rust_verification wrapper"
    if not any("justfile.raw" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile raw-cargo drift was silent: {repo_errors!r}")
    if not any("justfile.spoof" in error and expected in error for error in repo_errors):
        raise AssertionError(f"spoofed justfile managed-guard drift was silent: {repo_errors!r}")
    if not any("justfile.managed-spoof" in error and expected in error for error in repo_errors):
        raise AssertionError(f"spoofed managed just recipe guard drift was silent: {repo_errors!r}")
    if not any("justfile.managed-exact-guard-raw" in error and expected in error for error in repo_errors):
        raise AssertionError(f"exact-guard managed just recipe raw cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-dollar.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script command-substitution raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-quoted.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script quoted command-substitution raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-substitution-backtick.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script backtick raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-find-exec.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script find-exec raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-su.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script su raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-su-shell-arg.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script su shell-arg raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-runuser.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script runuser raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-flock.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script flock raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-flock-option-arg.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script flock option-arg raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/multiline-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script multiline eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/multiline-quoted-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script multiline quoted eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/comment-blind.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script comment-blinded raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/nested-var-eval.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script nested variable eval raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-guard-text.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script guard-text raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/raw-redirection.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script redirected raw-cargo drift was silent: {repo_errors!r}")
    if not any("scripts/symlink-cargo.sh" in error and "cargo --target-dir raw target override" in error for error in repo_errors):
        raise AssertionError(f"symlinked cargo raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/copy-cargo.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"copied cargo raw-cargo drift was silent: {repo_errors!r}")
    false_repo_raw = [error for error in repo_errors if "scripts/non-rust-" in error]
    if false_repo_raw:
        raise AssertionError(f"non-Rust path commands must stay allowed: {false_repo_raw!r}")
    expected = "repo automation must not compile cargo-nextest from source"
    if not any("justfile.setup" in error and expected in error for error in repo_errors):
        raise AssertionError(f"justfile cargo-install drift was silent: {repo_errors!r}")
    if not any("justfile.setup.absolute" in error and expected in error for error in repo_errors):
        raise AssertionError(f"absolute cargo-install drift was silent: {repo_errors!r}")
    expected = "repo automation must not compile cargo-deny from source"
    if not any("justfile.setup.timeout" in error and expected in error for error in repo_errors):
        raise AssertionError(f"wrapped cargo-deny install drift was silent: {repo_errors!r}")
    expected = "repo automation must not compile cargo-nextest from source"
    if not any("justfile.setup.xargs" in error and expected in error for error in repo_errors):
        raise AssertionError(f"wrapped cargo-nextest install drift was silent: {repo_errors!r}")
    expected = "S3 active mutable target cache must be rejected"
    if not any("scripts/local.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/workspace.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"workspace S3 sync drift was silent: {repo_errors!r}")
    if not any("scripts/nested-s3-shell.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"nested shell S3 sync drift was silent: {repo_errors!r}")
    expected = "CARGO_TARGET_DIR raw target override must be classified"
    if not any("scripts/export-name-word.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"export name-word raw-storage drift was silent: {repo_errors!r}")
    expected = "S3 active mutable target cache must be rejected"
    if not any("scripts/s3api.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/s3api-get.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api get-object raw-storage drift was silent: {repo_errors!r}")


def assert_cargo_named_just_recipe_headers_are_not_raw_cargo_commands() -> None:
    verifier = load_verifier()
    errors = verifier.verify_repo_automation_texts(
        {
            "justfile": (
                "cargo-shim-tests:\n"
                "    python3 -m pytest scripts/test_cargo_shim.py -q\n"
            )
        }
    )
    expected = "repo automation raw Cargo must use managed rust_verification wrapper"
    if any(expected in error for error in errors):
        raise AssertionError(f"cargo-named just recipe header was treated as raw cargo: {errors!r}")


def assert_ci_lint_runs_rust_verification_cache_retention_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_rust_verification_cache_retention.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run rust verification cache retention self-tests")


def assert_ci_lint_runs_verify_remote_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_verify_remote.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run remote verification watcher self-tests")


def assert_ci_lint_runs_ci_provenance_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_ci_provenance.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run CI provenance self-tests")


def assert_ci_lint_runs_command_understanding_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_command_understanding.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run command understanding self-tests")


def assert_ci_lint_runs_rust_probe_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_run_rust_probe.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run Rust Probe runner self-tests")


def assert_ci_lint_runs_cancel_obsolete_dispatch_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_cancel_obsolete_dispatch_runs.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run dispatch cancellation self-tests")


def test_ci_test_manifest_self_tests_are_gated() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    if "scripts/test_ci_test_manifest.py" not in justfile:
        raise AssertionError(
            "ci-lint-workflow must invoke scripts/test_ci_test_manifest.py so the "
            "manifest parser's self-tests are gated"
        )


def assert_github_scripts_are_repo_automation_fenced() -> None:
    verifier = load_verifier()
    expected_glob = (verifier.REPO_ROOT / ".github" / "scripts", "*.sh")
    if expected_glob not in verifier.DEFAULT_REPO_AUTOMATION_GLOBS:
        raise AssertionError(".github/scripts/*.sh must be covered by repo automation globs")
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    if ".github/scripts/*.sh" not in justfile:
        raise AssertionError("ci-lint-workflow must scan .github/scripts/*.sh")

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
    assert_clean()
    assert_workflows_clean({"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW})
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
    assert_v6_red_raw_storage_checks_all_ci_automation()
    assert_cargo_named_just_recipe_headers_are_not_raw_cargo_commands()
    assert_v6_red_yaml_anchor_jobs_do_not_hide_raw_storage()
    assert_v6_red_yaml_anchor_steps_do_not_hide_raw_storage()
    assert_v6_red_yaml_steps_aliases_are_rejected()
    assert_v6_red_static_path_classifier_ignores_host_filesystem_resolution()
    assert_v6_red_local_composite_actions_are_scanned()
    assert_v6_red_additional_workflows_are_scanned()
    assert_shell_logical_lines_handles_crlf_continuations()
    assert_workflow_hygiene_reviewer_regressions()
    assert_error("workflow must define PR-only concurrency", without_pr_concurrency(BASE_WORKFLOW))
    assert_error(
        "concurrency group must split noop PR runs from full CI runs",
        replace_once(BASE_WORKFLOW, "format('pr-{0}-noop', github.event.number)", "format('pr-{0}-full', github.event.number)"),
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
            '--expected-event-class "noop"',
        ),
    )
    assert_error(
        "gate shared verdict call must include carry_forward_args=()",
        replace_once(
            BASE_WORKFLOW,
            "carry_forward_args=()",
            "carry_forward_args=(--carry-forward-verified false)",
        ),
    )
    assert_error(
        "gate must verify carry-forward through trusted base-tree ci_provenance.py",
        replace_once(BASE_WORKFLOW, 'python3 "$verdict_script" resolve-gate-carry-forward', 'python3 "$verdict_script" skip-carry-forward'),
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
            'git check-ref-format "refs/heads/$base_branch"',
            "echo skip-base-ref-format-check",
        ),
        (
            'git archive "$base_ref" scripts/ ci/github-actions-runners.toml',
            'git archive "$base_ref" scripts/',
        ),
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
        if marker == "steps.verdict_base.outputs.script":
            mutated_workflow = replace_once_after(mutated_workflow, "  gate:\n", marker, replacement)
        assert_error(
            f"gate must use trusted base-tree ci_provenance.py check-ci-gate verdict ({marker})",
            mutated_workflow,
        )
    assert_error(
        "concurrency group must split deferred PR runs from full CI runs",
        replace_once(BASE_WORKFLOW, "format('pr-{0}-deferred', github.event.number)", "github.ref_name"),
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
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))))
        || github.event_name == 'workflow_dispatch' }}""",
            "cancel-in-progress: true",
        ),
    )
    assert_error(
        "cancel-in-progress must not cancel noop PR runs",
        replace_once(
            BASE_WORKFLOW,
            """cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
             || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != ''))))
        || github.event_name == 'workflow_dispatch' }}""",
            """cancel-in-progress: >-
    ${{ github.event_name == 'pull_request'
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        || github.event_name == 'workflow_dispatch' }}""",
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
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.run_id)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        && format('pr-{0}-deferred', github.event.number)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == false
        && (github.event.action == 'reopened'
            || (github.event.action == 'edited' && !(github.event.changes.base.ref.from != '')))
        && format('pr-{0}-noop', github.event.number)
        || github.event_name == 'pull_request'
        && format('pr-{0}-full', github.event.number)
        || github.event_name == 'workflow_dispatch'
        && github.event.inputs.full_ci == 'true'
        && format('{0}-dispatch-full', github.ref_name)
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
        "workflow_dispatch runs must split full and iteration concurrency groups",
        replace_once(
            BASE_WORKFLOW,
            "        || github.event_name == 'workflow_dispatch'\n"
            "        && github.event.inputs.full_ci == 'true'\n"
            "        && format('{0}-dispatch-full', github.ref_name)\n"
            "        || github.event_name == 'workflow_dispatch'\n"
            "        && format('{0}-dispatch-iteration', github.ref_name)\n",
            "",
        ),
    )
    assert_parse_jobs_strips_comments()
    assert_strip_comment_handles_single_quoted_backslash()
    assert_command_parse_cache_is_transparent()
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
            "      - uses: actions/cache@example\n        if: needs.detector.outputs.build_required != 'true'",
            "      - uses: actions/cache@example",
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
        "check-aarch64 must use setup.outputs.managed_target_dir",
        replace_once(
            BASE_WORKFLOW,
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
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
            "      - uses: actions/cache@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n",
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
        "deny shared Cargo registry/git cache save must be single-owner",
        replace_once(
            BASE_WORKFLOW,
            "          save-if: ${{ github.job == 'test-archive' }}",
            "          save-if: true",
        ),
    )
    assert_error(
        "deny shared Cargo registry/git cache save must be single-owner",
        replace_once(
            BASE_WORKFLOW,
            "          save-if: ${{ github.job == 'test-archive' }}",
            "          cache-comment: |\n            save-if: ${{ github.job == 'test-archive' }}",
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
            """            if ! just test-archive-run "$NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/nextest-archive-extract" --partition "count:${shard}/${shards}"; then
              status=1
            fi""",
            """            just test-archive-run "$NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/nextest-archive-extract" --partition "count:${shard}/${shards}"
            if [[ "${shard}" == "never" ]]; then
              status=1
            fi""",
        ),
    )
    assert_error(
        "test-archive must use only shared Cargo registry/git rust-cache blocks",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive",
            "      - uses: Swatinem/rust-cache@example\n"
            "        with:\n"
            "          cache-targets: true\n"
            "          workspaces: . -> ${{ steps.setup.outputs.managed_target_dir_relative }}\n"
            "          key: nextest-archive-build-v1\n"
            "      - name: Restore nextest archive",
        ),
    )
    assert_error(
        "test-archive must opt into managed target dir",
        replace_once(
            BASE_WORKFLOW,
            '          include-nextest-version: "true"\n'
            '          include-managed-target-dir: "true"\n'
            "      - uses: Swatinem/rust-cache@example",
            '          include-nextest-version: "true"\n'
            "      - uses: Swatinem/rust-cache@example",
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
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}
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
        if: ${{ (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}
""",
            "",
        ),
    )
    assert_error(
        "test-archive must save target cache only on target cache miss",
        replace_once(
            BASE_WORKFLOW,
            "        if: ${{ (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}\n",
            "",
        ),
    )
    assert_error(
        "test-archive managed target cache key must include src/**",
        BASE_WORKFLOW.replace("'src/**', ", ""),
    )
    assert_error(
        "test-archive managed target cache key must include tests/**",
        BASE_WORKFLOW.replace(", 'tests/**'", ""),
    )
    assert_error(
        "test-archive must not save a second archive-build cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive",
            "      - name: Archive build cache key marker\n"
            "        run: echo nextest-archive-build-v1\n"
            "      - name: Restore nextest archive",
        ),
    )
    assert_error(
        "test-archive must restore nextest archive cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Restore nextest archive\n        id: nextest-archive-cache\n        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5\n",
            "",
        ),
    )
    assert_error(
        "test-archive must save nextest archive cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Save nextest archive\n        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'\n        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5\n",
            "",
        ),
    )
    assert_error(
        "nextest archive cache key must use nextest fingerprint output",
        replace_once(
            BASE_WORKFLOW,
            "          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
            "          key: nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock') }}",
        ),
    )
    assert_error(
        "test-archive cache must not use restore-keys",
        replace_once(
            BASE_WORKFLOW,
            "          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}\n      - name: Restore root binary sidecars",
            "          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}\n          restore-keys: nextest-archive-v2-\n      - name: Restore root binary sidecars",
        ),
    )
    # #400: every managed-target cache must declare a restore-keys prefix fallback.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - run: just fmt-check\n      - run: just clippy",
        ),
    )
    assert_error(
        "test-archive managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-\n      - name: Install cargo-nextest",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml', '.config/nextest.toml', 'build.rs', 'gated_source_roots.manifest', 'src/**', 'tests/**') }}\n      - name: Install cargo-nextest",
        ),
    )
    assert_error(
        "check-aarch64 managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-\n      - if: needs.detector.outputs.build_required != 'true'",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - if: needs.detector.outputs.build_required != 'true'",
        ),
    )
    assert_error(
        "source-fence managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-\n      - run: just source-fence",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - run: just source-fence",
        ),
    )
    assert_error(
        "build managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-\n      - name: Install zig",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - name: Install zig",
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
            "      - uses: actions/cache@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
            "      - uses: actions/cache@example\n        name: \"Cache with restore-keys: probe\"\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just fmt-check\n      - run: just clippy",
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
            "      - name: Upload artifact\n        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1",
            "      - name: Upload artifact\n        uses: actions/upload-artifact@v7",
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
            "          key: ${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}",
            "          key: nextest-archive-v2-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('extra-input.txt', 'Cargo.lock') }}",
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
        "test needs nextest-fingerprint",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
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
        replace_once(BASE_WORKFLOW, "needs.test-archive.result", "omitted.test-archive.result"),
    )
    assert_error(
        "test must use always()",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]\n    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' }}",
            "  test:\n    name: test\n    needs: [ci-policy, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]",
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
        "source-fence must run just source-fence",
        replace_once(BASE_WORKFLOW, "- run: just source-fence", "- run: echo source-fence"),
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
            "  ci-provenance-emit:\n    name: ci-provenance-emit\n    needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build]\n    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}",
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
    assert_error(
        "ci-provenance-emit retention-days must match TOML",
        replace_once(
            BASE_WORKFLOW,
            "          name: ci-provenance-attempt-${{ github.run_attempt }}\n          path: ci-provenance.json\n          if-no-files-found: error\n          retention-days: 30",
            "          name: ci-provenance-attempt-${{ github.run_attempt }}\n          path: ci-provenance.json\n          if-no-files-found: error\n          retention-days: 7",
        ),
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
    assert_error(
        "ci.yml bolt-v2-binary retention-days must be 3",
        replace_once(
            BASE_WORKFLOW,
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256\n          retention-days: 3",
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256\n          retention-days: 30",
        ),
    )
    assert_error(
        "ci.yml bolt-v2-binary retention-days must be 3",
        replace_once(
            BASE_WORKFLOW,
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256\n          retention-days: 3",
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256",
        ),
    )
    assert_error(
        "ci.yml bolt-v2-binary retention-days must be 3",
        replace_once(
            BASE_WORKFLOW,
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256\n          retention-days: 3",
            "          name: bolt-v2-binary\n          path: |\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2\n            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256\n          retention-days: 3\n          retention-days: 30",
        ),
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
            "      - run: just source-fence",
            """      - run: |
          cargo install --git https://github.com/nextest-rs/nextest --package cargo-nextest --locked
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
        "clippy must use setup.outputs.managed_target_dir",
        replace_once(
            BASE_WORKFLOW,
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
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
    assert_v6_deploy_artifact_s3_stays_allowed()
    assert_v6_red_workflow_policy_gaps()
    assert_nextest_fingerprint_reuse_adversarial_gaps_are_reported()
    assert_ci_provenance_config_contract()
    assert_runner_contract_rejects_missing_and_extra_jobs()
    assert_runner_contract_rejects_unmapped_workflow_jobs()
    assert_runner_contract_requires_meter_workflows_for_managed_workflows()
    assert_runner_contract_requires_meter_api_limits()
    assert_runner_contract_requires_fingerprint_archive_tier_coupling()
    assert_debug_workflow_rejects_non_manual_trigger()
    assert_debug_workflow_checks_each_ssh_runner_step()
    assert_bootstrap_uses_onepassword_key_generation()
    assert_sync_errors_redact_command_arguments()
    assert_sync_public_key_uses_stdin()
    assert_security_key_public_prefix_is_validated()
    assert_backtester_detect_includes_runner_config()
    assert_backtester_ci_requires_pr_event_types()
    assert_backtester_ci_defers_managed_heavy_on_draft_prs()
    assert_actionlint_rejects_stale_config_variables()
    assert_actionlint_requires_pr_event_types()
    assert_ci_docs_pass_stub_is_absent()
    assert_source_fence_static_ignores_comments()
    assert_local_verification_gate_recipes_are_enforced()
    assert_nextest_fingerprint_reuse_governance_covers_sidecar_helper()
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
    assert_test_archive_sccache_fail_open_contract()
    assert_test_archive_sccache_retry_preserves_compile_failures()
    assert_ci_detector_forces_build_on_workflow_dispatch()
    assert_ci_base_ref_archives_use_scripts_directory()
    assert_ci_detector_docs_only_archive_includes_lane_policy()
    assert_merge_group_support_gaps_are_reported()
    assert_mergify_config_gaps_are_reported()
    assert_ci_policy_heavy_lane_gaps_are_reported()
    assert_gate_policy_truth_table_gaps_are_reported()
    assert_ci_concurrency_split_gaps_are_reported()
    assert_mergify_proof_pr_concurrency_gaps_are_reported()
    assert_mergify_proof_prefix_alignment_holds()
    assert_mergify_proof_prefix_alignment_detects_drift()
    assert_dispatch_cancel_watchdog_gaps_are_reported()
    assert_merge_readiness_progress_gaps_are_reported()
    assert_merge_readiness_finalizer_gaps_are_reported()
    assert_coverage_enforcer_workflow_gaps_are_reported()

    verifier = load_verifier()
    runner_config = REPO_ROOT / "ci" / "github-actions-runners.toml"
    assert runner_config.exists(), "ci/github-actions-runners.toml must exist"
    real_workflows = verifier.repo_workflow_texts()
    runner_errors = verifier.verify_github_actions_runner_contract(real_workflows)
    assert not runner_errors, runner_errors
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
