#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"
GATE_NEEDS = "needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]"
DEPLOY_NEEDS = "needs: [gate, build, detector, fmt-check, deny, clippy, check-aarch64, source-fence, test]"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ci_workflow_hygiene", VERIFIER_PATH)
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

jobs:
  detector:
    name: detector
    runs-on: ubuntu-latest
    steps:
      - run: echo detector

  fmt-check:
    name: fmt-check
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
          toolchain-components: rustfmt
      - run: just fmt-check

  deny:
    name: deny
    needs: detector
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          include-deny-version: "true"
      - uses: Swatinem/rust-cache@example
        with:
          key: deny
      - name: Install cargo-deny
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-deny@${{ steps.setup.outputs.deny_version }}
          fallback: none
      - run: just deny

  clippy:
    name: clippy
    needs: detector
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          toolchain-components: clippy
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          key: clippy
      - run: just clippy

  check-aarch64:
    name: check-aarch64
    needs: detector
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          include-build-values: "true"
          use-default-target: "true"
          include-managed-target-dir: "true"
      - name: Install aarch64 cross compiler
        run: sudo apt-get install -y gcc-aarch64-linux-gnu libc6-dev-arm64-cross
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          key: check-aarch64
      - run: just check-aarch64

  source-fence:
    name: source-fence
    needs: detector
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          key: source-fence
      - run: just source-fence

  test-shards:
    name: nextest shard ${{ matrix.shard }} of 4
    needs: [detector, source-fence]
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3, 4]
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          include-nextest-version: "true"
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          shared-key: nextest-v3
          save-if: ${{ matrix.shard == 1 }}
      - name: Show shard reproduction command
        run: |
          echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"
      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - run: just test -- --partition count:${{ matrix.shard }}/4

  test:
    name: test
    needs: test-shards
    if: ${{ always() }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          if [[ "${{ needs.test-shards.result }}" != "success" ]]; then
            exit 1
          fi

  build:
    name: build
    needs: detector
    if: needs.detector.outputs.build_required == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          include-build-values: "true"
          use-default-target: "true"
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          key: build
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
          cargo-zigbuild --version
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

  gate:
    name: gate
    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]
    if: ${{ always() }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          if [[ "${{ needs.detector.result }}" != "success" ]]; then
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
    needs: [gate, build, detector, fmt-check, deny, clippy, check-aarch64, source-fence, test]
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    steps:
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
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
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
  claude-config-read-token:
    required: true
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
  rust_verification_source_repo:
    value: ${{ steps.shared.outputs.rust_verification_source_repo }}
  rust_verification_source_sha:
    value: ${{ steps.shared.outputs.rust_verification_source_sha }}
  rust_verification_ci_install_script:
    value: ${{ steps.shared.outputs.rust_verification_ci_install_script }}
  managed_target_dir:
    value: ${{ steps.target_dir.outputs.managed_target_dir }}
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
        echo "rust_verification_source_repo=$(just --evaluate rust_verification_source_repo)" >> "$GITHUB_OUTPUT"
        echo "rust_verification_source_sha=$(just --evaluate rust_verification_source_sha)" >> "$GITHUB_OUTPUT"
        echo "rust_verification_ci_install_script=$(just --evaluate rust_verification_ci_install_script)" >> "$GITHUB_OUTPUT"
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
    - name: Install managed Rust owner
      shell: bash
      env:
        CLAUDE_CONFIG_READ_TOKEN: ${{ inputs.claude-config-read-token }}
      run: |
        bash "${{ steps.shared.outputs.rust_verification_ci_install_script }}" "${{ steps.shared.outputs.rust_verification_source_repo }}" "${{ steps.shared.outputs.rust_verification_source_sha }}"
    - name: Resolve managed target dir
      if: ${{ inputs.include-managed-target-dir == 'true' }}
      id: target_dir
      shell: bash
      run: |
        echo "managed_target_dir=$(python3 "${{ steps.shared.outputs.rust_verification_owner }}" target-dir --repo "$GITHUB_WORKSPACE")" >> "$GITHUB_OUTPUT"
    - name: Setup Rust toolchain
      shell: bash
      run: echo setup
"""


BASE_NEXTEST_CONFIG = """
[test-groups]
live-node = { max-threads = 1 }

[[profile.default.overrides]]
filter = 'binary(=bolt_v2) & (test(~bolt_v3_client_registration::tests::) | test(~bolt_v3_live_node::tests::) | test(~platform::runtime::tests::))'
test-group = 'live-node'

[[profile.default.overrides]]
filter = 'binary(=bolt_v3_adapter_mapping) | binary(=bolt_v3_client_registration) | binary(=bolt_v3_controlled_connect) | binary(=bolt_v3_credential_log_suppression) | binary(=bolt_v3_live_canary_gate) | binary(=bolt_v3_readiness) | binary(=bolt_v3_strategy_registration) | binary(=bolt_v3_submit_admission) | binary(=bolt_v3_tiny_canary_operator) | binary(=config_parsing) | binary(=eth_chainlink_taker_runtime) | binary(=lake_batch) | binary(=live_node_run) | binary(=nt_runtime_capture) | binary(=platform_runtime) | binary(=polymarket_bootstrap) | binary(=venue_contract)'
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


def without_inline_need(line: str, job: str) -> str:
    return line.replace(f"{job}, ", "").replace(f", {job}", "")


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
        "missing test(~platform::runtime::tests::)",
        nextest_config=BASE_NEXTEST_CONFIG.replace(
            " | test(~platform::runtime::tests::)",
            "",
        ),
    )


