#!/usr/bin/env python3
"""Tests for verify_bolt_v3_capital_admission_clamp_consumers.py."""

from __future__ import annotations

import unittest
from pathlib import Path

import verify_bolt_v3_capital_admission_clamp_consumers as verifier


class CapitalAdmissionClampConsumerVerifierTests(unittest.TestCase):
    def test_non_reject_consumer_of_clamped_allowance_fails_closed(self) -> None:
        violations = verifier.find_disallowed_field_references(
            {
                "src/bolt_v3_new_consumer.rs": "\n".join(
                    [
                        "fn admits(snapshot: &PredictionMarketAdmissionSnapshot) -> bool {",
                        "    snapshot.conditional_token_allowance > Decimal::ZERO",
                        "}",
                    ]
                )
            }
        )

        self.assertEqual(len(violations), 1)
        self.assertIn("conditional_token_allowance", violations[0].message)

    def test_non_reject_consumer_of_clamped_position_fails_closed(self) -> None:
        violations = verifier.find_disallowed_field_references(
            {
                "src/bolt_v3_new_consumer.rs": "\n".join(
                    [
                        "fn exits(snapshot: &PredictionMarketAdmissionSnapshot) -> bool {",
                        "    snapshot.yes_position >= Decimal::ONE",
                        "}",
                    ]
                )
            }
        )

        self.assertEqual(len(violations), 1)
        self.assertIn("yes_position", violations[0].message)

    def test_existing_sell_reject_reads_are_allowlisted(self) -> None:
        source = Path(verifier.REPO_ROOT / verifier.CAPITAL_ADMISSION).read_text(
            encoding="utf-8"
        )

        violations = verifier.find_disallowed_field_references(
            {verifier.CAPITAL_ADMISSION: source}
        )

        self.assertEqual(violations, [])

    def test_reject_path_must_keep_allowance_and_position_guards(self) -> None:
        unsafe_source = "\n".join(
            [
                "match request.side {",
                "    IntentSide::Sell => {",
                "        return Ok(LiabilityQuote::fixture());",
                "    }",
                "    _ => {}",
                "}",
            ]
        )

        messages = [
            violation.message
            for violation in verifier.reject_path_violations(unsafe_source)
        ]

        self.assertIn(
            "sell allowance reject path must read conditional_token_allowance before permit",
            messages,
        )
        self.assertIn(
            "sell position reject path must compare outcome_position before permit",
            messages,
        )

    def test_current_tree_has_no_clamp_consumer_violations(self) -> None:
        self.assertEqual(verifier.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
