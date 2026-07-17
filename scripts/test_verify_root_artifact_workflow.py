#!/usr/bin/env python3
"""Mutation tests for the root-artifact workflow source fence."""

from __future__ import annotations

import importlib.util
import pathlib
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = pathlib.Path(__file__).with_name("verify_root_artifact_workflow.py")
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/root-artifact.yml"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_root_artifact_workflow", VERIFIER_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError(f"could not load {VERIFIER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def replace_once(text: str, old: str, new: str) -> str:
    if text.count(old) != 1:
        raise AssertionError(f"mutation anchor must occur once: {old!r}")
    return text.replace(old, new, 1)


def assert_mutation_rejected(verifier, baseline: str, label: str, old: str, new: str, expected: str) -> None:
    errors = verifier.root_artifact_workflow_errors(replace_once(baseline, old, new))
    if not any(expected in error for error in errors):
        raise AssertionError(f"{label} mutation was not rejected with {expected!r}: {errors}")


def main() -> int:
    verifier = load_verifier()
    baseline = WORKFLOW_PATH.read_text(encoding="utf-8")
    baseline_errors = verifier.root_artifact_workflow_errors(baseline)
    if baseline_errors:
        raise AssertionError(f"real root-artifact workflow must pass: {baseline_errors}")

    cases = (
        (
            "automatic trigger",
            "on:\n  workflow_dispatch:",
            "on:\n  push:\n  workflow_dispatch:",
            "dispatch-only",
        ),
        (
            "second producer",
            "  produce:\n",
            "  shadow-producer:\n    runs-on: ubuntu-latest\n    steps: []\n\n  produce:\n",
            "exactly two jobs",
        ),
        (
            "second Cargo build",
            "            build --locked --profile \"$BUILD_PROFILE\" --target \"$BUILD_TARGET\" --bin bolt-v2",
            "            build --locked --profile \"$BUILD_PROFILE\" --target \"$BUILD_TARGET\" --bin bolt-v2\n          python3 scripts/rust_verification.py cargo --repo \"$GITHUB_WORKSPACE\" -- build --locked",
            "exactly one governed Cargo invocation",
        ),
        (
            "hidden Cargo test",
            "            build --locked --profile \"$BUILD_PROFILE\"",
            "            test --locked --profile \"$BUILD_PROFILE\"",
            "must not run Cargo tests",
        ),
        (
            "hidden nextest",
            "          set -euo pipefail\n          [[ \"$BOLT_RUST_VERIFICATION_SCCACHE\" == \"1\" ]]",
            "          set -euo pipefail\n          cargo nextest run\n          [[ \"$BOLT_RUST_VERIFICATION_SCCACHE\" == \"1\" ]]",
            "must not run Cargo tests",
        ),
        (
            "wrapper requirement removal",
            "          [[ \"$ENABLED\" == \"true\" ]]\n",
            "",
            "must require enabled sccache",
        ),
        (
            "wrapper opt-in removal",
            "          [[ \"$BOLT_RUST_VERIFICATION_SCCACHE\" == \"1\" ]]\n",
            "",
            "must require the governed wrapper opt-in",
        ),
        (
            "retry",
            "      - name: Build once through mandatory sccache\n",
            "      - name: Build once through mandatory sccache\n        retry: 2\n",
            "must not retry",
        ),
        (
            "result fallback",
            "      - name: Stage exactly one executable\n",
            "      - uses: actions/download-artifact@v4\n\n      - name: Stage exactly one executable\n",
            "must not consume fallback artifacts",
        ),
        (
            "omitted overlay case removal",
            "            rm \"$omitted_root/$overlay\"\n",
            "",
            "must test omitted overlays",
        ),
        (
            "byte evidence removal",
            "          artifact_bytes=\"$(stat -c '%s' \"$stage_dir/bolt-v2\")\"\n",
            "",
            "must measure staged executable bytes",
        ),
        (
            "final digest guard removal",
            "          [[ \"$final_sha256\" == \"$INITIAL_SHA256\" ]]\n",
            "",
            "must preserve the staged executable digest",
        ),
        (
            "artifact publisher",
            "      - name: Record inert run evidence\n",
            "      - uses: actions/upload-artifact@v4\n\n      - name: Record inert run evidence\n",
            "must not publish reusable artifacts",
        ),
        (
            "launch authority",
            "          } >> \"$GITHUB_STEP_SUMMARY\"\n",
            "          } >> \"$GITHUB_STEP_SUMMARY\"\n          ops launch\n",
            "must not create operational authority",
        ),
        (
            "merge authority",
            "          } >> \"$GITHUB_STEP_SUMMARY\"\n",
            "          } >> \"$GITHUB_STEP_SUMMARY\"\n          gh pr merge --auto\n",
            "must not create operational authority",
        ),
    )
    for label, old, new, expected in cases:
        assert_mutation_rejected(verifier, baseline, label, old, new, expected)

    print("OK: root-artifact workflow mutation tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