def assert_nextest_live_node_group_covers_bolt_v3_builders() -> None:
    for binary in (
        "bolt_v3_adapter_mapping",
        "bolt_v3_client_registration",
        "bolt_v3_controlled_connect",
        "bolt_v3_credential_log_suppression",
        "bolt_v3_readiness",
        "bolt_v3_strategy_registration",
        "bolt_v3_submit_admission",
        "config_parsing",
    ):
        assert_error(
            f"missing binary(={binary})",
            nextest_config=BASE_NEXTEST_CONFIG.replace(f"binary(={binary}) | ", "").replace(
                f" | binary(={binary})",
                "",
            ),
        )


def main() -> int:
    assert_clean()
    assert_workflows_clean({"ci.yml": BASE_WORKFLOW, "advisory.yml": BASE_ADVISORY_WORKFLOW})
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
        "test-shards",
        "test",
        "build",
        "gate",
        "deploy",
    ):
        assert_error(f"missing required job {job}", without_job(BASE_WORKFLOW, job))
    for job in ("detector", "fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test", "build"):
        assert_error("gate needs " + job, replace_once(BASE_WORKFLOW, GATE_NEEDS, without_inline_need(GATE_NEEDS, job)))
        assert_error(
            f"gate must check needs.{job}.result",
            replace_once(BASE_WORKFLOW, f"needs.{job}.result", f"omitted.{job}.result"),
        )
    for job in ("gate", "build", "detector", "fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test"):
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
        replace_once(BASE_WORKFLOW, "      - run: just check-aarch64", "      - run: echo skip check-aarch64"),
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
            "          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n          key: check-aarch64",
            "          key: check-aarch64",
        ),
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
        "test-shards must run partitioned nextest through just test",
        replace_once(BASE_WORKFLOW, "      - run: just test -- --partition count:${{ matrix.shard }}/4", "      - run: just test"),
    )
    assert_error(
        "test-shards must log shard reproduction command",
        replace_once(
            BASE_WORKFLOW,
            '      - name: Show shard reproduction command\n        run: |\n          echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"\n',
            "",
        ),
    )
    assert_error(
        "test-shards reproduction command must use YAML block scalar",
        replace_once(
            BASE_WORKFLOW,
            '        run: |\n          echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"',
            '        run: echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"',
        ),
    )
    assert_clean(
        replace_once(
            BASE_WORKFLOW,
            '      - name: Show shard reproduction command\n        run: |\n          echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"',
            '      - run: |\n          echo "reproduce locally: just test -- --partition count:${{ matrix.shard }}/4"',
        )
    )
    assert_error(
        "test-shards cache must use shared nextest key",
        replace_once(BASE_WORKFLOW, "          shared-key: nextest-v3", "          key: nextest-v3-shard-${{ matrix.shard }}-of-4"),
    )
    assert_error(
        "test-shards cache must save only from shard 1",
        replace_once(BASE_WORKFLOW, "          save-if: ${{ matrix.shard == 1 }}\n", ""),
    )
    assert_error(
        "test-shards needs source-fence",
        replace_once(
            BASE_WORKFLOW,
            "  test-shards:\n    name: nextest shard ${{ matrix.shard }} of 4\n    needs: [detector, source-fence]",
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
        replace_once(BASE_WORKFLOW, "  test:\n    name: test\n    needs: test-shards\n    if: ${{ always() }}", "  test:\n    name: test\n    needs: test-shards"),
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
    assert_error(
        "test-shards needs source-fence",
        replace_once(
            BASE_WORKFLOW,
            "  test-shards:\n    name: nextest shard ${{ matrix.shard }} of 4\n    needs: [detector, source-fence]",
            "  test-shards:\n    name: nextest shard ${{ matrix.shard }} of 4\n    needs: detector",
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
            "if: needs.detector.outputs.build_required == 'true'",
            "if: needs.detector.outputs.build_required != 'true'",
        ),
    )
    assert_error(
        "build must gate on needs.detector.outputs.build_required",
        replace_once(
            replace_once(BASE_WORKFLOW, "    if: needs.detector.outputs.build_required == 'true'\n", ""),
            "      - uses: ./.github/actions/setup-environment",
            "      - if: needs.detector.outputs.build_required == 'true'\n        uses: ./.github/actions/setup-environment",
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
        "advisory.yml advisories setup token must come from secrets.CLAUDE_CONFIG_READ_TOKEN",
        {
            "ci.yml": BASE_WORKFLOW,
            "advisory.yml": replace_once(
                BASE_ADVISORY_WORKFLOW,
                "claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}",
                "claude-config-read-token: ${{ secrets.OTHER_TOKEN }}",
            ),
        },
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
            "      - run: just test -- --partition count:${{ matrix.shard }}/4",
            """      - run: |
          cargo install --git https://github.com/nextest-rs/nextest --package cargo-nextest --locked
          just test -- --partition count:${{ matrix.shard }}/4""",
        ),
    )
    assert_error(
        "ci.yml test-shards install-action fallback must be none",
        replace_once(
            BASE_WORKFLOW,
            "          fallback: none\n      - run: just test -- --partition count:${{ matrix.shard }}/4",
            "          fallback: cargo-install\n      - run: just test -- --partition count:${{ matrix.shard }}/4",
        ),
    )
    assert_error(
        "ci.yml test-shards must install cargo-nextest before just test",
        replace_once(
            BASE_WORKFLOW,
            """      - name: Install cargo-nextest
        uses: taiki-e/install-action@3771e22aa892e03fd35585fae288baad1755695c
        with:
          tool: cargo-nextest@${{ steps.setup.outputs.nextest_version }}
          fallback: none
      - run: just test -- --partition count:${{ matrix.shard }}/4""",
            """      - run: just test -- --partition count:${{ matrix.shard }}/4
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
          cargo-zigbuild --version""",
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
          cargo-zigbuild --version
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
          cargo-zigbuild --version""",
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
            "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n    if: ${{ always() }}",
            "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n    if: ${{ always() && false }}",
        ),
    )
    assert_error(
        "gate must use always()",
        replace_once(
            replace_once(
                BASE_WORKFLOW,
                "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n    if: ${{ always() }}\n",
                "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n",
            ),
            "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n    runs-on: ubuntu-latest\n    steps:\n      - run: |",
            "  gate:\n    name: gate\n    needs: [detector, fmt-check, deny, clippy, check-aarch64, source-fence, test, build]\n    runs-on: ubuntu-latest\n    steps:\n      - if: ${{ always() }}\n        run: |",
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
        replace_once(BASE_WORKFLOW, "if: startsWith(github.ref, 'refs/tags/v')", "if: ${{ always() }}"),
    )
    assert_error(
        "deploy must be tag-gated",
        replace_once(
            replace_once(BASE_WORKFLOW, "    if: startsWith(github.ref, 'refs/tags/v')\n", ""),
            "      - run: echo deploy",
            "      - if: startsWith(github.ref, 'refs/tags/v')\n        run: echo deploy",
        ),
    )
    assert_error(
        "clippy uses managed target dir but setup does not opt in",
        replace_once(
            BASE_WORKFLOW,
            '          include-managed-target-dir: "true"\n'
            "      - uses: Swatinem/rust-cache@example\n"
            "        with:\n"
            "          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n"
            "          key: clippy",
            "      - uses: Swatinem/rust-cache@example\n"
            "        with:\n"
            "          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n"
            "          key: clippy",
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
        "test-shards must use setup.outputs.managed_target_dir",
        replace_once(
            BASE_WORKFLOW,
            "          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n"
            "          shared-key: nextest-v3\n"
            "          save-if: ${{ matrix.shard == 1 }}",
            "          shared-key: nextest-v3\n"
            "          save-if: ${{ matrix.shard == 1 }}",
        ),
    )
    assert_error(
        "clippy must use setup.outputs.managed_target_dir",
        replace_once(
            BASE_WORKFLOW,
            "          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n          key: clippy",
            "          # cache-directories: ${{ steps.setup.outputs.managed_target_dir }}\n          key: clippy",
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
                "    - name: Lint workflow contract",
                "    - name: Moved lint workflow contract",
            ),
            "    - name: Install managed Rust owner",
            """    - name: Lint workflow contract
      if: ${{ inputs.lint-workflow-contract == 'true' }}
      shell: bash
      run: just ci-lint-workflow
    - name: Install managed Rust owner""",
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
    print("OK: CI workflow hygiene verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
