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
    current_source = {
        str(path.relative_to(VERIFIER.REPO_ROOT))
        for path in (VERIFIER.REPO_ROOT / "src").rglob("*.rs")
    }
    missing_current_source = current_source - scanned
    extra = scanned - current_source
    if missing_current_source or extra:
        raise AssertionError(
            "scan universe must match current src/**/*.rs exactly; "
            f"missing={sorted(missing_current_source)}, extra={sorted(extra)}"
        )

    required = {
        "src/main.rs",
        "src/bin/stream_to_lake.rs",
        "src/bolt_v3_adapters.rs",
        "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs",
        "src/bolt_v3_archetypes/mod.rs",
        "src/bolt_v3_client_registration.rs",
        "src/bolt_v3_config.rs",
        "src/bolt_v3_decision_evidence.rs",
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_market_families/mod.rs",
        "src/bolt_v3_market_families/updown.rs",
        "src/bolt_v3_instrument_filters.rs",
        "src/bolt_v3_no_submit_readiness.rs",
        "src/bolt_v3_no_submit_readiness_schema.rs",
        "src/bolt_v3_providers/binance.rs",
        "src/bolt_v3_providers/mod.rs",
        "src/bolt_v3_providers/polymarket.rs",
        "src/bolt_v3_providers/polymarket/fees.rs",
        "src/bolt_v3_readiness.rs",
        "src/bolt_v3_secrets.rs",
        "src/bolt_v3_strategy_registration.rs",
        "src/bolt_v3_submit_admission.rs",
        "src/bolt_v3_tiny_canary_evidence.rs",
        "src/bolt_v3_validate.rs",
        "src/bounded_config_read.rs",
        "src/execution_state.rs",
        "src/lake_batch.rs",
        "src/lib.rs",
        "src/log_sweep.rs",
        "src/nt_runtime_capture.rs",
        "src/raw_types.rs",
        "src/secrets.rs",
        "src/strategies/binary_oracle_edge_taker.rs",
        "src/strategies/mod.rs",
        "src/strategies/registry.rs",
        "src/venue_contract.rs",
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
        '#[cfg(test)]\nfn helper() { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(all(test, feature = "fixture"))]\nfn helper() { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(any(test))]\nfn helper() { const X: &str = "skip"; }\nconst Y: &str = "keep";':
            ['"keep"'],
        '#[cfg(any(test, unix))]\nfn helper() { const X: &str = "keep_feature"; }':
            ['"keep_feature"'],
        '#[cfg(test)]\nfn helper() {}\nmod tests { const X: &str = "keep_test_like"; }':
            ['"keep_test_like"'],
    }
    for source, expected in cases.items():
        emitted = [literal.literal for literal in scan_source(source)]
        if emitted != expected:
            raise AssertionError(f"expected {expected}, got {emitted} for {source!r}")


def test_cfg_test_item_stripping_keeps_following_production_literals() -> None:
    assert_emits(
        """
        /*
        #[cfg(test)]
        */
        pub const PRODUCTION_AFTER_COMMENT: &str = "phase9_comment_probe";
        """,
        '"phase9_comment_probe"',
    )
    assert_emits(
        r'''
        pub const FIXTURE_TEXT: &str = "
        #[cfg(test)]
        ";

        pub const PRODUCTION_AFTER_STRING: &str = "phase9_string_probe";
        ''',
        '"phase9_string_probe"',
    )
    assert_emits(
        """
        struct FixtureFields {
            live_field: i32,
            #[cfg(test)]
            fixture_field: i32
        }

        pub const PRODUCTION_AFTER_FIELD: &str = "phase9_field_probe";
        """,
        '"phase9_field_probe"',
    )
    assert_emits(
        """
        enum FixtureVariants {
            LiveVariant,
            #[cfg(test)]
            FixtureVariant
        }

        pub const PRODUCTION_AFTER_VARIANT: &str = "phase9_variant_probe";
        """,
        '"phase9_variant_probe"',
    )


