#!/usr/bin/env python3
"""Verify active Bolt-v3 docs match current order-intent source scope."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md"
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
RESEARCH_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/research.md"
TASKS_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/tasks.md"
CONTRACT_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/contracts/order-intent-layer.md"
SPEC_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/spec.md"
DATA_MODEL_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/data-model.md"
AGENTS_DOC = REPO_ROOT / "AGENTS.md"
FEATURE_JSON = REPO_ROOT / ".specify/feature.json"
ORDER_INTENT_FEATURE_DIR = "specs/023-nt-order-intent-layer"
ORDER_INTENT_PLAN = f"{ORDER_INTENT_FEATURE_DIR}/plan.md"
SPECKIT_BLOCK_PATTERN = re.compile(
    r"<!-- SPECKIT START -->(?P<body>.*?)<!-- SPECKIT END -->",
    re.DOTALL,
)
BACKTICKED_PLAN_PATTERN = re.compile(r"`(?P<path>specs/[^`]+/plan\.md)`")

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
)
STALE_STATUS_MAP_PHRASES = (
    "Single-value enums (`RuntimeMode::Live`, `OmsType::Netting`, `CatalogFsProtocol::File`, `RotationKind::None`)",
)
REQUIRED_STATUS_MAP_PHRASES = (
    "Strategy `oms_type` delegates to NT `OmsType` variants instead of a Bolt-only netting allowlist",
    "order-template validation follows the pinned NT single-order `OrderFactory` surface",
)
STALE_RESEARCH_PHRASES = (
    "current archetype accepts coherent short-side",
    "current archetype supports coherent short-side",
)
STALE_TASKS_PHRASES = (
    "Allow coherent short-side contracts while keeping incoherent long/short contracts rejected",
    "Phase 47 blocks completion because current-head multi-agent review found forced-flat exit order semantics still synthesized as Market/TIF/reduce-only fields in strategy code rather than carried as a TOML-owned NT order template.",
    "Phase 48 blocks completion because latest-head multi-agent review found active schema docs still describe removed market-exit fields and `manage_stop=true` can silently route non-market `forced_exit_order` configs through NT's built-in market close path.",
    "Phase 50 blocks completion because current-head PR-body/Greptile evidence and source inspection found maker entry sizing still uses taker-side book depth and external Managed close still drops a resting pending entry without NT cancel.",
    "Phase 34 blocks completion because multi-agent pinned-NT review found TrailingStopMarket validation still requires optional fields that NT defaults.",
    "Phase 50 closes the current-head maker lifecycle/sizing review findings; only terminal reviewer/no-mistakes state remains open in T224.",
    "Phase 51 closes the TrailingStopMarket schema-default drift and equivalent-wording verifier gap; only terminal reviewer/no-mistakes state remains open in T228.",
    "Phase 52 remains open until T233 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.",
    "Phase 53 remains open until T236 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.",
    "Phase 54 remains open until T240 records focused verification, branch cleanliness, exact-head PR checks, and terminal or timed-out reviewer/no-mistakes state.",
)
REQUIRED_TASKS_PHRASES = (
    "Phase 50 is closed by T224 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.",
    "Phase 51 is closed by T228 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.",
    "Phase 52 is closed by T233 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.",
    "Phase 53 is closed by T236 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.",
    "Phase 54 is closed by T240 verification, with no-mistakes wait-cap state recorded as non-terminal reviewer evidence rather than approval.",
)
STALE_CONTRACT_PHRASES = (
    "Long and short position contracts are coherent.",
)
STALE_SPEC_PHRASES = (
    "config validation does not reject the shape merely because it is short-side",
)
STALE_DATA_MODEL_PHRASES = (
    "`expire_time_unix_nanos` only when GTD is enabled by a reviewed slice",
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


def validate_speckit_context(agents_doc: str | None, feature_json: str | None) -> list[str]:
    findings: list[str] = []

    if agents_doc is not None:
        if not agents_doc.strip():
            findings.append("AGENTS.md is empty; missing active Speckit block")
        else:
            match = SPECKIT_BLOCK_PATTERN.search(agents_doc)
            if match is None:
                findings.append("AGENTS.md missing active Speckit block")
            else:
                speckit_block = match.group("body")
                plan_paths = [
                    plan_match.group("path") for plan_match in BACKTICKED_PLAN_PATTERN.finditer(speckit_block)
                ]
                if plan_paths != [ORDER_INTENT_PLAN]:
                    findings.append(
                        "AGENTS.md active Speckit block must contain exactly "
                        f"`{ORDER_INTENT_PLAN}` as its plan pointer, got {plan_paths!r}"
                    )
                if "specs/023-nt-research-analytics-platform/plan.md" in speckit_block:
                    findings.append(
                        "AGENTS.md active Speckit block still points at stale research-analytics plan"
                    )

    if feature_json is not None:
        if not feature_json.strip():
            findings.append(".specify/feature.json is empty; missing feature_directory")
            return findings
        try:
            parsed = json.loads(feature_json)
        except json.JSONDecodeError as exc:
            findings.append(f".specify/feature.json is not valid JSON: {exc.msg}")
        else:
            if not isinstance(parsed, dict):
                findings.append(".specify/feature.json must be a JSON object")
                return findings
            feature_directory = parsed.get("feature_directory")
            if feature_directory != ORDER_INTENT_FEATURE_DIR:
                findings.append(
                    ".specify/feature.json points to "
                    f"{feature_directory!r}, expected {ORDER_INTENT_FEATURE_DIR!r}"
                )

    return findings


def validate_docs(
    schema: str,
    status_map: str,
    research: str = "",
    tasks: str = "",
    contract: str = "",
    spec: str = "",
    data_model: str = "",
    agents_doc: str | None = None,
    feature_json: str | None = None,
) -> list[str]:
    findings: list[str] = []

    for phrase in STALE_SCHEMA_PHRASES:
        if phrase in schema:
            findings.append(f"schema still contains stale phrase: {phrase}")

    for phrase in REQUIRED_SCHEMA_PHRASES:
        if phrase not in schema:
            findings.append(f"schema missing current phrase: {phrase}")

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

    for phrase in STALE_STATUS_MAP_PHRASES:
        if phrase in status_map:
            findings.append(f"status map still contains stale phrase: {phrase}")

    for phrase in REQUIRED_STATUS_MAP_PHRASES:
        if phrase not in status_map:
            findings.append(f"status map missing current phrase: {phrase}")

    for phrase in STALE_RESEARCH_PHRASES:
        if phrase in research:
            findings.append(f"research still contains stale phrase: {phrase}")

    for phrase in STALE_TASKS_PHRASES:
        if phrase in tasks:
            findings.append(f"tasks still contains stale phrase: {phrase}")

    if tasks:
        for phrase in REQUIRED_TASKS_PHRASES:
            if phrase not in tasks:
                findings.append(f"tasks missing current phrase: {phrase}")

    for phrase in STALE_CONTRACT_PHRASES:
        if phrase in contract:
            findings.append(f"contract still contains stale phrase: {phrase}")

    for phrase in STALE_SPEC_PHRASES:
        if phrase in spec:
            findings.append(f"spec still contains stale phrase: {phrase}")

    for phrase in STALE_DATA_MODEL_PHRASES:
        if phrase in data_model:
            findings.append(f"data model still contains stale phrase: {phrase}")

    findings.extend(unsupported_scope_overclaims("schema", schema))
    findings.extend(unsupported_scope_overclaims("status map", status_map))
    findings.extend(unsupported_scope_overclaims("research", research))
    findings.extend(unsupported_scope_overclaims("tasks", tasks))
    findings.extend(unsupported_scope_overclaims("contract", contract))
    findings.extend(unsupported_scope_overclaims("spec", spec))
    findings.extend(unsupported_scope_overclaims("data model", data_model))
    findings.extend(validate_speckit_context(agents_doc, feature_json))

    return findings


def main() -> int:
    findings = validate_docs(
        SCHEMA_DOC.read_text(encoding="utf-8"),
        STATUS_MAP.read_text(encoding="utf-8"),
        RESEARCH_DOC.read_text(encoding="utf-8"),
        TASKS_DOC.read_text(encoding="utf-8"),
        CONTRACT_DOC.read_text(encoding="utf-8"),
        SPEC_DOC.read_text(encoding="utf-8"),
        DATA_MODEL_DOC.read_text(encoding="utf-8"),
        AGENTS_DOC.read_text(encoding="utf-8"),
        FEATURE_JSON.read_text(encoding="utf-8"),
    )
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 schema/status docs match current order-intent source scope.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
