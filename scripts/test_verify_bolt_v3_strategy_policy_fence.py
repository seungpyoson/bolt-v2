#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 strategy policy fence."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_strategy_policy_fence.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_strategy_policy_fence", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class StrategyPolicyFenceTests(unittest.TestCase):
    def labels_for(self, source: str) -> set[str]:
        return {
            violation.label
            for violation in VERIFIER.find_violations_in_text("probe.rs", source)
        }

    def test_detects_removed_policy_hardcodes(self) -> None:
        labels = self.labels_for(
            """
            subscribe_any(topic, handler, None);
            if info.get_str("market_slug") == Some("x") {}
            matches!((a, b, c, d), (
                OrderSide::Buy,
                PositionSide::Long,
                OrderSide::Sell,
                PositionSide::Long,
            ));
            book.max_buy_execution_within_vwap_slippage_bps(50);
            match side {
                OutcomeSide::Up => self.active.books.up.best_ask,
                OutcomeSide::Down => self.active.books.down.best_ask,
            }
            """
        )

        self.assertIn("dead runtime-selection bus path", labels)
        self.assertIn("inline updown NT metadata interpretation", labels)
        self.assertIn("fixed long-only position contract tuple", labels)
        self.assertIn("buy-only entry VWAP helper", labels)
        self.assertIn("buy-biased entry price block", labels)

    def test_identifier_rules_do_not_match_substrings(self) -> None:
        labels = self.labels_for(
            """
            let runtime_selection_topic_suffix = "configured";
            not_subscribe_any(topic, handler, None);
            platform.runtime.selection_mode();
            actor.try_get_actor_unchecked_extra();
            book.not_max_buy_execution_within_vwap_slippage_bps(50);
            """
        )

        self.assertEqual(labels, set())

    def test_current_strategy_has_no_policy_hardcode_violations(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    unittest.main()
