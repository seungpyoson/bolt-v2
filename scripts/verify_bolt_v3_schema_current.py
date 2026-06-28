#!/usr/bin/env python3
"""Verify active Bolt-v3 docs match current order-intent source scope."""

from __future__ import annotations

import re
import sys
from pathlib import Path

from bolt_v3_source_roots import STRATEGY_SOURCE_ROOTS, module_text


REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md"
RUNTIME_CONTRACTS_DOC = REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
VALIDATE_SOURCE = REPO_ROOT / "src/bolt_v3_validate.rs"
DECISION_EVIDENCE_SOURCE = REPO_ROOT / "src/bolt_v3_decision_evidence.rs"
ARCHETYPE_BINARY_ORACLE_SOURCE = (
    REPO_ROOT / "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs"
)
POSITION_CONTRACT_SOURCE = REPO_ROOT / "src/bolt_v3_position_contract.rs"
RUST_STRUCT_FIELD_PATTERN = re.compile(r"^\s*(?P<field>[a-z][a-z0-9_]*):\s*[^,]+,\s*$")
SCHEMA_FIELD_LINE_PATTERN = re.compile(r"^\s*-\s*(?P<fields>[^:]+):")
BACKTICKED_FIELD_PATTERN = re.compile(r"`(?P<field>[a-z][a-z0-9_]*)`")
SUPPORTED_STRATEGY_SCHEMA_VERSION_PATTERN = re.compile(
    r"pub const SUPPORTED_STRATEGY_SCHEMA_VERSION: u32 = (?P<version>\d+);"
)
DECISION_EVIDENCE_SCHEMA_VERSION_PATTERN = re.compile(
    r"pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = (?P<version>\d+);"
)
DEFAULT_DECISION_EVIDENCE_SCHEMA_VERSION = 14
STRATEGY_SCHEMA_EXAMPLE_PATTERN = re.compile(
    r"schema_version = (?P<version>\d+)\nstrategy_instance_id = ",
    re.MULTILINE,
)
POSITION_CONTRACT_HELPER_NAMES = (
    "expected_position_side_for_entry_order",
    "expected_exit_order_side_for_position",
    "is_observed_open_side",
)

