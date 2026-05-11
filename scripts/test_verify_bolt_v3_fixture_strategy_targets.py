#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 fixture strategy/target verifier."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_fixture_strategy_targets.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_bolt_v3_fixture_strategy_targets", SCRIPT
    )
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, files: dict[str, str]) -> None:
    for relative_path, content in files.items():
        path = root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")


def fixture_files() -> dict[str, str]:
    return {
        "tests/fixtures/bolt_v3_existing_strategy/root.toml": """
strategy_files = ["strategies/first.toml"]
""",
        "tests/fixtures/bolt_v3_existing_strategy/strategies/first.toml": """
strategy_instance_id = "STRATEGY-FIXTURE-001"

[target]
configured_target_id = "target_fixture"
""",
    }


def test_fixture_strategy_id_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_fixture_strategy_targets"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_readiness.rs"] = (
            'fn probe() { let _ = "STRATEGY-FIXTURE-001"; }\n'
        )
        write_fixture(root, files)

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_instrument_readiness.rs"
        assert "fixture strategy/target literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_fixture_target_id_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_fixture_strategy_targets"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_gate.rs"] = (
            'fn probe() { let _ = "target_fixture"; }\n'
        )
        write_fixture(root, files)

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_instrument_gate.rs"
        assert "fixture strategy/target literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_derived_fixture_strategy_lookup_is_clean() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_fixture_strategy_targets"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_readiness.rs"] = """
fn probe(strategy: &LoadedStrategy) {
    let _ = strategy.config.strategy_instance_id.as_str();
    let _ = strategy.config.target["configured_target_id"].as_str();
}
"""
        write_fixture(root, files)

        assert verifier.scan_root(root) == []
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    tests = [
        test_fixture_strategy_id_literal_is_a_finding,
        test_fixture_target_id_literal_is_a_finding,
        test_derived_fixture_strategy_lookup_is_clean,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 fixture strategy/target verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
