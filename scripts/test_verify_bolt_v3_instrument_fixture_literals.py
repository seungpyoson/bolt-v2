#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 instrument fixture-literal verifier."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_instrument_fixture_literals.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_bolt_v3_instrument_fixture_literals", SCRIPT
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
        "tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml": """
schema_version = 1

[[markets]]
name = "selected"
condition_id = "condition-fixture"
question_id = "question-fixture"
market_slug = "fixture-market"
start_ms = 1000
end_ms = 2000

[[markets.legs]]
outcome = "Up"
token_id = "token-up"
instrument_id = "token-up.POLYMARKET"

[[markets.legs]]
outcome = "Down"
token_id = "token-down"
instrument_id = "token-down.POLYMARKET"
""",
    }


def test_instrument_id_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_instrument_fixture_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_readiness.rs"] = (
            'fn probe() { let _ = "token-up.POLYMARKET"; }\n'
        )
        write_fixture(root, files)

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_instrument_readiness.rs"
        assert "instrument fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_selected_market_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_instrument_fixture_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_gate.rs"] = (
            'fn probe() { let _ = "fixture-market"; }\n'
        )
        write_fixture(root, files)

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_instrument_gate.rs"
        assert "instrument fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_derived_fixture_lookup_is_clean() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_instrument_fixture_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        files = fixture_files()
        files["tests/bolt_v3_instrument_readiness.rs"] = """
fn probe(fixture: &SelectedMarketFixture) {
    let _ = fixture.market_slug.as_str();
    let _ = fixture.legs[0].instrument_id.as_str();
}
"""
        write_fixture(root, files)

        assert verifier.scan_root(root) == []
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    tests = [
        test_instrument_id_literal_is_a_finding,
        test_selected_market_literal_is_a_finding,
        test_derived_fixture_lookup_is_clean,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 instrument fixture-literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