ENABLED_ORDER_TYPES = (
    "limit",
    "market",
    "stop_market",
    "stop_limit",
    "market_if_touched",
    "limit_if_touched",
    "trailing_stop_market",
)
FACTORY_GAP_ORDER_TYPES = ("market_to_limit", "trailing_stop_limit")
ORDER_TEMPLATE_FIELDS = (
    "expire_time_unix_nanos",
    "trigger_price",
    "activation_price",
    "trigger_type",
    "trigger_instrument_id",
    "trailing_offset",
    "trailing_offset_type",
)
DECISION_EVIDENCE_JSONL_CONTRACT_PHRASE_TEMPLATE = (
    "Decision-evidence JSONL records use `schema_version = {version}` for `order_intent`, "
    "`admission_decision`, `strategy_input_snapshot`, `capital_admission_rebuild`, "
    "`submit_reservation_metadata`, `submit_reservation_fill`, `entry_skip`, "
    "`exit_decision`, `loss_governor_halt`, and `requote_throttle` envelopes."
)
STATUS_MAP_FORCED_EXIT_BUILDER_PHRASE = (
    "Order construction uses the shared `src/bolt_v3_order_intent.rs` builder for "
    "entry, exit, and configured `[parameters.forced_exit_order]` templates"
)
STALE_SCHEMA_PHRASES = (
    "- current allowed value:\n  - `netting`",
    "- allowed values for the current archetype:\n  - `limit`\n  - `market`",
    "To avoid hidden policy, the current archetype supports only these combinations:",
    "Any other combination fails validation for this archetype.",
    "for the current `binary_oracle_edge_taker` archetype, the only allowed value is `false`",
    "The current archetype validates coherent long and short position contracts",
    "market_exit_time_in_force",
    "market_exit_reduce_only",
    "separate market-exit TOML fields",
    "`record_type`",
    "payload key matching the record type",
    "`financial_envelope` fields:",
)
REQUIRED_SCHEMA_PHRASES = (
    "delegates accepted values to NautilusTrader `OmsType`",
    "The current source-level tests prove `netting`, `hedging`, and `unspecified` parse and validate",
    "unsupported for this archetype because the pinned NT single-order `OrderFactory` exposes no public constructor",
    "The current archetype accepts the long position contract only",
    "Short-side position contracts are parsed but rejected until strategy-owned short economics, collateral, and exit semantics exist",
    "Entry `is_quote_quantity = true` is supported by sizing the entry quantity as quote notional",
    "Exit `is_quote_quantity = true` is rejected because exits are sized from held base position quantity",
    "Forced-flat exits use the configured `forced_exit_order` template",
    "When `manage_stop = true`, pinned NautilusTrader `Strategy::close_all_positions` submits market close orders",
    "`trigger_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TriggerType::Default`",
    "`trailing_offset_type` is optional for `trailing_stop_market`; NT defaults omitted values to `TrailingOffsetType::Price`",
    "Each line is a single JSON object with `schema_version`, `recorded_at_utc_ns`, `gate_version`, `gate_id`, `kind`, and the matching payload field: `intent`, `decision`, `snapshot`, `audit`, `metadata`, `fill`, `entry_skip`, `exit_decision`, `loss_governor_halt`, or `requote_throttle`.",
    "The `kind` field is `order_intent` for `intent` payloads, `admission_decision` for `decision` payloads, `strategy_input_snapshot` for `snapshot` payloads, `capital_admission_rebuild` for startup rebuild audit payloads, `submit_reservation_metadata` for admitted reservation metadata, `submit_reservation_fill` for fill metadata, `entry_skip` for entry skip rationale, `exit_decision` for exit rationale, `loss_governor_halt` for loss-governor halt transitions, and `requote_throttle` for maker requote budget throttle transitions.",
    "`capital_admission_rebuild`, `submit_reservation_metadata`, and `submit_reservation_fill` payloads support startup reservation recovery and fail closed on pre-schema-14 reservation records.",
)
STALE_STATUS_MAP_PHRASES = (
    "Single-value enums (`RuntimeMode::Live`, `OmsType::Netting`, `CatalogFsProtocol::File`, `RotationKind::None`)",
)
REQUIRED_STATUS_MAP_PHRASES = (
    "Strategy `oms_type` delegates to NT `OmsType` variants instead of a Bolt-only netting allowlist",
    "order-template validation follows the pinned NT single-order `OrderFactory` surface",
    STATUS_MAP_FORCED_EXIT_BUILDER_PHRASE,
)
UNSUPPORTED_SCOPE_PATTERNS = (
    re.compile(
        r"\b(short[- ]side|short position contracts?|short contracts?|short entry|short exit)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(exit[- ]quote[- ]quantity|exit quote quantity|exit is_quote_quantity|exit_order\.is_quote_quantity)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b((every|all|any)(?:\s+\w+){0,4}\s+(strategy|strategies|venue|venues)|strategy/venue|strategies/venues)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(live/canary|canary-proven|live-proven|exchange execution|live execution|"
        r"production-live)\b|"
        r"\b(enables?|enabled|supports?|supported|proves?|proven)\b.{0,80}"
        r"\b(live|canary)\s+(support|trading|execution)\b|"
        r"\b(live|canary)\s+(support|trading|execution)\b.{0,80}"
        r"\b(enables?|enabled|supports?|supported|proves?|proven)\b",
        re.IGNORECASE,
    ),
)
UNSUPPORTED_SCOPE_OVERCLAIM_PATTERN = re.compile(
    r"\b(accepts?|accepted|allows?|allowed|enables?|enabled|supports?|supported)\b",
    re.IGNORECASE,
)
UNSUPPORTED_SCOPE_GUARD_PATTERN = re.compile(
    r"\b(rejects?|rejected|unsupported|historical|supersedes?|superseded|"
    r"not supported|cannot|blocks?|blocked|blockers?|missing|needs?|"
    r"before|outside|beyond|fails?|failed|wrong|instead of)\b",
    re.IGNORECASE,
)
REQUIRES_DEFAULTED_TRAILING_STOP_MARKET_FIELD_PATTERN = re.compile(
    r"\b(required|mandatory)\b|\bmust\s+be\s+(provided|set|configured|supplied)\b",
    re.IGNORECASE,
)
OPTIONAL_OR_DEFAULT_GUARD_PATTERN = re.compile(
    r"\b(optional|not\s+required|defaults?|defaulted|omitted)\b",
    re.IGNORECASE,
)


def extract_section(text: str, heading: str, next_heading_prefix: str = "#### ") -> str:
    marker = f"{next_heading_prefix}{heading}"
    start = text.find(marker)
    if start == -1:
        return ""
    next_start = text.find(f"\n{next_heading_prefix}", start + len(marker))
    if next_start == -1:
        return text[start:]
    return text[start:next_start]


def extract_labeled_section(text: str, marker: str) -> str:
    start = text.find(marker)
    if start == -1:
        return ""
    next_start = text.find("\n`", start + len(marker))
    if next_start == -1:
        return text[start:]
    return text[start:next_start]


