#!/usr/bin/env python3
"""Self-tests for the CI workflow hygiene verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py"


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
          include-build-values: "true"
          include-managed-target-dir: "true"
      - uses: Swatinem/rust-cache@example
        with:
          cache-directories: ${{ steps.setup.outputs.managed_target_dir }}
          key: clippy
      - run: just clippy

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

  test:
    name: test
    needs: [detector, source-fence]
    runs-on: ubuntu-latest
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
          key: nextest
      - run: just test

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
      - run: just build

  gate:
    name: gate
    needs: [detector, fmt-check, deny, clippy, source-fence, test, build]
    if: ${{ always() }}
    runs-on: ubuntu-latest
    steps:
      - run: |
          echo "${{ needs.detector.result }}"
          echo "${{ needs.fmt-check.result }}"
          echo "${{ needs.deny.result }}"
          echo "${{ needs.clippy.result }}"
          echo "${{ needs.source-fence.result }}"
          echo "${{ needs.test.result }}"
          echo "${{ needs.build.result }}"
          echo "${{ needs.detector.outputs.build_required }}"

  deploy:
    name: deploy
    needs: [gate, build, detector, fmt-check, deny, clippy, source-fence, test]
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    steps:
      - run: echo deploy
"""


BASE_ACTION = """
name: Setup Environment
inputs:
  include-managed-target-dir:
    description: Whether to resolve the managed target dir.
    required: false
    default: "false"
outputs:
  managed_target_dir:
    value: ${{ steps.target_dir.outputs.managed_target_dir }}
runs:
  using: composite
  steps:
    - name: Resolve managed target dir
      if: ${{ inputs.include-managed-target-dir == 'true' }}
      id: target_dir
      shell: bash
      run: echo "managed_target_dir=/tmp/target" >> "$GITHUB_OUTPUT"
"""


def assert_clean(workflow: str = BASE_WORKFLOW, action: str = BASE_ACTION) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action)
    if errors:
        raise AssertionError(f"expected no errors, got: {errors}")


def assert_error(fragment: str, workflow: str = BASE_WORKFLOW, action: str = BASE_ACTION) -> None:
    verifier = load_verifier()
    errors = verifier.verify_text(workflow, action)
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


def main() -> int:
    assert_clean()
    assert_error("missing required job source-fence", without_job(BASE_WORKFLOW, "source-fence"))
    assert_error(
        "gate needs source-fence",
        replace_once(
            BASE_WORKFLOW,
            "needs: [detector, fmt-check, deny, clippy, source-fence, test, build]",
            "needs: [detector, fmt-check, deny, clippy, test, build]",
        ),
    )
    assert_error(
        "gate must check needs.source-fence.result",
        replace_once(BASE_WORKFLOW, 'echo "${{ needs.source-fence.result }}"', 'echo "source-fence omitted"'),
    )
    assert_error(
        "deploy needs source-fence",
        replace_once(
            BASE_WORKFLOW,
            "needs: [gate, build, detector, fmt-check, deny, clippy, source-fence, test]",
            "needs: [gate, build, detector, fmt-check, deny, clippy, test]",
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
        "fmt-check opts into managed target dir but does not use it",
        replace_once(
            BASE_WORKFLOW,
            "          toolchain-components: rustfmt",
            '          toolchain-components: rustfmt\n          include-managed-target-dir: "true"',
        ),
    )
    assert_error(
        "setup action target dir step must be conditional",
        action=BASE_ACTION.replace("      if: ${{ inputs.include-managed-target-dir == 'true' }}\n", ""),
    )
    print("OK: CI workflow hygiene verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
