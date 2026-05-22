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

### `[parameters]`
"""

CURRENT_STATUS_MAP = """
Strategy `oms_type` delegates to NT `OmsType` variants instead of a Bolt-only netting allowlist.
Configured order-template validation follows the pinned NT single-order `OrderFactory` surface.
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


def test_validate_docs_rejects_wrong_active_speckit_context() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc=(
            "shell commands, and other important information, read the current plan:\n"
            "`specs/023-nt-research-analytics-platform/plan.md`\n"
        ),
        feature_json='{"feature_directory": "specs/023-nt-research-analytics-platform"}',
    )

    expected_fragments = [
        "AGENTS.md",
        ".specify/feature.json",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected Speckit context fragment {fragment!r}, got {findings!r}")


def test_validate_docs_checks_active_speckit_block_not_any_substring() -> None:
    stale_active_block = """
Historical context: `specs/023-nt-order-intent-layer/plan.md`

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/024-other-feature/plan.md`
<!-- SPECKIT END -->
"""
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc=stale_active_block,
        feature_json='{"feature_directory": "specs/023-nt-order-intent-layer"}',
    )
    if not any("AGENTS.md" in finding and "active Speckit block" in finding for finding in findings):
        raise AssertionError(f"expected active-block pointer finding, got {findings!r}")

    correct_active_block_with_history = """
Historical context: `specs/023-nt-research-analytics-platform/plan.md`

<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/023-nt-order-intent-layer/plan.md`
<!-- SPECKIT END -->
"""
    history_findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc=correct_active_block_with_history,
        feature_json='{"feature_directory": "specs/023-nt-order-intent-layer"}',
    )
    if history_findings:
        raise AssertionError(f"expected historical stale pointer outside active block to pass, got {history_findings!r}")


def test_validate_docs_rejects_same_block_wrong_active_plan_even_with_current_note() -> None:
    same_block_note = """
<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/024-other-feature/plan.md`

Historical replacement target: `specs/023-nt-order-intent-layer/plan.md`
<!-- SPECKIT END -->
"""
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc=same_block_note,
        feature_json='{"feature_directory": "specs/023-nt-order-intent-layer"}',
    )
    if not any("AGENTS.md" in finding and "active Speckit block" in finding for finding in findings):
        raise AssertionError(f"expected same-block active-plan finding, got {findings!r}")


def test_validate_docs_rejects_empty_speckit_context_files() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc="",
        feature_json="",
    )
    expected_fragments = [
        "AGENTS.md is empty",
        ".specify/feature.json is empty",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected empty-file fragment {fragment!r}, got {findings!r}")


def test_validate_docs_rejects_non_object_feature_json_without_crashing() -> None:
    try:
        findings = VERIFIER.validate_docs(
            CURRENT_SCHEMA,
            CURRENT_STATUS_MAP,
            agents_doc=(
                "<!-- SPECKIT START -->\n"
                "`specs/023-nt-order-intent-layer/plan.md`\n"
                "<!-- SPECKIT END -->\n"
            ),
            feature_json="[]",
        )
    except AttributeError as exc:
        raise AssertionError("expected verifier finding for non-object feature JSON") from exc

    if not any(".specify/feature.json" in finding and "JSON object" in finding for finding in findings):
        raise AssertionError(f"expected non-object feature JSON finding, got {findings!r}")


def test_validate_docs_rejects_malformed_feature_json_without_crashing() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        agents_doc=(
            "<!-- SPECKIT START -->\n"
            "`specs/023-nt-order-intent-layer/plan.md`\n"
            "<!-- SPECKIT END -->\n"
        ),
        feature_json="{",
    )

    if not any(".specify/feature.json" in finding and "not valid JSON" in finding for finding in findings):
        raise AssertionError(f"expected malformed feature JSON finding, got {findings!r}")


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
        "- [x] T013 [US2] GREEN: Allow coherent short-side contracts while keeping incoherent long/short contracts rejected",
    )

    expected_fragments = [
        "research still contains stale phrase",
        "tasks still contains stale phrase",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(f"expected {fragment!r} in findings, got {findings!r}")


def test_validate_docs_rejects_stale_contract_short_side_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        contract="- Long and short position contracts are coherent.",
    )

    if not any("contract" in finding and "short" in finding for finding in findings):
        raise AssertionError(f"expected contract short-side finding, got {findings!r}")


def test_validate_docs_rejects_stale_spec_short_side_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        spec="Given short-side entry and exit contracts, config validation does not reject the shape merely because it is short-side.",
    )

    if not any("spec" in finding and "short-side" in finding for finding in findings):
        raise AssertionError(f"expected spec short-side finding, got {findings!r}")


