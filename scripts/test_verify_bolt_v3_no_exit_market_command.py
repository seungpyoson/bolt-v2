#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 ExitMarket command fence."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_no_exit_market_command.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_no_exit_market_command",
    SCRIPT_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class NoExitMarketCommandFenceTests(unittest.TestCase):
    def test_detects_strategy_command_exit_market(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            command_tx.send(StrategyCommand::ExitMarket {
                strategy_id,
                instrument_id,
            })?;
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].label, "NT ExitMarket command sender")
        self.assertEqual(violations[0].line, 2)

    def test_detects_imported_exit_market_variant(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            use nautilus_trader::system::commands::StrategyCommand::ExitMarket;
            sender.send(ExitMarket { strategy_id, instrument_id })?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3})

    def test_detects_snake_case_nt_market_exit_apis(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            trader.market_exit_strategy(&strategy_id)?;
            controller.exit_market(&instrument_id)?;
            strategy.market_exit()?;
            Strategy::market_exit(&mut strategy)?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3, 4, 5})

    def test_detects_direct_nt_market_exit_lifecycle_apis(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            self.cancel_all_orders(instrument_id, None)?;
            self.close_all_positions(instrument_id, None)?;
            self.close_position(position_id, None)?;
            Strategy::close_position(self, position_id, None)?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3, 4, 5})

    def test_detects_raw_identifier_lifecycle_calls(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            self.r#market_exit()?;
            Strategy::r#market_exit(self)?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3})

    def test_detects_function_item_lifecycle_references(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            let strategy_exit = Strategy::market_exit;
            let trader_exit = Trader::market_exit_strategy;
            let closer = Strategy::close_all_positions;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3, 4})

    def test_detects_other_nt_venue_mutating_apis(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            self.submit_order_list(order_list)?;
            self.cancel_orders(client_order_ids, None)?;
            self.modify_order(client_order_id, params)?;
            self.modify_orders(updates, None, None)?;
            Strategy::submit_order_list(self, order_list)?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3, 4, 5, 6})

    def test_detects_transitive_market_exit_lifecycle_apis(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/strategies/probe.rs",
            """
            self.reset_market_exit_state();
            Strategy::check_market_exit(self, event);
            <Self as Strategy>::stop(self);
            Strategy::on_time_event(self, &event)?;
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3, 4, 5})

    def test_policy_module_allows_only_routed_chokepoint_apis(self) -> None:
        # The chokepoint file exempts ONLY the APIs Bolt routes through the
        # shadow-mode execution-policy gate (cancel_all_orders and the newly-added
        # modify_order). Every other venue-mutating API (close_position, ...) still
        # fails even here, so the exemption is per-API, never a blanket file pass.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_order_execution.rs",
            """
            self.cancel_all_orders(instrument_id, None, Some(client_id), None)?;
            self.modify_order(client_order_id, qty, price, None, client_id, None)?;
            self.close_position(position_id, Some(client_id), None)?;
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 4)
        self.assertEqual(violations[0].label, "NT venue-mutating lifecycle API")
        self.assertIn("close_position", violations[0].excerpt)

    def test_policy_module_modify_order_exemption_is_scoped_to_chokepoint(self) -> None:
        # modify_order is exempt ONLY in the chokepoint file; the same call in any
        # other module is still a bypass and must fail.
        violations = VERIFIER.find_violations_in_text(
            "src/strategies/binary_oracle_maker/mod.rs",
            """
            self.modify_order(client_order_id, qty, price, None, client_id, None)?;
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)
        self.assertEqual(violations[0].label, "NT venue-mutating lifecycle API")

    def test_chokepoint_exemption_is_exact_match_not_substring(self) -> None:
        # Unit-tests the `is_routed_chokepoint_api` exemption contract directly:
        # exact set membership, never a substring test. The exact routed APIs are
        # exempt...
        self.assertTrue(VERIFIER.is_routed_chokepoint_api("modify_order"))
        self.assertTrue(VERIFIER.is_routed_chokepoint_api("cancel_all_orders"))
        # ...but a DIFFERENT, unrouted API that merely embeds an allowed name as a
        # substring is NOT exempted. NOTE: this is a forward-proofing contract test,
        # not a guard against a currently-reachable integration bypass. Through the
        # real pipeline `match.group("api")` is always exactly one listed token (the
        # forbidden regex's `(?:\.|::)` / `(?![A-Za-z0-9_])` boundary rejects
        # impostor names like `force_modify_order` before they reach this function —
        # that boundary is covered by `test_identifier_rules_do_not_match_substrings
        # _or_comments`). A prior substring form was therefore not exploitable via
        # the pipeline; pinning the exemption to exact membership keeps the allowlist
        # from silently widening to a near-miss name if the regex ever changes.
        for impostor in (
            "force_modify_order",
            "modify_order_internal",
            "cancel_all_orders_bypass",
            "cancel_orders",
        ):
            self.assertFalse(
                VERIFIER.is_routed_chokepoint_api(impostor),
                f"{impostor} must not be treated as a routed chokepoint API",
            )

    def test_policy_module_does_not_exempt_near_miss_api_names(self) -> None:
        # `cancel_orders` is a different (unrouted) API from the allowed
        # `cancel_all_orders`; it must still fail even inside the chokepoint file.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_order_execution.rs",
            """
            self.cancel_orders(client_order_ids, None)?;
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)
        self.assertEqual(violations[0].label, "NT venue-mutating lifecycle API")
        self.assertIn("cancel_orders", violations[0].excerpt)

    def test_identifier_rules_do_not_match_substrings_or_comments(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            // StrategyCommand::ExitMarket is allowed in comments.
            let last_exit_market_command = "documented elsewhere";
            let command = StrategyCommand::ExitMarketDisabled;
            let market_exit_interval_ms = 250;
            let market_exit_max_attempts = 7;
            self.market_exit_disabled();
            self.not_market_exit();
            self.close_position_limit();
            sender.send(NotExitMarket { strategy_id })?;
            """,
        )

        self.assertEqual(violations, [])

    def test_collect_scan_strips_cfg_test_items_before_matching(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            probe = Path(temp_dir) / "probe.rs"
            probe.write_text(
                """
                #[cfg(test)]
                mod tests {
                    fn helper() {
                        self.market_exit().unwrap();
                    }
                }

                fn production() {
                    self.close_position(position_id, None)?;
                }
                """,
                encoding="utf-8",
            )

            violations = VERIFIER.find_violations_in_text(
                "src/probe.rs",
                VERIFIER.production_text(probe),
            )

        self.assertEqual({violation.line for violation in violations}, {10})

    def test_empty_source_file_set_fails_closed(self) -> None:
        violations = VERIFIER.collect_violations_from_files([])
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].label, "Rust source files under src: enforcement set is empty")

    def test_current_bolt_src_has_no_exit_market_command_senders(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
