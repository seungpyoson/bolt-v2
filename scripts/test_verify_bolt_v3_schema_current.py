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
Decision-evidence JSONL records use `schema_version = 14` for `order_intent`, `admission_decision`, `strategy_input_snapshot`, `capital_admission_rebuild`, `submit_reservation_metadata`, `submit_reservation_fill`, `entry_skip`, `exit_decision`, `loss_governor_halt`, and `requote_throttle` envelopes.
Each line is a single JSON object with `schema_version`, `recorded_at_utc_ns`, `gate_version`, `gate_id`, `kind`, and the matching payload field: `intent`, `decision`, `snapshot`, `audit`, `metadata`, `fill`, `entry_skip`, `exit_decision`, `loss_governor_halt`, or `requote_throttle`.
The `kind` field is `order_intent` for `intent` payloads, `admission_decision` for `decision` payloads, `strategy_input_snapshot` for `snapshot` payloads, `capital_admission_rebuild` for startup rebuild audit payloads, `submit_reservation_metadata` for admitted reservation metadata, `submit_reservation_fill` for fill metadata, `entry_skip` for entry skip rationale, `exit_decision` for exit rationale, `loss_governor_halt` for loss-governor halt transitions, and `requote_throttle` for maker requote budget throttle transitions.
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


def test_validate_docs_rejects_short_side_overclaims_in_scoped_docs() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        "The current archetype accepts coherent short-side entry/exit contracts.",
        "- [x] T013 [US2] GREEN: Current strategy supports short-side position contracts.",
    )

    expected_fragments = [
        "research contains unsupported current-scope overclaim",
        "tasks contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_stale_contract_short_side_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        contract="- Current contract supports short position contracts.",
    )

    if not any("contract" in finding and "short" in finding for finding in findings):
        raise AssertionError(f"expected contract short-side finding, got {findings!r}")


def test_validate_docs_rejects_stale_spec_short_side_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        spec="Config validation accepts short-side entry and exit contracts.",
    )

    if not any("spec" in finding and "short-side" in finding for finding in findings):
        raise AssertionError(f"expected spec short-side finding, got {findings!r}")


def test_validate_docs_rejects_blanket_non_gtd_expiry_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        data_model="- `expire_time_unix_nanos` is exclusively for GTD orders",
    )

    if not any("data model" in finding and "non-GTD expiry" in finding for finding in findings):
        raise AssertionError(f"expected data-model expiry finding, got {findings!r}")


def test_validate_docs_keeps_non_gtd_expiry_no_regression_case() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        data_model="- `expire_time_unix_nanos` only when GTD is enabled by a reviewed slice",
    )

    if not any("data model" in finding and "non-GTD expiry" in finding for finding in findings):
        raise AssertionError(f"expected data-model expiry finding, got {findings!r}")


def test_validate_docs_allows_current_gtd_expiry_usage_statement() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        data_model="- `expire_time_unix_nanos` is only used for GTD orders.",
    )

    if any("data model" in finding and "non-GTD expiry" in finding for finding in findings):
        raise AssertionError(f"expected current-state expiry usage wording to pass, got {findings!r}")


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


def completed_phase_task_rows() -> str:
    return """
- [x] T224 Verify Phase 50 focused checks and exact-head PR state
- [x] T228 Verify Phase 51 focused checks and exact-head PR state
- [x] T233 Verify Phase 52 focused checks and exact-head PR state
- [x] T236 Verify Phase 53 focused checks and exact-head PR state
- [x] T240 Verify Phase 54 focused checks and exact-head PR state
"""


