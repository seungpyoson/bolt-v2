#!/usr/bin/env python3
"""Self-tests for repo-local Rust verification ownership."""

from __future__ import annotations

import fnmatch
import pathlib
import subprocess
import sys
from ci_workflow_hygiene_test_helpers import repo_git_command
from verify_ci_workflow_hygiene import repo_workflow_paths


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
OWNER = REPO_ROOT / "scripts" / "rust_verification.py"
REMOVED_SCRIPTS = (
    REPO_ROOT / "scripts" / "install_ci_rust_verification_owner.sh",
    REPO_ROOT / "scripts" / "require_rust_verification_owner.sh",
)
RUNTIME_SURFACE_PATTERNS = (
    "justfile",
    ".githooks/*",
    ".github/actions/**/*.yaml",
    ".github/actions/**/*.yml",
    ".github/workflows/*.yaml",
    ".github/workflows/*.yml",
    "scripts/cargo-shim",
    "scripts/install-cargo-shim",
    "scripts/*.py",
    "scripts/*.sh",
    "tests/*.sh",
)
RUNTIME_SURFACE_EXCLUDES = (
    "scripts/test_rust_verification_decoupling.py",
    "scripts/dormant_review/*",
)
REQUIRED_RUNTIME_SURFACES = (
    ".github/workflows/stale.yml",
    ".github/workflows/summary.yml",
    ".githooks/post-checkout",
    "scripts/cargo-shim",
    "scripts/install-cargo-shim",
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


def tracked_runtime_surfaces() -> tuple[pathlib.Path, ...]:
    result = subprocess.run(
        repo_git_command("ls-files", "-z"),
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)

    active_workflows = set(repo_workflow_paths())
    surfaces: list[pathlib.Path] = []
    for relative in result.stdout.split("\0"):
        if not relative:
            continue
        if any(fnmatch.fnmatch(relative, pattern) for pattern in RUNTIME_SURFACE_EXCLUDES):
            continue
        if relative.startswith(".github/workflows/") and relative not in active_workflows:
            continue
        if any(fnmatch.fnmatch(relative, pattern) for pattern in RUNTIME_SURFACE_PATTERNS):
            path = REPO_ROOT / relative
            if path.exists():
                surfaces.append(path)
    return tuple(sorted(surfaces))


def assert_runtime_surface_discovery_covers_current_repo() -> None:
    discovered = {path.relative_to(REPO_ROOT).as_posix() for path in tracked_runtime_surfaces()}
    missing = sorted(set(REQUIRED_RUNTIME_SURFACES) - discovered)
    if missing:
        raise AssertionError(f"runtime surface discovery missed: {missing}")


def assert_dormant_directory_is_not_a_runtime_surface() -> None:
    relative = "scripts/dormant_review/broken.py"
    included = any(fnmatch.fnmatch(relative, pattern) for pattern in RUNTIME_SURFACE_PATTERNS)
    excluded = any(fnmatch.fnmatch(relative, pattern) for pattern in RUNTIME_SURFACE_EXCLUDES)
    if included and not excluded:
        raise AssertionError(f"dormant runtime surface remains active: {relative}")


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
    for path in tracked_runtime_surfaces():
        text = path.read_text(encoding="utf-8")
        for fragment in FORBIDDEN_RUNTIME_FRAGMENTS:
            if fragment in text:
                offenders.append(f"{path.relative_to(REPO_ROOT)} contains {fragment!r}")
    if offenders:
        raise AssertionError("\n".join(offenders))


def main() -> int:
    assert_runtime_surface_discovery_covers_current_repo()
    assert_dormant_directory_is_not_a_runtime_surface()
    assert_owner_cli_contract()
    assert_no_external_owner_install_path()
    print("OK: Rust verification decoupling self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    sys.exit(main())
