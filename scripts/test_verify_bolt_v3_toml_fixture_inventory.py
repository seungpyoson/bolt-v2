#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 TOML fixture inventory verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_toml_fixture_inventory.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_toml_fixture_inventory", SCRIPT_PATH)
assert SPEC is not None
verifier = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = verifier
SPEC.loader.exec_module(verifier)


def write_file(root: Path, relative_path: str, text: str) -> None:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_missing_fixture_inventory_is_a_finding() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "tests/fixtures/bolt_v3/root.toml", "schema_version = 1\n")

        findings = verifier.scan_root(root)
        assert any("missing Bolt-v3 TOML fixture inventory" in finding.message for finding in findings)


def test_unlisted_and_stale_fixture_paths_are_findings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "tests/fixtures/bolt_v3/root.toml", "schema_version = 1\n")
        write_file(
            root,
            "tests/fixtures/bolt_v3_fixture_inventory.toml",
            """
[[fixtures]]
path = "tests/fixtures/bolt_v3/missing.toml"
purpose = "stale fixture entry"
""",
        )

        messages = [finding.message for finding in verifier.scan_root(root)]
        assert "unlisted Bolt-v3 TOML fixture: tests/fixtures/bolt_v3/root.toml" in messages
        assert "stale Bolt-v3 TOML fixture inventory path: tests/fixtures/bolt_v3/missing.toml" in messages


def test_duplicate_and_blank_purpose_are_findings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "tests/fixtures/bolt_v3/root.toml", "schema_version = 1\n")
        write_file(
            root,
            "tests/fixtures/bolt_v3_fixture_inventory.toml",
            """
[[fixtures]]
path = "tests/fixtures/bolt_v3/root.toml"
purpose = ""

[[fixtures]]
path = "tests/fixtures/bolt_v3/root.toml"
purpose = "duplicate fixture entry"
""",
        )

        messages = [finding.message for finding in verifier.scan_root(root)]
        assert "inventory fixture tests/fixtures/bolt_v3/root.toml must define nonempty purpose" in messages
        assert "duplicate inventory fixture path: tests/fixtures/bolt_v3/root.toml" in messages


def test_complete_inventory_is_clean() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(root, "tests/fixtures/bolt_v3/root.toml", "schema_version = 1\n")
        write_file(
            root,
            "tests/fixtures/bolt_v3_fixture_inventory.toml",
            """
[[fixtures]]
path = "tests/fixtures/bolt_v3/root.toml"
purpose = "baseline fixture"
""",
        )

        assert verifier.scan_root(root) == []


def main() -> int:
    tests = [
        test_missing_fixture_inventory_is_a_finding,
        test_unlisted_and_stale_fixture_paths_are_findings,
        test_duplicate_and_blank_purpose_are_findings,
        test_complete_inventory_is_clean,
    ]
    for test in tests:
        test()

    print("OK: Bolt-v3 TOML fixture inventory verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
