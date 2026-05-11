#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 reference-policy fixture-literal verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_reference_policy_literals.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_reference_policy_literals", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, relative_path: str, text: str) -> None:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_reference_source_id_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_reference_policy/eth_usd_stream.toml",
            """
publish_topic = "reference.eth_usd"
min_publish_interval_milliseconds = 100

[[inputs]]
source_id = "eth_usd_oracle_anchor"
source_type = "oracle"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 2.0
stale_after_milliseconds = 1500
disable_after_milliseconds = 5000
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            'fn probe() { let _ = "eth_usd_oracle_anchor"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_reference_policy.rs"
        assert "reference-policy fixture literal" in findings[0].message


def test_reference_instrument_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_reference_policy/eth_usd_stream.toml",
            """
publish_topic = "reference.eth_usd"
min_publish_interval_milliseconds = 100

[[inputs]]
source_id = "eth_usd_oracle_anchor"
source_type = "oracle"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 2.0
stale_after_milliseconds = 1500
disable_after_milliseconds = 5000
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            'fn probe() { let _ = "ETHUSD.CHAINLINK"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert "reference-policy fixture literal" in findings[0].message


def test_reference_stream_parameter_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_strategy_registration.rs",
            'fn probe() { let _ = "reference_stream_id"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_strategy_registration.rs"
        assert "reference stream parameter-key literal" in findings[0].message


def test_reference_stream_parameter_literal_in_source_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "src/bolt_v3_validate.rs",
            'fn probe() { let _ = "reference_stream_id"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "src/bolt_v3_validate.rs"
        assert "reference stream parameter-key literal" in findings[0].message


def test_auto_disable_reason_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            'fn probe() { let _ = "auto-disabled after 2100ms without a fresh reference update"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_reference_policy.rs"
        assert "reference auto-disable reason literal" in findings[0].message


def test_reference_policy_scenario_value_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            "fn probe() { let oracle_price = 100.0; }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_reference_policy.rs"
        assert "reference-policy scenario value literal" in findings[0].message


def test_reference_policy_manual_disable_reason_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            'fn probe() { let _ = format!("test disables {}", "source"); }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_reference_policy.rs"
        assert "reference-policy manual disable reason literal" in findings[0].message


def test_derived_reference_fixture_lookup_is_clean() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_reference_policy/eth_usd_stream.toml",
            """
publish_topic = "reference.eth_usd"
min_publish_interval_milliseconds = 100

[[inputs]]
source_id = "eth_usd_oracle_anchor"
source_type = "oracle"
instrument_id = "ETHUSD.CHAINLINK"
base_weight = 2.0
stale_after_milliseconds = 1500
disable_after_milliseconds = 5000
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_reference_policy.rs",
            "fn probe(source_id: &str, instrument_id: &str) { let _ = (source_id, instrument_id); }\n",
        )

        assert verifier.scan_root(root) == []


def test_reference_policy_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_reference_policy.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("reference policy test file must be enforced")


def test_reference_producer_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_reference_producer.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("reference producer test file must be enforced")


def test_adapter_mapping_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_adapter_mapping.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("adapter mapping test file must be enforced")


def test_reference_actor_registration_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_reference_actor_registration.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("reference actor registration test file must be enforced")


def test_strategy_registration_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_strategy_registration.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("strategy registration test file must be enforced")


def test_validate_source_file_is_enforced() -> None:
    verifier = load_verifier()
    if "src/bolt_v3_validate.rs" not in verifier.ENFORCED_SOURCE_FILES:
        raise AssertionError("bolt-v3 validate source file must be enforced")


def main() -> int:
    tests = [
        test_reference_source_id_literal_is_a_finding,
        test_reference_instrument_literal_is_a_finding,
        test_reference_stream_parameter_literal_is_a_finding,
        test_reference_stream_parameter_literal_in_source_is_a_finding,
        test_auto_disable_reason_literal_is_a_finding,
        test_reference_policy_scenario_value_literal_is_a_finding,
        test_reference_policy_manual_disable_reason_literal_is_a_finding,
        test_derived_reference_fixture_lookup_is_clean,
        test_reference_policy_file_is_enforced,
        test_reference_producer_file_is_enforced,
        test_adapter_mapping_file_is_enforced,
        test_reference_actor_registration_file_is_enforced,
        test_strategy_registration_file_is_enforced,
        test_validate_source_file_is_enforced,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 reference-policy fixture-literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
