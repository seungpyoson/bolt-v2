#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import contextlib
import io
import importlib.util
import pathlib
import re
import subprocess
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
SYNC_CI_DEBUG_SSH_PATH = REPO_ROOT / "scripts" / "sync_ci_debug_ssh_secret.py"
DEBUG_WORKFLOW_PATH = ".github/workflows/ci-runner-debug.yml"
SSH_RUNNER_ACTION = "ubicloud/ssh-runner@b6ccad69f047c476b84a54a990f89b1ea5f2a828"
GATE_NEEDS = "needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build, ci-provenance-emit, same-sha-main-evidence]"
DEPLOY_NEEDS = "needs: [gate, same-sha-main-evidence, build, detector, fmt-check, deny, clippy, check-aarch64, source-fence, test]"
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
  "fmt-check",
  "deny",
  "clippy",
  "check-aarch64",
  "source-fence",
  "test-archive",
  "test-shards",
  "test",
]
conditional_jobs = ["build"]
conditional_job_outputs = { build = "detector.build_required" }

[ci_provenance.full_ci.jobs.detector]
check_name = "detector"

[ci_provenance.full_ci.jobs.fmt-check]
check_name = "fmt-check"

[ci_provenance.full_ci.jobs.deny]
check_name = "deny"

[ci_provenance.full_ci.jobs.clippy]
check_name = "clippy"

[ci_provenance.full_ci.jobs.check-aarch64]
check_name = "check-aarch64"

[ci_provenance.full_ci.jobs.source-fence]
check_name = "source-fence"

[ci_provenance.full_ci.jobs.test-archive]
check_name = "nextest archive"

[ci_provenance.full_ci.jobs.test-shards]
check_name_template = "nextest shard {shard} of {shard_count}"
shard_count = 4

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

[ci_provenance.api_limits]
workflow_runs_per_page = 100
run_jobs_per_page = 100
run_artifacts_per_page = 100
max_lookback_pages = 10
max_lookback_age_seconds = 2592000

[ci_provenance.artifacts]
retention_days = 30

[ci_provenance.policy]
draft_pr_synchronize = "defer"
draft_pr_opened = "defer"
draft_pr_reopened = "defer"
converted_to_draft = "defer"
ready_pr = "full"
ready_for_review = "full"
workflow_dispatch = "full"
main_push = "full"
tag = "tag_reuse"
unknown_event = "full"

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

on:
  pull_request:
    branches: [main]
    paths-ignore:
      - 'AGENTS.md'
      - 'CLAUDE.md'
      - 'GEMINI.md'
      - 'REASONIX.md'
      - 'LICENSE'
      - 'SECURITY.md'
      - '.github/ISSUE_TEMPLATE/**'
      - '.claude/**'
      - '.codex/**'
      - '.gemini/**'
      - '.opencode/**'
      - '.pi/**'
      - '.specify/**'
  push:
    branches: [main]
    tags: ["v*"]