def test_validate_docs_rejects_blanket_non_gtd_expiry_claim() -> None:
    findings = VERIFIER.validate_docs(
        CURRENT_SCHEMA,
        CURRENT_STATUS_MAP,
        data_model="- `expire_time_unix_nanos` only when GTD is enabled by a reviewed slice",
    )

    if not any(
        "data model" in finding and "expire_time_unix_nanos" in finding for finding in findings
    ):
        raise AssertionError(f"expected data-model expiry finding, got {findings!r}")


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


def test_validate_docs_rejects_completed_phase50_blocker_wording() -> None:
    stale_tasks = """
- [x] T221 [US3] RED: Add regression proving post-only entry book-impact cap derives depth from the passive book side
- [x] T222 [US3] RED: Add regression proving Managed external position close cancels a resting pending entry before flattening
- [x] T223 [US3] GREEN: Fix strategy-owned sizing and lifecycle paths without changing shared NT order construction
- Phase 50 blocks completion because current-head PR-body/Greptile evidence and source inspection found maker entry sizing still uses taker-side book depth and external Managed close still drops a resting pending entry without NT cancel.
"""

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 50",
        "taker-side book depth",
        "without NT cancel",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed-Phase-50 fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_completed_phase47_and_phase48_blocker_wording() -> None:
    stale_tasks = """
- [x] T209 [US3] GREEN: Add a single TOML-owned forced-exit order template path and remove the hardcoded forced-flat market-order synthesis
- [x] T214 [US3] GREEN: Update active schema docs/verifier and add the NT manage-stop compatibility guard without adding venue or maker/taker policy
- Phase 47 blocks completion because current-head multi-agent review found forced-flat exit order semantics still synthesized as Market/TIF/reduce-only fields in strategy code rather than carried as a TOML-owned NT order template.
- Phase 48 blocks completion because latest-head multi-agent review found active schema docs still describe removed market-exit fields and `manage_stop=true` can silently route non-market `forced_exit_order` configs through NT's built-in market close path.
"""

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 47",
        "synthesized as Market/TIF/reduce-only fields",
        "Phase 48",
        "manage_stop=true",
    ]
    for fragment in expected_fragments:
        if not any(fragment in finding for finding in findings):
            raise AssertionError(
                f"expected stale completed dependency fragment {fragment!r}, got {findings!r}"
            )


