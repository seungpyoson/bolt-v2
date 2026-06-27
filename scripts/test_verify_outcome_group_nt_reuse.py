#!/usr/bin/env python3
"""Task 0 self-tests for verify_outcome_group_nt_reuse.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_outcome_group_nt_reuse.py")
SPEC = importlib.util.spec_from_file_location("verify_outcome_group_nt_reuse", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


NT_REV = "6be5a5094716790a8ca2875445fde4fa2586107e"
BOLT_REV = "5f39d352c081446f309605e49d6beaba86931ca5"


def valid_ledger(capability_overrides: str = "") -> str:
    entries = []
    for capability in VERIFIER.REQUIRED_CAPABILITIES:
        disposition = "wrap_nt" if capability == "order_book_depth" else "reuse_nt"
        reason = f"{capability} uses pinned NT surfaces."
        required_tests = f'"{capability} reuse regression"'
        extra = ""
        if capability == "settlement_signals":
            disposition = "reject_for_now"
            reason = (
                "Polymarket close signals require subscribe_new_markets=true at the pinned "
                "NT revision, which Bolt rejects for controlled-connect scope."
            )
        if capability == "neg_risk_market_id":
            disposition = "surface_in_nt"
            reason = (
                "NT parses negRiskMarketID but does not surface it through BinaryOption.info "
                "at the pinned revision; first slice must use one accepted surfacing path."
            )
        entries.append(
            f"""
            [capabilities.{capability}]
            disposition = "{disposition}"
            owner_module = "src/bolt_v3_outcome_groups.rs"
            reason = "{reason}"
            required_tests = [{required_tests}]

            [[capabilities.{capability}.source_anchors]]
            repo = "nautilus_trader"
            rev = "{NT_REV}"
            path = "crates/model/src/orders/list.rs"
            lines = "1-8"
            symbol = "{capability}"
            evidence = "Pinned NT evidence for {capability}."

            [[capabilities.{capability}.source_anchors]]
            repo = "bolt-v2"
            rev = "{BOLT_REV}"
            path = "src/bolt_v3_live_node.rs"
            lines = "2058-2063"
            symbol = "{capability}"
            evidence = "Pinned Bolt evidence for {capability}."
            """
        )
    return textwrap.dedent(
        f"""
        # Outcome Group NT Evidence

        ```toml outcome_group_nt_capability_ledger
        [ledger]
        version = 1
        nt_revision = "{NT_REV}"
        bolt_revision = "{BOLT_REV}"

        {''.join(entries)}
        {capability_overrides}
        ```
        """
    )


def good_execution_source() -> str:
    return """
        use nautilus_model::orders::OrderList;
        use nautilus_model::messages::execution::SubmitOrderList;
        use nautilus_model::messages::execution::{CancelOrder, ModifyOrder};
        use crate::bolt_v3_executable_cost::ExecutableBookQuote;
        use nautilus_model::data::OrderBookDepth10;

        fn submit_basket(command: SubmitOrderList, list: OrderList) {
            let _ = (command, list);
        }

        fn repair(cancel: CancelOrder, modify: ModifyOrder) {
            let _ = (cancel, modify);
        }

        fn scanner(depth: &OrderBookDepth10, quote: ExecutableBookQuote<'_>) {
            let _ = (depth, quote);
        }

        struct BasketStore {
            basket_id: String,
            order_list_id: String,
            admission_evidence_sha256: String,
        }
    """


def write_fixture(
    root: Path,
    ledger_text: str | None = None,
    sources: dict[str, str] | None = None,
    justfile_text: str | None = None,
) -> None:
    evidence = root / "docs/superpowers/plans/2026-06-13-outcome-group-nt-evidence.md"
    evidence.parent.mkdir(parents=True, exist_ok=True)
    evidence.write_text(ledger_text if ledger_text is not None else valid_ledger(), encoding="utf-8")

    default_sources = {}
    for relative_root in VERIFIER.OUTCOME_GROUP_SOURCE_ROOTS:
        if relative_root.endswith(".rs"):
            default_sources[relative_root] = good_execution_source()
        else:
            default_sources[f"{relative_root}/mod.rs"] = good_execution_source()

    default_sources.update(sources or {})
    for rel, source in default_sources.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(source), encoding="utf-8")

    (root / "Justfile").write_text(
        justfile_text
        if justfile_text is not None
        else textwrap.dedent(
            """
            source-fence-static:
                python3 scripts/test_verify_outcome_group_nt_reuse.py
                python3 scripts/verify_outcome_group_nt_reuse.py
            """
        ),
        encoding="utf-8",
    )


class OutcomeGroupNtReuseVerifierTests(unittest.TestCase):
    def collect(self, *, ledger_text: str | None = None, sources: dict[str, str] | None = None) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_fixture(root, ledger_text=ledger_text, sources=sources)
            return VERIFIER.collect_findings(root)

    def assert_has_finding(self, findings: list[str], needle: str) -> None:
        self.assertTrue(
            any(needle in finding for finding in findings),
            f"expected finding containing {needle!r}, got {findings!r}",
        )

    def test_outcome_group_source_files_use_shared_registry(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_fixture(root)

            actual = [
                path.relative_to(root).as_posix()
                for path in VERIFIER.outcome_group_source_files(root)
            ]
            expected = [
                path.relative_to(root).as_posix()
                for path in VERIFIER.source_set_files(
                    VERIFIER.OUTCOME_GROUP_SOURCE_ROOTS, repo_root=root
                )
            ]

        self.assertEqual(actual, expected)

    def test_comment_only_submit_order_list_with_per_leg_submit_loop_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_basket_execution.rs": """
                    // SubmitOrderList
                    fn submit_each_leg(strategy: &mut Strategy, legs: Vec<OrderAny>) {
                        for leg in legs {
                            strategy.submit_order(leg);
                        }
                    }
                """
            }
        )

        self.assert_has_finding(findings, "must reference NT OrderList/SubmitOrderList")
        self.assert_has_finding(findings, "per-leg submit loop")

    def test_custom_scanner_order_book_struct_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_outcome_group_scanner.rs": """
                    struct LocalOrderBook {
                        bids: std::collections::BTreeMap<Price, Quantity>,
                        asks: std::collections::BTreeMap<Price, Quantity>,
                    }
                """
            }
        )

        self.assert_has_finding(findings, "scanner must reference NT book/depth primitives")
        self.assert_has_finding(findings, "custom order-book model")

    def test_direct_per_leg_venue_submit_path_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_basket_execution.rs": """
                    use nautilus_model::orders::OrderList;
                    use nautilus_model::messages::execution::SubmitOrderList;

                    fn submit_direct(client: &PolymarketExecutionClient, legs: Vec<OrderAny>) {
                        for leg in legs {
                            client.submit_order(leg);
                        }
                    }
                """
            }
        )

        self.assert_has_finding(findings, "direct venue submit path")

    def test_complete_set_archetype_is_not_treated_as_execution_shell(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_archetypes/complete_set_arbitrage.rs": """
                    pub fn validate_strategy() {}
                """
            }
        )

        self.assertEqual(findings, [])

    def test_direct_venue_cancel_path_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/strategies/complete_set_arbitrage/repair.rs": """
                    fn cancel_direct(clob: &PolymarketClobClient, order_id: &str) {
                        clob.cancel_order(order_id);
                    }
                """
            }
        )

        self.assert_has_finding(findings, "repair/unwind must reference NT cancel/modify commands")
        self.assert_has_finding(findings, "direct venue cancel path")

    def test_general_order_cache_duplication_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_basket_store.rs": """
                    struct BasketStore {
                        order_history: Vec<OrderStatusReport>,
                        orders: std::collections::HashMap<ClientOrderId, OrderAny>,
                    }
                """
            }
        )

        self.assert_has_finding(findings, "general order-cache/history model")

    def test_opaque_proof_variant_branch_outside_model_or_normalizer_fails(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_basket_admission.rs": """
                    use crate::bolt_v3_outcome_groups::GroupingProof;

                    fn admit(grouping_proof: &GroupingProof) -> bool {
                        matches!(grouping_proof, GroupingProof::PolymarketNegRisk { .. })
                    }
                """
            }
        )

        self.assert_has_finding(findings, "opaque outcome-group proof variant branch")

    def test_nt_wrapping_fixture_passes(self) -> None:
        findings = self.collect(
            sources={
                "src/bolt_v3_basket_execution.rs": good_execution_source(),
                "src/bolt_v3_outcome_group_scanner.rs": good_execution_source(),
                "src/bolt_v3_basket_repair.rs": good_execution_source(),
                "src/bolt_v3_basket_store.rs": good_execution_source(),
                "src/strategies/complete_set_arbitrage/mod.rs": """
                    fn nt_submit_contract() {
                        crate::bolt_v3_order_execution::nt_order_management_contract();
                    }
                """,
            }
        )

        self.assertEqual(findings, [])

    def test_source_fence_static_wiring_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_fixture(
                root,
                sources={"src/bolt_v3_basket_execution.rs": good_execution_source()},
                justfile_text="source-fence-static:\n    python3 scripts/other.py\n",
            )

            findings = VERIFIER.collect_findings(root)

        self.assert_has_finding(findings, "source-fence-static must run python3 scripts/test_verify_outcome_group_nt_reuse.py")
        self.assert_has_finding(findings, "source-fence-static must run python3 scripts/verify_outcome_group_nt_reuse.py")


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
