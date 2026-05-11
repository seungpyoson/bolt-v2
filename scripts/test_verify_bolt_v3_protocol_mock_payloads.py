#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 protocol mock payload verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_protocol_mock_payloads.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_protocol_mock_payloads", SCRIPT)
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


def test_inline_protocol_json_body_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_polymarket_fee_provider.rs",
            'fn probe() { let body = r#"{"base_fee":"0"}"#; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_polymarket_fee_provider.rs"
        assert "protocol mock payload" in findings[0].message


def test_fixture_payload_read_is_clean() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_polymarket_fee_provider.rs",
            'fn probe() { let body = include_str!("fixtures/fee.json"); }\n',
        )

        assert verifier.scan_root(root) == []


def test_fee_provider_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_polymarket_fee_provider.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("Polymarket fee provider test file must be enforced")


def test_order_lifecycle_tracer_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_order_lifecycle_tracer.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("order lifecycle tracer test file must be enforced")


def main() -> int:
    tests = [
        test_inline_protocol_json_body_is_a_finding,
        test_fixture_payload_read_is_clean,
        test_fee_provider_file_is_enforced,
        test_order_lifecycle_tracer_file_is_enforced,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 protocol mock payload verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