def test_validate_docs_rejects_completed_phase34_default_blocker_wording() -> None:
    stale_tasks = """
- [x] T150 [US2] Verify focused TrailingStopMarket tests, schema/source fences as possible, branch cleanliness, exact-head CI, and reviewer/no-mistakes state
- Phase 34 blocks completion because multi-agent pinned-NT review found TrailingStopMarket validation still requires optional fields that NT defaults.
- Phase 51 closes the TrailingStopMarket schema-default drift and equivalent-wording verifier gap; only terminal reviewer/no-mistakes state remains open in T228.
"""

    findings = VERIFIER.validate_docs(CURRENT_SCHEMA, CURRENT_STATUS_MAP, tasks=stale_tasks)
    expected_fragments = [
        "Phase 34",
        "optional fields that NT defaults",
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


def test_validate_docs_rejects_gtd_broad_support_and_live_canary_overclaims() -> None:
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


def test_validate_docs_rejects_equivalent_live_canary_and_broad_venue_overclaims() -> None:
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


def test_extracts_phase8_financial_envelope_fields_from_source_struct() -> None:
    rust_source = """
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Phase8FinancialEnvelopeEvidenceFile {
    max_live_order_count: u32,
    entry_side: String,
    entry_activation_price: Option<f64>,
}

impl Phase8FinancialEnvelopeEvidenceFile {
"""

    fields = VERIFIER.extract_phase8_financial_envelope_fields(rust_source)

    expected_fields = [
        "max_live_order_count",
        "entry_side",
        "entry_activation_price",
    ]
    if fields != expected_fields:
        raise AssertionError(f"expected source-derived fields {expected_fields!r}, got {fields!r}")


def test_validate_docs_rejects_financial_envelope_schema_missing_source_field() -> None:
    rust_source = """
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Phase8FinancialEnvelopeEvidenceFile {
    max_live_order_count: u32,
    entry_side: String,
}

impl Phase8FinancialEnvelopeEvidenceFile {
"""
    complete_schema = (
        CURRENT_SCHEMA
        + """
`financial_envelope` fields:

- `max_live_order_count`: integer
- `entry_side`: string

`pre_run_state` fields:
"""
    )
    missing_schema = complete_schema.replace("- `entry_side`: string\n", "")

    complete_findings = VERIFIER.validate_docs(
        complete_schema,
        CURRENT_STATUS_MAP,
        financial_envelope_source=rust_source,
    )
    if complete_findings:
        raise AssertionError(f"expected complete synthetic schema to pass, got {complete_findings!r}")

    missing_findings = VERIFIER.validate_docs(
        missing_schema,
        CURRENT_STATUS_MAP,
        financial_envelope_source=rust_source,
    )
    if not any("financial_envelope" in finding and "`entry_side`" in finding for finding in missing_findings):
        raise AssertionError(f"expected missing source-derived field finding, got {missing_findings!r}")


def test_validate_docs_rejects_financial_envelope_schema_extra_doc_field() -> None:
    rust_source = """
#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Phase8FinancialEnvelopeEvidenceFile {
    max_live_order_count: u32,
    entry_side: String,
}

impl Phase8FinancialEnvelopeEvidenceFile {
"""
    complete_schema = (
        CURRENT_SCHEMA
        + """
`financial_envelope` fields:

- `max_live_order_count`: integer
- `entry_side`: string

`pre_run_state` fields:
"""
    )
    extra_schema = complete_schema.replace(
        "- `entry_side`: string\n",
        "- `entry_side`: string\n- `stale_doc_field`: string\n",
    )

    complete_findings = VERIFIER.validate_docs(
        complete_schema,
        CURRENT_STATUS_MAP,
        financial_envelope_source=rust_source,
    )
    if complete_findings:
        raise AssertionError(f"expected complete synthetic schema to pass, got {complete_findings!r}")

    extra_findings = VERIFIER.validate_docs(
        extra_schema,
        CURRENT_STATUS_MAP,
        financial_envelope_source=rust_source,
    )
    if not any("financial_envelope" in finding and "`stale_doc_field`" in finding for finding in extra_findings):
        raise AssertionError(f"expected extra doc field finding, got {extra_findings!r}")


def main() -> int:
    tests = [
        test_extract_section_stops_at_next_matching_heading,
        test_validate_docs_accepts_current_terms,
        test_validate_docs_rejects_wrong_active_speckit_context,
        test_validate_docs_checks_active_speckit_block_not_any_substring,
        test_validate_docs_rejects_same_block_wrong_active_plan_even_with_current_note,
        test_validate_docs_rejects_empty_speckit_context_files,
        test_validate_docs_rejects_non_object_feature_json_without_crashing,
        test_validate_docs_rejects_malformed_feature_json_without_crashing,
        test_validate_docs_rejects_superseded_tuple_policy_and_netting_only_status,
        test_validate_docs_rejects_short_side_overclaims_in_scoped_docs,
        test_validate_docs_rejects_stale_contract_short_side_claim,
        test_validate_docs_rejects_stale_spec_short_side_claim,
        test_validate_docs_rejects_blanket_non_gtd_expiry_claim,
        test_validate_docs_rejects_removed_market_exit_fields_and_requires_forced_exit_order,
        test_validate_docs_rejects_trailing_stop_market_required_default_field_claims,
        test_validate_docs_rejects_equivalent_trailing_stop_market_default_field_requirements,
        test_validate_docs_rejects_completed_phase50_blocker_wording,
        test_validate_docs_rejects_completed_phase47_and_phase48_blocker_wording,
        test_validate_docs_rejects_completed_phase34_default_blocker_wording,
        test_validate_docs_requires_phase51_dependency_note_when_tasks_are_checked,
        test_validate_docs_rejects_terminal_only_final_dependency_notes_after_wait_cap,
        test_validate_docs_rejects_current_unsupported_scope_overclaims_everywhere,
        test_validate_docs_rejects_gtd_broad_support_and_live_canary_overclaims,
        test_validate_docs_rejects_equivalent_live_canary_and_broad_venue_overclaims,
        test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause,
        test_validate_docs_allows_live_trading_config_value_without_support_claim,
        test_validate_docs_allows_spec_architecture_risk_context,
        test_validate_docs_requires_all_enabled_and_factory_gap_order_types,
        test_extracts_phase8_financial_envelope_fields_from_source_struct,
        test_validate_docs_rejects_financial_envelope_schema_missing_source_field,
        test_validate_docs_rejects_financial_envelope_schema_extra_doc_field,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 schema-current verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
