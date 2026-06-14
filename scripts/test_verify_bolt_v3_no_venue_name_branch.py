#!/usr/bin/env python3
"""Unit tests for the FR-080 venue-name string-literal branch fence."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).resolve().parent / "verify_bolt_v3_no_venue_name_branch.py"
_spec = importlib.util.spec_from_file_location("verify_bolt_v3_no_venue_name_branch", SCRIPT_PATH)
VERIFIER = importlib.util.module_from_spec(_spec)
import sys as _sys; _sys.modules[_spec.name] = VERIFIER  # noqa: E702 — needed for dataclass(frozen=True) under Python 3.14
_spec.loader.exec_module(VERIFIER)


class FenceTests(unittest.TestCase):
    def _one(self, snippet: str) -> list:
        return VERIFIER.find_violations_in_text("src/probe.rs", snippet)

    # --- positives: the fence MUST catch each (exactly one violation) ---

    def test_name_eq_literal(self):
        self.assertEqual(len(self._one('if venue_id == "polymarket" {')), 1)

    def test_literal_eq_name(self):
        self.assertEqual(len(self._one('if "BINANCE" == venue.venue_id() {')), 1)

    def test_contains(self):
        self.assertEqual(len(self._one('if venue_name.contains("bybit") {')), 1)

    def test_starts_with_dotted(self):
        self.assertEqual(len(self._one('if self.venue_id.starts_with("okx") {')), 1)

    def test_eq_ignore_ascii_case(self):
        self.assertEqual(len(self._one('if venue.eq_ignore_ascii_case("hyperliquid") {')), 1)

    def test_matches_arm(self):
        self.assertEqual(len(self._one('if matches!(venue_id, "deribit") {')), 1)

    def test_uppercase_literal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id == "POLYMARKET" {')), 1)

    def test_accessor_call_on_left_is_caught(self):
        self.assertEqual(len(self._one('if venue.venue_id() == "binance" {')), 1)

    def test_accessor_chain_method_is_caught(self):
        self.assertEqual(len(self._one('if venue.as_str().starts_with("okx") {')), 1)

    def test_matches_with_accessor_is_caught(self):
        self.assertEqual(len(self._one('if matches!(venue.as_str(), "deribit") {')), 1)

    def test_not_equal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id != "binance" {')), 1)

    # --- raw / byte literal spellings must NOT bypass (false-negative class) ---

    def test_raw_string_literal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id == r"polymarket" {')), 1)

    def test_raw_hashed_string_literal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id == r#"polymarket"# {')), 1)

    def test_byte_string_literal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id.as_bytes() == b"polymarket" {')), 1)

    def test_raw_string_in_method_is_caught(self):
        self.assertEqual(len(self._one('if venue_id.starts_with(r"okx") {')), 1)

    # --- additional idiomatic branch forms ---

    def test_if_let_is_caught(self):
        self.assertEqual(len(self._one('if let "polymarket" = venue_id.as_str() {')), 1)

    def test_turbofish_accessor_is_caught(self):
        self.assertEqual(len(self._one('if venue_id.as_ref::<str>() == "polymarket" {')), 1)

    def test_venue_prefixed_identifier_is_caught(self):
        # `venue`-prefixed reads (venue_wrapper, venue_key) are venue-identity reads.
        self.assertEqual(len(self._one('if venue_wrapper == "polymarket" {')), 1)

    def test_match_arm_with_venue_scrutinee_is_caught(self):
        self.assertEqual(len(self._one('match venue.as_str() { "polymarket" => a, _ => b }')), 1)

    def test_match_arm_with_guard_is_caught(self):
        self.assertEqual(len(self._one('match venue_id { "polymarket" if c => a, _ => b }')), 1)

    def test_match_arm_alternation_first_is_caught(self):
        self.assertEqual(len(self._one('match venue_id { "polymarket" | "x" => a, _ => b }')), 1)

    def test_match_arm_alternation_last_is_caught(self):
        self.assertEqual(len(self._one('match venue_id { "x" | "polymarket" => a, _ => b }')), 1)

    def test_nested_generic_turbofish_is_caught(self):
        self.assertEqual(len(self._one('if venue.cast::<Cow<str>>() == "polymarket" {}')), 1)

    def test_while_let_is_caught(self):
        self.assertEqual(len(self._one('while let "polymarket" = venue_id {}')), 1)

    def test_some_wrapped_if_let_with_venue_read_is_caught(self):
        self.assertEqual(len(self._one('if let Some("polymarket") = venue_id.as_opt() {}')), 1)

    # --- soundness: char literals & lifetimes must not desync the scanner ---

    def test_char_literal_does_not_hide_following_compare(self):
        self.assertEqual(len(self._one('let q = \'"\'; if venue_id == "binance" { }')), 1)

    def test_lifetime_does_not_break_following_compare(self):
        self.assertEqual(len(self._one("fn f<'a>(v: &'a str) { if venue_id == \"binance\" {} }")), 1)

    def test_multiline_raw_string_preserves_following_line_number(self):
        src = 'let d = r#"\nmany\nlines\n"#;\nif venue_id == "binance" { }\n'
        violations = self._one(src)
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 5)

    # --- negatives: the fence MUST NOT flag any of these (false-positive class) ---

    def test_comment_is_not_a_violation(self):
        self.assertEqual(self._one('// venue_id == "polymarket" historical note'), [])

    def test_identifier_substring_is_not_a_violation(self):
        self.assertEqual(self._one('let venue_id_polymarket = 1;'), [])

    def test_arg_position_literal_is_not_a_violation(self):
        self.assertEqual(self._one('fast_spot("bybit", cfg);'), [])

    def test_identifier_prefixed_venue_is_not_a_violation(self):
        # `venue` glued inside a longer identifier must NOT false-positive.
        self.assertEqual(self._one('if subvenue == "binance" {'), [])
        self.assertEqual(self._one('if myVenue == "binance" {'), [])
        self.assertEqual(self._one('if revenue == "binance" {'), [])

    def test_venue_assignment_is_not_a_violation(self):
        # Assigning a venue literal is not a branch.
        self.assertEqual(self._one('let x = "polymarket";'), [])

    def test_raw_hashed_body_phrase_is_not_a_violation(self):
        # FP class: a venue-branch phrase inside a raw-string BODY is data, not code.
        self.assertEqual(self._one('const D: &str = r#"if venue_id == "polymarket" x"#;'), [])

    def test_multiline_raw_hashed_doc_const_is_not_a_violation(self):
        src = 'const NOTE: &str = r#"\nLegacy: venue_id == "polymarket" to branch.\n"#;'
        self.assertEqual(self._one(src), [])

    def test_match_arm_with_non_venue_scrutinee_is_not_a_violation(self):
        # FP class: a benign domain-word arm under a non-venue scrutinee.
        self.assertEqual(self._one('match correction_mode { "gamma" => a, _ => b }'), [])

    def test_nested_non_venue_match_inside_venue_match_is_not_a_violation(self):
        # FP class: a benign nested match must not be flagged by the outer venue match.
        self.assertEqual(
            self._one('match venue_id { _ => { match mode { "gamma" => a, _ => b } } }'), []
        )

    def test_venue_literal_in_arm_body_is_not_a_violation(self):
        # A venue literal returned as an arm body is not a branch on the name.
        self.assertEqual(self._one('match venue_id { _ => "polymarket" }'), [])

    def test_guard_with_non_venue_operand_is_not_a_violation(self):
        # Documented boundary: a guard comparing a non-venue operand is uncaught.
        self.assertEqual(
            self._one('match venue_id { p if some_str == "binance" => a, _ => b }'), []
        )

    def test_bare_arm_without_match_context_is_not_a_violation(self):
        self.assertEqual(self._one('"polymarket" => handle(),'), [])

    def test_char_literal_quote_does_not_false_positive(self):
        self.assertEqual(self._one('let q = \'"\'; let s = "polymarket data";'), [])

    # --- documented completeness boundary (deliberately uncaught) ---

    def test_non_venue_operand_is_a_documented_false_negative(self):
        # Comparing an arbitrarily-named var to a venue literal needs flow analysis.
        self.assertEqual(self._one('if some_str == "polymarket" {'), [])

    def test_constructed_literal_is_a_documented_false_negative(self):
        self.assertEqual(self._one('if venue_id == concat!("poly", "market") {'), [])

    # --- structural ---

    def test_empty_source_set_fails_closed(self):
        with self.assertRaises(RuntimeError):
            VERIFIER.collect_violations_from_files([])

    def test_current_bolt_src_is_clean(self):
        # Preventive fence: there are zero venue-name compares in src today.
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
