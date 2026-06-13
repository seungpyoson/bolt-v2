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
            let _ = KillSwitchState::Armed;
            self.kill_switch = true;
            self.forced_reduction_submit(order);
            self.cancel_orders(vec![order_id]);
            self.cancel_all_orders();
            self.close_position(position_id, None, None);
            self.close_all_positions(None, None);
            self.flatten_all_positions();
            """
        )

        self.assertIn("dead runtime-selection bus path", labels)
        self.assertIn("inline updown NT metadata interpretation", labels)
        self.assertIn("fixed long-only position contract tuple", labels)
        self.assertIn("buy-only entry VWAP helper", labels)
        self.assertIn("buy-biased entry price block", labels)
        self.assertIn("strategy-local kill switch policy", labels)
        self.assertIn("direct kill-switch action bypass", labels)

    def test_identifier_rules_do_not_match_substrings(self) -> None:
        labels = self.labels_for(
            """
            let runtime_selection_topic_suffix = "configured";
            not_subscribe_any(topic, handler, None);
            platform.runtime.selection_mode();
            actor.try_get_actor_unchecked_extra();
            book.not_max_buy_execution_within_vwap_slippage_bps(50);
            let not_a_kill_switch_suffix = true;
            self.cancel_allocation();
            """
        )

        self.assertEqual(labels, set())

    def test_detects_nt_batch_cancel_and_close_position_bypass_helpers(self) -> None:
        labels = self.labels_for(
            """
            self.cancel_orders(vec![order_id]);
            self.close_position(position_id, None, None);
            self.close_all_positions(None, None);
            """
        )

        self.assertIn("direct kill-switch action bypass", labels)

    def test_code_rules_ignore_banned_tokens_inside_strings_and_comments(self) -> None:
        # An error/doc string or comment that *names* a banned action is not a code
        # bypass. This mirrors the production archetype validation message that
        # references `close_all_positions` to explain a config rule.
        labels = self.labels_for(
            """
            let _ = make_error(
                "manage_stop=true uses Strategy::close_all_positions market orders; set manage_stop=false to route a non-market forced_exit_order through the forced-flat path",
            );
            // cancel_all_orders and flatten_all_positions live in the supervisor module
            let note = "cancel_orders and close_position are documented helpers";
            """
        )

        self.assertEqual(labels, set())

    def test_string_mention_does_not_mask_adjacent_real_call(self) -> None:
        # A literal mention must not blank out a genuine adjacent code call.
        labels = self.labels_for(
            """
            let msg = "close_all_positions is the NT market-exit path";
            self.close_all_positions(None, None);
            """
        )

        self.assertIn("direct kill-switch action bypass", labels)

    def test_literal_targeting_rule_still_matches_inside_strings(self) -> None:
        # Stripping literals for code rules must not disable the one rule that
        # deliberately targets hardcoded NT metadata string content.
        labels = self.labels_for('let slug = info.get_str("market_slug");')

        self.assertIn("inline updown NT metadata interpretation", labels)

    def test_current_strategy_has_no_policy_hardcode_violations(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