concurrency:
  group: >-
    ${{ github.event_name == 'pull_request'
        && format('pr-{0}', github.event.number)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

permissions:
  contents: read
  actions: read

jobs:
  detector:
    name: detector
    runs-on: ubuntu-latest
    steps:
      - run: echo detector

  fmt-check:
    name: fmt-check
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
          toolchain-components: rustfmt
      - run: just fmt-check

  deny:
    name: deny
    needs: detector
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
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
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - run: just deny

  clippy:
    name: clippy
    needs: detector
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          just-version: ${{ env.JUST_VERSION }}
          toolchain-components: clippy
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
      - run: just clippy

  check-aarch64:
    name: check-aarch64
    needs: detector
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
    needs: detector
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
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

  test-archive:
    name: nextest archive
    needs: detector
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    env:
      NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst
    steps:
      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
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
          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
      - name: Install cargo-nextest
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - name: Build nextest archive
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        run: |
          mkdir -p "$(dirname "$NEXTEST_ARCHIVE_PATH")"
          just test-archive "$NEXTEST_ARCHIVE_PATH"
      - name: Save nextest archive
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5
        with:
          path: ${{ env.NEXTEST_ARCHIVE_PATH }}
          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
      - name: Upload nextest archive
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: nextest-archive
          path: ${{ env.NEXTEST_ARCHIVE_PATH }}
          if-no-files-found: error
          retention-days: 1
      - name: Publish nextest archive fingerprint
        run: |
          mkdir -p .nextest-archive-fingerprint
          printf '%s\\n' "nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}" > .nextest-archive-fingerprint/cache-key.txt
      - name: Upload nextest archive fingerprint
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: nextest-archive-fingerprint-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          path: .nextest-archive-fingerprint/cache-key.txt
          if-no-files-found: error
          retention-days: 30

  test-shards:
    name: nextest shard ${{ matrix.shard }} of 4
    needs: test-archive
    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3, 4]
    steps:
      - uses: ./.github/actions/setup-environment
        id: setup
        with:
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          include-managed-target-dir: "true"
      - name: Download nextest archive
        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          name: nextest-archive
          path: ${{ runner.temp }}/nextest-archive
      - name: Resolve archive extraction root
        id: archive-root
        run: |
          archive_extract_root="$(dirname "${{ steps.setup.outputs.managed_target_dir }}")"
          echo "archive_extract_root=$archive_extract_root" >> "$GITHUB_OUTPUT"
      - name: Show shard reproduction command
        run: |
          echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"
      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4

  test:
    name: test
    needs: test-shards
    if: ${{ !startsWith(github.ref, 'refs/tags/v') && always() }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          if [[ "${{ needs.test-shards.result }}" != "success" ]]; then
            exit 1
          fi

  build:
    name: build
    needs: detector
    if: ${{ !startsWith(github.ref, 'refs/tags/v') && needs.detector.outputs.build_required == 'true' }}
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
        run: |
          version="${{ steps.setup.outputs.zigbuild_version }}"
          archive="cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"
          base_url="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${version}"
          curl \\
            --retry 10 \\
            --retry-delay 3 \\
            --retry-all-errors \\
            --fail \\
            --location \\
            --show-error \\
            --silent \\
            --output "$archive" \\
            "$base_url/$archive"
          expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"
          actual="$(sha256sum "$archive" | awk '{print $1}')"
          test "$actual" = "$expected"
          tar --extract --xz --file "$archive"
          mkdir -p "$HOME/.cargo/bin"
          mv cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild "$HOME/.cargo/bin/cargo-zigbuild"
          chmod +x "$HOME/.cargo/bin/cargo-zigbuild"
          test -x "$HOME/.cargo/bin/cargo-zigbuild"
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

  ci-provenance-emit:
    name: ci-provenance-emit
    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test-archive, test-shards, test, build]
    if: ${{ always() && !startsWith(github.ref, 'refs/tags/v') }}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        continue-on-error: true
        with:
          pattern: nextest-archive-fingerprint-*
          path: .ci-provenance/fingerprint
          merge-multiple: true
      - name: Emit CI provenance
        run: >
          python3 scripts/ci_provenance.py emit-full-ci
          --output ci-provenance.json
          --required-job detector=${{ needs.detector.result }}
          --required-job fmt-check=${{ needs.fmt-check.result }}
          --required-job deny=${{ needs.deny.result }}
          --required-job clippy=${{ needs.clippy.result }}
          --required-job check-aarch64=${{ needs.check-aarch64.result }}
          --required-job source-fence=${{ needs.source-fence.result }}
          --required-job test-archive=${{ needs.test-archive.result }}
          --required-job test-shards=${{ needs.test-shards.result }}
          --required-job test=${{ needs.test.result }}
          --conditional-job build.required=${{ needs.detector.outputs.build_required }}
          --conditional-job build.result=${{ needs.build.result }}
          --nextest-fingerprint-path .ci-provenance/fingerprint/cache-key.txt
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
    name: gate
    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build, ci-provenance-emit, same-sha-main-evidence]
    if: ${{ always() }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          tag_ref="${{ startsWith(github.ref, 'refs/tags/v') }}"
          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "$tag_ref" == "true" ]]; then
            if [[ "${{ needs.same-sha-main-evidence.result }}" != "success" ]]; then
              exit 1
            fi
            if [[ "${{ needs.fmt-check.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.clippy.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.check-aarch64.result }}" != "success" ]]; then
              exit 1
            fi
            if [[ "${{ needs.source-fence.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.test.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.build.result }}" != "skipped" ]]; then
              exit 1
            fi
            if [[ "${{ needs.ci-provenance-emit.result }}" != "skipped" ]]; then
              exit 1
            fi
            exit 0
          fi
          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then
            exit 1
          fi
          if [[ "${{ needs.ci-provenance-emit.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.fmt-check.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.deny.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.clippy.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.check-aarch64.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.source-fence.result }}" != "success" ]]; then
            exit 1
          fi
          if [[ "${{ needs.test.result }}" != "success" ]]; then
            exit 1
          fi
          build_required="${{ needs.detector.outputs.build_required }}"
          build_result="${{ needs.build.result }}"
          if [[ "$build_required" == "true" ]]; then
            if [[ "$build_result" != "success" ]]; then
              exit 1
            fi
          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 1
          fi

  deploy:
    name: deploy
    needs: [gate, same-sha-main-evidence, build, detector, fmt-check, deny, clippy, check-aarch64, source-fence, test]
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
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
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
  zigbuild_x86_64_unknown_linux_gnu_sha256:
    value: ${{ steps.shared.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}
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
      shell: bash
      run: echo "${{ inputs.just-version }}"
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
          echo "zigbuild_x86_64_unknown_linux_gnu_sha256=$(just --evaluate zigbuild_x86_64_unknown_linux_gnu_sha256)" >> "$GITHUB_OUTPUT"
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


def repo_workflow_text(path: str) -> str:
    return (REPO_ROOT / path).read_text()


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


def runner_config_load_error(config_text: str) -> str:
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
            "ci_provenance.full_ci.jobs.test-shards shard_count",
            valid.replace("shard_count = 4", "shard_count = 3"),
        ),
        (
            "ci_provenance.full_ci.jobs.test-shards template count",
            valid.replace(
                'check_name_template = "nextest shard {shard} of {shard_count}"',
                'check_name_template = "nextest shard {shard} of 3"',
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
    ]
    for fragment, config_text in cases:
        error = runner_config_load_error(config_text)
        if fragment not in error:
            raise AssertionError(f"expected {fragment!r}, got {error!r}")


def assert_runner_contract_rejects_missing_and_extra_jobs() -> None:
    verifier = load_verifier()
    workflow_name = ".github/workflows/ci.yml"
    workflow = repo_workflow_text(workflow_name)
    renamed = replace_once(workflow, "  fmt-check:\n", "  fmt-renamed:\n")
    errors = verifier.verify_github_actions_runner_contract({workflow_name: renamed})
    if not any("fmt-check" in error and "missing from workflow" in error for error in errors):
        raise AssertionError(f"runner contract must reject TOML job without workflow job, got: {errors}")
    if not any(
        "fmt-renamed" in error and "ci/github-actions-runners.toml" in error
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
                'included_workflows = ["ci", "backtester_ci", "ci_runner_debug"]',
                'included_workflows = ["ci", "ci_runner_debug"]',
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


def assert_source_fence_static_ignores_comments() -> None:
    verifier = load_verifier()
    justfile_text = """
source-fence-static:
    # cargo fetch and scripts/verify_runtime_capture_yaml.py stay in source-fence
    # python3 scripts/rust_verification.py cargo --repo . -- test stays remote-only
    python3 scripts/test_verify_runtime_capture_yaml.py
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
        && format('pr-{0}', github.event.number)
        || format('{0}-{1}', github.ref_name, github.sha) }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

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
    assert_error(
        "nextest config missing live-node test group",
        nextest_config=BASE_NEXTEST_CONFIG.replace("live-node = { max-threads = 1 }", ""),
    )
    assert_error(
        "nextest live-node test group max-threads must be 1",
        nextest_config=BASE_NEXTEST_CONFIG.replace("max-threads = 1", "max-threads = 2"),
    )
    assert_error(
        "nextest config must assign LiveNode test paths to live-node group",
        nextest_config=BASE_NEXTEST_CONFIG.replace("binary(=venue_contract)", "binary(=config_schema)"),
    )
    assert_error(
        "nextest config must assign LiveNode test paths to live-node group",
        nextest_config=BASE_NEXTEST_CONFIG.replace("test-group = 'live-node'", "test-group = 'other'"),
    )
    assert_error(
        "missing test(~bolt_v3_live_node::tests::)",
        nextest_config=BASE_NEXTEST_CONFIG.replace(
            " | test(~bolt_v3_live_node::tests::)",
            "",
        ),
    )


def assert_nextest_live_node_group_covers_bolt_v3_builders() -> None:
    verifier = load_verifier()
    for binary in verifier.LIVE_NODE_NEXTEST_BINARIES:
        assert_error(
            f"missing binary(={binary})",
            nextest_config=BASE_NEXTEST_CONFIG.replace(f"binary(={binary}) | ", "").replace(
                f" | binary(={binary})",
                "",
            ),
        )


# Pin-consistency fixtures. The base SHA already appears throughout BASE_WORKFLOW
# and BASE_ADVISORY_WORKFLOW; SHA_ALT is a different valid 40-hex SHA used to
# exercise drift, and SHA_BASE_UPPER is the base SHA in uppercase to exercise
# normalization.
PIN_CONSISTENCY_SHA_BASE = "3771e22aa892e03fd35585fae288baad1755695c"
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
        "      - run: echo detector",
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


def run_verifier_main_with_no_mistakes(no_mistakes_text: str) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / "scripts" / "verify_ci_workflow_hygiene.py"
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(VERIFIER_PATH.read_text())

        workflow_dir = tmp_path / ".github" / "workflows"
        workflow_dir.mkdir(parents=True)
        (workflow_dir / "ci.yml").write_text(BASE_WORKFLOW)

        action_path = tmp_path / ".github" / "actions" / "setup-environment" / "action.yml"
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)

        (tmp_path / ".no-mistakes.yaml").write_text(no_mistakes_text)
        write_rust_verification_policy_fixtures(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_no_mistakes_entrypoint")
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
        workflow_dir.mkdir(parents=True)
        (workflow_dir / "ci.yml").write_text(BASE_WORKFLOW)

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
        workflow_dir.mkdir(parents=True)
        (workflow_dir / "ci.yml").write_text(BASE_WORKFLOW)
        (workflow_dir / workflow_name).write_text(workflow_text)

        action_path = tmp_path / ".github" / "actions" / "setup-environment" / "action.yml"
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)

        nextest_path = tmp_path / ".config" / "nextest.toml"
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)
        write_rust_verification_policy_fixtures(tmp_path)

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_extra_workflow_entrypoint")
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
    bad = """jobs:
  detect:
    name: bvs-detect
    outputs:
      bvs_changed: ${{ steps.detect.outputs.bvs_changed }}
    steps:
      - id: detect
        run: echo "bvs_changed=false" >> "$GITHUB_OUTPUT"
  gate:
    name: backtester-gate
    needs: [detect, fmt, clippy, test]
    if: ${{ always() }}
    steps:
      - run: |
          if [[ "${{ needs.detect.outputs.bvs_changed }}" != "true" ]]; then
            echo "no crate changes; gate is a no-op"
            exit 0
          fi
"""
    errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": bad})
    assert any("backtester-gate must check needs.detect.result" in error for error in errors), errors
    good = bad.replace(
        '          if [[ "${{ needs.detect.outputs.bvs_changed }}" != "true" ]]; then',
        '          if [[ "${{ needs.detect.result }}" != "success" ]]; then\n'
        '            echo "bvs-detect did not succeed (${{ needs.detect.result }})"\n'
        "            exit 1\n"
        "          fi\n"
        '          if [[ "${{ needs.detect.outputs.bvs_changed }}" != "true" ]]; then',
    )
    good_errors = verifier.verify_repo_automation_texts({".github/workflows/backtester-ci.yml": good})
    assert not [
        error for error in good_errors if "backtester-gate must check needs.detect.result" in error
    ], good_errors


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
            "scripts/raw-runuser.sh": "#!/usr/bin/env bash\nrunuser -u user -- cargo build\n",
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
    if not any("scripts/raw-runuser.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"script runuser raw-cargo drift was silent: {repo_errors!r}")
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


def assert_ci_lint_runs_command_understanding_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_command_understanding.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run command understanding self-tests")


def assert_cargo_zigbuild_probe_has_no_redundant_true() -> None:
    workflow = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    redundant = 'test -x "$HOME/.cargo/bin/cargo-zigbuild" && true'
    if redundant in workflow:
        raise AssertionError("cargo-zigbuild executable probe must not use redundant && true")


def main() -> int:
    assert_ci_lint_runs_rust_verification_cache_retention_tests()
    assert_ci_lint_runs_verify_remote_tests()
    assert_ci_lint_runs_command_understanding_tests()
    assert_cargo_zigbuild_probe_has_no_redundant_true()
    assert_clean()
    assert_workflows_clean({"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW})
    assert_pin_consistency_cross_file_mismatch_errors()
    assert_pin_consistency_same_sha_no_error()
    assert_pin_consistency_rejects_mutable_tag()
    assert_pin_consistency_ignores_non_uses_mentions()
    assert_pin_consistency_accepts_uppercase_sha()
    assert_pin_consistency_intra_file_mismatch_uses_pin_drift_wording()
    assert_pin_consistency_rejects_multi_line_mutable_tag()
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
    assert_error("workflow must define PR-only concurrency", without_pr_concurrency(BASE_WORKFLOW))
    assert_error(
        "concurrency group must key pull_request runs by PR number",
        replace_once(BASE_WORKFLOW, "format('pr-{0}', github.event.number)", "github.ref_name"),
    )
    assert_error(
        "concurrency group must keep non-PR runs isolated by ref and SHA",
        replace_once(BASE_WORKFLOW, "format('{0}-{1}', github.ref_name, github.sha)", "github.ref_name"),
    )
    assert_error(
        "cancel-in-progress must be limited to pull_request events",
        replace_once(
            BASE_WORKFLOW,
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
            "cancel-in-progress: true",
        ),
    )
    assert_error(
        "concurrency group must branch on pull_request event",
        replace_once(
            BASE_WORKFLOW,
            """  group: >-
    ${{ github.event_name == 'pull_request'
        && format('pr-{0}', github.event.number)
        || format('{0}-{1}', github.ref_name, github.sha) }}""",
            "  group: format('pr-{0}', github.event.number)",
        ),
    )
    assert_error(
        "concurrency group must branch on pull_request event",
        replace_once(
            BASE_WORKFLOW,
            "github.event_name == 'pull_request'\n        &&",
            "github.event_name != 'pull_request'\n        &&",
        ),
    )
    assert_error(
        "concurrency group must key pull_request runs by PR number",
        replace_once(
            BASE_WORKFLOW,
            """  group: >-
    ${{ github.event_name == 'pull_request'
        && format('pr-{0}', github.event.number)
        || format('{0}-{1}', github.ref_name, github.sha) }}""",
            """  group: >-
    ${{ github.event_name == 'pull_request'
        && format('{0}-{1}', github.ref_name, github.sha)
        || format('pr-{0}', github.event.number) }}""",
        ),
    )
    assert_parse_jobs_strips_comments()
    assert_strip_comment_handles_single_quoted_backslash()
    assert_required_job_indentation_is_actionable()
    assert_body_exits_requires_top_level_exit()
    assert_nextest_live_node_group_required()
    assert_nextest_live_node_group_covers_bolt_v3_builders()
    for job in (
        "detector",
        "fmt-check",
        "deny",
        "clippy",
        "check-aarch64",
        "source-fence",
        "test-archive",
        "test-shards",
        "test",
        "build",
        "same-sha-main-evidence",
        "gate",
        "deploy",
    ):
        assert_error(f"missing required job {job}", without_job(BASE_WORKFLOW, job))
    for job in ("detector", "fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test", "build"):
        assert_error("gate needs " + job, replace_once(BASE_WORKFLOW, GATE_NEEDS, without_inline_need(GATE_NEEDS, job)))
        if job == "build":
            continue
        if job == "check-aarch64":
            assert_error(
                f"gate must check needs.{job}.result",
                BASE_WORKFLOW.replace(
                    f'"${{{{ needs.{job}.result }}}}" != "success"',
                    f'"${{{{ omitted.{job}.result }}}}" != "success"',
                ),
            )
            continue
        assert_error(
            f"gate must check needs.{job}.result",
            replace_once(
                BASE_WORKFLOW,
                f'"${{{{ needs.{job}.result }}}}" != "success"',
                f'"${{{{ omitted.{job}.result }}}}" != "success"',
            ),
        )
    for job in (
        "gate",
        "same-sha-main-evidence",
        "build",
        "detector",
        "fmt-check",
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
            "  check-aarch64:\n    name: check-aarch64\n    needs: detector",
            "  check-aarch64:\n    name: check-aarch64",
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
        "check-aarch64 must have no job-level if condition",
        replace_once(
            BASE_WORKFLOW,
            "  check-aarch64:\n    name: check-aarch64\n    needs: detector\n    runs-on: ubuntu-latest",
            "  check-aarch64:\n    name: check-aarch64\n    needs: detector\n    if: needs.detector.outputs.build_required != 'true'\n    runs-on: ubuntu-latest",
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
        "test-shards matrix must set fail-fast false",
        replace_once(BASE_WORKFLOW, "      fail-fast: false", "      fail-fast: true"),
    )
    assert_error(
        "test-shards matrix shard must be [1, 2, 3, 4]",
        replace_once(BASE_WORKFLOW, "        shard: [1, 2, 3, 4]", "        shard: [1, 2, 3]"),
    )
    assert_error(
        "test-shards name must describe nextest shard",
        replace_once(
            BASE_WORKFLOW,
            "    name: nextest shard ${{ matrix.shard }} of 4",
            "    name: test (${{ matrix.shard }})",
        ),
    )
    assert_error(
        "test-shards must run partitioned nextest from archive",
        replace_once(
            BASE_WORKFLOW,
            '      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4',
            "      - run: just test",
        ),
    )
    assert_error(
        "test-shards must resolve managed target dir",
        replace_once(
            BASE_WORKFLOW,
            '          include-nextest-version: "true"\n          include-managed-target-dir: "true"\n      - name: Download nextest archive',
            '          include-nextest-version: "true"\n      - name: Download nextest archive',
        ),
    )
    assert_error(
        "test-shards must extract archive to managed target parent",
        replace_once(
            BASE_WORKFLOW,
            '          archive_extract_root="$(dirname "${{ steps.setup.outputs.managed_target_dir }}")"',
            '          archive_extract_root="$RUNNER_TEMP/nextest-archive-extract"',
        ),
    )
    assert_error(
        "test-shards must log shard reproduction command",
        replace_once(
            BASE_WORKFLOW,
            '      - name: Show shard reproduction command\n        run: |\n          echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"\n',
            "",
        ),
    )
    assert_error(
        "test-shards reproduction command must use YAML block scalar",
        replace_once(
            BASE_WORKFLOW,
            '        run: |\n          echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"',
            '        run: echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"',
        ),
    )
    assert_clean(
        replace_once(
            BASE_WORKFLOW,
            '      - name: Show shard reproduction command\n        run: |\n          echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"',
            '      - run: |\n          echo "reproduce locally: just test-archive-run .nextest-archive/nextest-archive.tar.zst <managed-target-parent> --partition count:${{ matrix.shard }}/4"',
        )
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
        "test-archive must not opt into managed target dir",
        replace_once(
            BASE_WORKFLOW,
            '          include-nextest-version: "true"',
            '          include-nextest-version: "true"\n          include-managed-target-dir: "true"',
        ),
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
        "test-archive cache key must include Rust and test graph inputs",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md'",
                "'tests/**'",
            ),
            "'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md'",
            "'tests/**'",
        ),
    )
    assert_error(
        "test-archive cache must not use restore-keys",
        replace_once(
            BASE_WORKFLOW,
            "          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - name: Install cargo-nextest",
            "          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: nextest-archive-v1-\n      - name: Install cargo-nextest",
        ),
    )
    # #400: every managed-target cache must declare a restore-keys prefix fallback.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - run: just clippy",
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
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          restore-keys: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: a restore-keys block-scalar declaring an unrelated
    # cache family prefix must fail the per-job prefix check.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          restore-keys: |\n            nextest-archive-v1-\n      - run: just clippy",
        ),
    )
    # #400 parser tightness: an empty block-scalar body (no prefix line under
    # `restore-keys: |`) must not be treated as a satisfied restore-keys.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          restore-keys: |\n      - run: just clippy",
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
            "          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          restore-keys: |2\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
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
            "      - uses: actions/cache@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "      - uses: actions/cache@example\n        name: \"Cache with restore-keys: probe\"\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
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
        "test-archive must upload nextest archive artifact",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Upload nextest archive\n        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1\n",
            "",
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
        "test-archive must publish nextest archive fingerprint",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Publish nextest archive fingerprint
        run: |
          mkdir -p .nextest-archive-fingerprint
          printf '%s\\n' "nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}" > .nextest-archive-fingerprint/cache-key.txt
      - name: Upload nextest archive fingerprint
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: nextest-archive-fingerprint-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'scripts/command_understanding.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
          path: .nextest-archive-fingerprint/cache-key.txt
          if-no-files-found: error
          retention-days: 30
""",
            "",
        ),
    )
    assert_error(
        "test-archive cache and fingerprint keys must match",
        BASE_WORKFLOW.replace(
            "key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock'",
            "key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('extra-input.txt', 'Cargo.lock'",
        ),
    )
    assert_error(
        "test-shards must download nextest archive artifact",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Download nextest archive\n        uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1\n",
            "",
        ),
    )
    assert_error(
        "test-shards must not restore a per-shard Rust target cache",
        replace_once(
            BASE_WORKFLOW,
            "      - name: Download nextest archive",
            "      - uses: Swatinem/rust-cache@example\n        with:\n          key: nextest-v3-shard-${{ matrix.shard }}-of-4\n      - name: Download nextest archive",
        ),
    )
    assert_error(
        "test-archive needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  test-archive:\n    name: nextest archive\n    needs: detector",
            "  test-archive:\n    name: nextest archive\n    needs: fmt-check",
        ),
    )
    assert_error(
        "test-archive must not need source-fence",
        replace_once(
            BASE_WORKFLOW,
            "  test-archive:\n    name: nextest archive\n    needs: detector",
            "  test-archive:\n    name: nextest archive\n    needs: [detector, source-fence]",
        ),
    )
    assert_error(
        "test-shards needs test-archive",
        replace_once(
            BASE_WORKFLOW,
            "  test-shards:\n    name: nextest shard ${{ matrix.shard }} of 4\n    needs: test-archive",
            "  test-shards:\n    name: nextest shard ${{ matrix.shard }} of 4\n    needs: detector",
        ),
    )
    assert_error(
        "test needs test-shards",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: test-shards",
            "  test:\n    name: test\n    needs: detector",
        ),
    )
    assert_error(
        "test must check needs.test-shards.result",
        replace_once(BASE_WORKFLOW, "needs.test-shards.result", "omitted.test-shards.result"),
    )
    assert_error(
        "test must use always()",
        replace_once(
            BASE_WORKFLOW,
            "  test:\n    name: test\n    needs: test-shards\n    if: ${{ !startsWith(github.ref, 'refs/tags/v') && always() }}",
            "  test:\n    name: test\n    needs: test-shards",
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
        "fmt-check must not need detector",
        replace_once(
            BASE_WORKFLOW,
            "  fmt-check:\n    name: fmt-check",
            "  fmt-check:\n    name: fmt-check\n    needs: detector",
        ),
    )
    assert_error(
        "source-fence needs detector",
        replace_once(
            BASE_WORKFLOW,
            "  source-fence:\n    name: source-fence\n    needs: detector",
            "  source-fence:\n    name: source-fence",
        ),
    )
    assert_error(
        "source-fence must run just source-fence",
        replace_once(BASE_WORKFLOW, "- run: just source-fence", "- run: echo source-fence"),
    )
    for job in ("fmt-check", "deny", "clippy", "source-fence", "test-archive", "test-shards", "test"):
        assert_error(f"{job} must skip on tag reuse", without_job_if(BASE_WORKFLOW, job))
    assert_error(
        "fmt-check must run just fmt-check",
        replace_once(BASE_WORKFLOW, "- run: just fmt-check", "- run: echo skip fmt-check"),
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
        "pull_request paths-ignore must match baseline",
        replace_once(
            BASE_WORKFLOW,
            "      - '.specify/**'\n",
            "      - '.specify/**'\n      - 'docs/**'\n",
        ),
    )
    assert_error(
        "pull_request paths-ignore must match baseline",
        replace_once(BASE_WORKFLOW, "      - '.claude/**'\n", ""),
    )
    assert_error(
        "pull_request paths-ignore must match baseline",
        replace_once(BASE_WORKFLOW, "      - '.specify/**'\n", ""),
    )
    assert_error(
        "pull_request paths-ignore must match baseline",
        replace_once(
            BASE_WORKFLOW,
            "    branches: [main]\n    paths-ignore:\n",
            "    branches: [main]\n    # paths-ignore:\n",
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
            "  build:\n    name: build\n    needs: detector",
            "  build:\n    name: build",
        ),
    )
    assert_error(
        "build must gate on needs.detector.outputs.build_required",
        replace_once(
            BASE_WORKFLOW,
            "if: ${{ !startsWith(github.ref, 'refs/tags/v') && needs.detector.outputs.build_required == 'true' }}",
            "if: ${{ needs.detector.outputs.build_required != 'true' }}",
        ),
    )
    assert_error(
        "build must gate on needs.detector.outputs.build_required",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "    if: ${{ !startsWith(github.ref, 'refs/tags/v') && needs.detector.outputs.build_required == 'true' }}\n",
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
            "    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test-archive, test-shards, test, build]",
            "    needs: [detector, fmt-check, deny, clippy, check-aarch64, test-archive, test-shards, test, build]",
        ),
    )
    assert_error(
        "ci-provenance-emit must use always()",
        replace_once(
            BASE_WORKFLOW,
            "    if: ${{ always() && !startsWith(github.ref, 'refs/tags/v') }}",
            "    if: ${{ !startsWith(github.ref, 'refs/tags/v') }}",
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
        "ci-provenance-emit fingerprint download path must match emitter argument",
        replace_once(
            BASE_WORKFLOW,
            "--nextest-fingerprint-path .ci-provenance/fingerprint/cache-key.txt",
            "--nextest-fingerprint-path .ci-provenance/wrong/cache-key.txt",
        ),
    )
    assert_error(
        "ci-provenance-emit fingerprint download path must match emitter argument",
        replace_once(
            BASE_WORKFLOW,
            "          path: .ci-provenance/fingerprint",
            "          path: .ci-provenance/downloaded",
        ),
    )
    assert_error(
        "ci-provenance-emit fingerprint download path must match emitter argument",
        replace_once(
            BASE_WORKFLOW,
            "          path: .ci-provenance/fingerprint",
            "          path: .ci-provenance/fingerprint-backup",
        ),
    )
    assert_error(
        "ci-provenance-emit fingerprint download path must match emitter argument",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "          path: .ci-provenance/fingerprint",
                "          path: .ci-provenance/downloaded",
            ),
            "          python3 scripts/ci_provenance.py emit-full-ci",
            "          printf '%s\\n' 'path: .ci-provenance/fingerprint'\n"
            "          python3 scripts/ci_provenance.py emit-full-ci",
        ),
    )
    assert_error(
        "ci-provenance-emit must record nextest fingerprint when present",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "          pattern: nextest-archive-fingerprint-*",
                "          pattern: nextest-archive-fingerprint-backup-*",
            ),
            "          python3 scripts/ci_provenance.py emit-full-ci",
            "          printf '%s\\n' 'pattern: nextest-archive-fingerprint-*'\n"
            "          python3 scripts/ci_provenance.py emit-full-ci",
        ),
    )
    assert_error(
        "ci-provenance-emit must record nextest fingerprint when present",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "        continue-on-error: true",
                "        continue-on-error: false",
            ),
            "          python3 scripts/ci_provenance.py emit-full-ci",
            "          printf '%s\\n' 'continue-on-error: true'\n"
            "          python3 scripts/ci_provenance.py emit-full-ci",
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
            '          if [[ "${{ needs.ci-provenance-emit.result }}" != "success" ]]; then\n',
            '          if [[ "${{ needs.ci-provenance-emit.outputs.nextest_fingerprint }}" != "" ]]; then\n',
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
        "gate must check same-sha-main-evidence success",
        replace_once(
            BASE_WORKFLOW,
            '"${{ needs.same-sha-main-evidence.result }}" != "success"',
            '"${{ needs.same-sha-main-evidence.result }}" != "skipped"',
        ),
    )
    assert_error(
        "gate must require build skipped on tag reuse",
        replace_once(
            BASE_WORKFLOW,
            '"${{ needs.build.result }}" != "skipped"',
            '"${{ needs.build.result }}" != "success"',
        ),
    )
    assert_error(
        "gate must check same-sha-main-evidence success",
        replace_once(
            BASE_WORKFLOW,
            '          if [[ "$tag_ref" == "true" ]]; then\n',
            '          if [[ "$tag_ref" == "true" ]]; then\n            exit 0\n',
        ),
    )
    assert_error(
        "gate must check same-sha-main-evidence skip on non-tag",
        replace_once(
            BASE_WORKFLOW,
            '          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then\n',
            '          exit 0\n          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then\n',
        ),
    )
    assert_error(
        "gate must require fmt-check skipped on tag reuse",
        replace_once(
            BASE_WORKFLOW,
            '            if [[ "${{ needs.fmt-check.result }}" != "skipped" ]]; then\n              exit 1\n',
            '            if [[ "${{ needs.fmt-check.result }}" != "skipped" ]]; then\n              true && exit 0\n              exit 1\n',
        ),
    )
    assert_error(
        "gate must require deny skipped on tag reuse",
        replace_once(
            BASE_WORKFLOW,
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              exit 1\n',
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              echo "deny failed" && exit 0\n              exit 1\n',
        ),
    )
    assert_error(
        "gate must require deny skipped on tag reuse",
        replace_once(
            BASE_WORKFLOW,
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              exit 1\n',
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              true || exit 1\n',
        ),
    )
    assert_error(
        "gate must require deny skipped on tag reuse",
        replace_once(
            BASE_WORKFLOW,
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              exit 1\n',
            '            if [[ "${{ needs.deny.result }}" != "skipped" ]]; then\n              echo \\\n              exit 1\n',
        ),
    )
    assert_error(
        "gate must check same-sha-main-evidence skip on non-tag",
        replace_once(
            BASE_WORKFLOW,
            '          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then\n',
            '          true && exit 0\n          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then\n',
        ),
    )
    check_aarch64_condition = '"${{ needs.check-aarch64.result }}" != "success"'
    tag_check = BASE_WORKFLOW.find(check_aarch64_condition)
    standard_check = BASE_WORKFLOW.find(check_aarch64_condition, tag_check + len(check_aarch64_condition))
    if tag_check < 0 or standard_check < 0:
        raise AssertionError("gate check-aarch64 fixture must include tag and standard topology checks")
    assert_error(
        "gate must check needs.check-aarch64.result",
        BASE_WORKFLOW[:standard_check]
        + BASE_WORKFLOW[standard_check:].replace(
            check_aarch64_condition,
            '"${{ omitted.check-aarch64.result }}" != "success"',
            1,
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
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
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
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
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
            "uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c",
            "uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c-suffix",
        ),
    )
    assert_error(
        "ci.yml deny must install cargo-deny before just deny",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-deny
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - run: just deny""",
            """      - run: just deny
      - name: Install cargo-deny
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
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
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none""",
            """      - name: Install cargo-nextest
        if: steps.nextest-archive-cache.outputs.cache-hit != 'true'
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
        "ci.yml test-shards must install cargo-nextest with pinned taiki-e/install-action",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none""",
            """      - name: Install cargo-nextest
        run: |
          cargo install cargo-nextest --version "${{ steps.setup.outputs.nextest_version }}" --locked""",
        ),
    )
    assert_error(
        "ci.yml test-shards must not compile cargo-nextest from source",
        replace_once(
            BASE_WORKFLOW,
            '      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4',
            """      - run: |
          cargo install --git https://github.com/nextest-rs/nextest --package cargo-nextest --locked
          just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4""",
        ),
    )
    assert_error(
        "ci.yml test-shards install-action fallback must be none",
        replace_once(
            BASE_WORKFLOW,
            '          fallback: none\n      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4',
            '          fallback: cargo-install\n      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4',
        ),
    )
    assert_error(
        "ci.yml test-shards must install cargo-nextest before just test-archive-run",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4""",
            """      - run: just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" "${{ steps.archive-root.outputs.archive_extract_root }}" --partition count:${{ matrix.shard }}/4
      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none""",
        ),
    )
    assert_error(
        "ci.yml build must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            """          version="${{ steps.setup.outputs.zigbuild_version }}"
          archive="cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"
          base_url="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${version}"
          curl \\
            --retry 10 \\
            --retry-delay 3 \\
            --retry-all-errors \\
            --fail \\
            --location \\
            --show-error \\
            --silent \\
            --output "$archive" \\
            "$base_url/$archive"
          expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"
          actual="$(sha256sum "$archive" | awk '{print $1}')"
          test "$actual" = "$expected"
          tar --extract --xz --file "$archive"
          mkdir -p "$HOME/.cargo/bin"
          mv cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild "$HOME/.cargo/bin/cargo-zigbuild"
          chmod +x "$HOME/.cargo/bin/cargo-zigbuild"
          test -x "$HOME/.cargo/bin/cargo-zigbuild\"""",
            '          cargo install cargo-zigbuild --version "${{ steps.setup.outputs.zigbuild_version }}" --locked',
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
        "ci.yml fmt-check must not compile cargo-zigbuild from source",
        replace_once(
            BASE_WORKFLOW,
            "      - run: just fmt-check",
            """      - run: |
          cargo install --path vendor/cargo-zigbuild --locked
          just fmt-check""",
        ),
    )
    assert_error(
        "ci.yml build must verify cargo-zigbuild archive checksum",
        replace_once(BASE_WORKFLOW, '          test "$actual" = "$expected"\n', ""),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild from checksum-verified prebuilt release",
        replace_once(
            BASE_WORKFLOW,
            '          test "$actual" = "$expected"\n          tar --extract --xz --file "$archive"',
            '          tar --extract --xz --file "$archive"\n          test "$actual" = "$expected"',
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild from checksum-verified prebuilt release",
        replace_once(
            replace_once(BASE_WORKFLOW, '          test "$actual" = "$expected"\n', ""),
            "      - run: just build",
            '''      - run: |
          just build
          test "$actual" = "$expected"''',
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild from checksum-verified prebuilt release",
        replace_once(BASE_WORKFLOW, "          --retry-all-errors \\\n", ""),
    )
    assert_error(
        "ci.yml build must use pinned cargo-zigbuild archive sha256",
        replace_once(
            BASE_WORKFLOW,
            '          expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"\n',
            """          curl --fail --location --show-error --silent --output "$archive.sha256" "$base_url/$archive.sha256"
          expected="$(awk '{print $1}' "$archive.sha256")"
""",
        ),
    )
    assert_error(
        "ci.yml build must install cargo-zigbuild before just build",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-zigbuild
        run: |
          version="${{ steps.setup.outputs.zigbuild_version }}"
          archive="cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"
          base_url="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${version}"
          curl \\
            --retry 10 \\
            --retry-delay 3 \\
            --retry-all-errors \\
            --fail \\
            --location \\
            --show-error \\
            --silent \\
            --output "$archive" \\
            "$base_url/$archive"
          expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"
          actual="$(sha256sum "$archive" | awk '{print $1}')"
          test "$actual" = "$expected"
          tar --extract --xz --file "$archive"
          mkdir -p "$HOME/.cargo/bin"
          mv cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild "$HOME/.cargo/bin/cargo-zigbuild"
          chmod +x "$HOME/.cargo/bin/cargo-zigbuild"
          test -x "$HOME/.cargo/bin/cargo-zigbuild"
      - run: just build""",
            """      - run: just build
      - name: Install cargo-zigbuild
        run: |
          version="${{ steps.setup.outputs.zigbuild_version }}"
          archive="cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"
          base_url="https://github.com/rust-cross/cargo-zigbuild/releases/download/v${version}"
          curl \\
            --retry 10 \\
            --retry-delay 3 \\
            --retry-all-errors \\
            --fail \\
            --location \\
            --show-error \\
            --silent \\
            --output "$archive" \\
            "$base_url/$archive"
          expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"
          actual="$(sha256sum "$archive" | awk '{print $1}')"
          test "$actual" = "$expected"
          tar --extract --xz --file "$archive"
          mkdir -p "$HOME/.cargo/bin"
          mv cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild "$HOME/.cargo/bin/cargo-zigbuild"
          chmod +x "$HOME/.cargo/bin/cargo-zigbuild"
          test -x "$HOME/.cargo/bin/cargo-zigbuild\"""",
        ),
    )
    assert_workflows_error(
        "advisory.yml advisories must install cargo-deny before just deny-advisories",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                """      - name: Install cargo-deny
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - name: Check advisories
        run: just deny-advisories""",
                """      - name: Check advisories
        run: just deny-advisories
      - name: Install cargo-deny
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
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
            f"  gate:\n    name: gate\n    {GATE_NEEDS}\n    if: ${{{{ always() }}}}",
            f"  gate:\n    name: gate\n    {GATE_NEEDS}\n    if: ${{{{ always() && false }}}}",
        ),
    )
    assert_error(
        "gate must use always()",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                f"  gate:\n    name: gate\n    {GATE_NEEDS}\n    if: ${{{{ always() }}}}\n",
                f"  gate:\n    name: gate\n    {GATE_NEEDS}\n",
            ),
            f"  gate:\n    name: gate\n    {GATE_NEEDS}\n    runs-on: ubuntu-latest\n    steps:\n      - run: |",
            f"  gate:\n    name: gate\n    {GATE_NEEDS}\n    runs-on: ubuntu-latest\n    steps:\n      - if: ${{{{ always() }}}}\n        run: |",
        ),
    )
    assert_error(
        "gate must check needs.detector.result",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            exit 1
          fi
""",
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            echo "detector failed"
          fi
""",
        ),
    )
    assert_error(
        "gate must check needs.detector.result",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            exit 1
          fi
""",
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            exit 0
            exit 1
          fi
""",
        ),
    )
    assert_error(
        "gate must check needs.detector.result",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            exit 1
          fi
""",
            """          if [[ "${{ needs.detector.result }}" != "success" ]]; then
            if [[ "$inner_result" != "success" ]]; then
              exit 1
            fi
          fi
""",
        ),
    )
    assert_error(
        "gate must check needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            """            if [[ "$build_result" != "success" ]]; then
              exit 1
            fi
""",
            """            if [[ "$build_result" != "success" ]]; then
              echo "build failed"
            fi
""",
        ),
    )
    assert_error(
        "gate must check needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            """            if [[ "$build_result" != "success" ]]; then
              exit 1
            fi
""",
            """            if [[ "$build_result" != "success" ]]; then
              exit 0
              exit 1
            fi
""",
        ),
    )
    assert_error(
        "gate must check needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            """          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 1
""",
            """          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            echo "build failed"
""",
        ),
    )
    assert_error(
        "gate must check needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            """          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 1
""",
            """          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 0
            exit 1
""",
        ),
    )
    assert_error(
        "gate must check needs.build.result",
        replace_once(
            BASE_WORKFLOW,
            """          if [[ "$build_required" == "true" ]]; then
            if [[ "$build_result" != "success" ]]; then
              exit 1
            fi
          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 1
          fi
""",
            """          if [[ "$build_required" == "true" ]]; then
            echo "build required"
          fi
          if [[ "$build_result" != "success" ]]; then
            exit 1
          elif [[ "$build_result" != "success" && "$build_result" != "skipped" ]]; then
            exit 1
          fi
""",
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
        "fmt-check opts into managed target dir but does not use it",
        replace_once(
            BASE_WORKFLOW,
            "          toolchain-components: rustfmt",
            '          toolchain-components: rustfmt\n          include-managed-target-dir: "true"',
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
    assert_ci_provenance_config_contract()
    assert_runner_contract_rejects_missing_and_extra_jobs()
    assert_runner_contract_rejects_unmapped_workflow_jobs()
    assert_runner_contract_requires_meter_workflows_for_managed_workflows()
    assert_runner_contract_requires_meter_api_limits()
    assert_debug_workflow_rejects_non_manual_trigger()
    assert_debug_workflow_checks_each_ssh_runner_step()
    assert_bootstrap_uses_onepassword_key_generation()
    assert_sync_errors_redact_command_arguments()
    assert_sync_public_key_uses_stdin()
    assert_security_key_public_prefix_is_validated()
    assert_backtester_detect_includes_runner_config()
    assert_actionlint_rejects_stale_config_variables()
    assert_source_fence_static_ignores_comments()
    assert_rust_verification_policy_parse_errors_are_domain_specific()

    verifier = load_verifier()
    runner_config = REPO_ROOT / "ci" / "github-actions-runners.toml"
    assert runner_config.exists(), "ci/github-actions-runners.toml must exist"
    real_ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text()
    runner_errors = verifier.verify_github_actions_runner_contract(
        {".github/workflows/ci.yml": real_ci}
    )
    assert not runner_errors, runner_errors
    actionlint_errors = verifier.verify_actionlint_runner_contract(
        verifier.repo_workflow_texts()
    )
    assert not actionlint_errors, actionlint_errors

    print("OK: CI workflow hygiene verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
