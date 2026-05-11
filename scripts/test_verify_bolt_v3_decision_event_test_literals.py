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
}
""",
        )

        findings = verifier.scan_root(root)
        messages = [finding.message for finding in findings]
        assert "inline decision-event type value; use exported event contract constant" in messages
        assert "inline decision-event fact key; use exported event contract constant" in messages
        assert "inline decision-event JSON object fixture; move fixture data out of Rust test" in messages


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
    let _ = decision_event_json_fixture("entry_archetype_metrics.json");
}
""",
        )

        assert verifier.scan_root(root) == []


def test_decision_event_handoff_file_is_enforced() -> None:
    if "tests/bolt_v3_decision_event_handoff.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("decision event handoff test file must be enforced")


def main() -> int:
    tests = [
        test_inline_event_contract_literals_are_findings,
        test_exported_constants_and_fixture_helpers_are_clean,
        test_decision_event_handoff_file_is_enforced,
    ]
    for test in tests:
        test()

    print("OK: Bolt-v3 decision-event test-literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
