#!/usr/bin/env python3
"""Shared fixtures for CI workflow hygiene analyzer tests."""

from __future__ import annotations

import ast
import contextlib
import importlib.util
import io
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap

from ci_test_manifest import CiTestManifest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]

VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS = (
    "-c",
    "gc.auto=0",
    "-c",
    "maintenance.auto=false",
)

GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG = (
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
)

DEBUG_TEST_WORKFLOW_PATH = ".github/workflows/debug-test.yml"
GATE_NEEDS = "needs: [ci-policy, detector, deny, clippy, check-aarch64, source-fence, nextest-fingerprint, test-archive, nextest-fingerprint-reuse, test, build, ci-provenance-emit, same-sha-main-evidence]"
GATE_NAME = "name: ${{ needs.ci-policy.outputs.gate_name }}"

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

BASE_WORKFLOW = """
name: CI
run-name: >-
  ${{ github.event_name == 'workflow_dispatch'
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
  merge_group:
    types: [checks_requested]

concurrency:
  group: >-
    ${{ github.event_name == 'pull_request'
        && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
            || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
        && format('pr-{0}-mergify-proof-{1}', github.event.number, github.event.pull_request.head.sha)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == true
        && contains(fromJSON('["opened","synchronize","reopened","converted_to_draft","edited"]'), github.event.action)
        && format('pr-{0}-deferred', github.event.number)
        || github.event_name == 'pull_request'
        && github.event.pull_request.draft == false
        && (github.event.action == 'reopened'
            || (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false)))
        && format('pr-{0}-noop', github.event.number)
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
        && !(github.event.pull_request.draft == false
             && (github.event.action == 'reopened'
                 || (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false))))
        || github.event_name == 'workflow_dispatch' }}

permissions:
  contents: read
  actions: read
  issues: read

env:
  JUST_VERSION: "1.49.0"
  RUST_VERIFICATION_ROOT_BASE: ${{ github.workspace }}/.rust-verification

jobs:
  merge-readiness-progress:
    name: merge-readiness-progress
    if: >-
      ${{ github.event_name == 'pull_request'
          && github.event.pull_request.draft == false
          && (startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/')
              || startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/'))
          && !(github.event.action == 'edited'
               && !(github.event.changes.base.ref.from && true || false)) }}
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
          PR_AUTHOR_ID: ${{ github.event.pull_request.user.id || '' }}
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
          author_args=()
          if python3 "$policy_script" ci-policy --help | grep -q -- "--pull-request-author-id"; then
            author_args=(--pull-request-author-id "$PR_AUTHOR_ID")
          fi
          python3 "$policy_script" ci-policy \
            --config "$policy_config" \
            --event-name "${{ github.event_name }}" \
            --event-action "${{ github.event.action || '' }}" \
            --pull-request-draft "${{ github.event.pull_request.draft || false }}" \
            --pull-request-head-ref "$PR_HEAD_REF" \
            "${author_args[@]}" \
            --pull-request-base-changed "${{ github.event.changes.base.ref.from && true || false }}" \
            --docs-only "${{ needs.detector.outputs.docs_only || 'false' }}" \
            --ref "${{ github.ref }}" \
            | tee -a "$GITHUB_OUTPUT"

      - name: Summarize CI classification
        if: always()
        shell: bash
        env:
          CI_POLICY_PATH: ${{ steps.policy.outputs.ci_policy_path }}
          FULL_CI_REQUIRED: ${{ steps.policy.outputs.full_ci_required }}
          FULL_CI_DEFERRED: ${{ steps.policy.outputs.full_ci_deferred }}
          EXPECTED_EVENT_CLASS: ${{ steps.policy.outputs.expected_event_class }}
          POLICY_REASON: ${{ steps.policy.outputs.reason }}
        run: |
          class="promoted-cheap"
          if [[ "$FULL_CI_REQUIRED" == "true" ]]; then
            class="heavy proof"
          elif [[ "$CI_POLICY_PATH" == "iteration" ]]; then
            class="iteration lane"
          fi
          echo "CI classification: class=${class} policy=${CI_POLICY_PATH:-unknown} full_ci_required=${FULL_CI_REQUIRED:-false} deferred=${FULL_CI_DEFERRED:-false} event_class=${EXPECTED_EVENT_CLASS:-unknown} reason=${POLICY_REASON:-missing}" >> "$GITHUB_STEP_SUMMARY"

  detector:
    name: detector
    outputs:
      build_required: ${{ steps.build_required.outputs.value }}
      fingerprint_reuse_allowed: ${{ steps.fingerprint_reuse_allowed.outputs.value }}
      fingerprint_reuse_reason: ${{ steps.fingerprint_reuse_allowed.outputs.reason }}
      docs_only: ${{ steps.docs_only.outputs.docs_only }}
    runs-on: ubuntu-latest
    steps:
      # detector probe insertion point
      - name: Fetch detector base/head refs
        id: pr_refs
        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'
        shell: bash
        env:
          EVENT_NAME: ${{ github.event_name }}
          PR_NUMBER: ${{ github.event.pull_request.number || github.run_id }}
          PR_BASE_REF: ${{ github.event.pull_request.base.ref || '' }}
          DISPATCH_BASE_REF: ${{ github.event.repository.default_branch }}
          MERGE_GROUP_BASE_REF: ${{ github.event.merge_group.base_ref || '' }}
        run: |
          if [[ "$EVENT_NAME" == "pull_request" ]]; then
            base_branch="$PR_BASE_REF"
            base_ref="refs/remotes/origin/pr-base-${PR_NUMBER}"
            head_ref="refs/remotes/origin/pr-head-${PR_NUMBER}"
            git check-ref-format "refs/heads/$base_branch"
            git fetch --no-tags origin \
              "+refs/heads/${base_branch}:${base_ref}" \
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
        if: github.event_name == 'pull_request' || github.event_name == 'workflow_dispatch' || github.event_name == 'merge_group'
        shell: bash
        run: |
          base_ref="${{ steps.pr_refs.outputs.base_ref }}"
          head_ref="${{ steps.pr_refs.outputs.head_ref }}"
          if [[ "${{ github.event_name }}" == "workflow_dispatch" ]]; then
            diff_range="${base_ref}..${head_ref}"
          else
            diff_range="${base_ref}...${head_ref}"
          fi
          changed="$(git diff --name-only "$diff_range" -- \
            .github/actions/setup-environment/action.yml \
            ci/nextest-fingerprint.toml \
            ci/github-actions-runners.toml \
            scripts/nextest_fingerprint.py \
            scripts/test_nextest_fingerprint.py \
            scripts/root_bin_sidecars.py \
            scripts/test_root_bin_sidecars.py \
            scripts/config_validators.py \
            scripts/ci_provenance.py \
            scripts/test_ci_provenance.py \
            scripts/verify_ci_workflow_hygiene.py \
            scripts/test_verify_ci_workflow_hygiene.py)"
          if [[ -n "$changed" ]]; then
            echo "any_changed=true" >> "$GITHUB_OUTPUT"
          else
            echo "any_changed=false" >> "$GITHUB_OUTPUT"
          fi

      - name: Block self-authorizing governance edits
        id: self_authorizing_governance
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          set -euo pipefail
          base_ref="${{ steps.pr_refs.outputs.base_ref }}"
          head_ref="${{ steps.pr_refs.outputs.head_ref }}"
          if [[ -z "$base_ref" || -z "$head_ref" ]]; then
            echo "self-authorizing governance detector missing PR diff context"
            exit 1
          fi
          changed="$(git diff --name-only "${base_ref}...${head_ref}" -- \
            AGENTS.md \
            .specify/memory/constitution.md \
            .pr_agent.toml \
            ci/ai-review.toml)"
          if [[ -z "$changed" ]]; then
            exit 0
          fi
          base_tree="$RUNNER_TEMP/self-authorizing-governance-base-tree"
          mkdir -p "$base_tree"
          git archive "$base_ref" \
            .github/ \
            .config/ \
            ci/ \
            crates/backtesting-vertical-slice/ci/ \
            scripts/ \
            tests/ \
            AGENTS.md \
            Cargo.toml \
            justfile \
            .mergify.yml \
            .no-mistakes.yaml \
            .pr_agent.toml \
            | tar -x -C "$base_tree"
          python3 "$base_tree/scripts/verify_ci_workflow_hygiene.py" self-authorizing-governance \
            --repo "$GITHUB_WORKSPACE" \
            --base "$base_ref" \
            --head "$head_ref"

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
          if [[ "${{ steps.fingerprint_reuse_inputs_changed.outputs.any_changed }}" == "true" ]]; then
            echo "value=false" >> "$GITHUB_OUTPUT"
            echo "reason=governance-changed" >> "$GITHUB_OUTPUT"
          elif [[ "${{ github.event_name }}" == "pull_request" || "${{ github.event_name }}" == "workflow_dispatch" || "${{ github.event_name }}" == "merge_group" ]]; then
            echo "value=true" >> "$GITHUB_OUTPUT"
            echo "reason=consumer-event" >> "$GITHUB_OUTPUT"
          else
            echo "value=false" >> "$GITHUB_OUTPUT"
            echo "reason=non-consumer-event" >> "$GITHUB_OUTPUT"
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
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
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
          install-rust-linker: "true"
          build-jobs-key: ci.clippy
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Restore clippy managed target cache
        id: clippy-managed-target-cache
        uses: actions/cache/restore@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-
      - run: just fmt-check
      - run: just clippy
      - name: Save clippy managed target cache
        id: clippy-managed-target-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.clippy-managed-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}

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
          build-jobs-key: ci.check-aarch64
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
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Restore check-aarch64 managed target cache
        id: check-aarch64-managed-target-cache
        uses: actions/cache/restore@example
        if: needs.detector.outputs.build_required != 'true'
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-
      - if: needs.detector.outputs.build_required != 'true'
        run: just check-aarch64
      - name: Save check-aarch64 managed target cache
        id: check-aarch64-managed-target-cache-save
        if: ${{ needs.detector.outputs.build_required != 'true' && github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.check-aarch64-managed-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}

  source-fence:
    name: source-fence
    needs: [ci-policy, detector]
    if: ${{ needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs' }}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@example
        with:
          ref: ${{ needs.ci-policy.outputs.ci_policy_path == 'docs' && github.event.pull_request.head.sha || github.sha }}
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-managed-target-dir: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.source-fence
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Restore source-fence managed target cache
        id: source-fence-managed-target-cache
        uses: actions/cache/restore@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}
          restore-keys: |
            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-
      - run: |
          if [[ "${{ needs.ci-policy.outputs.full_ci_required }}" == "true" ]]; then
            just source-fence
          else
            just source-fence-static
          fi
      - name: Save source-fence managed target cache
        id: source-fence-managed-target-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.source-fence-managed-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}

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
        id: upload-nextest-fingerprint
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ${{ steps.nextest-fingerprint.outputs.nextest_fingerprint_artifact_name }}
          path: .nextest-archive-fingerprint/cache-key.txt
          if-no-files-found: error
          retention-days: 14

  nextest-fingerprint-reuse:
    name: nextest fingerprint reuse
    needs: [ci-policy, detector, nextest-fingerprint]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && contains(fromJSON('["pull_request","workflow_dispatch","merge_group"]'), github.event_name) && needs.detector.outputs.fingerprint_reuse_allowed == 'true' && github.ref != 'refs/heads/main' }}
    runs-on: ubuntu-latest
    outputs:
      reuse_found: ${{ steps.reuse.outputs.reuse_found }}
      source_run_id: ${{ steps.reuse.outputs.source_run_id }}
      source_sha: ${{ steps.reuse.outputs.source_sha }}
      source_artifact_id: ${{ steps.reuse.outputs.source_artifact_id }}
      root_run_id: ${{ steps.reuse.outputs.root_run_id }}
      root_head_sha: ${{ steps.reuse.outputs.root_head_sha }}
      root_fingerprint_digest: ${{ steps.reuse.outputs.root_fingerprint_digest }}
      reason: ${{ steps.reuse.outputs.reason }}
    steps:
      - name: Prepare trusted base provenance emitter
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

      - name: Resolve nextest fingerprint reuse
        id: reuse
        shell: bash
        env:
          GITHUB_TOKEN: ${{ github.token }}
        run: |
          required_emitter="scripts/ci_provenance.py"
          trusted_base_emitter="${{ steps.reuse_provenance_base.outputs.script }}"
          if [[ -n "$trusted_base_emitter" ]]; then
            required_emitter="$trusted_base_emitter"
          fi
          python3 scripts/ci_provenance.py resolve-fingerprint \
            --current-run-id "${{ github.run_id }}" \
            --current-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}" \
            --require-inherited-emitter "$required_emitter" \
            | tee -a "$GITHUB_OUTPUT"

  test-archive:
    name: nextest archive
    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && needs.detector.result == 'success' && needs.nextest-fingerprint.result == 'success' && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}
    runs-on: ubuntu-latest
    outputs:
      nextest_archive_cache_key: ${{ steps.root-nextest-cache-keys.outputs.nextest_archive_cache_key }}
      root_bin_sidecars_cache_key: ${{ steps.root-nextest-cache-keys.outputs.root_bin_sidecars_cache_key }}
      archive_build_target_cache_key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}
      nextest_archive_cache_hit: ${{ steps.nextest-archive-cache.outcome == 'skipped' && 'skipped' || (steps.nextest-archive-cache.outputs.cache-hit || 'false') }}
      root_bin_sidecars_cache_hit: ${{ steps.root-bin-sidecars-cache.outcome == 'skipped' && 'skipped' || (steps.root-bin-sidecars-cache.outputs.cache-hit || 'false') }}
      archive_build_target_cache_hit: ${{ steps.test-target-cache.outcome == 'skipped' && 'skipped' || (steps.test-target-cache.outputs.cache-hit || 'false') }}
      nextest_archive_cache_save_outcome: ${{ steps.nextest-archive-cache-save.outputs.save-status || (steps.nextest-archive-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}
      root_bin_sidecars_cache_save_outcome: ${{ steps.root-bin-sidecars-cache-save.outputs.save-status || (steps.root-bin-sidecars-cache-save.outcome == 'skipped' && 'skipped' || 'failed') }}
      archive_build_target_cache_save_outcome: ${{ steps.test-target-cache-save.outcome }}
    env:
      NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst
      ROOT_BIN_SIDECARS_PATH: .nextest-archive/root-bin-sidecars.tar.gz
      NEXTEST_ARTIFACT_CACHE_ENABLED: ${{ vars.CI_NEXTEST_ARCHIVE_S3_ENABLED }}
      NEXTEST_ARTIFACT_CACHE_BUCKET: ${{ vars.CI_SCCACHE_BUCKET }}
      NEXTEST_ARTIFACT_CACHE_REGION: ${{ vars.CI_SCCACHE_REGION }}
      NEXTEST_ARTIFACT_CACHE_KEY_PREFIX: ${{ vars.CI_NEXTEST_ARCHIVE_S3_KEY_PREFIX }}
    steps:
      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          include-managed-target-dir: "true"
          install-rust-linker: "true"
          build-jobs-key: ci.test-archive
      - name: Resolve root nextest cache keys
        id: root-nextest-cache-keys
        shell: bash
        run: |
          {
            echo "nextest_archive_cache_key=${{ needs.nextest-fingerprint.outputs.nextest_archive_prefix }}v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-shards-${{ needs.nextest-fingerprint.outputs.nextest_shards }}-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
            echo "root_bin_sidecars_cache_key=root-bin-sidecars-v${{ needs.nextest-fingerprint.outputs.nextest_schema }}-${{ runner.os }}-${{ runner.arch }}-${{ needs.nextest-fingerprint.outputs.nextest_profile }}-profile-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
            echo "archive_build_target_cache_key=managed-target-v1-${{ runner.os }}-${{ runner.arch }}-test-archive-test-${{ needs.nextest-fingerprint.outputs.nextest_digest }}"
          } >> "$GITHUB_OUTPUT"
      - name: Resolve nextest artifact cache eligibility
        id: nextest-artifact-cache
        continue-on-error: true
        run: |
          echo "eligible=true" >> "$GITHUB_OUTPUT"
          echo "role_arn=$ROLE_ARN" >> "$GITHUB_OUTPUT"
          echo "cache_mode=read_write" >> "$GITHUB_OUTPUT"
      - name: Configure AWS credentials for nextest artifact cache
        id: nextest-artifact-cache-aws
        if: steps.nextest-artifact-cache.outputs.eligible == 'true'
        continue-on-error: true
        uses: aws-actions/configure-aws-credentials@example
        with:
          role-to-assume: ${{ steps.nextest-artifact-cache.outputs.role_arn }}
          aws-region: ${{ vars.CI_SCCACHE_REGION }}
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Restore nextest archive from S3
        id: nextest-archive-cache
        if: steps.nextest-artifact-cache.outputs.eligible == 'true' && steps.nextest-artifact-cache-aws.outcome == 'success'
        env:
          CACHE_KEY: ${{ steps.root-nextest-cache-keys.outputs.nextest_archive_cache_key }}
          DIGEST: ${{ needs.nextest-fingerprint.outputs.nextest_digest }}
        run: |
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/nextest-archive/${CACHE_KEY}.tar.zst"
          metadata_digest="$(aws s3api head-object --bucket "$NEXTEST_ARTIFACT_CACHE_BUCKET" --key "$object_key" --query 'Metadata."nextest-digest"' --output text 2>/dev/null)"
          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::nextest archive S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
          aws s3 cp "s3://${NEXTEST_ARTIFACT_CACHE_BUCKET}/${object_key}" "$NEXTEST_ARCHIVE_PATH"
          echo "cache-hit=false" >> "$GITHUB_OUTPUT"
          echo "restore-result=miss" >> "$GITHUB_OUTPUT"
          echo "restore-reason=fixture-miss" >> "$GITHUB_OUTPUT"
          exit 0
      - name: Restore root binary sidecars from S3
        id: root-bin-sidecars-cache
        if: steps.nextest-artifact-cache.outputs.eligible == 'true' && steps.nextest-artifact-cache-aws.outcome == 'success'
        env:
          CACHE_KEY: ${{ steps.root-nextest-cache-keys.outputs.root_bin_sidecars_cache_key }}
          DIGEST: ${{ needs.nextest-fingerprint.outputs.nextest_digest }}
        run: |
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/root-bin-sidecars/${CACHE_KEY}.tar.gz"
          metadata_digest="$(aws s3api head-object --bucket "$NEXTEST_ARTIFACT_CACHE_BUCKET" --key "$object_key" --query 'Metadata."nextest-digest"' --output text 2>/dev/null)"
          if [[ "$metadata_digest" != "$DIGEST" ]]; then
            echo "::error::root binary sidecar S3 object ${object_key} has missing or mismatched nextest-digest metadata; expected ${DIGEST}, got ${metadata_digest:-<empty>}. Delete the object or repopulate it from a main push."
            exit 1
          fi
          aws s3 cp "s3://${NEXTEST_ARTIFACT_CACHE_BUCKET}/${object_key}" "$ROOT_BIN_SIDECARS_PATH"
          echo "cache-hit=false" >> "$GITHUB_OUTPUT"
          echo "restore-result=miss" >> "$GITHUB_OUTPUT"
          echo "restore-reason=fixture-miss" >> "$GITHUB_OUTPUT"
          exit 0
      - name: Summarize nextest archive S3 state
        if: always()
        shell: bash
        env:
          S3_ELIGIBLE: ${{ steps.nextest-artifact-cache.outputs.eligible || 'false' }}
          S3_CACHE_MODE: ${{ steps.nextest-artifact-cache.outputs.cache_mode || 'none' }}
          S3_AWS_OUTCOME: ${{ steps.nextest-artifact-cache-aws.outcome }}
          NEXTEST_RESTORE_OUTCOME: ${{ steps.nextest-archive-cache.outcome }}
          NEXTEST_RESTORE_HIT: ${{ steps.nextest-archive-cache.outputs.cache-hit || '' }}
          NEXTEST_RESTORE_RESULT: ${{ steps.nextest-archive-cache.outputs.restore-result || '' }}
          NEXTEST_RESTORE_REASON: ${{ steps.nextest-archive-cache.outputs.restore-reason || '' }}
          SIDECAR_RESTORE_OUTCOME: ${{ steps.root-bin-sidecars-cache.outcome }}
          SIDECAR_RESTORE_HIT: ${{ steps.root-bin-sidecars-cache.outputs.cache-hit || '' }}
          SIDECAR_RESTORE_RESULT: ${{ steps.root-bin-sidecars-cache.outputs.restore-result || '' }}
          SIDECAR_RESTORE_REASON: ${{ steps.root-bin-sidecars-cache.outputs.restore-reason || '' }}
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
          archive_restore="$(restore_state "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$NEXTEST_RESTORE_OUTCOME" "$NEXTEST_RESTORE_RESULT" "$NEXTEST_RESTORE_HIT")"
          archive_reason="$(restore_reason "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$NEXTEST_RESTORE_OUTCOME" "$NEXTEST_RESTORE_REASON")"
          sidecar_restore="$(restore_state "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$SIDECAR_RESTORE_OUTCOME" "$SIDECAR_RESTORE_RESULT" "$SIDECAR_RESTORE_HIT")"
          sidecar_reason="$(restore_reason "$S3_ELIGIBLE" "$S3_AWS_OUTCOME" "$SIDECAR_RESTORE_OUTCOME" "$SIDECAR_RESTORE_REASON")"
          {
            echo "Root nextest archive S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${archive_restore} reason=${archive_reason}"
            echo "Root binary sidecars S3: eligible=${S3_ELIGIBLE:-false} mode=${S3_CACHE_MODE:-none} aws=${S3_AWS_OUTCOME:-skipped} restore=${sidecar_restore} reason=${sidecar_reason}"
          } >> "$GITHUB_STEP_SUMMARY"
      - name: Restore archive build target cache
        id: test-target-cache
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true'
        uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}
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
      - name: Save nextest archive to S3
        id: nextest-archive-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.nextest-artifact-cache-aws.outcome == 'success' && steps.nextest-archive-cache.outputs.cache-hit != 'true' }}
        continue-on-error: true
        env:
          CACHE_KEY: ${{ steps.root-nextest-cache-keys.outputs.nextest_archive_cache_key }}
        run: |
          if [[ ! -s "$NEXTEST_ARCHIVE_PATH" ]]; then
            echo "save-status=skipped" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/nextest-archive/${CACHE_KEY}.tar.zst"
          if aws s3 cp "$NEXTEST_ARCHIVE_PATH" "s3://${NEXTEST_ARTIFACT_CACHE_BUCKET}/${object_key}"; then
            echo "save-status=success" >> "$GITHUB_OUTPUT"
          else
            echo "save-status=failed" >> "$GITHUB_OUTPUT"
            exit 1
          fi
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
      - name: Save root binary sidecars to S3
        id: root-bin-sidecars-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.nextest-artifact-cache.outputs.cache_mode == 'read_write' && steps.nextest-artifact-cache-aws.outcome == 'success' && steps.root-bin-sidecars-cache.outputs.cache-hit != 'true' }}
        continue-on-error: true
        env:
          CACHE_KEY: ${{ steps.root-nextest-cache-keys.outputs.root_bin_sidecars_cache_key }}
        run: |
          if [[ ! -s "$ROOT_BIN_SIDECARS_PATH" ]]; then
            echo "save-status=skipped" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          object_key="${NEXTEST_ARTIFACT_CACHE_KEY_PREFIX%/}/root-bin-sidecars/${CACHE_KEY}.tar.gz"
          if aws s3 cp "$ROOT_BIN_SIDECARS_PATH" "s3://${NEXTEST_ARTIFACT_CACHE_BUCKET}/${object_key}"; then
            echo "save-status=success" >> "$GITHUB_OUTPUT"
          else
            echo "save-status=failed" >> "$GITHUB_OUTPUT"
            exit 1
          fi
      - name: Save archive build target cache
        id: test-target-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && (steps.nextest-archive-cache.outputs.cache-hit != 'true' || steps.root-bin-sidecars-cache.outputs.cache-hit != 'true') && steps.test-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: ${{ steps.root-nextest-cache-keys.outputs.archive_build_target_cache_key }}
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
            partition_log="$RUNNER_TEMP/nextest-archive-partition-${shard}.log"
            set +e
            just test-archive-run "$NEXTEST_ARCHIVE_PATH" "$RUNNER_TEMP/nextest-archive-extract" --partition "count:${shard}/${shards}" 2>&1 | tee "$partition_log"
            rc="${PIPESTATUS[0]}"
            set -e
            if [[ "$rc" -ne 0 ]]; then
              status=1
              echo "::error title=nextest archive partition failed::shard=${shard}/${shards} exit=${rc}"
              echo "last relevant log lines for nextest archive partition ${shard}/${shards}:"
              tail -80 "$partition_log"
            fi
            echo "::endgroup::"
          done
          exit "$status"

  cache-persistence-audit:
    name: cache persistence audit
    needs: [ci-policy, nextest-fingerprint-reuse, test-archive]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' && needs.test-archive.result == 'success' && needs.nextest-fingerprint-reuse.outputs.reuse_found != 'true' }}
    runs-on: ubuntu-latest
    permissions:
      contents: read
      actions: read
    steps:
      - uses: actions/checkout@example
      - name: Probe saved cache keys
        continue-on-error: true
        shell: bash
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          python3 scripts/ci_storage_audit.py \
            --repo "$GITHUB_REPOSITORY" \
            --github-event-name "$GITHUB_EVENT_NAME" \
            --github-ref "$GITHUB_REF" \
            --github-base-ref "$GITHUB_BASE_REF" \
            --github-default-branch "${{ github.event.repository.default_branch }}" \
            --github-step-summary "$GITHUB_STEP_SUMMARY" \
            --github-annotations \
            --restore-hit "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_hit }}" \
            --restore-hit "root binary sidecars=${{ needs.test-archive.outputs.root_bin_sidecars_cache_hit }}" \
            --restore-hit "archive build target=${{ needs.test-archive.outputs.archive_build_target_cache_hit }}" \
            --save-outcome "nextest archive=${{ needs.test-archive.outputs.nextest_archive_cache_save_outcome }}" \
            --save-outcome "root binary sidecars=${{ needs.test-archive.outputs.root_bin_sidecars_cache_save_outcome }}" \
            --save-outcome "archive build target=${{ needs.test-archive.outputs.archive_build_target_cache_save_outcome }}" \
            --cache-key "archive-build-target=${{ needs.test-archive.outputs.archive_build_target_cache_key }}"

  test:
    name: test
    needs: [ci-policy, detector, nextest-fingerprint, nextest-fingerprint-reuse, test-archive]
    if: ${{ always() && needs.ci-policy.outputs.full_ci_required == 'true' }}
    runs-on: ubuntu-latest
    steps:
      - env:
          DETECTOR_ALLOWED: ${{ needs.detector.outputs.fingerprint_reuse_allowed || 'false' }}
          DETECTOR_REASON: ${{ needs.detector.outputs.fingerprint_reuse_reason || 'unknown' }}
          REUSE_FOUND: ${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || 'false' }}
          REUSE_SOURCE_RUN: ${{ needs.nextest-fingerprint-reuse.outputs.source_run_id || 'none' }}
          REUSE_SOURCE_SHA: ${{ needs.nextest-fingerprint-reuse.outputs.source_sha || 'none' }}
          REUSE_ARTIFACT: ${{ needs.nextest-fingerprint-reuse.outputs.source_artifact_id || 'none' }}
          REUSE_REASON: ${{ needs.nextest-fingerprint-reuse.outputs.reason || '' }}
        run: |
          detector_allowed="${DETECTOR_ALLOWED:-false}"
          detector_reason="${DETECTOR_REASON:-unknown}"
          reuse_found="${REUSE_FOUND:-false}"
          source_run="${REUSE_SOURCE_RUN:-none}"
          source_sha="${REUSE_SOURCE_SHA:-none}"
          artifact="${REUSE_ARTIFACT:-none}"
          reason="${REUSE_REASON:-}"
          decision="not-applicable"
          if [[ "$detector_allowed" == "true" ]]; then
            if [[ "$reuse_found" == "true" ]]; then
              decision="allowed"
            else
              decision="refused"
            fi
            [[ -n "$reason" ]] || reason="no-reusable-fingerprint"
          elif [[ "$detector_reason" == "governance-changed" ]]; then
            decision="refused"
            reason="$detector_reason"
          else
            reason="$detector_reason"
          fi
          echo "Nextest reuse: decision=${decision} detector_allowed=${detector_allowed} reuse_found=${reuse_found} source_run=${source_run:-none} source_sha=${source_sha:-none} artifact=${artifact:-none} reason=${reason:-none}" >> "$GITHUB_STEP_SUMMARY"
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
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.root_run_id }}" ]]; then
              echo "nextest fingerprint reuse did not expose root_run_id"
              exit 1
            fi
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.root_head_sha }}" ]]; then
              echo "nextest fingerprint reuse did not expose root_head_sha"
              exit 1
            fi
            if [[ -z "${{ needs.nextest-fingerprint-reuse.outputs.root_fingerprint_digest }}" ]]; then
              echo "nextest fingerprint reuse did not expose root_fingerprint_digest"
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
          install-rust-linker: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-on-failure: true
          cache-bin: false
          cache-targets: false
          shared-key: cargo-registry-git-v1
          save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && github.job == 'test-archive' }}
      - name: Restore build managed target cache
        id: build-managed-target-cache
        uses: actions/cache/restore@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}
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
      - name: Save build managed target cache
        id: build-managed-target-cache-save
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' && steps.build-managed-target-cache.outputs.cache-hit != 'true' }}
        uses: actions/cache/save@example
        with:
          path: ${{ steps.setup.outputs.managed_target_dir }}
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml') }}
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
        id: upload-bolt-v2-binary
        if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}
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
    if: ${{ always() && (needs.ci-policy.outputs.full_ci_required == 'true' || needs.ci-policy.outputs.ci_policy_path == 'docs') }}
    runs-on: ubuntu-latest
    steps:
      - name: Prepare trusted base provenance tree
        id: provenance_base
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
          reuse_found="${{ needs.nextest-fingerprint-reuse.outputs.reuse_found || 'false' }}"
          if [[ "$reuse_found" == "true" ]]; then
            if ! python3 "$provenance_script" emit-inherited-ci --help >/dev/null; then
              echo "trusted base provenance emitter does not support inherited CI records" >&2
              exit 1
            fi
            python3 "$provenance_script" emit-inherited-ci \
              --config "$provenance_config" \
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
              --nextest-fingerprint "${{ needs.nextest-fingerprint.outputs.nextest_fingerprint }}" \
              --root-run-id "${{ needs.nextest-fingerprint-reuse.outputs.root_run_id }}" \
              --root-head-sha "${{ needs.nextest-fingerprint-reuse.outputs.root_head_sha }}" \
              --root-fingerprint-digest "${{ needs.nextest-fingerprint-reuse.outputs.root_fingerprint_digest }}"
          else
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
          fi
      - name: Upload CI provenance
        id: upload-ci-provenance
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: ci-provenance-attempt-${{ github.run_attempt }}
          path: ci-provenance.json
          if-no-files-found: error
          retention-days: 14

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
  install-rust-linker:
    description: Whether to install the configured Rust fast linker.
    required: false
    default: "false"
  build-jobs-key:
    required: false
    default: ""
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
  cargo_build_jobs:
    value: ${{ steps.shared.outputs.cargo_build_jobs }}
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
      env:
        BUILD_JOBS_KEY: ${{ inputs.build-jobs-key }}
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
        if [ -n "$BUILD_JOBS_KEY" ]; then
          cargo_build_jobs="$(
            python3 - ci/github-actions-runners.toml "$BUILD_JOBS_KEY" <<'PY'
        import pathlib
        import sys
        import tomllib

        config = tomllib.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
        value = config.get("cargo_build_jobs")
        for part in sys.argv[2].split("."):
            if not isinstance(value, dict) or part not in value:
                raise SystemExit(f"cargo_build_jobs.{sys.argv[2]} missing")
            value = value.get(part)
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise SystemExit(f"cargo_build_jobs.{sys.argv[2]} must be a positive integer")
        print(value)
        PY
          )"
          echo "cargo_build_jobs=$cargo_build_jobs" >> "$GITHUB_OUTPUT"
          echo "CARGO_BUILD_JOBS=$cargo_build_jobs" >> "$GITHUB_ENV"
        fi
    - name: Install Rust linker
      if: ${{ inputs.install-rust-linker == 'true' }}
      shell: bash
      run: |
        mapfile -t rust_linker_programs < <(python3.12 "${{ steps.shared.outputs.rust_verification_owner }}" fast-linker-programs --repo "$GITHUB_WORKSPACE")
        if [ "${#rust_linker_programs[@]}" -eq 0 ]; then
          echo "::error::remote_fast_linker has no configured programs"
          exit 1
        fi
        for rust_linker_program in "${rust_linker_programs[@]}"; do
          if command -v "$rust_linker_program" >/dev/null; then
            echo "BOLT_RUST_FAST_LINKER=$rust_linker_program" >> "$GITHUB_ENV"
            exit 0
          fi
        done
        if sudo apt-get update; then
          for rust_linker_program in "${rust_linker_programs[@]}"; do
            if sudo apt-get install -y --no-install-recommends "$rust_linker_program"; then
              echo "BOLT_RUST_FAST_LINKER=$rust_linker_program" >> "$GITHUB_ENV"
              exit 0
            fi
          done
        fi
        echo "::warning::failed to install any configured Rust linker; continuing without fast linker"
        echo "Rust linker: unavailable; continuing without BOLT_RUST_FAST_LINKER" >> "$GITHUB_STEP_SUMMARY"
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
filter = 'binary(=bolt_v3_adapter_mapping) | binary(=bolt_v3_client_registration) | binary(=bolt_v3_controlled_connect) | binary(=bolt_v3_credential_log_suppression) | binary(=bolt_v3_readiness) | binary(=bolt_v3_strategy_registration) | binary(=bolt_v3_submit_admission) | binary(=chainlink_startup_boot) | binary(=config_parsing) | binary(=lake_batch) | binary(=nt_runtime_capture) | binary(=venue_contract)'
test-group = 'live-node'
"""

