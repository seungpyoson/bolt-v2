#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 schema-current verifier."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_schema_current.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_schema_current", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


CURRENT_SCHEMA = """
#### `oms_type`

This field delegates accepted values to NautilusTrader `OmsType`.
The current source-level tests prove `netting`, `hedging`, and `unspecified` parse and validate.

### `[parameters.entry_order]`, `[parameters.exit_order]`, and `[parameters.forced_exit_order]`

The current archetype accepts the long position contract only.
Short-side position contracts are parsed but rejected until strategy-owned short economics, collateral, and exit semantics exist.

#### `side`

- `buy`
- `sell`

#### `position_side`

- `long`
- `short`

#### `order_type`

- `limit`
- `market`
- `stop_market`
- `stop_limit`
- `market_if_touched`
- `limit_if_touched`
- `trailing_stop_market`
- `market_to_limit`
- `trailing_stop_limit`

`market_to_limit` and `trailing_stop_limit` are unsupported for this archetype because the pinned NT single-order `OrderFactory` exposes no public constructor.

#### Optional NT order-template fields

- `expire_time_unix_nanos`
- `trigger_price`
- `activation_price`
- `trigger_type`
- `trigger_instrument_id`
- `trailing_offset`
- `trailing_offset_type`

`trigger_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TriggerType::Default`.
`trailing_offset_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TrailingOffsetType::Price`.

Entry `is_quote_quantity = true` is supported by sizing the entry quantity as quote notional.
Exit `is_quote_quantity = true` is rejected because exits are sized from held base position quantity.
Forced-flat exits use the configured `forced_exit_order` template.
When `manage_stop = true`, pinned NautilusTrader `Strategy::close_all_positions` submits market close orders.
Decision-evidence JSONL records use `schema_version = 14` for `order_intent`, `admission_decision`, `strategy_input_snapshot`, `capital_admission_rebuild`, `submit_reservation_metadata`, `submit_reservation_fill`, `entry_skip`, `exit_decision`, `loss_governor_halt`, `requote_throttle`, and `venue_truth_capture_failure` envelopes.
Each line is a single JSON object with `schema_version`, `recorded_at_utc_ns`, `gate_version`, `gate_id`, `kind`, and the matching payload field: `intent`, `decision`, `snapshot`, `audit`, `metadata`, `fill`, `entry_skip`, `exit_decision`, `loss_governor_halt`, `requote_throttle`, or `capture_failure`.
The `kind` field is `order_intent` for `intent` payloads, `admission_decision` for `decision` payloads, `strategy_input_snapshot` for `snapshot` payloads, `capital_admission_rebuild` for startup rebuild audit payloads, `submit_reservation_metadata` for admitted reservation metadata, `submit_reservation_fill` for fill metadata, `entry_skip` for entry skip rationale, `exit_decision` for exit rationale, `loss_governor_halt` for loss-governor halt transitions, `requote_throttle` for maker requote budget throttle transitions, and `venue_truth_capture_failure` for degraded venue REST capture authority evidence.
`strategy_input_snapshot` payloads carry source-bound entry decision inputs captured before order-intent recording.
`capital_admission_rebuild`, `submit_reservation_metadata`, and `submit_reservation_fill` payloads support startup reservation recovery and fail closed on pre-schema-14 reservation records.

### `[parameters]`
"""

CURRENT_STATUS_MAP = """
Strategy `oms_type` delegates to NT `OmsType` variants instead of a Bolt-only netting allowlist.
Configured order-template validation follows the pinned NT single-order `OrderFactory` surface.
Order construction uses the shared `src/bolt_v3_order_intent.rs` builder for entry, exit, and configured `[parameters.forced_exit_order]` templates.
"""

CURRENT_RUNTIME_CONTRACTS = """
Order-submission and pre-submit rejection events map compiled NT order-template fields through
`order_fields`: `expire_time_unix_nanos`, `trigger_price`, `activation_price`, `trigger_type`,
`trigger_instrument_id`, `trailing_offset`, and `trailing_offset_type`.
"""


def test_extract_section_stops_at_next_matching_heading() -> None:
    section = VERIFIER.extract_section(
        """
#### `one`
body one
#### `two`
body two
""",
        "`one`",
    )
    if "body one" not in section or "body two" in section:
        raise AssertionError(f"unexpected section extraction: {section!r}")


def test_validate_docs_accepts_current_terms() -> None:
    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP)
    if findings:
        raise AssertionError(f"expected no findings, got {findings!r}")


def test_validate_docs_checks_decision_evidence_schema_version_source() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        decision_evidence_source="pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = 11;",
    )

    if "schema missing decision-evidence JSONL schema v11 contract" not in findings:
        raise AssertionError(f"expected decision-evidence schema source drift finding, got {findings!r}")


