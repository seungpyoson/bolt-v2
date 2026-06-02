#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 strategy policy fence."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
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
            let flatten = BoltV3KillSwitchFlattenSupervisor;
            let plan = flatten.plan_flatten(request);
            """
        )

        self.assertIn("dead runtime-selection bus path", labels)
        self.assertIn("inline updown NT metadata interpretation", labels)
        self.assertIn("fixed long-only position contract tuple", labels)
        self.assertIn("buy-only entry VWAP helper", labels)
        self.assertIn("buy-biased entry price block", labels)
        self.assertIn("strategy-local kill switch policy", labels)
        self.assertIn("direct kill-switch action bypass", labels)
        self.assertIn("global kill-switch flatten supervisor policy", labels)

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

    def test_detects_global_cancel_supervisor_imports_and_calls(self) -> None:
        labels = self.labels_for(
            """
            use crate::bolt_v3_kill_switch_cancel::BoltV3KillSwitchCancelSupervisor;
            let supervisor = BoltV3KillSwitchCancelSupervisor::new(policy);
            let plan = supervisor.plan_cancel(request);
            """
        )

        self.assertIn("global kill-switch cancel supervisor policy", labels)

    def test_detects_global_flatten_supervisor_imports_and_calls(self) -> None:
        labels = self.labels_for(
            """
            use crate::bolt_v3_kill_switch_flatten::BoltV3KillSwitchFlattenSupervisor;
            let flatten_supervisor = BoltV3KillSwitchFlattenSupervisor;
            let plan = flatten_supervisor.plan_flatten(request);
            """
        )

        self.assertIn("global kill-switch flatten supervisor policy", labels)

    def test_current_strategy_has_no_policy_hardcode_violations(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])

    def test_collect_violations_rejects_oversized_strategy_source_before_reading(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            repo_root = Path(directory)
            strategy_path = repo_root / VERIFIER.STRATEGY_PATH
            strategy_path.parent.mkdir(parents=True)
            strategy_path.write_text(
                "x" * (VERIFIER.MAX_STRATEGY_SOURCE_BYTES + 1),
                encoding="utf-8",
            )

            original_repo_root = VERIFIER.REPO_ROOT
            try:
                VERIFIER.REPO_ROOT = repo_root
                with self.assertRaises(VERIFIER.StrategyPolicyFenceReadError):
                    VERIFIER.collect_violations()
            finally:
                VERIFIER.REPO_ROOT = original_repo_root


if __name__ == "__main__":
    unittest.main()
