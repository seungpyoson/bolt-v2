#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 market-family coupling fence."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_market_family_coupling.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_market_family_coupling",
    SCRIPT_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class MarketFamilyCouplingFenceTests(unittest.TestCase):
    def test_detects_static_family_calling_updown_function(self) -> None:
        violations = VERIFIER.find_static_binary_event_violations_in_text(
            "fn maker_quote_targets(inputs: Inputs) { updown::maker_quote_targets(inputs); }"
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 1)

    def test_detects_static_family_importing_updown_module(self) -> None:
        violations = VERIFIER.find_static_binary_event_violations_in_text(
            "use super::{FairProbabilityInputs, updown};\n"
        )

        self.assertEqual(len(violations), 1)

    def test_ignores_comments_and_string_literals(self) -> None:
        violations = VERIFIER.find_static_binary_event_violations_in_text(
            """
            // updown is discussed in a comment.
            const NOTE: &str = "updown appears only in a literal";
            binary_outcome::maker_quote_targets(inputs);
            """
        )

        self.assertEqual(violations, [])

    def test_allows_neutral_binary_outcome_shared_module(self) -> None:
        violations = VERIFIER.find_static_binary_event_violations_in_text(
            """
            use super::{FairProbabilityInputs, binary_outcome};
            fn maker_quote_targets(inputs: Inputs) {
                binary_outcome::maker_quote_targets(inputs);
            }
            """
        )

        self.assertEqual(violations, [])

    def test_current_static_binary_event_has_no_sibling_family_dependency(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
