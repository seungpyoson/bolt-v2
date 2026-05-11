#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 decision-event test-literal verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_decision_event_test_literals.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_decision_event_test_literals",
    SCRIPT_PATH,
)
assert SPEC is not None
verifier = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = verifier
SPEC.loader.exec_module(verifier)


def write_file(root: Path, relative_path: str, text: str) -> None:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_inline_event_contract_literals_are_findings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_decision_event_handoff.rs",
            """
fn probe(decoded: Decoded) {
    assert_eq!(decoded.decision_event_type, "entry_evaluation");
    let _ = decoded.event_facts.get("entry_no_action_reason");
    let _ = json!({"expected_edge_basis_points": 42.0});
    let _ = "invalid_quantity";
}
""",
        )

        findings = verifier.scan_root(root)
        messages = [finding.message for finding in findings]
        assert "inline decision-event type value; use exported event contract constant" in messages
        assert "inline decision-event fact key; use exported event contract constant" in messages
        assert "inline decision-event JSON object fixture; move fixture data out of Rust test" in messages
        assert "inline decision-event reason value; use exported event contract constant" in messages


def test_order_intent_gate_direct_event_fixture_construction_is_a_finding() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_intent_gate.rs",
            """
fn probe() {
    let _ = BoltV3DecisionEventCommonFields {
        strategy_instance_id: "strategy-alpha".to_string(),
    };
    let _ = BoltV3OrderSubmissionFacts {
        instrument_id: "ETH-UP.POLYMARKET".to_string(),
    };
}
""",
        )

        findings = verifier.scan_root(root)
        messages = [finding.message for finding in findings]
        assert (
            "direct decision-event common-field fixture construction; derive common fields from v3 TOML and release identity"
            in messages
        )
        assert (
            "direct order-submission fact fixture construction; load order fact fixture data outside Rust test code"
            in messages
        )


def test_decision_event_context_identity_literal_is_a_finding() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_decision_event_context.rs",
            """
fn probe() {
    let _ = "release-sha";
    let _ = "config-hash";
    let _ = "38b912a8b0fe14e4046773973ff46a3b798b1e3e";
    let _ = "123e4567-e89b-12d3-a456-426614174002";
    let _ = "eth_updown_5m";
}
""",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 5
        assert {
            "inline decision-event context fixture literal; derive from v3 TOML, release identity, or generated trace id"
        } == {finding.message for finding in findings}


def test_decision_event_handoff_fixture_literal_is_a_finding() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_decision_event_handoff.rs",
            """
fn probe() {
    let _ = "strategy-alpha";
    let _ = "target-eth-updown";
    let _ = BoltV3OrderSubmissionFacts {
        instrument_id: "ETH-UP.POLYMARKET".to_string(),
    };
}
""",
        )

        findings = verifier.scan_root(root)
        messages = [finding.message for finding in findings]
        assert (
            "inline decision-event context fixture literal; derive from v3 TOML, release identity, or generated trace id"
            in messages
        )
        assert (
            "direct order-submission fact fixture construction; load order fact fixture data outside Rust test code"
            in messages
        )


def test_eth_chainlink_runtime_context_literal_is_a_finding() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/eth_chainlink_taker_runtime.rs",
            """
fn probe() {
    let _ = "release-sha";
    let _ = "target-eth-updown";
}
""",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 2
        assert {
            "inline decision-event context fixture literal; derive from v3 TOML, release identity, or generated trace id"
        } == {finding.message for finding in findings}


def test_exported_constants_and_fixture_helpers_are_clean() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_decision_event_handoff.rs",
            """
fn probe(decoded: Decoded) {
    assert_eq!(decoded.decision_event_type, BOLT_V3_ENTRY_EVALUATION_EVENT_VALUE);
    let _ = decoded.event_facts.get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY);
    let _ = BOLT_V3_ENTRY_PRE_SUBMIT_REJECTION_INVALID_QUANTITY_REASON;
    let _ = decision_event_json_fixture("entry_archetype_metrics.json");
}
""",
        )

        assert verifier.scan_root(root) == []


def test_decision_event_handoff_file_is_enforced() -> None:
    if "tests/bolt_v3_decision_event_handoff.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("decision event handoff test file must be enforced")


def test_order_intent_gate_file_is_enforced() -> None:
    if "tests/bolt_v3_order_intent_gate.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("order intent gate test file must be enforced")


def test_decision_event_context_file_is_enforced() -> None:
    if "tests/bolt_v3_decision_event_context.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("decision event context test file must be enforced")


def test_eth_chainlink_runtime_file_is_enforced() -> None:
    if "tests/eth_chainlink_taker_runtime.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("eth chainlink runtime test file must be enforced")


def main() -> int:
    tests = [
        test_inline_event_contract_literals_are_findings,
        test_order_intent_gate_direct_event_fixture_construction_is_a_finding,
        test_decision_event_context_identity_literal_is_a_finding,
        test_decision_event_handoff_fixture_literal_is_a_finding,
        test_eth_chainlink_runtime_context_literal_is_a_finding,
        test_exported_constants_and_fixture_helpers_are_clean,
        test_decision_event_handoff_file_is_enforced,
        test_order_intent_gate_file_is_enforced,
        test_decision_event_context_file_is_enforced,
        test_eth_chainlink_runtime_file_is_enforced,
    ]
    for test in tests:
        test()

    print("OK: Bolt-v3 decision-event test-literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