def extract_labeled_schema_fields(text: str, marker: str) -> list[str]:
    section = extract_labeled_section(text, marker)
    fields: list[str] = []
    seen: set[str] = set()
    for line in section.splitlines():
        match = SCHEMA_FIELD_LINE_PATTERN.match(line)
        if match is None:
            continue
        for field in BACKTICKED_FIELD_PATTERN.findall(match.group("fields")):
            if field in seen:
                continue
            seen.add(field)
            fields.append(field)
    return fields


def extract_supported_strategy_schema_version(validate_source: str) -> int | None:
    match = SUPPORTED_STRATEGY_SCHEMA_VERSION_PATTERN.search(validate_source)
    if match is None:
        return None
    return int(match.group("version"))


def extract_decision_evidence_schema_version(decision_evidence_source: str) -> int | None:
    match = DECISION_EVIDENCE_SCHEMA_VERSION_PATTERN.search(decision_evidence_source)
    if match is None:
        return None
    return int(match.group("version"))


def has_rust_function(source: str, name: str) -> bool:
    return re.search(rf"(?m)^\s*(?:pub(?:\(crate\))?\s+)?fn\s+{re.escape(name)}\s*\(", source) is not None


def validate_position_contract_helpers(
    archetype_source: str,
    strategy_source: str,
    position_contract_source: str,
) -> list[str]:
    findings: list[str] = []
    for helper_name in POSITION_CONTRACT_HELPER_NAMES:
        if not has_rust_function(position_contract_source, helper_name):
            findings.append(
                f"position-contract helper `{helper_name}` must have one shared source definition"
            )
        if has_rust_function(archetype_source, helper_name):
            findings.append(
                f"archetype source must import shared position-contract helper `{helper_name}` instead of defining it"
            )
        if has_rust_function(strategy_source, helper_name):
            findings.append(
                f"strategy source must import shared position-contract helper `{helper_name}` instead of defining it"
            )
    return findings


def unsupported_scope_overclaims(label: str, text: str) -> list[str]:
    findings: list[str] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        if not any(pattern.search(line) for pattern in UNSUPPORTED_SCOPE_PATTERNS):
            continue
        if not UNSUPPORTED_SCOPE_OVERCLAIM_PATTERN.search(line):
            continue
        if UNSUPPORTED_SCOPE_GUARD_PATTERN.search(line):
            continue
        findings.append(
            f"{label} contains unsupported current-scope overclaim on line {line_number}: {line.strip()}"
        )
    return findings


def section_requires_defaulted_trailing_stop_market_field(section: str) -> bool:
    for line in section.splitlines():
        if "`trailing_stop_market`" not in line:
            continue
        if OPTIONAL_OR_DEFAULT_GUARD_PATTERN.search(line):
            continue
        if REQUIRES_DEFAULTED_TRAILING_STOP_MARKET_FIELD_PATTERN.search(line):
            return True
    return False


