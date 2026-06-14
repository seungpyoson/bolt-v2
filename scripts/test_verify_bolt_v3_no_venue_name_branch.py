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

    def test_comment_is_not_a_violation(self):
        self.assertEqual(self._one('// venue_id == "polymarket" historical note'), [])

    def test_identifier_substring_is_not_a_violation(self):
        self.assertEqual(self._one('let venue_id_polymarket = 1;'), [])

    def test_arg_position_literal_is_not_a_violation(self):
        self.assertEqual(self._one('fast_spot("bybit", cfg);'), [])

    def test_accessor_call_on_left_is_caught(self):
        # Most idiomatic form: a venue-name accessor method on the LEFT of ==.
        self.assertEqual(len(self._one('if venue.venue_id() == "binance" {')), 1)

    def test_accessor_chain_method_is_caught(self):
        self.assertEqual(len(self._one('if venue.as_str().starts_with("okx") {')), 1)

    def test_matches_with_accessor_is_caught(self):
        self.assertEqual(len(self._one('if matches!(venue.as_str(), "deribit") {')), 1)

    def test_bare_match_arm_is_caught(self):
        self.assertEqual(len(self._one('"polymarket" => handle(),')), 1)

    def test_identifier_prefixed_venue_is_not_a_violation(self):
        # `venue` glued inside a longer identifier must NOT false-positive.
        self.assertEqual(self._one('if subvenue == "binance" {'), [])
        self.assertEqual(self._one('if myVenue == "binance" {'), [])

    def test_non_venue_match_arm_is_not_a_violation(self):
        self.assertEqual(self._one('"foo" => handle(),'), [])

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
