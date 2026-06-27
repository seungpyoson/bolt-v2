#!/usr/bin/env python3
"""Verify outcome-group code reuses pinned NautilusTrader capabilities."""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from typing import Any

from bolt_v3_source_roots import OUTCOME_GROUP_SOURCE_ROOTS, source_set_files
from verify_bolt_v3_provider_leaks import production_text
from verify_bolt_v3_pure_rust_runtime import strip_rust_comments_and_literals


REPO_ROOT = Path(__file__).resolve().parent.parent
LEDGER_PATH = Path("docs/bolt-v3/research/outcome-groups/nt-capability-ledger.toml")

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

VALID_DISPOSITIONS = {
    "reuse_nt",
    "wrap_nt",
    "surface_in_nt",
    "bolt_shim",
    "reject_for_now",
}

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

HEX_REV_RE = re.compile(r"^[0-9a-f]{7,40}$")
LINES_RE = re.compile(r"^\d+(?:-\d+)?$")


def _is_non_empty_string(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _is_non_empty_string_list(value: object) -> bool:
    return isinstance(value, list) and bool(value) and all(_is_non_empty_string(item) for item in value)


def _repo_relative_path(value: object) -> bool:
    if not _is_non_empty_string(value):
        return False
    path = Path(str(value))
    return not path.is_absolute() and ".." not in path.parts


def load_ledger(root: Path, findings: list[str]) -> dict[str, Any] | None:
    path = root / LEDGER_PATH
    if not path.is_file():
        findings.append(f"{LEDGER_PATH}: missing NT capability ledger")
        return None

    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as error:
        findings.append(f"{LEDGER_PATH}: invalid TOML ledger: {error}")
        return None


def validate_anchor(capability: str, index: int, anchor: object, findings: list[str]) -> None:
    if not isinstance(anchor, dict):
        findings.append(f"{capability} source_anchors[{index}] must be a table")
        return

    for field in ("repo", "rev", "path", "lines", "symbol", "evidence"):
        if not _is_non_empty_string(anchor.get(field)):
            findings.append(f"{capability} source_anchors[{index}] missing {field}")

    rev = anchor.get("rev")
    if _is_non_empty_string(rev) and HEX_REV_RE.fullmatch(str(rev)) is None:
        findings.append(f"{capability} source_anchors[{index}] rev must be a pinned hex revision")

    path = anchor.get("path")
    if not _repo_relative_path(path):
        findings.append(f"{capability} source_anchors[{index}] path must be repo-relative")

    lines = anchor.get("lines")
    if _is_non_empty_string(lines) and LINES_RE.fullmatch(str(lines)) is None:
        findings.append(f"{capability} source_anchors[{index}] lines must be a concrete line or range")


def validate_ledger(root: Path) -> list[str]:
    findings: list[str] = []
    data = load_ledger(root, findings)
    if data is None:
        return findings

    ledger = data.get("ledger")
    if not isinstance(ledger, dict):
        findings.append(f"{LEDGER_PATH}: missing [ledger] table")
    else:
        if ledger.get("version") != 1:
            findings.append(f"{LEDGER_PATH}: ledger.version must be 1")
        for field in ("nt_revision", "bolt_revision"):
            value = ledger.get(field)
            if not _is_non_empty_string(value) or HEX_REV_RE.fullmatch(str(value)) is None:
                findings.append(f"{LEDGER_PATH}: ledger.{field} must be a pinned hex revision")

    capabilities = data.get("capabilities")
    if not isinstance(capabilities, dict):
        findings.append(f"{LEDGER_PATH}: missing [capabilities] table")
        return findings

    for capability in REQUIRED_CAPABILITIES:
        entry = capabilities.get(capability)
        if not isinstance(entry, dict):
            findings.append(f"{LEDGER_PATH}: missing capability {capability}")
            continue

        disposition = entry.get("disposition")
        if disposition not in VALID_DISPOSITIONS:
            findings.append(f"{capability} disposition must be one of {sorted(VALID_DISPOSITIONS)}")

        if not _is_non_empty_string(entry.get("owner_module")):
            findings.append(f"{capability} missing owner_module")

        reason = entry.get("reason")
        if not _is_non_empty_string(reason):
            findings.append(f"{capability} missing reason")

        required_tests = entry.get("required_tests")
        if not _is_non_empty_string_list(required_tests):
            findings.append(f"{capability} missing required_tests")

        anchors = entry.get("source_anchors")
        if not isinstance(anchors, list) or not anchors:
            findings.append(f"{capability} missing source_anchors")
        else:
            for index, anchor in enumerate(anchors):
                validate_anchor(capability, index, anchor, findings)

        if disposition == "bolt_shim":
            if not _is_non_empty_string(reason):
                findings.append(f"{capability} bolt_shim requires reason")
            if not _is_non_empty_string_list(required_tests):
                findings.append(f"{capability} bolt_shim requires required_tests")

    return findings


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
    findings.extend(validate_ledger(root))
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