def test_validate_docs_rejects_completed_phase50_blocker_rewording() -> None:
    stale_tasks = """
- [x] T221 [US3] RED: Add regression proving post-only entry book-impact cap derives depth from the passive book side
- [x] T222 [US3] RED: Add regression proving Managed external position close cancels a resting pending entry before flattening
- [x] T223 [US3] GREEN: Fix strategy-owned sizing and lifecycle paths without changing shared NT order construction
- Phase 50 still prevents completion because current-head PR-body/Greptile evidence and source inspection found maker entry sizing still uses taker-side book depth and external Managed close still drops a resting pending entry without NT cancel.
""" + completed_phase_task_rows()

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 50",
        "open/blocking",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed-Phase-50 fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_keeps_completed_phase_blocker_no_regression_case() -> None:
    stale_tasks = """
- Phase 50 blocks completion because current-head review found maker entry sizing still uses taker-side book depth.
""" + completed_phase_task_rows()

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 50",
        "open/blocking",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed-Phase-50 fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_completed_phase47_and_phase48_blocker_rewording() -> None:
    stale_tasks = """
- [x] T209 [US3] GREEN: Add a single TOML-owned forced-exit order template path and remove the hardcoded forced-flat market-order synthesis
- [x] T214 [US3] GREEN: Update active schema docs/verifier and add the NT manage-stop compatibility guard without adding venue or maker/taker policy
- Phase 47 is blocked pending current-head multi-agent review closure for forced-flat exit order semantics still synthesized as Market/TIF/reduce-only fields in strategy code rather than carried as a TOML-owned NT order template.
- Phase 48 still prevents completion because latest-head multi-agent review found active schema docs still describe removed market-exit fields and `manage_stop=true` can silently route non-market `forced_exit_order` configs through NT's built-in market close path.
""" + completed_phase_task_rows()

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 47",
        "Phase 48",
        "open/blocking",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed dependency fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_completed_phase34_default_blocker_rewording() -> None:
    stale_tasks = """
- [x] T150 [US2] Verify focused TrailingStopMarket tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state
- Phase 34 is blocked pending multi-agent pinned-NT review closure for TrailingStopMarket validation still requiring optional fields that NT defaults.
- Phase 51 closes the TrailingStopMarket schema-default drift and equivalent-wording verifier gap; only terminal reviewer/no-mistakes state remains open in T228.
""" + completed_phase_task_rows()

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 34",
        "open/blocking",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed-Phase-34 fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_requires_phase51_dependency_note_when_tasks_are_checked() -> None:
    tasks_without_phase51_dependency = """
## Phase 51: TDD Slice 47 - TrailingStopMarket Schema Default Drift

- [x] T225 [P] [US2] Record current-head multi-agent and pinned NT evidence for optional `TrailingStopMarket` default fields
- [x] T230 [US2] GREEN: Generalize the verifier to reject equivalent TrailingStopMarket default-field requirement wording without flagging optional/default-pass-through wording

## Dependencies & Execution Order

- Phase 50 closes the current-head maker lifecycle/sizing review findings; only terminal reviewer/no-mistakes state remains open in T224.
"""

    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        tasks=tasks_without_phase51_dependency,
    )
    if not any("Phase 51" in finding for finding in findings):
        raise AssertionError(f"expected missing Phase 51 dependency finding, got {findings!r}")


def test_validate_docs_rejects_terminal_only_final_dependency_notes_after_wait_cap() -> None:
    stale_tasks = """
## Dependencies & Execution Order

- Phase 50 closes the current-head maker lifecycle/sizing review findings; only terminal reviewer/no-mistakes state remains open in T224.
- Phase 51 closes the TrailingStopMarket schema-default drift and equivalent-wording verifier gap; only terminal reviewer/no-mistakes state remains open in T228.
- Phase 52 remains open until T233 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.
- Phase 53 remains open until T236 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.
- Phase 54 remains open until T240 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.
"""

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 50",
        "Phase 51",
        "Phase 52",
        "Phase 53",
        "Phase 54",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale terminal dependency fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_current_unsupported_scope_overclaims_everywhere() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP
        + "\nThe current strategy supports short-side position contracts and exit quote-quantity orders.",
        "The current archetype enables exit is_quote_quantity = true.",
        "- [x] Enable short-side position contracts and exit quote-quantity orders in the current strategy",
    )

    expected_fragments = [
        "status map contains unsupported current-scope overclaim",
        "research contains unsupported current-scope overclaim",
        "tasks contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_gtd_broad_support_and_live_execution_overclaims() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA
        + "\nThe current order-intent layer supports GTD maker orders in live execution.",
        CURRENT_STATUS_MAP + "\nMaker support is enabled for every strategy and venue.",
        "Live/canary-proven exchange execution is supported for maker orders.",
        "- [x] Enable maker support for every strategy/venue",
    )

    expected_fragments = [
        "schema contains unsupported current-scope overclaim",
        "status map contains unsupported current-scope overclaim",
        "research contains unsupported current-scope overclaim",
        "tasks contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_equivalent_live_execution_and_broad_venue_overclaims() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA + "\nThe current order-intent layer enables GTD live trading.",
        CURRENT_STATUS_MAP + "\nCanary support is enabled for maker orders.",
        "Maker support is enabled for all configured venues.",
        "- [x] Maker support is enabled for all configured venues",
    )

    expected_fragments = [
        "schema contains unsupported current-scope overclaim",
        "status map contains unsupported current-scope overclaim",
        "research contains unsupported current-scope overclaim",
        "tasks contains unsupported current-scope overclaim",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP
        + "\nThe current strategy supports short-side contracts without additional economics.",
        "Exit quote quantity is supported without base-inventory conversion.",
        "- [x] Exit quote quantity supported without base-inventory conversion",
    )

    expected_fragments = [
        "status map contains unsupported current-scope overclaim",
        "research contains unsupported current-scope overclaim",
        "tasks contains unsupported current-scope overclaim",
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


def test_validate_docs_allows_spec_architecture_risk_context() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        spec="If the boundary is wrong, maker, taker, GTD, short-side, spot, binary option, perpetual, and option support will keep accreting hardcoded local policy instead of using NT.",
    )

    if findings:
        raise AssertionError(f"expected no findings for guarded risk wording, got {findings!r}")


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