def all_standalone_live_node_manifest(verifier=None) -> CiTestManifest:
    if verifier is None:
        verifier = load_verifier()
    member_to_harness = {member: member for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    harness_to_members = {member: (member,) for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)

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
    "chainlink_startup_boot",
    "bolt_v3_polymarket_venue_truth",
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

[remote_compile_cache]
enabled = true
enable_env = "BOLT_RUST_VERIFICATION_SCCACHE"
ci_env = "GITHUB_ACTIONS"
wrapper_env = "SCCACHE_PATH"
wrapper_program = "sccache"

[remote_fast_linker]
enabled = true
ci_env = "GITHUB_ACTIONS"
linker_env = "BOLT_RUST_FAST_LINKER"
programs = ["mold", "lld"]

{LOCAL_LANE_POLICY_TOML}
"""

def write_rust_verification_policy_fixtures(root: pathlib.Path) -> None:
    root_policy = root / "ci" / "rust-verification.toml"
    root_policy.parent.mkdir(parents=True, exist_ok=True)
    root_policy.write_text(BASE_RUST_VERIFICATION_POLICY, encoding="utf-8")
    bvs_policy = root / "crates" / "backtesting-vertical-slice" / "ci" / "rust-verification.toml"
    bvs_policy.parent.mkdir(parents=True, exist_ok=True)
    bvs_policy.write_text(BASE_BVS_RUST_VERIFICATION_POLICY, encoding="utf-8")

def write_runner_config_fixture(root: pathlib.Path) -> None:
    runner_config = root / "ci" / "github-actions-runners.toml"
    runner_config.parent.mkdir(parents=True, exist_ok=True)
    runner_config.write_text((REPO_ROOT / "ci" / "github-actions-runners.toml").read_text(), encoding="utf-8")
    capture_config = root / "ci" / "chainlink-reference-fixture-capture-provenance.toml"
    capture_config.write_text(
        (REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml").read_text(),
        encoding="utf-8",
    )
    actionlint = root / ".github" / "actionlint.yaml"
    actionlint.parent.mkdir(parents=True, exist_ok=True)
    actionlint.write_text((REPO_ROOT / ".github" / "actionlint.yaml").read_text(), encoding="utf-8")

def write_temp_runner_config(root: pathlib.Path, config_text: str) -> pathlib.Path:
    ci_dir = root / "ci"
    ci_dir.mkdir(parents=True, exist_ok=True)
    config_path = ci_dir / "github-actions-runners.toml"
    config_path.write_text(config_text, encoding="utf-8")
    (ci_dir / "chainlink-reference-fixture-capture-provenance.toml").write_text(
        (REPO_ROOT / "ci" / "chainlink-reference-fixture-capture-provenance.toml").read_text(),
        encoding="utf-8",
    )
    return config_path

def write_repo_text(repo: pathlib.Path, relative: str, text: str) -> None:
    path = repo / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")

def repo_git_command(*args: str) -> list[str]:
    return ["git", *GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS, *args]

def run_repo_git(repo: pathlib.Path, *args: str) -> str:
    completed = subprocess.run(
        repo_git_command(*args),
        cwd=repo,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return completed.stdout

def suppress_repo_auto_maintenance(repo: pathlib.Path) -> None:
    """Persist the suppression inside `repo`'s own config.

    `repo_git_command` only reaches the git processes this suite launches. Git
    drops the repo-scoped config environment when it runs a command against a
    *different* repository, so `git push` never carries `-c gc.auto=0` into the
    remote's `receive-pack`, and git spawned by the code under test never sees
    it either. Both then detach a writer into a fixture directory the test is
    about to delete. A fixture repo that carries the setting in its own config
    is covered whoever runs git against it, bare or not.
    """
    for key, value in GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
        subprocess.run(
            repo_git_command("-C", str(repo), "config", key, value),
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

def init_fixture_repo(repo: pathlib.Path, *init_args: str) -> pathlib.Path:
    """`git init` a fixture repo that never spawns auto-maintenance."""
    subprocess.run(
        repo_git_command("init", *init_args, str(repo)),
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    suppress_repo_auto_maintenance(repo)
    return repo

def commit_repo(repo: pathlib.Path, message: str) -> str:
    run_repo_git(repo, "add", ".")
    run_repo_git(
        repo,
        "-c",
        "user.name=CI Test",
        "-c",
        "user.email=ci-test@example.invalid",
        "commit",
        "-m",
        message,
    )
    return run_repo_git(repo, "rev-parse", "HEAD").strip()

def init_self_authorizing_fixture_repo(tmp: pathlib.Path) -> pathlib.Path:
    repo = tmp / "repo"
    repo.mkdir()
    run_repo_git(repo, "init", "--initial-branch", "main")
    for relative in (
        "AGENTS.md",
        ".specify/memory/constitution.md",
        ".pr_agent.toml",
        "ci/ai-review.toml",
    ):
        write_repo_text(repo, relative, "SSM is the only secret source.\n")
    write_repo_text(
        repo,
        ".github/workflows/ci.yml",
        "name: CI\npermissions:\n  contents: read\n",
    )
    commit_repo(repo, "base")
    return repo

def copy_self_authorizing_base_tree(destination: pathlib.Path) -> pathlib.Path:
    base_tree = destination / "base-tree"
    shutil.copytree(REPO_ROOT / "scripts", base_tree / "scripts")
    (base_tree / "ci").mkdir()
    shutil.copy2(REPO_ROOT / "ci" / "rust-verification.toml", base_tree / "ci" / "rust-verification.toml")
    return base_tree

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

def yaml_scalar_literal(value: object) -> str:
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    return str(value)

def repo_source_text(path: str | pathlib.Path) -> str:
    source_path = pathlib.Path(path)
    if not source_path.is_absolute():
        source_path = REPO_ROOT / source_path
    return source_path.read_text().replace("\r\n", "\n")

def repo_workflow_text(path: str) -> str:
    return repo_source_text(path)

def inline_matrix_values(job_lines: list[str], matrix_key: str) -> tuple[int, ...]:
    prefix = f"        {matrix_key}: "
    values = [line.removeprefix(prefix) for line in job_lines if line.startswith(prefix)]
    if len(values) != 1:
        raise AssertionError(f"expected exactly one {matrix_key!r} matrix entry, found {len(values)}")
    parsed = ast.literal_eval(values[0])
    if not isinstance(parsed, list) or not all(isinstance(value, int) for value in parsed):
        raise AssertionError(f"{matrix_key!r} matrix entry must be an inline integer list")
    return tuple(parsed)

def assert_no_inline_matrix_key(job_lines: list[str], matrix_key: str) -> None:
    prefix = f"        {matrix_key}: "
    if any(line.startswith(prefix) for line in job_lines):
        raise AssertionError(f"unexpected {matrix_key!r} matrix entry")

def assert_error(
    fragment: str,
    workflow: str = BASE_WORKFLOW,
    action: str = BASE_ACTION,
    nextest_config: str = BASE_NEXTEST_CONFIG,
) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action, nextest_config)
    if "ROOT_TEST_ARCHIVE_JOB_SHA256" not in fragment:
        errors = [error for error in errors if "ROOT_TEST_ARCHIVE_JOB_SHA256" not in error]
    if not any(fragment in error for error in errors):
        raise AssertionError(f"expected error containing {fragment!r}, got: {errors}")

def shard_partition_argument_denominators(job_lines: list[str]) -> tuple[int, ...]:
    return tuple(
        int(denominator)
        for denominator in re.findall(
            r"(?m)^\s*(?:BOLT_RUST_VERIFICATION_SCCACHE=0\s+)?just bte-test\b[^\n]*\s--partition\s+\"count:\${{\s*matrix\.shard\s*}}/([1-9][0-9]*)\"\s+--(?:\s|$)",
            "\n".join(job_lines),
        )
    )

def ci_provenance_config_fixture() -> str:
    return (REPO_ROOT / "ci" / "github-actions-runners.toml").read_text()

def runner_config_load_error(config_text: str, verifier=None) -> str:
    if verifier is None:
        verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        config_path = write_temp_runner_config(pathlib.Path(tmp), config_text)
        try:
            verifier.load_github_actions_runners_config(config_path)
        except Exception as exc:  # noqa: BLE001 - loader raises domain errors.
            return str(exc)
    return ""

def without_inline_need(line: str, job: str) -> str:
    return line.replace(f"{job}, ", "").replace(f", {job}", "")

def workflow_with_detector_probe(script: str) -> str:
    return replace_once(
        BASE_WORKFLOW,
        "      # detector probe insertion point",
        "      - name: V6 raw Rust storage policy probe\n        run: |\n"
        + textwrap.indent(script.strip(), "          "),
    )

def write_repo_workflows(workflow_dir: pathlib.Path) -> None:
    workflow_dir.mkdir(parents=True)
    for path in sorted((REPO_ROOT / ".github" / "workflows").glob("*.y*ml")):
        (workflow_dir / path.name).write_text(path.read_text(encoding="utf-8"), encoding="utf-8")

def write_storage_tripwire_policy_fixture(root: pathlib.Path) -> pathlib.Path:
    policy_path = root / "ci" / "storage-tripwire.toml"
    policy_path.parent.mkdir(parents=True, exist_ok=True)
    policy_path.write_text((REPO_ROOT / "ci" / "storage-tripwire.toml").read_text(encoding="utf-8"), encoding="utf-8")
    return policy_path

def run_verifier_main_with_no_mistakes(
    no_mistakes_text: str,
    *,
    write_mergify_config: bool = True,
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
        sccache_action_path = tmp_path / ".github" / "actions" / "sccache-setup" / "action.yml"
        sccache_action_path.parent.mkdir(parents=True)
        sccache_action_path.write_text(repo_source_text(".github/actions/sccache-setup/action.yml"))
        sccache_stats_action_path = tmp_path / ".github" / "actions" / "sccache-stats" / "action.yml"
        sccache_stats_action_path.parent.mkdir(parents=True)
        sccache_stats_action_path.write_text(repo_source_text(".github/actions/sccache-stats/action.yml"))
        sccache_eligibility_path = tmp_path / "scripts" / "sccache_eligibility.py"
        sccache_eligibility_path.write_text(repo_source_text("scripts/sccache_eligibility.py"))
        sccache_config_path = tmp_path / "ci" / "sccache-location.toml"
        sccache_config_path.parent.mkdir(parents=True, exist_ok=True)
        sccache_config_path.write_text(repo_source_text("ci/sccache-location.toml"))

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)

        (tmp_path / ".no-mistakes.yaml").write_text(no_mistakes_text)
        if write_mergify_config:
            (tmp_path / ".mergify.yml").write_text((REPO_ROOT / ".mergify.yml").read_text())
        write_rust_verification_policy_fixtures(tmp_path)
        write_runner_config_fixture(tmp_path)
        storage_policy_path = write_storage_tripwire_policy_fixture(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_no_mistakes_entrypoint")
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
