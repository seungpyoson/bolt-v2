#!/usr/bin/env python3
"""Verify active Bolt-v3 docs match current order-intent source scope."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DOC = REPO_ROOT / "docs/bolt-v3/2026-04-25-bolt-v3-schema.md"
STATUS_MAP = REPO_ROOT / "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
RESEARCH_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/research.md"
TASKS_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/tasks.md"
CONTRACT_DOC = REPO_ROOT / "specs/023-nt-order-intent-layer/contracts/order-intent-layer.md"

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
)
REQUIRED_SCHEMA_PHRASES = (
    "delegates accepted values to NautilusTrader `OmsType`",
    "The current source-level tests prove `netting`, `hedging`, and `unspecified` parse and validate",
    "unsupported for this archetype because the pinned NT single-order `OrderFactory` exposes no public constructor",
    "The current archetype accepts the long position contract only",
    "Short-side position contracts are parsed but rejected until strategy-owned short economics, collateral, and exit semantics exist",
    "Entry `is_quote_quantity = true` is supported by sizing the entry quantity as quote notional",
    "Exit `is_quote_quantity = true` is rejected because exits are sized from held base position quantity",
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
)
STALE_CONTRACT_PHRASES = (
    "Long and short position contracts are coherent.",
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
    r"before|outside|beyond|fails?|failed)\b",
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


def validate_docs(
    schema: str,
    status_map: str,
    research: str = "",
    tasks: str = "",
    contract: str = "",
) -> list[str]:
    findings: list[str] = []

    for phrase in STALE_SCHEMA_PHRASES:
        if phrase in schema:
            findings.append(f"schema still contains stale phrase: {phrase}")

    for phrase in REQUIRED_SCHEMA_PHRASES:
        if phrase not in schema:
            findings.append(f"schema missing current phrase: {phrase}")

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
        "`[parameters.entry_order]` and `[parameters.exit_order]`",
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

    for phrase in STALE_CONTRACT_PHRASES:
        if phrase in contract:
            findings.append(f"contract still contains stale phrase: {phrase}")

    findings.extend(unsupported_scope_overclaims("schema", schema))
    findings.extend(unsupported_scope_overclaims("status map", status_map))
    findings.extend(unsupported_scope_overclaims("research", research))
    findings.extend(unsupported_scope_overclaims("tasks", tasks))
    findings.extend(unsupported_scope_overclaims("contract", contract))

    return findings


def main() -> int:
    findings = validate_docs(
        SCHEMA_DOC.read_text(encoding="utf-8"),
        STATUS_MAP.read_text(encoding="utf-8"),
        RESEARCH_DOC.read_text(encoding="utf-8"),
        TASKS_DOC.read_text(encoding="utf-8"),
        CONTRACT_DOC.read_text(encoding="utf-8"),
    )
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: Bolt-v3 schema/status docs match current order-intent source scope.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
