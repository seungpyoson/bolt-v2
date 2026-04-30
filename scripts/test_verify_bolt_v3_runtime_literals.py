#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 runtime literal verifier."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_runtime_literals.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_runtime_literals", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


def scan_source(source: str) -> list[object]:
    scratch = VERIFIER.REPO_ROOT / ".tmp_verify_bolt_v3_runtime_literals"
    if scratch.exists():
        shutil.rmtree(scratch)
    scratch.mkdir()
    path = scratch / "probe.rs"
    try:
        path.write_text(source, encoding="utf-8")
        return [
            literal
            for literal in VERIFIER.scan_file(path)
            if not VERIFIER.is_ignored_by_rule(literal)
        ]
    finally:
        shutil.rmtree(scratch)


def assert_emits(source: str, *expected: str) -> None:
    emitted = {literal.literal for literal in scan_source(source)}
    missing = set(expected) - emitted
    if missing:
        raise AssertionError(f"missing {sorted(missing)} from emitted {sorted(emitted)}")


def assert_no_emits(source: str, *unexpected: str) -> None:
    emitted = {literal.literal for literal in scan_source(source)}
    present = set(unexpected) & emitted
    if present:
        raise AssertionError(f"unexpected {sorted(present)} in emitted {sorted(emitted)}")


def test_scan_universe() -> None:
    scanned = {
        str(path.relative_to(VERIFIER.REPO_ROOT))
        for pattern in VERIFIER.SCAN_GLOBS
        for path in VERIFIER.REPO_ROOT.glob(pattern)
        if path.is_file()
    }
    required = {
        "src/bolt_v3_adapters.rs",
        "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs",
        "src/bolt_v3_archetypes/mod.rs",
        "src/bolt_v3_client_registration.rs",
        "src/bolt_v3_config.rs",
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_market_families/mod.rs",
        "src/bolt_v3_market_families/updown.rs",
        "src/bolt_v3_market_identity.rs",
        "src/bolt_v3_providers/binance.rs",
        "src/bolt_v3_providers/mod.rs",
        "src/bolt_v3_providers/polymarket.rs",
        "src/bolt_v3_secrets.rs",
        "src/bolt_v3_validate.rs",
    }
    missing = required - scanned
    if missing:
        raise AssertionError(f"scan universe missing {sorted(missing)}")


def test_cfg_test_module_ranges() -> None:
    cases = {
        '#[cfg(test)]\nmod tests { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(test)] pub mod tests { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(test)]\npub mod tests { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(test)]\nfn helper() {}\nmod tests { const X: &str = "keep_test_like"; }':
            ['"keep_test_like"'],
    }
    for source, expected in cases.items():
        emitted = [literal.literal for literal in scan_source(source)]
        if emitted != expected:
            raise AssertionError(f"expected {expected}, got {emitted} for {source!r}")


def test_bypass_shapes_emit() -> None:
    assert_emits(
        """
        pub fn tuple_probe() {
            let _x = [("custom_runtime_key", 999u32)];
        }
        pub const BARE: &str = "custom_runtime_key";
        pub const TYPED: u32 = 30u32;
        pub const ZERO: i64 = 0_i64;
        pub const FLOAT: f64 = 1.5_f64;
        pub const HEX: u32 = 0x1F;
        pub const BIN: u32 = 0b1010;
        pub const OCT: u32 = 0o777;
        pub const DIAG: &str = "venue_failed_takes_priority";
        pub const BRACE: &str = "venue_id={}";
        pub const BYTE: &[u8] = b"abc";
        pub const RAW_BYTE: &[u8] = br#"abc"#;
        """,
        '"custom_runtime_key"',
        "999u32",
        "30u32",
        "0_i64",
        "1.5_f64",
        "0x1F",
        "0b1010",
        "0o777",
        '"venue_failed_takes_priority"',
        '"venue_id={}"',
        'b"abc"',
        'br#"abc"#',
    )


def test_context_shape_bypasses_emit() -> None:
    assert_emits(
        """
        pub fn context_shape_probe(value: &str, data: &Data) -> Result<(), &'static str> {
            let _ = value.contains("custom_runtime_key");
            let _ = keys.join("custom_runtime_key");
            let _ = ("custom_runtime_key", data.base_url_http.as_str());
            if cadence_seconds % 60 != 0 {}
            return Err("custom_runtime_key");
        }
        """,
        '"custom_runtime_key"',
        "60",
    )


def test_not_diagnostic_guard_is_word_bounded() -> None:
    assert_emits(
        """
        pub const UNDERSCORE: &str = "not_a_runtime_key";
        pub const SUBWORD: &str = "venue_notional_limit";
        """,
        '"not_a_runtime_key"',
        '"venue_notional_limit"',
    )
    assert_no_emits(
        """
        pub const DIAGNOSTIC: &str = "is not ready";
        """,
        '"is not ready"',
    )


def test_char_literals_do_not_corrupt_scanning() -> None:
    assert_emits(
        """
        const CLOSE: char = '}';
        const QUOTE: char = '"';
        const HEX: char = '\\x41';
        const UNICODE: char = '\\u{41}';
        const BYTE_CLOSE: u8 = b'}';
        const AFTER: &str = "after_chars";
        """,
        '"after_chars"',
    )
    assert_no_emits(
        """
        const CLOSE: char = '}';
        const QUOTE: char = '"';
        const HEX: char = '\\x41';
        const UNICODE: char = '\\u{41}';
        const BYTE_CLOSE: u8 = b'}';
        """,
        '"',
        "'}'",
        "b'}'",
    )


def test_allowlist_exactness() -> None:
    allowed = VERIFIER.load_allowed()
    scanned = {literal.key() for literal in VERIFIER.scan_literals()}
    stale = allowed - scanned
    unclassified = scanned - allowed
    if stale or unclassified:
        raise AssertionError(
            f"allowlist mismatch: stale={sorted(stale)}, unclassified={sorted(unclassified)}"
        )


def main() -> int:
    tests = [
        test_scan_universe,
        test_cfg_test_module_ranges,
        test_bypass_shapes_emit,
        test_context_shape_bypasses_emit,
        test_not_diagnostic_guard_is_word_bounded,
        test_char_literals_do_not_corrupt_scanning,
        test_allowlist_exactness,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 runtime literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
