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


def main() -> int:
    tests = [
        test_extract_section_stops_at_next_matching_heading,
        test_validate_docs_accepts_current_terms,
        test_validate_docs_rejects_superseded_tuple_policy_and_netting_only_status,
        test_validate_docs_rejects_short_side_overclaims_in_scoped_docs,
        test_validate_docs_rejects_stale_contract_short_side_claim,
        test_validate_docs_rejects_stale_spec_short_side_claim,
        test_validate_docs_rejects_blanket_non_gtd_expiry_claim,
        test_validate_docs_rejects_removed_market_exit_fields_and_requires_forced_exit_order,
        test_validate_docs_rejects_trailing_stop_market_required_default_field_claims,
        test_validate_docs_rejects_current_unsupported_scope_overclaims_everywhere,
        test_validate_docs_rejects_gtd_broad_support_and_live_canary_overclaims,
        test_validate_docs_rejects_equivalent_live_canary_and_broad_venue_overclaims,
        test_validate_docs_rejects_short_and_exit_quote_overclaims_with_without_clause,
        test_validate_docs_allows_live_trading_config_value_without_support_claim,
        test_validate_docs_allows_spec_architecture_risk_context,
        test_validate_docs_requires_all_enabled_and_factory_gap_order_types,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 schema-current verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