def test_validate_docs_rejects_decision_evidence_and_maker_scope_doc_drift() -> None:
    stale_schema = CURRENT_SCHEMA.replace("schema_version = 14", "schema_version = 13")
    stale_runtime_contracts = CURRENT_RUNTIME_CONTRACTS.replace("`activation_price`, ", "")
    stale_status_map = CURRENT_STATUS_MAP.replace("forced_exit_order", "exit_order")
    stale_maker_contract = (
        "Forced-flat exits are blocked pending a TOML-owned forced-exit override.\n"
        "The expiry policy must be approved before enabling GTD.\n"
        "Maps TOML-owned timing into NT expire_time. This policy is not implemented.\n"
    )
    stale_maker_data_model = (
        "Rejected values:\n"
        "- The expiry policy must be approved before enabling GTD.\n"
    )

    findings = VERIFIER.validate_docs(
        stale_schema,
        stale_status_map,
        runtime_contracts=stale_runtime_contracts,
        maker_scope_contract=stale_maker_contract,
        maker_scope_data_model=stale_maker_data_model,
    )

    expected_fragments = [
        "schema missing decision-evidence JSONL schema v14 contract",
        "runtime contracts missing order-template evidence field",
        "status map missing current phrase: Order construction uses",
        "maker scope contract: forced-exit override still described as missing",
        "maker scope data model: GTD still described as blocked pending expiry policy",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_keeps_maker_scope_stale_no_regression_case() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        maker_scope_contract=(
            "Forced-flat exits use the same configured maker exit shape until a separate "
            "TOML-owned forced-exit override exists.\n"
            "bolt-v3 must not enable `gtd` until this contract adds an explicit TOML-owned "
            "expiry policy.\n"
        ),
    )

    expected_fragments = [
        "maker scope contract: forced-exit override still described as missing",
        "maker scope contract: GTD still described as blocked pending expiry policy",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_allows_current_forced_exit_override_requirement() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        maker_scope_contract="A separate forced-exit override template is required.",
    )

    if any("forced-exit override still described as missing" in finding for finding in findings):
        raise AssertionError(f"expected current forced-exit override requirement to pass, got {findings!r}")


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
        test_validate_docs_rejects_short_side_overclaims_in_scoped_docs,
        test_validate_docs_rejects_stale_contract_short_side_claim,
        test_validate_docs_rejects_stale_spec_short_side_claim,
        test_validate_docs_rejects_blanket_non_gtd_expiry_claim,
        test_validate_docs_keeps_non_gtd_expiry_no_regression_case,
        test_validate_docs_allows_current_gtd_expiry_usage_statement,
        test_validate_docs_rejects_removed_market_exit_fields_and_requires_forced_exit_order,
        test_validate_docs_rejects_trailing_stop_market_required_default_field_claims,
        test_validate_docs_rejects_equivalent_trailing_stop_market_default_field_requirements,
        test_validate_docs_rejects_completed_phase50_blocker_rewording,
        test_validate_docs_keeps_completed_phase_blocker_no_regression_case,
        test_validate_docs_rejects_completed_phase47_and_phase48_blocker_rewording,
        test_validate_docs_rejects_completed_phase34_default_blocker_rewording,
        test_validate_docs_requires_phase51_dependency_note_when_tasks_are_checked,
        test_validate_docs_rejects_terminal_only_final_dependency_notes_after_wait_cap,
        test_validate_docs_rejects_current_unsupported_scope_overclaims_everywhere,
        test_validate_docs_rejects_gtd_broad_support_and_live_execution_overclaims,
        test_validate_docs_rejects_equivalent_live_execution_and_broad_venue_overclaims,
        test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause,
        test_validate_docs_allows_live_trading_config_value_without_support_claim,
        test_validate_docs_allows_spec_architecture_risk_context,
        test_validate_docs_requires_all_enabled_and_factory_gap_order_types,
        test_validate_docs_rejects_decision_evidence_and_maker_scope_doc_drift,
        test_validate_docs_keeps_maker_scope_stale_no_regression_case,
        test_validate_docs_allows_current_forced_exit_override_requirement,
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
