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
    def violations_for(
        self, source: str, path: str = "src/strategies/probe.rs"
    ) -> list[object]:
        return VERIFIER.find_violations_in_text(path, source)

    def labels_for(self, source: str) -> set[str]:
        return {violation.label for violation in self.violations_for(source)}

    def direct_nt_violations_for(
        self, source: str, path: str = "src/strategies/probe.rs"
    ) -> list[object]:
        return [
            violation
            for violation in self.violations_for(source, path=path)
            if violation.label == "direct NT venue mutation call"
        ]

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
            self.cancel_all_orders();
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

    def test_detects_direct_nt_venue_mutation_calls_from_strategy_source(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            self.submit_order(order, None, Some(client_id), None)?;
            self.submit_order_list(order_list, None, Some(client_id), None)?;
            self.modify_order(client_order_id, None, None, None, None)?;
            self.cancel_order(client_order_id, Some(client_id), None)?;
            self.cancel_orders(&client_order_ids, Some(client_id), None)?;
            self.cancel_all_orders(None, Some(client_id), None)?;
            self.close_position(instrument_id, position_id, Some(client_id), None)?;
            self.close_all_positions(instrument_id, Some(client_id), None)?;
            """
        )

        self.assertEqual(
            len(direct_violations),
            8,
            "every current NT Strategy venue mutation API must be detected",
        )

    def test_detects_ufcs_direct_nt_venue_mutation_calls_from_strategy_source(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            <Self as Strategy>::submit_order(self, order, None, Some(client_id), None)?;
            <Self as Strategy>::submit_order_list(self, order_list, None, Some(client_id), None)?;
            <Self as Strategy>::modify_order(self, client_order_id, None, None, None, None)?;
            <Self as Strategy>::cancel_order(self, client_order_id, Some(client_id), None)?;
            <Self as Strategy>::cancel_orders(self, &client_order_ids, Some(client_id), None)?;
            <Self as Strategy>::cancel_all_orders(self, None, Some(client_id), None)?;
            <Self as Strategy>::close_position(self, instrument_id, position_id, Some(client_id), None)?;
            <Self as Strategy>::close_all_positions(self, instrument_id, Some(client_id), None)?;
            """
        )

        self.assertEqual(
            len(direct_violations),
            8,
            "every forbidden NT mutation API must be detected through UFCS syntax",
        )

    def test_detects_alias_and_type_qualified_nt_venue_mutation_calls(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            use nautilus_trading::Strategy as NtStrategy;
            NtStrategy::submit_order(self, order, None, Some(client_id), None)?;
            <Self as NtStrategy>::cancel_order(self, client_order_id, Some(client_id), None)?;
            Self::modify_order(self, client_order_id, None, None, None, None)?;
            <BinaryOracleEdgeTaker>::cancel_all_orders(self, instrument_id, None, Some(client_id), None)?;
            let submit = Self::submit_order;
            let cancel = <Self as NtStrategy>::cancel_order;
            """
        )

        self.assertEqual(
            len(direct_violations),
            6,
            "aliases, inherent-qualified forms, and method pointers must be fenced",
        )

    def test_detects_lowercase_alias_qualified_nt_venue_mutation_calls(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            use nautilus_trading::Strategy as nt_strategy;
            nt_strategy::submit_order(self, order, None, Some(client_id), None)?;
            let submit = nt_strategy::submit_order;
            Self::submit_order::<Probe>(self, order, None, Some(client_id), None)?;
            """
        )

        self.assertEqual(
            len(direct_violations),
            3,
            "lowercase aliases and turbofish-qualified mutation calls must be fenced",
        )

    def test_detects_raw_sink_wrapper_methods_outside_policy_boundary(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            self.submit_order_via_nt(order, context)?;
            self.cancel_order_via_nt(client_order_id, Some(client_id), None)?;
            """
        )

        self.assertEqual(
            len(direct_violations),
            2,
            "private NT sink wrapper names must still be fenced outside the policy module",
        )

    def test_detects_future_mutation_method_name_variants(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            self.submit_order_with_params(order, params)?;
            self.submit_order_list_with_params(orders, params)?;
            self.modify_order_with_params(client_order_id, params)?;
            self.cancel_order_with_params(client_order_id, params)?;
            self.cancel_orders_with_params(client_order_ids, params)?;
            self.cancel_all_orders_with_params(instrument_id, params)?;
            self.modify_order_in_place(&mut order, Some(quantity), None, None)?;
            """
        )

        self.assertEqual(
            len(direct_violations),
            7,
            "nearby mutation method variants must be fenced before a future NT bump can use them",
        )

    def test_detects_nt_command_transport_and_managed_lifecycle_mutation_paths(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            self.core_mut();
            self.core_mut().order_manager().send_risk_command(command);
            self.core_mut().order_manager().send_exec_command(command);
            self.core_mut().order_manager().send_emulator_command(command);
            self.core_mut().order_manager().send_algo_command(command);
            StrategyCore::order_manager(core);
            self.expire_gtd_order(event);
            self.reactivate_gtd_timers();
            self.set_gtd_expiry(client_order_id, expiry);
            self.cancel_gtd_expiry(client_order_id);
            self.finalize_market_exit(position_id);
            self.cancel_market_exit(position_id);
            self.deny_order(order);
            self.deny_order_list(order_list);
            """
        )

        self.assertEqual(
            len(direct_violations),
            22,
            "raw command transport and NT-managed lifecycle helpers must be fenced",
        )

    def test_detects_raw_msgbus_trading_command_injection_paths(self) -> None:
        direct_violations = self.direct_nt_violations_for(
            """
            use nautilus_common::msgbus::{self, MessagingSwitchboard};
            use nautilus_common::messages::execution::TradingCommand;

            fn bypass(command: TradingCommand) {
                msgbus::send_trading_command(
                    MessagingSwitchboard::risk_engine_queue_execute(),
                    command,
                );
                send_trading_command(
                    MessagingSwitchboard::exec_engine_queue_execute(),
                    TradingCommand::SubmitOrder(submit),
                );
                let send = msgbus::send_trading_command;
                let send_any = msgbus::send_any;
                let send_any_value = msgbus::send_any_value;
                send_any_value(endpoint, boxed_command);
                send_any(endpoint, boxed_command);
            }
            """
        )

        self.assertEqual(
            len(direct_violations),
            11,
            "raw msgbus trading-command injection must be fenced at the primitive layer",
        )

    def test_detects_strategy_local_execution_policy_construction(self) -> None:
        labels = self.labels_for(
            """
            let live_policy = BoltV3OrderExecutionPolicy::live();
            let shadow_policy = BoltV3OrderExecutionPolicy::shadow();
            let custom_policy = BoltV3OrderExecutionPolicy::from_mode(mode);
            context.with_order_execution_policy(live_policy);
            """
        )

        self.assertIn("strategy-local execution policy construction", labels)
        self.assertIn("strategy-local execution policy override", labels)

    def test_detects_strategy_local_execution_policy_aliases_and_method_pointers(
        self,
    ) -> None:
        labels = self.labels_for(
            """
            use crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy as P;
            type PolicyAlias = BoltV3OrderExecutionPolicy;
            let make_live = P::live;
            let _policy = make_live();
            let _mode = BoltV3OrderExecutionMode::Live;
            """
        )

        self.assertIn("strategy-local execution policy type reference", labels)

    def test_policy_reference_is_repo_wide_not_strategy_path_scoped(self) -> None:
        probe = VERIFIER.REPO_ROOT / "src/rogue_registered_strategy.rs"
        probe.write_text(
            "fn bypass(mode: BoltV3OrderExecutionMode) {\n"
            "    let _policy = BoltV3OrderExecutionPolicy::from_mode(mode);\n"
            "}\n",
            encoding="utf-8",
        )
        try:
            labels = {
                violation.label
                for violation in VERIFIER.collect_violations()
                if violation.path == "src/rogue_registered_strategy.rs"
            }
        finally:
            probe.unlink(missing_ok=True)

        self.assertIn("strategy-local execution policy construction", labels)
        self.assertIn("strategy-local execution policy type reference", labels)

    def test_production_strategy_registry_rejects_outside_strategy_module_builders(
        self,
    ) -> None:
        labels = {
            violation.label
            for violation in self.violations_for(
                "fn production_strategy_registry() -> Result<StrategyRegistry> {\n"
                "    registry.register::<crate::rogue_registered_strategy::RogueBuilder>()?;\n"
                "    Ok(registry)\n"
                "}\n",
                path="src/strategies/mod.rs",
            )
        }

        self.assertIn("registered strategy outside strategy module tree", labels)

    def test_direct_nt_mutation_allowlist_is_exactly_the_policy_module(self) -> None:
        source = """
        self.submit_order(order, None, Some(client_id), None)?;
        self.submit_order_via_nt(order, context)?;
        """

        self.assertEqual(
            self.direct_nt_violations_for(source, path="src/bolt_v3_order_execution.rs"),
            [],
            "the policy module is the only direct mutation allowlist path",
        )
        self.assertEqual(
            len(self.direct_nt_violations_for(source, path="src/strategies/future.rs")),
            2,
            "the same calls must be rejected from strategy code",
        )

    def test_mutation_fence_scans_all_production_src_files(self) -> None:
        scanned = {
            str(path.relative_to(VERIFIER.REPO_ROOT))
            for path in VERIFIER.source_files_for_mutation_fence()
        }

        self.assertIn("src/strategies/mod.rs", scanned)
        self.assertIn("src/bin/shadow_pnl_report.rs", scanned)
        self.assertIn("src/bolt_v3_order_execution.rs", scanned)
        self.assertNotIn(
            "src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs",
            scanned,
        )

    def test_strategy_policy_fence_scans_future_strategy_modules(self) -> None:
        probe = VERIFIER.REPO_ROOT / "src/strategies/__policy_fence_probe.rs"
        probe.write_text(
            "fn bypass(mode: BoltV3OrderExecutionMode) {\n"
            "    let _policy = BoltV3OrderExecutionPolicy::from_mode(mode);\n"
            "}\n",
            encoding="utf-8",
        )
        try:
            violations = [
                violation
                for violation in VERIFIER.collect_violations()
                if violation.path == "src/strategies/__policy_fence_probe.rs"
            ]
        finally:
            probe.unlink(missing_ok=True)

        self.assertIn(
            "strategy-local execution policy construction",
            [violation.label for violation in violations],
            "future strategy modules must not escape strategy-policy rules",
        )

    def test_strategy_source_roots_must_be_digest_gated(self) -> None:
        probe_dir = VERIFIER.REPO_ROOT / "src/strategies/__digest_probe"
        probe = probe_dir / "mod.rs"
        probe_dir.mkdir()
        probe.write_text("pub struct ProbeStrategy;\n", encoding="utf-8")
        try:
            violations = [
                violation
                for violation in VERIFIER.collect_violations()
                if violation.path == "src/strategies/__digest_probe"
            ]
        finally:
            probe.unlink(missing_ok=True)
            probe_dir.rmdir()

        self.assertEqual(
            [violation.label for violation in violations],
            ["ungated production strategy source root"],
            "every production strategy source root must be covered by gated source integrity",
        )

    def test_maker_strategy_source_root_is_recognized_as_gated(self) -> None:
        # The maker is sealed by its own digest (`MAKER_SOURCE_ROOTS`), a
        # separate seal from the taker's `STRATEGY_SOURCE_ROOTS`. The policy
        # fence must still count it as gated; were the gated set derived from the
        # taker tuple alone, this root would be wrongly flagged as ungated.
        self.assertIn(
            "src/strategies/binary_oracle_maker",
            VERIFIER.gated_strategy_source_root_names(),
        )
        self.assertNotIn(
            "src/strategies/binary_oracle_maker",
            VERIFIER.ungated_production_strategy_source_roots(),
        )


    def test_shared_policy_does_not_blanket_impl_raw_sink_for_every_strategy(self) -> None:
        source = (VERIFIER.REPO_ROOT / "src/bolt_v3_order_execution.rs").read_text(
            encoding="utf-8"
        )

        self.assertNotRegex(
            source,
            r"impl\s*<\s*T\s*>\s*BoltV3NtVenueMutationSink\s+for\s+T",
            "raw NT mutation sink must not be blanket-implemented for every Strategy",
        )

    def test_source_roots_include_shared_order_execution_policy(self) -> None:
        self.assertIn(
            "src/bolt_v3_order_execution.rs",
            VERIFIER.STRATEGY_SOURCE_ROOTS,
            "shared order execution policy must stay in the strategy source fence set",
        )

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

    def test_current_strategy_has_no_policy_hardcode_violations(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
