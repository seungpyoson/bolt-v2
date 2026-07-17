#!/usr/bin/env python3
"""Fail-closed static contract for the root-artifact workflow."""

from __future__ import annotations

import pathlib
import re
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW_PATH = REPO_ROOT / ".github/workflows/root-artifact.yml"


def uncommented_text(text: str) -> str:
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))


def root_artifact_workflow_errors(text: str) -> list[str]:
    source = uncommented_text(text)
    errors: list[str] = []

    trigger_match = re.search(r"^on:\n(?P<body>.*?)(?=^concurrency:)", source, re.MULTILINE | re.DOTALL)
    triggers = re.findall(r"^  ([A-Za-z0-9_-]+):", trigger_match.group("body") if trigger_match else "", re.MULTILINE)
    if triggers != ["workflow_dispatch"]:
        errors.append("root-artifact must remain dispatch-only")

    jobs_match = re.search(r"^jobs:\n(?P<body>.*)\Z", source, re.MULTILINE | re.DOTALL)
    jobs = re.findall(r"^  ([A-Za-z0-9_-]+):\n", jobs_match.group("body") if jobs_match else "", re.MULTILINE)
    if jobs != ["preflight", "produce"]:
        errors.append("root-artifact must define exactly two jobs: preflight and produce")

    if source.count("uses: ./.github/actions/sccache-setup") != 1:
        errors.append("root-artifact must have exactly one governed sccache writer")
    if len(re.findall(r"\bcargo --repo\b", source)) != 1:
        errors.append("root-artifact must have exactly one governed Cargo invocation")
    if re.search(r"\bcargo\s+(?:test|nextest)\b|\bcargo --repo[^\n]*(?:\n[^\n]*)?\b(?:test|nextest)\b|\bnextest\b", source):
        errors.append("root-artifact must not run Cargo tests or nextest")
    if source.count('build --locked --profile "$BUILD_PROFILE" --target "$BUILD_TARGET" --bin bolt-v2') != 1:
        errors.append("root-artifact must perform exactly one locked root binary build")

    required_fragments = (
        ('[[ "$ENABLED" == "true" ]]', "root-artifact must require enabled sccache"),
        ('[[ "$CACHE_MODE" == "read_write" ]]', "root-artifact must require read-write sccache"),
        (
            '[[ "$BOLT_RUST_VERIFICATION_SCCACHE" == "1" ]]',
            "root-artifact must require the governed wrapper opt-in",
        ),
        (
            'rm "$omitted_root/$overlay"',
            "root-artifact must test omitted overlays",
        ),
        (
            'artifact_bytes="$(stat -c \'%s\' "$stage_dir/bolt-v2")"',
            "root-artifact must measure staged executable bytes",
        ),
        (
            '[[ "$final_sha256" == "$INITIAL_SHA256" ]]',
            "root-artifact must preserve the staged executable digest",
        ),
        (
            "authority: null",
            "root-artifact evidence must remain explicitly non-authoritative",
        ),
    )
    for fragment, error in required_fragments:
        if fragment not in source:
            errors.append(error)

    if re.search(r"^\s*(?:retry|continue-on-error):", source, re.MULTILINE):
        errors.append("root-artifact must not retry or continue after failure")
    if re.search(r"actions/download-artifact|restore-keys:", source):
        errors.append("root-artifact must not consume fallback artifacts or cached results")
    if "actions/upload-artifact" in source:
        errors.append("root-artifact must not publish reusable artifacts")
    if re.search(r"\bops\s+launch\b|\bsystemctl\b|\bgh\s+pr\s+merge\b|\bgit\s+push\b", source):
        errors.append("root-artifact must not create operational authority")

    return errors


def main() -> int:
    errors = root_artifact_workflow_errors(WORKFLOW_PATH.read_text(encoding="utf-8"))
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: root-artifact workflow verifier passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
