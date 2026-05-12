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


def test_inline_protocol_json_template_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            'fn probe() { let body = format!(r#"{{"orderID":"{ORDER_ID}"}}"#); }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "protocol mock payload" in findings[0].message


def test_order_lifecycle_protocol_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_existing_strategy/order_lifecycle_tracer.toml",
            """
[local_polymarket]
accepted_order_id = "local-order-fixture"
up_token_id = "token-up-fixture"
down_token_id = "token-down-fixture"
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            'fn probe() { let _ = "local-order-fixture"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "fixture-owned literal" in findings[0].message


def test_fee_provider_protocol_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_existing_strategy/polymarket_fee_provider.toml",
            """
[local_fee_provider]
token_id_suffix = "fee-token-fixture"
bind_addr = "127.0.0.1:0"
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_polymarket_fee_provider.rs",
            'fn probe() { let _ = "fee-token-fixture"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_polymarket_fee_provider.rs"
        assert "fixture-owned literal" in findings[0].message


def test_order_lifecycle_selected_binary_option_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_existing_strategy/order_lifecycle_tracer.toml",
            """
[selected_binary_option]
price_increment = "0.001"
size_increment = "0.01"
book_level_quantity = "100"
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            'fn probe() { let _ = Price::from("0.001"); }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "fixture-owned literal" in findings[0].message


def test_order_lifecycle_numeric_scenario_fixture_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3_existing_strategy/order_lifecycle_tracer.toml",
            """
[market_snapshot]
price_to_beat = 3100.0
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe() -> f64 { 3_100.0 }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "numeric fixture-owned literal" in findings[0].message


def test_order_lifecycle_price_precision_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe(bid: f64) { let _ = Price::new(bid, 3); }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "price precision literal" in findings[0].message


def test_order_lifecycle_duration_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "async fn probe() { sleep(Duration::from_millis(10)).await; }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "duration literal" in findings[0].message


def test_order_lifecycle_duration_margin_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            """
fn probe(loaded: LoadedBoltV3Config) {
    let _ = Duration::from_secs(
        loaded.root.nautilus.timeout_shutdown_seconds
            + loaded.root.nautilus.timeout_disconnection_seconds
            + 5,
    );
}
""".lstrip(),
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "duration margin literal" in findings[0].message


def test_order_lifecycle_fee_request_count_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe() { let _ = spawn_fee_rate_server(2); }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "fee request count literal" in findings[0].message


def test_order_lifecycle_poll_attempt_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe() { for _ in 0..50 { break; } }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "poll attempt literal" in findings[0].message


def test_order_lifecycle_timestamp_offset_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe(start_ts_ms: u64) { let _ = start_ts_ms + 400; }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "timestamp offset literal" in findings[0].message


def test_order_lifecycle_scenario_price_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe() { let _ = (3_101.0, 0.430); }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 2
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "scenario price literal" in findings[0].message
        assert "scenario price literal" in findings[1].message


def test_order_lifecycle_http_method_and_path_literals_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            'fn probe() { let _ = ("POST", "/order", "/fee-rate?token_id="); }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 3
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "HTTP method/path literal" in findings[0].message


def test_order_lifecycle_positions_response_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            'fn probe() { let _ = "[]".to_string(); }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "positions response body literal" in findings[0].message


def test_order_lifecycle_raw_millisecond_nanosecond_conversion_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            (
                "fn probe(ts_ms: u64, ts_ns: u64) {\n"
                "    let _ = ts_ms * 1_000_000;\n"
                "    let _ = ts_ns / 1_000_000;\n"
                "}\n"
            ),
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 2
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "raw millisecond/nanosecond conversion" in findings[0].message


def test_order_lifecycle_literal_unix_nanos_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            "fn probe() { let _ = UnixNanos::from(1_u64); }\n",
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "literal UnixNanos" in findings[0].message


def test_local_http_response_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_order_lifecycle_tracer.rs",
            (
                'fn probe() { let _ = "HTTP/1.1 200 OK\\r\\n"; }\n'
                'fn status() { let _ = "404 Not Found"; }\n'
            ),
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 2
        assert findings[0].path == "tests/bolt_v3_order_lifecycle_tracer.rs"
        assert "local HTTP response literal" in findings[0].message


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
        test_inline_protocol_json_template_is_a_finding,
        test_order_lifecycle_protocol_fixture_literal_is_a_finding,
        test_fee_provider_protocol_fixture_literal_is_a_finding,
        test_order_lifecycle_selected_binary_option_fixture_literal_is_a_finding,
        test_order_lifecycle_numeric_scenario_fixture_literal_is_a_finding,
        test_order_lifecycle_price_precision_literal_is_a_finding,
        test_order_lifecycle_duration_literal_is_a_finding,
        test_order_lifecycle_duration_margin_literal_is_a_finding,
        test_order_lifecycle_fee_request_count_literal_is_a_finding,
        test_order_lifecycle_poll_attempt_literal_is_a_finding,
        test_order_lifecycle_timestamp_offset_literal_is_a_finding,
        test_order_lifecycle_scenario_price_literal_is_a_finding,
        test_order_lifecycle_http_method_and_path_literals_are_findings,
        test_order_lifecycle_positions_response_literal_is_a_finding,
        test_order_lifecycle_raw_millisecond_nanosecond_conversion_is_a_finding,
        test_order_lifecycle_literal_unix_nanos_is_a_finding,
        test_local_http_response_literal_is_a_finding,
        test_fee_provider_file_is_enforced,
        test_order_lifecycle_tracer_file_is_enforced,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 protocol mock payload verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