def test_bypass_shapes_emit() -> None:
    assert_emits(
        """
        pub fn tuple_probe() {
            let _x = [("custom_runtime_key", 999u32)];
        }
        pub const BARE: &str = "custom_runtime_key";
        pub const TYPED: u32 = 30u32;
        pub const ZERO: i64 = 0_i64;
        pub const NEGATIVE: i64 = -1_i64;
        pub const FLOAT: f64 = 1.5_f64;
        pub fn subtract(value: i64) -> i64 { value - 2 }
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
        "-1_i64",
        "1.5_f64",
        "2",
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


def test_multiline_log_diagnostics_are_ignored_by_callsite() -> None:
    assert_no_emits(
        """
        pub fn log_probe(strategy_id: &str, value: &str) {
            log::warn!(
                "custom_strategy diagnostic value={:?}",
                strategy_id,
                value,
            );
        }
        """,
        '"custom_strategy diagnostic value={:?}"',
    )
    assert_no_emits(
        """
        pub fn tracing_probe(strategy_id: &str, value: &str) {
            tracing::error!(
                "custom_strategy diagnostic value={}",
                strategy_id,
                value,
            );
        }
        """,
        '"custom_strategy diagnostic value={}"',
    )
    assert_no_emits(
        """
        pub fn brace_log_probe(strategy_id: &str, value: &str) {
            log::warn! {
                "custom_strategy brace diagnostic value={}",
                strategy_id,
                value,
            };
        }
        """,
        '"custom_strategy brace diagnostic value={}"',
    )
    emitted = scan_source(
        """
        pub fn multiline_string_log_probe(strategy_id: &str) {
            log::warn!(
                "
                phase9_log_multiline_template={}
                ",
                strategy_id,
            );
        }
        """
    )
    if any("phase9_log_multiline_template" in literal.literal for literal in emitted):
        raise AssertionError(f"expected multiline log template to be ignored, got {emitted!r}")


def test_strategy_prefixed_placeholder_literals_are_not_name_bypassed() -> None:
    assert_emits(
        """
        pub fn policy_probe() {
            let label = "binary_oracle_edge_taker threshold=0.5 {}";
            let debug = "binary_oracle_edge_taker state={:?}";
        }
        """,
        '"binary_oracle_edge_taker threshold=0.5 {}"',
        '"binary_oracle_edge_taker state={:?}"',
    )


def test_multiline_string_literals_do_not_inherit_prior_log_callsite() -> None:
    emitted = scan_source(
        """
        pub fn earlier_log(strategy_id: &str) {
            log::warn!(
                "custom_strategy prior diagnostic value={}",
                strategy_id,
            );
        }

        pub const POLICY: &str = "
        phase9_multiline_policy={}
        ";
        """
    )
    if not any("phase9_multiline_policy" in literal.literal for literal in emitted):
        raise AssertionError(f"expected multiline policy literal to emit, got {emitted!r}")
    invalid_lines = [literal for literal in emitted if literal.line <= 0]
    if invalid_lines:
        raise AssertionError(f"literal lines must be positive, got {invalid_lines!r}")


def test_non_log_macro_literals_are_not_callsite_bypassed() -> None:
    assert_emits(
        """
        pub fn non_log_macro_probe(strategy_id: &str) {
            custom_warn!(
                "phase9_non_log_macro_policy={}",
                strategy_id,
            );
        }
        """,
        '"phase9_non_log_macro_policy={}"',
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


def test_provider_credential_log_modules_are_provider_scoped() -> None:
    scratch = VERIFIER.REPO_ROOT / ".tmp_verify_bolt_v3_runtime_literals_audit.toml"
    original_audit_path = VERIFIER.AUDIT_PATH
    invalid_paths = [
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_providers/mod.rs",
    ]
    try:
        for invalid_path in invalid_paths:
            scratch.write_text(
                f"""
[[allowed]]
path = "{invalid_path}"
kind = "string"
literal = "\\"nautilus_polymarket::common::credential\\""
context = "const MODULE: &str = \\"nautilus_polymarket::common::credential\\";"
classification = "provider_credential_log_module"
reason = "invalid probe"
""".lstrip(),
                encoding="utf-8",
            )
            VERIFIER.AUDIT_PATH = scratch
            try:
                VERIFIER.load_allowed()
            except ValueError as error:
                if "provider_credential_log_module must be owned" not in str(error):
                    raise AssertionError(f"unexpected error: {error}") from error
            else:
                raise AssertionError(
                    f"expected provider_credential_log_module scope failure for {invalid_path}"
                )
    finally:
        VERIFIER.AUDIT_PATH = original_audit_path
        scratch.unlink(missing_ok=True)


def main() -> int:
    tests = [
        test_scan_universe,
        test_cfg_test_module_ranges,
        test_cfg_test_item_stripping_keeps_following_production_literals,
        test_bypass_shapes_emit,
        test_context_shape_bypasses_emit,
        test_multiline_log_diagnostics_are_ignored_by_callsite,
        test_strategy_prefixed_placeholder_literals_are_not_name_bypassed,
        test_multiline_string_literals_do_not_inherit_prior_log_callsite,
        test_non_log_macro_literals_are_not_callsite_bypassed,
        test_not_diagnostic_guard_is_word_bounded,
        test_char_literals_do_not_corrupt_scanning,
        test_allowlist_exactness,
        test_provider_credential_log_modules_are_provider_scoped,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 runtime literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