def validate_docs(
    schema: str,
    status_map: str,
    runtime_contracts: str = "",
    validate_source: str = "",
    decision_evidence_source: str = "",
    archetype_source: str = "",
    strategy_source: str = "",
    position_contract_source: str = "",
) -> list[str]:
    findings: list[str] = []

    for phrase in STALE_SCHEMA_PHRASES:
        if phrase in schema:
            findings.append(f"schema still contains stale phrase: {phrase}")

    for phrase in REQUIRED_SCHEMA_PHRASES:
        if phrase not in schema:
            findings.append(f"schema missing current phrase: {phrase}")

    if decision_evidence_source:
        decision_evidence_schema_version = extract_decision_evidence_schema_version(
            decision_evidence_source
        )
        if decision_evidence_schema_version is None:
            findings.append("source missing BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION")
        else:
            decision_evidence_contract_phrase = (
                DECISION_EVIDENCE_JSONL_CONTRACT_PHRASE_TEMPLATE.format(
                    version=decision_evidence_schema_version
                )
            )
            if decision_evidence_contract_phrase not in schema:
                findings.append(
                    "schema missing decision-evidence JSONL schema "
                    f"v{decision_evidence_schema_version} contract"
                )

    if not decision_evidence_source:
        decision_evidence_contract_phrase = DECISION_EVIDENCE_JSONL_CONTRACT_PHRASE_TEMPLATE.format(
            version=DEFAULT_DECISION_EVIDENCE_SCHEMA_VERSION
        )
        if decision_evidence_contract_phrase not in schema:
            findings.append(
                "schema missing decision-evidence JSONL schema "
                f"v{DEFAULT_DECISION_EVIDENCE_SCHEMA_VERSION} contract"
            )

    if runtime_contracts:
        for field in ORDER_TEMPLATE_FIELDS:
            if f"`{field}`" not in runtime_contracts:
                findings.append(f"runtime contracts missing order-template evidence field `{field}`")

    trigger_type_section = extract_section(schema, "`trigger_type`")
    if section_requires_defaulted_trailing_stop_market_field(trigger_type_section):
        findings.append("schema trigger_type section still requires TrailingStopMarket NT-defaulted field")

    trailing_offset_type_section = extract_section(schema, "`trailing_offset_type`")
    if section_requires_defaulted_trailing_stop_market_field(trailing_offset_type_section):
        findings.append(
            "schema trailing_offset_type section still requires TrailingStopMarket NT-defaulted field"
        )

    if (
        "trailing_stop_market` templates without positive trigger or activation input, "
        "explicit trigger type, positive trailing offset, and trailing offset type"
    ) in schema:
        findings.append(
            "schema still requires explicit trigger type and trailing offset type for TrailingStopMarket"
        )

    order_type_section = extract_section(schema, "`order_type`")
    if not order_type_section:
        findings.append("schema missing `order_type` section")
    else:
        for order_type in ENABLED_ORDER_TYPES:
            if f"`{order_type}`" not in order_type_section:
                findings.append(f"schema order_type section missing enabled order type `{order_type}`")
        for order_type in FACTORY_GAP_ORDER_TYPES:
            if f"`{order_type}`" not in order_type_section:
                findings.append(f"schema order_type section missing factory-gap order type `{order_type}`")

    order_params_section = extract_section(
        schema,
        "`[parameters.entry_order]`, `[parameters.exit_order]`, and `[parameters.forced_exit_order]`",
        next_heading_prefix="### ",
    )
    if not order_params_section:
        findings.append("schema missing order-parameters section")
    else:
        for field in ORDER_TEMPLATE_FIELDS:
            if f"`{field}`" not in order_params_section:
                findings.append(f"schema order-parameters section missing `{field}`")
        for side in ("`buy`", "`sell`"):
            if side not in order_params_section:
                findings.append(f"schema order-parameters section missing side value {side}")
        for position_side in ("`long`", "`short`"):
            if position_side not in order_params_section:
                findings.append(
                    f"schema order-parameters section missing position_side value {position_side}"
                )

    if validate_source:
        supported_strategy_schema_version = extract_supported_strategy_schema_version(
            validate_source
        )
        if supported_strategy_schema_version is None:
            findings.append("source missing SUPPORTED_STRATEGY_SCHEMA_VERSION")
        else:
            strategy_schema_example_versions = [
                int(match.group("version"))
                for match in STRATEGY_SCHEMA_EXAMPLE_PATTERN.finditer(schema)
            ]
            if not strategy_schema_example_versions:
                findings.append("schema missing strategy schema_version example")
            for version in strategy_schema_example_versions:
                if version != supported_strategy_schema_version:
                    findings.append(
                        "schema strategy schema_version example "
                        f"{version} does not match source {supported_strategy_schema_version}"
                    )

    if archetype_source or strategy_source or position_contract_source:
        findings.extend(
            validate_position_contract_helpers(
                archetype_source,
                strategy_source,
                position_contract_source,
            )
        )

    for phrase in STALE_STATUS_MAP_PHRASES:
        if phrase in status_map:
            findings.append(f"status map still contains stale phrase: {phrase}")

    for phrase in REQUIRED_STATUS_MAP_PHRASES:
        if phrase not in status_map:
            findings.append(f"status map missing current phrase: {phrase}")

    findings.extend(unsupported_scope_overclaims("schema", schema))
    findings.extend(unsupported_scope_overclaims("runtime contracts", runtime_contracts))
    findings.extend(unsupported_scope_overclaims("status map", status_map))

    return findings


def main() -> int:
    findings = validate_docs(
        SCHEMA_DOC.read_text(encoding="utf-8"),
        STATUS_MAP.read_text(encoding="utf-8"),
        runtime_contracts=RUNTIME_CONTRACTS_DOC.read_text(encoding="utf-8"),
        validate_source=VALIDATE_SOURCE.read_text(encoding="utf-8"),
        decision_evidence_source=DECISION_EVIDENCE_SOURCE.read_text(encoding="utf-8"),
        archetype_source=ARCHETYPE_BINARY_ORACLE_SOURCE.read_text(encoding="utf-8"),
        strategy_source=module_text(STRATEGY_SOURCE_ROOTS),
        position_contract_source=POSITION_CONTRACT_SOURCE.read_text(encoding="utf-8")
        if POSITION_CONTRACT_SOURCE.exists()
        else "",
    )
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 schema/status docs match current order-intent source scope.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