def test_validate_docs_rejects_superseded_tuple_policy_and_netting_only_status() -> None:
    stale_schema = CURRENT_SCHEMA + """
- current allowed value:
  - `netting`
To avoid hidden policy, the current archetype supports only these combinations:
Any other combination fails validation for this archetype.
"""
    stale_status = (
        CURRENT_STATUS_MAP
        + "Single-value enums (`RuntimeMode::Live`, `OmsType::Netting`, `CatalogFsProtocol::File`, `RotationKind::None`)"
    )

    findings = VERIFIER.validate_docs(stale_schema, stale_status)
    expected_fragments = [
        "schema still contains stale phrase",
        "status map still contains stale phrase",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_removed_market_exit_fields_and_requires_forced_exit_order() -> None:
    stale_schema = (
        CURRENT_SCHEMA
        + """
market_exit_time_in_force = "gtc"
market_exit_reduce_only = true

Normal exits use the configured `exit_order` maker/taker shape. Forced-flat exits from freeze, stale-data, and thin-book predicates use the separate market-exit TOML fields: `market_exit_time_in_force` and `market_exit_reduce_only`.
"""
    )
    missing_forced_exit_schema = CURRENT_SCHEMA.replace("forced_exit_order", "")

    stale_findings = VERIFIER.validate_docs(stale_schema, CURRENT_STATUS_MAP)
    missing_findings = VERIFIER.validate_docs(missing_forced_exit_schema, CURRENT_STATUS_MAP)

    expected_stale_fragments = [
        "market_exit_time_in_force",
        "market_exit_reduce_only",
        "separate market-exit TOML fields",
    ]
    for fragment in expected_stale_fragments:
        if not any(fragment in finding for finding in stale_findings):
            raise AssertionError(f"expected stale forced-exit fragment {fragment!r}, got {stale_findings!r}")

    if not any("forced_exit_order" in finding for finding in missing_findings):
        raise AssertionError(f"expected missing forced_exit_order finding, got {missing_findings!r}")


def test_validate_docs_rejects_trailing_stop_market_required_default_field_claims() -> None:
    stale_schema = (
        CURRENT_SCHEMA
        + """
#### `trigger_type`

- required for `trailing_stop_market`

#### `trailing_offset_type`

- required for `trailing_stop_market`

Current validation rejects:

- `trailing_stop_market` templates without positive trigger or activation input, explicit trigger type, positive trailing offset, and trailing offset type
"""
    )

    findings = VERIFIER.validate_docs(stale_schema, CURRENT_STATUS_MAP)
    expected_fragments = [
        "trigger_type",
        "trailing_offset_type",
        "explicit trigger type",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected stale TrailingStopMarket fragment {fragment!r}, got {findings!r}")


def test_validate_docs_rejects_equivalent_trailing_stop_market_default_field_requirements() -> None:
    stale_schema = (
        CURRENT_SCHEMA
        + """
#### `trigger_type`

- `trigger_type` is required for `trailing_stop_market`

#### `trailing_offset_type`

- `trailing_offset_type` must be provided when order_type is `trailing_stop_market`
"""
    )

    findings = VERIFIER.validate_docs(stale_schema, CURRENT_STATUS_MAP)
    expected_fragments = [
        "trigger_type",
        "trailing_offset_type",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected equivalent stale TrailingStopMarket fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_current_unsupported_scope_overclaims_in_status_map() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP
        + "\nThe current strategy supports short-side position contracts and exit quote-quantity orders.",
    )

    expected_fragments = [
        "status map contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_gtd_broad_support_and_live_execution_overclaims() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA
        + "\nThe current order-intent layer supports GTD maker orders in live execution.",
        CURRENT_STATUS_MAP + "\nMaker support is enabled for every strategy and venue.",
    )

    expected_fragments = [
        "schema contains unsupported current-scope overclaim",
        "status map contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_equivalent_live_execution_and_broad_venue_overclaims() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA + "\nThe current order-intent layer enables GTD live trading.",
        CURRENT_STATUS_MAP + "\nCanary support is enabled for maker orders.",
    )

    expected_fragments = [
        "schema contains unsupported current-scope overclaim",
        "status map contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP
        + "\nThe current strategy supports short-side contracts without additional economics.",
    )

    expected_fragments = [
        "status map contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_allows_live_trading_config_value_without_support_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA
        + """
#### `mode`

- current allowed value for live trading:
  - `Live`
""",
        CURRENT_STATUS_MAP,
    )

    if findings:
        raise AssertionError(f"expected no findings, got {findings!r}")


def test_validate_docs_requires_all_enabled_and_factory_gap_order_types() -> None:
    missing_lit = CURRENT_SCHEMA.replace("- `limit_if_touched`\n", "")
    missing_gap = CURRENT_SCHEMA.replace("- `trailing_stop_limit`\n", "").replace(
        " and `trailing_stop_limit`", ""
    )

    lit_findings = VERIFIER.validate_docs(missing_lit, CURRENT_STATUS_MAP)
    if not any("`limit_if_touched`" in finding for finding in lit_findings):
        raise AssertionError(f"expected missing LimitIfTouched finding, got {lit_findings!r}")

    gap_findings = VERIFIER.validate_docs(missing_gap, CURRENT_STATUS_MAP)
    if not any("`trailing_stop_limit`" in finding for finding in gap_findings):
        raise AssertionError(f"expected missing TrailingStopLimit finding, got {gap_findings!r}")


def test_validate_docs_rejects_decision_evidence_and_runtime_contract_doc_drift() -> None:
    stale_schema = CURRENT_SCHEMA.replace("schema_version = 14", "schema_version = 13")
    stale_runtime_contracts = CURRENT_RUNTIME_CONTRACTS.replace("`activation_price`, ", "")
    stale_status_map = CURRENT_STATUS_MAP.replace("forced_exit_order", "exit_order")

    findings = VERIFIER.validate_docs(
        stale_schema,
        stale_status_map,
        runtime_contracts=stale_runtime_contracts,
    )

    expected_fragments = [
        "schema missing decision-evidence JSONL schema v14 contract",
        "runtime contracts missing order-template evidence field",
        "status map missing current phrase: Order construction uses",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_stale_strategy_schema_version_examples() -> None:
    stale_schema = CURRENT_SCHEMA.replace("schema_version = 2", "schema_version = 1")
    findings = VERIFIER.validate_docs(
        stale_schema,
        CURRENT_STATUS_MAP,
        validate_source="pub const SUPPORTED_STRATEGY_SCHEMA_VERSION: u32 = 2;",
    )

    if not any("strategy schema_version example" in finding for finding in findings):
        raise AssertionError(f"expected stale strategy schema version finding, got {findings!r}")


def test_validate_docs_rejects_stale_decision_evidence_record_type_wording() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA
        + "\nEach line has `record_type`, and a payload key matching the record type.\n",
        CURRENT_STATUS_MAP,
    )

    if not any("record_type" in finding for finding in findings):
        raise AssertionError(f"expected stale record_type wording finding, got {findings!r}")


def test_validate_docs_rejects_retired_financial_envelope_schema_section() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA
        + """
`financial_envelope` fields:

- `max_live_order_count`: integer
""",
        CURRENT_STATUS_MAP,
    )

    if not any("financial_envelope" in finding for finding in findings):
        raise AssertionError(f"expected retired financial_envelope finding, got {findings!r}")


def test_validate_docs_rejects_duplicate_position_contract_helpers() -> None:
    duplicate_source = """
fn expected_position_side_for_entry_order(order_side: OrderSide) -> Option<PositionSide> {
    todo!()
}
fn expected_exit_order_side_for_position(position_side: PositionSide) -> Option<OrderSide> {
    todo!()
}
fn is_observed_open_side(side: PositionSide) -> bool {
    todo!()
}
"""
    shared_source = duplicate_source.replace("fn ", "pub fn ")

    complete_findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        archetype_source="use crate::bolt_v3_position_contract::*;",
        strategy_source="use crate::bolt_v3_position_contract::*;",
        position_contract_source=shared_source,
    )
    if complete_findings:
        raise AssertionError(f"expected shared helper source to pass, got {complete_findings!r}")

    duplicate_findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        archetype_source=duplicate_source,
        strategy_source=duplicate_source,
        position_contract_source="",
    )
    expected_helpers = (
        "expected_position_side_for_entry_order",
        "expected_exit_order_side_for_position",
        "is_observed_open_side",
    )
    for helper_name in expected_helpers:
        if not any(helper_name in finding for finding in duplicate_findings):
            raise AssertionError(
                f"expected duplicate helper finding for {helper_name}, got {duplicate_findings!r}"
            )


