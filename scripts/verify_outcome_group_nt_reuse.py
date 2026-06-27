#!/usr/bin/env python3
"""Verify outcome-group code reuses pinned NautilusTrader capabilities."""

from __future__ import annotations

import re
import sys
from pathlib import Path

from bolt_v3_source_roots import OUTCOME_GROUP_SOURCE_ROOTS, source_set_files
from verify_bolt_v3_provider_leaks import production_text
from verify_bolt_v3_pure_rust_runtime import strip_rust_comments_and_literals


REPO_ROOT = Path(__file__).resolve().parent.parent

REQUIRED_CAPABILITIES: tuple[str, ...] = (
    "neg_risk_market_id",
    "source_grouping_proof",
    "submit_order_list",
    "order_book_depth",
    "order_management_cancel_modify",
    "nt_cache_reconciliation",
    "external_order_claims",
    "settlement_signals",
    "hip4_discovery",
    "min_size_precision",
    "provider_discovery",
)

APPROVED_PROVIDER_NORMALIZER_PREFIXES: tuple[str, ...] = (
    "src/bolt_v3_outcome_group_polymarket.rs",
    "src/bolt_v3_outcome_group_hyperliquid.rs",
    "src/bolt_v3_providers/polymarket.rs",
    "src/bolt_v3_providers/polymarket/",
    "src/bolt_v3_providers/hyperliquid.rs",
    "src/bolt_v3_providers/hyperliquid/",
)

OPAQUE_PROOF_ALLOWED_PREFIXES: tuple[str, ...] = (
    "src/bolt_v3_outcome_groups.rs",
    "src/bolt_v3_outcome_group_sources.rs",
    "src/bolt_v3_outcome_group_polymarket.rs",
    "src/bolt_v3_outcome_group_hyperliquid.rs",
)


def outcome_group_source_files(root: Path) -> list[Path]:
    return source_set_files(OUTCOME_GROUP_SOURCE_ROOTS, repo_root=root)


def scan_text(source: str) -> str:
    return strip_rust_comments_and_literals(production_text(source))


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def excerpt(text: str, pos: int) -> str:
    start = text.rfind("\n", 0, pos) + 1
    end = text.find("\n", pos)
    if end == -1:
        end = len(text)
    return text[start:end].strip()


def is_provider_or_normalizer(relative_path: str) -> bool:
    return any(
        relative_path == prefix or relative_path.startswith(prefix)
        for prefix in APPROVED_PROVIDER_NORMALIZER_PREFIXES
    )


def is_opaque_proof_allowed(relative_path: str) -> bool:
    return any(
        relative_path == prefix or relative_path.startswith(prefix)
        for prefix in OPAQUE_PROOF_ALLOWED_PREFIXES
    )


def add_pattern_findings(
    relative_path: str,
    text: str,
    pattern: re.Pattern[str],
    label: str,
    findings: list[str],
) -> None:
    for match in pattern.finditer(text):
        findings.append(
            f"{relative_path}:{line_number(text, match.start())}: {label}: {excerpt(text, match.start())}"
        )


PER_LEG_SUBMIT_RE = re.compile(
    r"\bfor\s+[A-Za-z_][A-Za-z0-9_]*\s+in\s+[^\{\n;]*(?:legs|orders)[^\{\n;]*"
    r"[\s\S]{0,500}\.\s*submit_order\s*\(",
    re.MULTILINE,
)
DIRECT_VENUE_SUBMIT_RE = re.compile(
    r"\b(?:PolymarketExecutionClient|PolymarketClobClient|HyperliquidExecutionClient)\b"
    r"|[A-Za-z_][A-Za-z0-9_]*\.\s*submit_order\s*\(",
    re.MULTILINE,
)
CUSTOM_BOOK_RE = re.compile(
    r"\bstruct\s+[A-Za-z_][A-Za-z0-9_]*(?:OrderBook|Book)[A-Za-z0-9_]*\b"
    r"|\bBTreeMap\s*<\s*Price\s*,\s*Quantity\s*>",
    re.MULTILINE,
)
DIRECT_VENUE_CANCEL_RE = re.compile(
    r"\b(?:PolymarketClobClient|PolymarketExecutionClient|HyperliquidExecutionClient)\b"
    r"|[A-Za-z_][A-Za-z0-9_]*\.\s*cancel_order\s*\(",
    re.MULTILINE,
)
ORDER_CACHE_DUPLICATION_RE = re.compile(
    r"\border_history\b|\bOrderStatusReport\b|\bHashMap\s*<\s*ClientOrderId\s*,\s*Order",
    re.MULTILINE,
)
DIRECT_CLIENT_RE = re.compile(
    r"\b(?:PolymarketGammaHttpClient|PolymarketExecutionClient|PolymarketClobClient|HyperliquidExecutionClient)\b"
    r"|\breqwest::Client\b|\btokio_tungstenite\b|\bnautilus_polymarket::http\b"
    r"|\bnautilus_hyperliquid::http::client\b",
    re.MULTILINE,
)
OPAQUE_PROOF_VARIANT_RE = re.compile(
    r"\b(?:GroupingProof|RoleBindingProof|OutcomeGroupSourceKind|SettlementSourceKind"
    r"|NormalizedPriceScaleEvidence|PriceScaleAssertionSource|OrderConstraintSource)::",
    re.MULTILINE,
)
NT_ORDER_LIST_CONTRACT_RE = re.compile(r"\bnt_order_management_contract\s*\(", re.MULTILINE)


