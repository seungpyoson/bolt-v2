#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 core-boundary verifier."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_core_boundary.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_core_boundary", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def write_fixture(root: Path, overrides: dict[str, str] | None = None) -> None:
    overrides = overrides or {}
    for rel, _patterns in VERIFIER.CHECKS:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(overrides.get(rel, "pub struct CleanBoundary;\n"), encoding="utf-8")


def run_with_root(root: Path) -> tuple[int, str]:
    original_root = VERIFIER.REPO_ROOT
    stdout = io.StringIO()
    stderr = io.StringIO()
    try:
        VERIFIER.REPO_ROOT = root
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = VERIFIER.main()
    finally:
        VERIFIER.REPO_ROOT = original_root
    return code, stderr.getvalue()


def test_clean_fixture_passes() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root)
        code, stderr = run_with_root(root)
    if code != 0:
        raise AssertionError(f"expected clean fixture to pass, got {code}: {stderr}")


def test_forbidden_closed_identity_fails_with_line_number() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
                "src/bolt_v3_config.rs": "\n\npub enum VenueKind { Polymarket }\n",
            },
        )
        code, stderr = run_with_root(root)
    if code != 1:
        raise AssertionError(f"expected forbidden fixture to fail, got {code}")
    if "src/bolt_v3_config.rs:3" not in stderr or "VenueKind" not in stderr:
        raise AssertionError(f"unexpected stderr: {stderr!r}")


def test_check_universe_names_required_core_files() -> None:
    checked_paths = {rel for rel, _patterns in VERIFIER.CHECKS}
    expected = {
        "src/bolt_v3_config.rs",
        "src/bolt_v3_providers/mod.rs",
        "src/bolt_v3_archetypes/mod.rs",
        "src/bolt_v3_market_families/mod.rs",
    }
    missing = expected - checked_paths
    if missing:
        raise AssertionError(f"core-boundary check universe missing {sorted(missing)}")


def main() -> int:
    tests = [
        test_clean_fixture_passes,
        test_forbidden_closed_identity_fails_with_line_number,
        test_check_universe_names_required_core_files,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 core-boundary verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
