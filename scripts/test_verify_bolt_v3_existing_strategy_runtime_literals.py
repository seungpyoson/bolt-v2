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


def test_strategy_archetype_literal_outside_constant_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            'fn probe() { strategy_factory(&trader, "eth_chainlink_taker", &raw).unwrap(); }\n',
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime archetype literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_market_selection_context_literal_fields_are_findings() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(
            root,
            """
fn probe() {
    build_context.bolt_v3_market_selection_context = Some(BoltV3MarketSelectionContext {
        market_selection_type: "rotating_market".to_string(),
        rotating_market_family: Some("updown".to_string()),
        underlying_asset: Some("ETH".to_string()),
        cadence_seconds: Some(300),
        market_selection_rule: Some("active_or_next".to_string()),
        retry_interval_seconds: Some(5),
        blocked_after_seconds: Some(60),
    });
}
""",
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "market-selection context literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_target_fixture_literal_value_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        strategy_dir = root / "tests/fixtures/bolt_v3_existing_strategy/strategies"
        strategy_dir.mkdir(parents=True)
        (root / "tests/fixtures/bolt_v3_existing_strategy/root.toml").write_text(
            'strategy_files = ["strategies/eth_chainlink_taker.toml"]\n',
            encoding="utf-8",
        )
        (strategy_dir / "eth_chainlink_taker.toml").write_text(
            """
[target]
market_selection_type = "rotating_market"
rotating_market_family = "updown"
underlying_asset = "ETH"
market_selection_rule = "active_or_next"
""".lstrip(),
            encoding="utf-8",
        )
        write_runtime_test(
            root,
            'fn probe() { let _ = serde_json::Value::String("rotating_market".to_string()); }\n',
        )

        findings = verifier.scan_root(root)

        assert findings
        assert "target fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_selected_market_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        fixture_path = (
            root
            / "tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml"
        )
        fixture_path.parent.mkdir(parents=True, exist_ok=True)
        fixture_path.write_text(
            """
[[markets]]
name = "runtime_fixture"
condition_id = "condition-fixture"
question_id = "question-fixture"
market_slug = "market-fixture"
start_ms = 0
end_ms = 300000

[[markets.legs]]
outcome = "Up"
token_id = "111"
instrument_id = "condition-fixture-111.POLYMARKET"
""",
            encoding="utf-8",
        )
        write_runtime_test(root, 'fn probe() { let _ = "market-fixture"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "selected-market fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_selected_market_fixture_name_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        fixture_path = (
            root
            / "tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml"
        )
        fixture_path.parent.mkdir(parents=True, exist_ok=True)
        fixture_path.write_text(
            """
[[markets]]
name = "runtime_fixture"
condition_id = "condition-fixture"
question_id = "question-fixture"
market_slug = "market-fixture"
start_ms = 0
end_ms = 300000

[[markets.legs]]
outcome = "Up"
token_id = "111"
instrument_id = "condition-fixture-111.POLYMARKET"
""",
            encoding="utf-8",
        )
        write_runtime_test(root, 'fn probe() { let _ = "runtime_fixture"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "selected-market fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_selection_ruleset_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(root, 'fn probe() { let _ = "PRIMARY"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "selection ruleset literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_reference_stream_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        root_fixture = root / "tests/fixtures/bolt_v3_existing_strategy/root.toml"
        root_fixture.parent.mkdir(parents=True, exist_ok=True)
        root_fixture.write_text(
            """
[reference_streams.eth_usd]
publish_topic = "reference.fixture"
min_publish_interval_milliseconds = 100

[[reference_streams.eth_usd.inputs]]
source_id = "reference_fixture_oracle"
source_type = "oracle"
instrument_id = "FIXTURE.CHAINLINK"
base_weight = 1.0
stale_after_milliseconds = 1500
disable_after_milliseconds = 5000
""",
            encoding="utf-8",
        )
        write_runtime_test(root, 'fn probe() { let _ = "reference.fixture"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "reference stream fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_resolution_basis_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        root_fixture = root / "tests/fixtures/bolt_v3_existing_strategy/root.toml"
        root_fixture.parent.mkdir(parents=True, exist_ok=True)
        root_fixture.write_text(
            """
[clients.fixture_oracle]
adapter_type = "chainlink"
venue = "fixturevenue"

[reference_streams.fixture_usd]
publish_topic = "reference.fixture"
min_publish_interval_milliseconds = 100

[[reference_streams.fixture_usd.inputs]]
source_id = "fixture_oracle_anchor"
source_type = "oracle"
data_client_id = "fixture_oracle"
instrument_id = "FIXTUREUSD.CHAINLINK"
base_weight = 1.0
stale_after_milliseconds = 1500
disable_after_milliseconds = 5000
""",
            encoding="utf-8",
        )
        write_runtime_test(root, 'fn probe() { let _ = "fixturevenue_fixtureusd"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "resolution-basis fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_selection_freeze_reason_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(root, 'fn probe() { let _ = "freeze window"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "selection freeze reason literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_price_to_beat_source_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(root, 'fn probe() { let _ = "polymarket_gamma_market_anchor"; }\n')

        findings = verifier.scan_root(root)

        assert findings
        assert "price-to-beat source literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_runtime_unix_nanos_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_existing_strategy_runtime_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_runtime_test(root, "fn probe() { let _ = UnixNanos::from(1_u64); }\n")

        findings = verifier.scan_root(root)

        assert findings
        assert "existing-strategy runtime timestamp literal" in findings[0].message
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

fn fixture_reference_publish_topic() -> String {
    fixture_reference_stream().publish_topic
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
        test_strategy_archetype_literal_outside_constant_is_a_finding,
        test_market_selection_context_literal_fields_are_findings,
        test_target_fixture_literal_value_is_a_finding,
        test_selected_market_fixture_literal_is_a_finding,
        test_selected_market_fixture_name_literal_is_a_finding,
        test_selection_ruleset_literal_is_a_finding,
        test_reference_stream_fixture_literal_is_a_finding,
        test_resolution_basis_fixture_literal_is_a_finding,
        test_selection_freeze_reason_literal_is_a_finding,
        test_price_to_beat_source_literal_is_a_finding,
        test_runtime_unix_nanos_literal_is_a_finding,
        test_canonical_fixture_definitions_are_allowed,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 existing-strategy runtime literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
