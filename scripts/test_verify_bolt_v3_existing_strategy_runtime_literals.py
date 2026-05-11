#!/usr/bin/env python3
"""Self-tests for the existing-strategy runtime literal verifier."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_existing_strategy_runtime_literals.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_bolt_v3_existing_strategy_runtime_literals", SCRIPT
    )
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_runtime_test(root: Path, content: str) -> None:
    path = root / "tests/eth_chainlink_taker_runtime.rs"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def test_strategy_id_literal_outside_fixture_config_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            'fn probe() { let _ = StrategyId::from("ETHCHAINLINKTAKER-RT-001"); }\n',
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime strategy id literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_reference_topic_literal_outside_helper_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            'fn probe() { let _ = "platform.reference.test.chainlink".to_string(); }\n',
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime reference topic literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_default_instrument_literal_outside_helper_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            'fn probe() { let _ = InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET"); }\n',
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime instrument id literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_test_node_fixture_literal_outside_fixture_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        fixture_path = root / "tests/fixtures/eth_chainlink_taker_runtime/test_node.toml"
        fixture_path.parent.mkdir(parents=True, exist_ok=True)
        fixture_path.write_text(
            """
[node]
name = "ETH-TAKER-RT"
trader_id = "BOLT-001"

[[data_clients]]
name = "TESTDATA"
[data_clients.config]
venue = "POLYMARKET"
event_slugs = ["eth-updown-5m"]

[[exec_clients]]
name = "TEST"
[exec_clients.config]
account_id = "TEST-ACCOUNT"
venue = "POLYMARKET"
""",
            encoding="utf-8",
        )
        write_runtime_test(root, 'fn probe() { let _ = "TESTDATA"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime test-node fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_canonical_fixture_definitions_are_allowed() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            """
fn strategy_raw_config() -> Value {
    toml::toml! {
        strategy_id = "ETHCHAINLINKTAKER-RT-001"
    }
    .into()
}

fn fixture_reference_publish_topic() -> &'static str {
    "platform.reference.test.chainlink"
}

fn eth_up_instrument_id() -> InstrumentId {
    InstrumentId::from("condition-eth-MKT-ETH-1-UP.POLYMARKET")
}

fn eth_down_instrument_id() -> InstrumentId {
    InstrumentId::from("condition-eth-MKT-ETH-1-DOWN.POLYMARKET")
}

fn probe() {
    let _ = strategy_id_from_fixture_config();
    let _ = fixture_reference_publish_topic().to_string();
    let _ = eth_up_instrument_id();
    let _ = eth_down_instrument_id();
}
""",
        )

        assert verifier.scan_root(root) == []
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    tests = [
        test_strategy_id_literal_outside_fixture_config_is_a_finding,
        test_reference_topic_literal_outside_helper_is_a_finding,
        test_default_instrument_literal_outside_helper_is_a_finding,
        test_test_node_fixture_literal_outside_fixture_is_a_finding,
        test_canonical_fixture_definitions_are_allowed,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 existing-strategy runtime literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