def main() -> int:
    tests = [
        test_extract_section_stops_at_next_matching_heading,
        test_validate_docs_accepts_current_terms,
        test_validate_docs_rejects_superseded_tuple_policy_and_netting_only_status,
        test_validate_docs_rejects_removed_market_exit_fields_and_requires_forced_exit_order,
        test_validate_docs_rejects_trailing_stop_market_required_default_field_claims,
        test_validate_docs_rejects_equivalent_trailing_stop_market_default_field_requirements,
        test_validate_docs_rejects_current_unsupported_scope_overclaims_in_status_map,
        test_validate_docs_rejects_gtd_broad_support_and_live_execution_overclaims,
        test_validate_docs_rejects_equivalent_live_execution_and_broad_venue_overclaims,
        test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause,
        test_validate_docs_allows_live_trading_config_value_without_support_claim,
        test_validate_docs_requires_all_enabled_and_factory_gap_order_types,
        test_validate_docs_rejects_decision_evidence_and_runtime_contract_doc_drift,
        test_validate_docs_rejects_stale_strategy_schema_version_examples,
        test_validate_docs_rejects_stale_decision_evidence_record_type_wording,
        test_validate_docs_rejects_retired_financial_envelope_schema_section,
        test_validate_docs_rejects_duplicate_position_contract_helpers,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 schema-current verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
