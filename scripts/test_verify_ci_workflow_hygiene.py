#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import contextlib
import io
import importlib.util
import pathlib
import re
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
GATE_NEEDS = "needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build, same-sha-main-evidence]"
DEPLOY_NEEDS = "needs: [gate, same-sha-main-evidence, build, detector, fmt-check, deny, clippy, check-aarch64, source-fence, test]"
EXACT_HEAD_GOVERNANCE_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
)


def load_verifier(
    path: pathlib.Path = VERIFIER_PATH, module_name: str = "verify_ci_workflow_hygiene"
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load verify_ci_workflow_hygiene.py")
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
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
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
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
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
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
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
          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
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
          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
      - name: Upload nextest archive
        uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: nextest-archive
          path: ${{ env.NEXTEST_ARCHIVE_PATH }}
          if-no-files-found: error
          retention-days: 1

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
          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}
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
          test -x "$HOME/.cargo/bin/cargo-zigbuild" && true
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
        uses: actions/upload-artifact@example
        with:
          name: bolt-v2-binary
          path: |
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2
            ${{ steps.managed_artifact.outputs.stage_dir }}/bolt-v2.sha256

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
    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build, same-sha-main-evidence]
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
            exit 0
          fi
          if [[ "${{ needs.same-sha-main-evidence.result }}" != "skipped" ]]; then
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
filter = 'binary(=bolt_v3_adapter_mapping) | binary(=bolt_v3_client_registration) | binary(=bolt_v3_controlled_connect) | binary(=bolt_v3_credential_log_suppression) | binary(=bolt_v3_live_canary_gate) | binary(=bolt_v3_readiness) | binary(=bolt_v3_strategy_registration) | binary(=bolt_v3_submit_admission) | binary(=bolt_v3_tiny_canary_operator) | binary(=config_parsing) | binary(=lake_batch) | binary(=nt_runtime_capture) | binary(=venue_contract)'
test-group = 'live-node'
"""


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
        "active target copied through neutral staging path": """
            mkdir /tmp/deploy
            cp -r target/debug /tmp/deploy/
            aws s3 sync /tmp/deploy s3://bolt-v2-active-cache/cache
        """,
        "active target streamed through s3 stdin": """
            tar -czf - target | aws s3 cp - s3://bolt-v2-active-cache/target.tar.gz
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
            "/tmp/builder build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "exec -a name cargo build --target-dir /tmp/raw-target",
            "cargo --target-dir raw target override must be classified",
        ),
        (
            "docker run --rm -v $PWD:/workspace -w /workspace rust:latest cargo build --target-dir /tmp/raw-target",
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
            'aws s3 sync "$(echo target)" s3://bolt-v2-active-cache/target',
            "S3 active mutable target cache must be rejected",
        ),
        (
            'export E=CARGO_TARGET_DIR; env FOO=bar bash -c "$E=/tmp/raw cargo check"',
            "CARGO_TARGET_DIR raw target override must be classified",
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

        temp_verifier = load_verifier(verifier_path, "verify_ci_workflow_hygiene_no_mistakes_entrypoint")
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = temp_verifier.main()
        return result, stdout.getvalue() + stderr.getvalue()


def assert_v6_red_no_mistakes_raw_cargo_is_reported() -> None:
    raw_fixture = """
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
  envblocksignal: env --block-signal cargo test
  anchored: &raw "cargo build --target-dir /tmp/raw"
  anchoralias: *raw
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
  rustup: rustup run stable cargo test
  pyinline: python -c 'import os; os.system("cargo test")'
  timeout: timeout 30 cargo test
  managedjustenv: BOLT_MANAGED_JUST=1 just managed-build
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
  no-mistakes-clippy-command: no-mistakes run -- clippy
  no-mistakes-nextest-command: no-mistakes run -- nextest run
  docs: just docs
"""
    allowed_fixture = """
commands:
  test: python3 scripts/rust_verification.py cargo --repo . -- test
  lint: python3 scripts/rust_verification.py cargo --repo . -- clippy --all-targets -- -D warnings
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
        "envblocksignal",
        "anchored",
        "anchoralias",
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
        "rustup",
        "pyinline",
        "timeout",
        "managedjustenv",
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
    fixture_result, fixture_errors = run_verifier_main_with_no_mistakes(raw_fixture)
    missing_fixture = [fragment for fragment in expected if fragment not in fixture_errors]
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
            f"false_fixture={false_fixture} fixture_errors={fixture_errors!r} "
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
    ]
    failures: list[str] = []
    for check in checks:
        try:
            check()
        except AssertionError as exc:
            failures.append(f"{check.__name__}: {exc}")
    if failures:
        raise AssertionError("v6 RED workflow policy coverage failures: " + " | ".join(failures))


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
            "scripts/symlink-cargo.sh": "ln -s $(which cargo) /tmp/mycargo\n/tmp/mycargo build --target-dir /tmp/raw\n",
            "scripts/copy-cargo.sh": "cp $(which cargo) /tmp/mycargo\n/tmp/mycargo build\n",
            "justfile.setup": "setup:\n    cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.absolute": "setup:\n    /usr/bin/cargo install cargo-nextest --version 0.9.132 --locked\n",
            "justfile.setup.timeout": "setup:\n    timeout 30 cargo install cargo-deny --version 0.18.2\n",
            "justfile.setup.xargs": "setup:\n    xargs cargo install cargo-nextest\n",
            "scripts/local.sh": "aws s3 sync \"$PWD\"/target s3://some-bucket/linux-cache\n",
            "scripts/workspace.sh": "aws s3 sync \"$GITHUB_WORKSPACE\" s3://some-bucket/workspace\n",
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
    if not any("scripts/symlink-cargo.sh" in error and "cargo --target-dir raw target override" in error for error in repo_errors):
        raise AssertionError(f"symlinked cargo raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/copy-cargo.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"copied cargo raw-cargo drift was silent: {repo_errors!r}")
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
    if not any("scripts/s3api.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api raw-storage drift was silent: {repo_errors!r}")
    if not any("scripts/s3api-get.sh" in error and expected in error for error in repo_errors):
        raise AssertionError(f"s3api get-object raw-storage drift was silent: {repo_errors!r}")


def assert_ci_lint_runs_rust_verification_cache_retention_tests() -> None:
    justfile = (REPO_ROOT / "justfile").read_text(encoding="utf-8")
    expected = "python3 scripts/test_rust_verification_cache_retention.py"
    if expected not in justfile:
        raise AssertionError("ci-lint-workflow must run rust verification cache retention self-tests")


def main() -> int:
    assert_ci_lint_runs_rust_verification_cache_retention_tests()
    assert_clean()
    assert_workflows_clean({"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW})
    assert_pin_consistency_cross_file_mismatch_errors()
    assert_pin_consistency_same_sha_no_error()
    assert_pin_consistency_rejects_mutable_tag()
    assert_pin_consistency_accepts_uppercase_sha()
    assert_pin_consistency_intra_file_mismatch_uses_pin_drift_wording()
    assert_pin_consistency_rejects_multi_line_mutable_tag()
    assert_pin_consistency_rejects_multi_line_valid_sha()
    assert_pin_consistency_accepts_double_quoted_sha()
    assert_pin_consistency_accepts_single_quoted_sha()
    assert_pin_consistency_rejects_mismatched_quotes()
    assert_prebuilt_tool_installs_accepts_uppercase_pinned_install_action()
    assert_v6_red_raw_storage_checks_all_ci_automation()
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
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
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
            "      - uses: actions/cache@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n",
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
            "          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - name: Install cargo-nextest",
            "          key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', '.config/nextest.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', 'build.rs', 'src/**', 'tests/**', 'benches/**', 'examples/**', 'crates/**', 'specs/**/*.md', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: nextest-archive-v1-\n      - name: Install cargo-nextest",
        ),
    )
    # #400: every managed-target cache must declare a restore-keys prefix fallback.
    assert_error(
        "clippy managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - run: just clippy",
        ),
    )
    assert_error(
        "check-aarch64 managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-\n      - if: needs.detector.outputs.build_required != 'true'",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-check-aarch64-dev-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - if: needs.detector.outputs.build_required != 'true'",
        ),
    )
    assert_error(
        "source-fence managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-\n      - run: just source-fence",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-source-fence-test-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - run: just source-fence",
        ),
    )
    assert_error(
        "build managed target cache must declare restore-keys prefix managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-",
        replace_once(
            BASE_WORKFLOW,
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-\n      - name: Install zig",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-build-aarch64-release-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n      - name: Install zig",
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
            "      - uses: actions/cache@example\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
            "      - uses: actions/cache@example\n        name: \"Cache with restore-keys: probe\"\n        with:\n          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}\n          restore-keys: |\n            managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-\n      - run: just clippy",
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
          test -x "$HOME/.cargo/bin/cargo-zigbuild" && true""",
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
          test -x "$HOME/.cargo/bin/cargo-zigbuild" && true
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
          test -x "$HOME/.cargo/bin/cargo-zigbuild" && true""",
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
            "          path: ${{ steps.setup.outputs.managed_target_dir }}\n          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
            "          key: managed-target-v1-${{ runner.os }}-${{ runner.arch }}-clippy-host-${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml', '.cargo/config.toml', 'ci/rust-verification.toml', 'scripts/rust_verification.py', 'justfile', '.github/workflows/ci.yml', '.github/actions/setup-environment/action.yml', '.no-mistakes.yaml') }}",
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
    print("OK: CI workflow hygiene verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