def references_nt_order_list_contract(text: str) -> bool:
    return (
        bool(re.search(r"\bOrderList\b", text))
        and bool(re.search(r"\bSubmitOrderList\b", text))
    ) or bool(NT_ORDER_LIST_CONTRACT_RE.search(text))


def validate_source_file(root: Path, path: Path) -> list[str]:
    findings: list[str] = []
    relative_path = path.relative_to(root).as_posix()
    text = scan_text(path.read_text(encoding="utf-8"))

    if not is_provider_or_normalizer(relative_path):
        add_pattern_findings(relative_path, text, DIRECT_CLIENT_RE, "direct venue/provider client import", findings)

    if not is_opaque_proof_allowed(relative_path):
        add_pattern_findings(
            relative_path,
            text,
            OPAQUE_PROOF_VARIANT_RE,
            "opaque outcome-group proof variant branch",
            findings,
        )

    if "scanner" in relative_path or "scan" in relative_path:
        if not re.search(r"\b(?:OrderBook|OrderBookDepth10|OrderBookDeltas|BookLevel|QuoteTick)\b", text):
            findings.append(f"{relative_path}: scanner must reference NT book/depth primitives")
        if not re.search(r"\bExecutableBookQuote\b", text):
            findings.append(f"{relative_path}: scanner must reference ExecutableBookQuote")
        add_pattern_findings(relative_path, text, CUSTOM_BOOK_RE, "custom order-book model", findings)

    if (
        "basket_execution" in relative_path
        or "outcome_group_execution" in relative_path
        or relative_path.startswith("src/strategies/complete_set_arbitrage/")
    ):
        if not references_nt_order_list_contract(text):
            findings.append(
                f"{relative_path}: basket execution must reference NT OrderList/SubmitOrderList or delegate to nt_order_management_contract"
            )
        add_pattern_findings(relative_path, text, PER_LEG_SUBMIT_RE, "per-leg submit loop", findings)
        add_pattern_findings(relative_path, text, DIRECT_VENUE_SUBMIT_RE, "direct venue submit path", findings)

    if "repair" in relative_path or "unwind" in relative_path:
        if not re.search(r"\b(?:CancelOrder|BatchCancelOrders|CancelAllOrders|ModifyOrder)\b", text):
            findings.append(f"{relative_path}: repair/unwind must reference NT cancel/modify commands")
        add_pattern_findings(relative_path, text, DIRECT_VENUE_CANCEL_RE, "direct venue cancel path", findings)

    if "basket_store" in relative_path:
        add_pattern_findings(
            relative_path,
            text,
            ORDER_CACHE_DUPLICATION_RE,
            "general order-cache/history model",
            findings,
        )

    return findings


def validate_outcome_sources(root: Path) -> list[str]:
    findings: list[str] = []
    for path in outcome_group_source_files(root):
        findings.extend(validate_source_file(root, path))
    return findings


def uncommented_justfile_text(text: str) -> str:
    lines = []
    for raw_line in text.splitlines():
        stripped = raw_line.lstrip()
        if stripped.startswith("#"):
            continue
        lines.append(raw_line)
    return "\n".join(lines)


def validate_just_wiring(root: Path) -> list[str]:
    path = root / "Justfile"
    if not path.is_file():
        path = root / "justfile"
    if not path.is_file():
        return ["Justfile: missing source-fence-static recipe"]

    text = uncommented_justfile_text(path.read_text(encoding="utf-8"))
    findings = []
    for command in (
        "python3 scripts/test_verify_outcome_group_nt_reuse.py",
        "python3 scripts/verify_outcome_group_nt_reuse.py",
    ):
        if command not in text:
            findings.append(f"source-fence-static must run {command}")
    return findings


def collect_findings(root: Path = REPO_ROOT) -> list[str]:
    findings: list[str] = []
    findings.extend(validate_outcome_sources(root))
    findings.extend(validate_just_wiring(root))
    return findings


def main() -> int:
    findings = collect_findings()
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1

    print("OK: outcome-group NT reuse verifier passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
