#!/usr/bin/env python3
"""Self-tests for repo-local Rust verification ownership."""

from __future__ import annotations

import pathlib
import subprocess
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
OWNER = REPO_ROOT / "scripts" / "rust_verification.py"
REMOVED_SCRIPTS = (
    REPO_ROOT / "scripts" / "install_ci_rust_verification_owner.sh",
    REPO_ROOT / "scripts" / "require_rust_verification_owner.sh",
)
RUNTIME_SURFACES = (
    REPO_ROOT / "justfile",
    REPO_ROOT / "tests" / "verify_build.sh",
    REPO_ROOT / ".github" / "actions" / "setup-environment" / "action.yml",
    REPO_ROOT / ".github" / "workflows" / "ci.yml",
    REPO_ROOT / ".github" / "workflows" / "advisory.yml",
    REPO_ROOT / ".github" / "workflows" / "dependabot-auto-merge.yml",
    REPO_ROOT / "scripts" / "verify_ci_workflow_hygiene.py",
)
FORBIDDEN_RUNTIME_FRAGMENTS = (
    "CLAUDE_CONFIG_READ_TOKEN",
    "claude-config-read-token",
    "seungpyoson/claude-config",
    "install_ci_rust_verification_owner.sh",
    "require_rust_verification_owner.sh",
    "/.claude/lib/rust_verification.py",
    "~/.claude/lib/rust_verification.py",
)


def assert_owner_cli_contract() -> None:
    if not OWNER.exists():
        raise AssertionError(f"missing repo-local Rust verification owner: {OWNER.relative_to(REPO_ROOT)}")
    result = subprocess.run(
        [sys.executable, str(OWNER), "repo-status", "--repo", str(REPO_ROOT)],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    if result.stdout.strip() != "managed":
        raise AssertionError(result.stdout)


def assert_no_external_owner_install_path() -> None:
    offenders: list[str] = []
    for path in REMOVED_SCRIPTS:
        if path.exists():
            offenders.append(f"{path.relative_to(REPO_ROOT)} still exists")
    for path in RUNTIME_SURFACES:
        text = path.read_text(encoding="utf-8")
        for fragment in FORBIDDEN_RUNTIME_FRAGMENTS:
            if fragment in text:
                offenders.append(f"{path.relative_to(REPO_ROOT)} contains {fragment!r}")
    if offenders:
        raise AssertionError("\n".join(offenders))


def main() -> int:
    assert_owner_cli_contract()
    assert_no_external_owner_install_path()
    print("OK: Rust verification decoupling self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
