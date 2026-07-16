#!/usr/bin/env python3
"""Verify bolt-v3 provider/runtime boundary evidence wiring."""

from __future__ import annotations

import dataclasses
import datetime as dt
import json
import os
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path

import ci_provenance
from ci_test_manifest import _mask_rust_non_code
from verify_bolt_v3_provider_leaks import production_text
from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
EXPECTED_NT_GIT = "https://github.com/seungpyoson/nautilus_trader.git"
REGISTRY = Path("src/bolt_v3_providers/boundary_registry.rs")
WIRE_BOUNDARY = Path("src/bolt_v3_wire_boundary.rs")
EXEMPTIONS = Path("ci/bolt-v3-boundary-exemptions.toml")
CAPTURE_PROVENANCE_CONFIG = Path("ci/chainlink-reference-fixture-capture-provenance.toml")
FIXTURE_DIR = Path("tests/fixtures/bolt_v3/boundary_evidence")

REQUIRED_CLASSES = {
    "WebSocketFrame",
    "ImdsMetadata",
    "AwsSdkResponse",
    "HttpResponseBody",
}
REQUIRED_WS_FEEDERS = (
    "ReferenceCurrentPriceHealth",
    "ReferenceLiveProbe",
)
REQUIRED_BINANCE_WS_REGISTRY_ENTRIES = {
    ("BINANCE_SPOT_SBE_ADAPTER_ID", "WebSocketFrame", "RealizedVolatilityObservation"),
    ("BINANCE_SPOT_SBE_ADAPTER_ID", "WebSocketFrame", "StrategySignalObservation"),
}
CANONICAL_CARGO_PIN_SURFACES = (
    Path("Cargo.toml"),
    Path("Cargo.lock"),
    Path("crates/backtesting-vertical-slice/Cargo.toml"),
    Path("crates/backtesting-vertical-slice/Cargo.lock"),
)
RUNTIME_CONTRACT_PIN_SURFACE = Path(
    "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"
)
ROOT_SCHEMA_PIN_SURFACE = Path("docs/bolt-v3/2026-04-25-bolt-v3-schema.md")
NT_NAMING_LEDGER_PIN_SURFACE = Path(
    "docs/bolt-v3/research/naming/nt-owned-name-audit.yaml"
)
NT_BOUNDARY_DOCTRINE_PIN_SURFACE = Path(
    "docs/bolt-v3/2026-04-28-nt-first-boundary-doctrine.md"
)
POLYMARKET_QUERY_FIXTURE_PIN_SURFACE = Path(
    "tests/fixtures/nt_polymarket_query_post_order_params_d636f176.txt"
)
NON_CARGO_PIN_SURFACES = (
    RUNTIME_CONTRACT_PIN_SURFACE,
    ROOT_SCHEMA_PIN_SURFACE,
    NT_BOUNDARY_DOCTRINE_PIN_SURFACE,
    NT_NAMING_LEDGER_PIN_SURFACE,
    POLYMARKET_QUERY_FIXTURE_PIN_SURFACE,
)
BINANCE_SOURCE_SYMBOLS = (
    "BinanceSpotDataClient::handle_ws_message",
    "handle_ws_message_uses_clock_timestamp_for_sbe_bbo_ts_init",
    "decode_market_data",
    "parse_trades_event",
    "parse_bbo_event",
    "parse_depth_snapshot",
    "parse_depth_diff",
)
BINANCE_BOUNDARY_CONSUMERS = (
    "RealizedVolatilityObservation",
    "StrategySignalObservation",
)
BINANCE_BOUNDARY_OWNER_HEADING = "### 11.5 NautilusTrader pin governance"
BINANCE_TIMESTAMP_TEST_TARGET = "binance_sbe_quote_timestamps"
BINANCE_TIMESTAMP_TEST_PATH = Path("tests/binance_sbe_quote_timestamps.rs")
BINANCE_TIMESTAMP_TEST_SAFE_TARGET_FIELDS = frozenset(
    {"name", "path", "harness", "test"}
)
BINANCE_TIMESTAMP_PARSER_ALIAS = "nt_binance_sbe_parse"
BINANCE_TIMESTAMP_PARSER_IMPORT_PATTERN = re.compile(
    r"\buse\s+::\s*nautilus_binance\s*::\s*spot\s*::\s*websocket\s*::\s*streams\s*::\s*parse"
    r"\s+as\s+(?P<alias>nt_binance_sbe_parse)\s*;"
)
BINANCE_TIMESTAMP_PARSER_SYMBOLS = (
    "parse_trades_event",
    "parse_bbo_event",
    "parse_depth_snapshot",
    "parse_depth_diff",
)
BINANCE_TIMESTAMP_TEST_CASE_RESULT_CONTRACTS = {
    "sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps": (
        "parse_trades_event",
        "trades",
    ),
    "sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps": (
        "parse_bbo_event",
        "quote",
    ),
    "sbe_depth_snapshot_preserves_unequal_event_and_adapter_initialization_stamps": (
        "parse_depth_snapshot",
        "deltas",
    ),
    "sbe_depth_diff_preserves_unequal_event_and_adapter_initialization_stamps": (
        "parse_depth_diff",
        "deltas",
    ),
}
BINANCE_TIMESTAMP_DEPTH_EXPECT_MESSAGES = {
    "parse_depth_snapshot": "non-empty SBE depth snapshot must produce deltas",
    "parse_depth_diff": "non-empty SBE depth diff must produce deltas",
}
BINANCE_TIMESTAMP_TEST_CASE_EVENT_CONTRACTS = {
    "sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps": (
        "transact_time_us",
        "TradesStreamEvent",
    ),
    "sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps": (
        "event_time_us",
        "BestBidAskStreamEvent",
    ),
    "sbe_depth_snapshot_preserves_unequal_event_and_adapter_initialization_stamps": (
        "event_time_us",
        "DepthSnapshotStreamEvent",
    ),
    "sbe_depth_diff_preserves_unequal_event_and_adapter_initialization_stamps": (
        "event_time_us",
        "DepthDiffStreamEvent",
    ),
}
BINANCE_TIMESTAMP_TRADE_PER_ITEM_ASSERTIONS = frozenset(
    {
        "per-trade event timestamp assertion",
        "per-trade initialization timestamp assertion",
    }
)
BINANCE_TIMESTAMP_TEST_CASE_REQUIREMENTS = {
    "sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps": (
        (
            "pinned parse_trades_event call",
            r"\bnt_binance_sbe_parse\s*::\s*parse_trades_event\s*\(",
        ),
        (
            "unequal event/init assertion",
            r"::\s*core\s*::\s*assert_ne\s*!\s*\(\s*expected_ts_event\s*,\s*adapter_ts_init",
        ),
        (
            "two-output assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*trades\s*\.\s*len\s*\(\s*\)\s*,\s*2",
        ),
        ("all-output iteration", r"\bfor\s+data\s+in\s+trades\b"),
        ("TradeTick extraction", r"\bData\s*::\s*Trade\b"),
        (
            "per-trade event timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*trade\s*\.\s*ts_event\s*,\s*expected_ts_event",
        ),
        (
            "per-trade initialization timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*trade\s*\.\s*ts_init\s*,\s*adapter_ts_init",
        ),
    ),
    "sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps": (
        (
            "pinned parse_bbo_event call",
            r"\bnt_binance_sbe_parse\s*::\s*parse_bbo_event\s*\(",
        ),
        (
            "unequal event/init assertion",
            r"::\s*core\s*::\s*assert_ne\s*!\s*\(\s*expected_ts_event\s*,\s*adapter_ts_init",
        ),
        (
            "quote event timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*quote\s*\.\s*ts_event\s*,\s*expected_ts_event",
        ),
        (
            "quote initialization timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*quote\s*\.\s*ts_init\s*,\s*adapter_ts_init",
        ),
    ),
    "sbe_depth_snapshot_preserves_unequal_event_and_adapter_initialization_stamps": (
        (
            "pinned parse_depth_snapshot call",
            r"\bnt_binance_sbe_parse\s*::\s*parse_depth_snapshot\s*\(",
        ),
        (
            "unequal event/init assertion",
            r"::\s*core\s*::\s*assert_ne\s*!\s*\(\s*expected_ts_event\s*,\s*adapter_ts_init",
        ),
        (
            "three-inner-delta assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*len\s*\(\s*\)\s*,\s*3",
        ),
        (
            "aggregate event timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*ts_event\s*,\s*expected_ts_event",
        ),
        (
            "aggregate initialization timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*ts_init\s*,\s*adapter_ts_init",
        ),
        (
            "all inner event timestamps assertion",
            r"::\s*core\s*::\s*assert\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*iter\s*\(\s*\)\s*\.\s*all\s*\(\s*\|\s*delta\s*\|\s*delta\s*\.\s*ts_event\s*==\s*expected_ts_event",
        ),
        (
            "all inner initialization timestamps assertion",
            r"::\s*core\s*::\s*assert\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*iter\s*\(\s*\)\s*\.\s*all\s*\(\s*\|\s*delta\s*\|\s*delta\s*\.\s*ts_init\s*==\s*adapter_ts_init",
        ),
    ),
    "sbe_depth_diff_preserves_unequal_event_and_adapter_initialization_stamps": (
        (
            "pinned parse_depth_diff call",
            r"\bnt_binance_sbe_parse\s*::\s*parse_depth_diff\s*\(",
        ),
        (
            "unequal event/init assertion",
            r"::\s*core\s*::\s*assert_ne\s*!\s*\(\s*expected_ts_event\s*,\s*adapter_ts_init",
        ),
        (
            "three-inner-delta assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*len\s*\(\s*\)\s*,\s*3",
        ),
        (
            "aggregate event timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*ts_event\s*,\s*expected_ts_event",
        ),
        (
            "aggregate initialization timestamp assertion",
            r"::\s*core\s*::\s*assert_eq\s*!\s*\(\s*deltas\s*\.\s*ts_init\s*,\s*adapter_ts_init",
        ),
        (
            "all inner event timestamps assertion",
            r"::\s*core\s*::\s*assert\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*iter\s*\(\s*\)\s*\.\s*all\s*\(\s*\|\s*delta\s*\|\s*delta\s*\.\s*ts_event\s*==\s*expected_ts_event",
        ),
        (
            "all inner initialization timestamps assertion",
            r"::\s*core\s*::\s*assert\s*!\s*\(\s*deltas\s*\.\s*deltas\s*\.\s*iter\s*\(\s*\)\s*\.\s*all\s*\(\s*\|\s*delta\s*\|\s*delta\s*\.\s*ts_init\s*==\s*adapter_ts_init",
        ),
    ),
}
PIN_TEXT_PATTERNS = {
    ROOT_SCHEMA_PIN_SURFACE: (
        re.compile(
            r"^- `qsize` must equal the pinned NT `LiveDataEngineConfig::default\(\)\.qsize` value, verified as `100000` at pinned NT rev `([0-9a-f]{40})`$",
            re.MULTILINE,
        ),
        re.compile(
            r"^\| `qsize` \| must equal the pinned NT `LiveDataEngineConfig::default\(\)\.qsize` value, verified as `100000` at pinned NT rev `([0-9a-f]{40})` \| `LiveDataEngineConfig\.qsize` \|$",
            re.MULTILINE,
        ),
        re.compile(
            r"^- `qsize` must equal the pinned NT `LiveExecEngineConfig::default\(\)\.qsize` value, verified as `100000` at pinned NT rev `([0-9a-f]{40})`$",
            re.MULTILINE,
        ),
        re.compile(
            r"^\| `qsize` \| must equal the pinned NT `LiveExecEngineConfig::default\(\)\.qsize` value, verified as `100000` at pinned NT rev `([0-9a-f]{40})` \| `LiveExecEngineConfig\.qsize` \|$",
            re.MULTILINE,
        ),
        re.compile(
            r"^- must equal the pinned NT `LiveRiskEngineConfig::default\(\)\.qsize` value, verified as `100000` at pinned NT rev `([0-9a-f]{40})`$",
            re.MULTILINE,
        ),
    ),
    NT_BOUNDARY_DOCTRINE_PIN_SURFACE: (
        re.compile(
            r"^Last NT pin compatibility verified rev: `([0-9a-f]{40})`$",
            re.MULTILINE,
        ),
    ),
    NT_NAMING_LEDGER_PIN_SURFACE: (
        re.compile(r'^nautilus_trader_revision:\s*"([0-9a-f]{40})"$', re.MULTILINE),
    ),
    POLYMARKET_QUERY_FIXTURE_PIN_SURFACE: (
        re.compile(r"^Revision: ([0-9a-f]{40})$", re.MULTILINE),
    ),
}
RUNTIME_CONTRACT_PIN_SECTIONS = (
    (
        "### 9.3 Common required fields",
        re.compile(r"^  - current value: `([0-9a-f]{40})`$", re.MULTILINE),
    ),
    (
        "### 11.5 NautilusTrader pin governance",
        re.compile(
            r"^The live Binance Spot SBE quote boundary is owned by NautilusTrader revision\s+`([0-9a-f]{40})`\.",
            re.MULTILINE,
        ),
    ),
    (
        "## 13. CLOB V2 Readiness Gate",
        re.compile(
            r"^Current status: this branch pins NautilusTrader to\s+`([0-9a-f]{40})` on the bolt pin-fork$",
            re.MULTILINE,
        ),
    ),
)
REQUIRED_NON_WS_REGISTRY_ENTRIES = {
    ("IMDS_METADATA_ADAPTER_ID", "ImdsMetadata", "DeployTargetHostFacts"),
    ("AWS_SSM_SECRET_SOURCE_ADAPTER_ID", "AwsSdkResponse", "SecretResolution"),
    ("polymarket::KEY", "HttpResponseBody", "PolymarketVenueTruthRuntime"),
}
REQUIRED_NON_WS_EXEMPTIONS = {
    ("Imdsv2HostFactsSource", "ImdsMetadata", "DeployTargetHostFacts"),
    ("AwsSsmSecretSource", "AwsSdkResponse", "SecretResolution"),
    ("POLYMARKET", "HttpResponseBody", "PolymarketVenueTruthRuntime"),
}
RUST_VISIBILITY_PREFIX = r"(?:pub(?:\s+|\s*\([^)]*\)\s*)?)?"
FORBIDDEN_NT_WIRE_PATH_PATTERNS = {
    r"\bnautilus_network\s*::\s*websocket\s*::": "nautilus_network::websocket",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*websocket\b": "nautilus_network::websocket",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*\{{[^;]*\bwebsocket\b": "nautilus_network::{websocket...}",
    r"\bextern\s+crate\s+nautilus_network\b": "extern crate nautilus_network",
    r"\bnautilus_network\s*::\s*transport\s*::\s*Message\b": "nautilus_network::transport::Message",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*transport\b": "nautilus_network::transport",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*\{{[^;]*\btransport\b": "nautilus_network::{transport...}",
    r"\bnautilus_network\s*::\s*socket\s*::\s*SocketClient\b": "nautilus_network::socket::SocketClient",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*socket\b": "nautilus_network::socket",
    rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s*::\s*\{{[^;]*\bsocket\b": "nautilus_network::{socket...}",
}
FORBIDDEN_NT_WIRE_SYMBOL_PATTERNS = {
    r"\bWebSocketClient\b": "WebSocketClient",
    r"\bWebSocketClientInner\b": "WebSocketClientInner",
    r"\bWebSocketClient\s*::\s*(connect|connect_url|connect_with_server|connect_stream|connect_with_rate_limiter)\s*\(": "WebSocketClient connect primitive",
    r"\bMessageReader\b": "MessageReader",
    r"\bMessageHandler\b": "MessageHandler",
    r"(?<!Web)\bSocketClient\s*::\s*connect\s*\(": "SocketClient::connect",
    r"(?<!Web)\bSocketClient\b": "SocketClient",
}
FORBIDDEN_WIRE_BOUNDARY_REEXPORT_PATTERN = (
    r"\bpub(?:\s+|\s*\([^)]*\)\s*)(?:use|type)\b[^;]*?"
    r"\b(WebSocketClient|WebSocketClientInner|MessageReader|MessageHandler|SocketClient)\b"
)
ALLOWED_AWS_SSM_PATHS = {
    "src/secrets.rs",
}
ALLOWED_IMDS_CONSTRUCTION_CONTEXTS = {
    "Box::new(Imdsv2HostFactsSource::new())",
    "deploy_target_status(config_root, &Imdsv2HostFactsSource::new())",
}


def read(root: Path, rel: Path | str) -> str:
    return (root / rel).read_text(encoding="utf-8")


def read_required_pin_surface(
    root: Path,
    surface: Path,
    findings: list[str],
) -> str | None:
    try:
        return read(root, surface)
    except OSError as error:
        findings.append(
            f"{surface}: NautilusTrader pin census required pin surface could not be "
            f"read: {error}"
        )
        return None


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def line_at(text: str, pos: int) -> str:
    return text.splitlines()[line_number(text, pos) - 1].strip()


def strip_string_literals_preserve_lines(text: str) -> str:
    output: list[str] = []
    i = 0
    quote: str | None = None
    raw_string_closer: str | None = None
    escaped = False

    while i < len(text):
        char = text[i]

        if raw_string_closer is not None:
            if text.startswith(raw_string_closer, i):
                output.extend(" " for _ in raw_string_closer)
                i += len(raw_string_closer)
                raw_string_closer = None
                continue
            output.append("\n" if char == "\n" else " ")
            i += 1
            continue

        if quote is not None:
            output.append("\n" if char == "\n" else " ")
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            i += 1
            continue

        if char == "r":
            match = re.match(r'r(#+)?"', text[i:])
            if match is not None:
                hashes = match.group(1) or ""
                opener_len = len(match.group(0))
                raw_string_closer = f'"{hashes}'
                output.extend(" " for _ in range(opener_len))
                i += opener_len
                continue

        if char == '"':
            quote = char
            escaped = False
            output.append(" ")
            i += 1
            continue

        output.append(char)
        i += 1

    return "".join(output)


def registry_entries(text: str) -> set[tuple[str, str, str]]:
    entries: set[tuple[str, str, str]] = set()
    for match in re.finditer(r"BoundaryRegistryEntry\s*\{(?P<body>.*?)\}", text, re.DOTALL):
        body = match.group("body")
        adapter = re.search(r"adapter_id:\s*([^,\n]+)", body)
        klass = re.search(r"class:\s*BoundaryEvidenceClass::([A-Za-z0-9_]+)", body)
        feeder = re.search(r"feeder:\s*BoundaryFeeder::([A-Za-z0-9_]+)", body)
        if adapter and klass and feeder:
            entries.add((adapter.group(1).strip(), klass.group(1), feeder.group(1)))
    return entries


def provider_binding_keys(text: str, findings: list[str]) -> set[str]:
    match = re.search(
        r"const\s+PROVIDER_BINDINGS\s*:\s*&\[ProviderBinding\]\s*=\s*&\[(?P<body>.*?)\n\];",
        text,
        re.DOTALL,
    )
    if match is None:
        findings.append("src/bolt_v3_providers/mod.rs: missing PROVIDER_BINDINGS")
        return set()
    return {
        key.strip()
        for key in re.findall(r'\bkey:\s*("[^"]+"|[A-Za-z0-9_:]+)', match.group("body"))
    }


def reference_price_metadata_client_keys(text: str) -> set[str]:
    keys: set[str] = set()
    for match in re.finditer(r"ReferencePriceProviderMetadata\s*\{(?P<body>.*?)\}", text, re.DOTALL):
        client_key = re.search(
            r'\bclient_venue_key:\s*("[^"]+"|[A-Za-z0-9_:]+)',
            match.group("body"),
        )
        if client_key is not None:
            keys.add(client_key.group(1).strip())
    return keys


def reference_live_probe_client_keys(text: str) -> set[str]:
    keys: set[str] = set()
    for match in re.finditer(
        r"validate_reference_live_probe_client\((?P<body>.*?)\);",
        text,
        re.DOTALL,
    ):
        key_matches = re.findall(
            r"\b([A-Za-z_][A-Za-z0-9_:]*::[A-Za-z0-9_]*KEY)\b",
            match.group("body"),
        )
        if key_matches:
            keys.add(key_matches[-1])
    return keys


def required_ws_registry_entries(provider_mod: str, findings: list[str]) -> set[tuple[str, str, str]]:
    binding_keys = provider_binding_keys(provider_mod, findings)
    metadata_keys = reference_price_metadata_client_keys(provider_mod)
    live_probe_keys = reference_live_probe_client_keys(provider_mod)
    reference_keys = metadata_keys | live_probe_keys
    for key in sorted(metadata_keys | live_probe_keys):
        if key not in binding_keys:
            findings.append(
                f"src/bolt_v3_providers/mod.rs: reference provider {key} missing PROVIDER_BINDINGS entry"
            )
    if not reference_keys:
        findings.append("src/bolt_v3_providers/mod.rs: no reference WebSocket provider keys found")
    return {
        (adapter, "WebSocketFrame", feeder)
        for adapter in reference_keys
        for feeder in REQUIRED_WS_FEEDERS
    }


def manifest_exemptions(root: Path) -> list[dict[str, object]]:
    manifest = tomllib.loads(read(root, EXEMPTIONS))
    if manifest.get("schema_version") != 1:
        raise ValueError(f"{EXEMPTIONS}: schema_version must be 1")
    rows = manifest.get("evidence_deferred")
    if not isinstance(rows, list):
        raise ValueError(f"{EXEMPTIONS}: evidence_deferred must be a list")
    return rows


def scan_registry(root: Path, findings: list[str]) -> set[tuple[str, str, str]]:
    text = read(root, REGISTRY)
    provider_mod = read(root, "src/bolt_v3_providers/mod.rs")
    for klass in REQUIRED_CLASSES:
        if f"{klass}," not in text:
            findings.append(f"{REGISTRY}: missing BoundaryEvidenceClass::{klass}")

    entries = registry_entries(text)
    required_entries = (
        REQUIRED_NON_WS_REGISTRY_ENTRIES
        | REQUIRED_BINANCE_WS_REGISTRY_ENTRIES
        | required_ws_registry_entries(provider_mod, findings)
    )
    missing = sorted(required_entries - entries)
    for entry in missing:
        findings.append(f"{REGISTRY}: missing registry entry {entry}")
    extra = sorted(entries - required_entries)
    for entry in extra:
        findings.append(f"{REGISTRY}: unexpected registry entry {entry}")

    cross_checks = {
        "reference_price_provider_metadata": (
            "client_venue_key: chainlink_reference::KEY",
            "client_venue_key: polyresearch::KEY",
        ),
        "validate_reference_live_probe_block": (
            "chainlink_reference::KEY",
            "polyresearch::KEY",
        ),
        "PROVIDER_BINDINGS": (
            "key: chainlink_reference::KEY",
            "key: polyresearch::KEY",
        ),
    }
    for label, needles in cross_checks.items():
        for needle in needles:
            if needle not in provider_mod:
                findings.append(f"src/bolt_v3_providers/mod.rs: {label} missing {needle}")

    return entries


def scan_exemptions(
    root: Path,
    entries: set[tuple[str, str, str]],
    findings: list[str],
    *,
    today: dt.date,
) -> None:
    rows = manifest_exemptions(root)
    seen: set[tuple[str, str, str]] = set()
    registry_by_resolved_adapter = {
        ("Imdsv2HostFactsSource", "ImdsMetadata", "DeployTargetHostFacts"),
        ("AwsSsmSecretSource", "AwsSdkResponse", "SecretResolution"),
        ("POLYMARKET", "HttpResponseBody", "PolymarketVenueTruthRuntime"),
    }
    registry_by_resolved_adapter.update(entries)
    for index, row in enumerate(rows):
        key = (str(row.get("adapter_id")), str(row.get("class")), str(row.get("feeder")))
        if key in seen:
            findings.append(f"{EXEMPTIONS}: duplicate evidence_deferred row {key}")
        seen.add(key)
        if key[1] == "WebSocketFrame":
            findings.append(f"{EXEMPTIONS}: WebSocketFrame must not be exempted: {key}")
        if key not in registry_by_resolved_adapter:
            findings.append(f"{EXEMPTIONS}: orphan evidence_deferred row {key}")
        issue = row.get("issue")
        if not isinstance(issue, int) or issue <= 0:
            findings.append(f"{EXEMPTIONS}: row {index} issue must be a positive integer")
        expires = row.get("expires_on")
        if not isinstance(expires, str):
            findings.append(f"{EXEMPTIONS}: row {index} expires_on must be an ISO date")
            continue
        try:
            expires_on = dt.date.fromisoformat(expires)
        except ValueError:
            findings.append(f"{EXEMPTIONS}: row {index} expires_on must be an ISO date")
            continue
        if expires_on < today:
            findings.append(f"{EXEMPTIONS}: row {index} expired on {expires_on.isoformat()}")

    missing = sorted(REQUIRED_NON_WS_EXEMPTIONS - seen)
    for key in missing:
        findings.append(f"{EXEMPTIONS}: missing required non-WS deferral {key}")


def github_issue_state(repo: str, issue: int, token: str | None) -> str:
    request = urllib.request.Request(
        f"https://api.github.com/repos/{repo}/issues/{issue}",
        headers={
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
            **({"Authorization": f"Bearer {token}"} if token else {}),
        },
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        payload = json.loads(response.read().decode("utf-8"))
    state = payload.get("state")
    if not isinstance(state, str):
        raise RuntimeError(f"GitHub issue #{issue} response has no state")
    return state


def scan_exemption_issue_state(root: Path, findings: list[str]) -> None:
    if root.resolve() != REPO_ROOT.resolve() or os.environ.get("GITHUB_ACTIONS") != "true":
        return
    repo = os.environ.get("GITHUB_REPOSITORY", "seungpyoson/bolt-v2")
    token = os.environ.get("GITHUB_TOKEN")
    for row in manifest_exemptions(root):
        issue = row.get("issue")
        if not isinstance(issue, int):
            continue
        try:
            state = github_issue_state(repo, issue, token)
        except (OSError, urllib.error.URLError, RuntimeError) as exc:
            findings.append(f"{EXEMPTIONS}: could not verify issue #{issue} state: {exc}")
            continue
        if state != "open":
            findings.append(f"{EXEMPTIONS}: issue #{issue} is {state}; remove or replace the deferral")


def boundary_source_paths(root: Path) -> list[Path]:
    return sorted((root / "src").rglob("*.rs"))


def scan_wire_boundary(root: Path, findings: list[str], source_paths: list[Path] | None = None) -> None:
    source_paths = boundary_source_paths(root) if source_paths is None else source_paths
    if not require_nonempty(source_paths, "Bolt-v3 boundary Rust source files", findings):
        return

    for path in source_paths:
        rel = path.relative_to(root).as_posix()
        text = production_text(path.read_text(encoding="utf-8"))
        scan_text = strip_string_literals_preserve_lines(text)
        if rel == WIRE_BOUNDARY.as_posix():
            for match in re.finditer(FORBIDDEN_WIRE_BOUNDARY_REEXPORT_PATTERN, scan_text, re.DOTALL):
                findings.append(
                    f"{rel}:{line_number(scan_text, match.start())}: wire boundary must not re-export raw NT wire symbol {match.group(1)}"
                )
        else:
            for pattern, label in FORBIDDEN_NT_WIRE_PATH_PATTERNS.items():
                for match in re.finditer(pattern, scan_text, re.DOTALL):
                    findings.append(
                        f"{rel}:{line_number(scan_text, match.start())}: raw NT wire module path {label} must go through {WIRE_BOUNDARY}"
                    )
            for pattern, label in FORBIDDEN_NT_WIRE_SYMBOL_PATTERNS.items():
                for match in re.finditer(pattern, scan_text):
                    findings.append(
                        f"{rel}:{line_number(scan_text, match.start())}: raw NT wire symbol {label} must go through {WIRE_BOUNDARY}"
                    )
            for alias_match in re.finditer(
                rf"\b{RUST_VISIBILITY_PREFIX}use\s+nautilus_network\s+as\s+([A-Za-z_][A-Za-z0-9_]*)\b",
                scan_text,
            ):
                alias = re.escape(alias_match.group(1))
                for module in ("websocket", "transport", "socket"):
                    for use_match in re.finditer(rf"\b{alias}\s*::\s*{module}\b", scan_text):
                        findings.append(
                            f"{rel}:{line_number(scan_text, use_match.start())}: raw NT wire module path nautilus_network::{module} must go through {WIRE_BOUNDARY}"
                        )

        if rel not in ALLOWED_AWS_SSM_PATHS and re.search(r"\baws_sdk_ssm::|\baws_sdk_ssm\b", text):
            findings.append(f"{rel}: aws_sdk_ssm usage must go through the registered SSM boundary")
        if re.search(r"\breqwest::|\bhyper::", text):
            findings.append(f"{rel}: HTTP response-body feeder must be registered before reqwest/hyper use")
        for match in re.finditer(r"Imdsv2HostFactsSource::new\(\)", text):
            context = line_at(text, match.start())
            if not any(allowed in context for allowed in ALLOWED_IMDS_CONSTRUCTION_CONTEXTS):
                findings.append(f"{rel}:{line_number(text, match.start())}: unregistered IMDS construction")


def scan_chainlink_tests(root: Path, findings: list[str]) -> None:
    chainlink = read(root, "src/bolt_v3_providers/chainlink_reference.rs")
    required_names = (
        "committed_real_capture_frame_decodes_through_production_handler",
        "binary_report_frame_for_active_subscription_emits_custom_reference_update",
        "invalid_utf8_binary_report_frame_emits_no_custom_data",
        "binary_report_frame_through_text_only_handler_emits_no_custom_data",
        "planted_drop_binary_arm_mutation_would_fail_the_binary_observation_test",
    )
    for name in required_names:
        if f"fn {name}" not in chainlink:
            findings.append(f"src/bolt_v3_providers/chainlink_reference.rs: missing test {name}")
    production = production_text(chainlink)
    if "WireMessage::Text(bytes) | WireMessage::Binary(bytes)" not in production:
        findings.append("src/bolt_v3_providers/chainlink_reference.rs: Chainlink handler must accept Text and Binary frames")
    health = read(root, "src/bolt_v3_reference_price_health.rs")
    if "chainlink_binary_loopback_observes_reference_update_through_health_msgbus" not in health:
        findings.append("src/bolt_v3_reference_price_health.rs: missing Chainlink loopback health/msgbus test")
    forbidden_shortcuts = (
        "ReferenceCurrentPriceHealthObservedUpdate {",
        "ReferencePriceUpdate::try_new",
    )
    loopback_match = re.search(
        r"async fn chainlink_binary_loopback_observes_reference_update_through_health_msgbus\(\).*?\n    \}",
        health,
        re.DOTALL,
    )
    if loopback_match:
        body = loopback_match.group(0)
        for shortcut in forbidden_shortcuts:
            if shortcut in body:
                findings.append(f"src/bolt_v3_reference_price_health.rs: loopback harness uses shortcut {shortcut}")


def scan_fixture_origin(root: Path, findings: list[str]) -> None:
    directory = root / FIXTURE_DIR
    sidecars = sorted(directory.glob("*.toml")) if directory.exists() else []
    if not sidecars:
        findings.append(f"{FIXTURE_DIR}: missing Chainlink fixture sidecar")
        return

    config_path = root / CAPTURE_PROVENANCE_CONFIG
    config = ci_provenance.load_config(
        config_path, require_workflows=False, require_deploy_window=False
    )
    for sidecar in sidecars:
        rel = sidecar.relative_to(root).as_posix()
        data = tomllib.loads(sidecar.read_text(encoding="utf-8"))
        required = {
            "schema_version",
            "adapter_id",
            "class",
            "feeder",
            "frame_kind",
            "signature_verified",
            "fixture",
            "fixture_sha256",
            "capture_artifact",
            "capture_head_sha",
            "capture_head_branch",
        }
        missing = sorted(key for key in required if key not in data)
        if missing:
            findings.append(f"{rel}: missing fields {missing}")
            continue
        if data["schema_version"] != 1:
            findings.append(f"{rel}: schema_version must be 1")
        if data["adapter_id"] != "CHAINLINK_REFERENCE_PRICE":
            findings.append(f"{rel}: adapter_id must be CHAINLINK_REFERENCE_PRICE")
        if data["class"] != "WebSocketFrame" or data["frame_kind"] != "binary":
            findings.append(f"{rel}: Chainlink fixture sidecar must declare WebSocketFrame/binary")
        if data["signature_verified"] is not False:
            findings.append(f"{rel}: signature_verified must be false")

        fixture = sidecar.parent / str(data["fixture"])
        artifact = sidecar.parent / str(data["capture_artifact"])
        try:
            fixture_digest = ci_provenance.sha256_file(fixture)
        except ci_provenance.ProvenanceError as exc:
            findings.append(f"{rel}: {exc}")
            continue
        if fixture_digest != data["fixture_sha256"]:
            findings.append(f"{rel}: fixture_sha256 does not match fixture bytes")
        if not artifact.exists():
            findings.append(f"{rel}: capture_artifact is missing")
            continue
        try:
            record = ci_provenance.artifact_record_from_zip(artifact.read_bytes())
            sidecar_config = dataclasses.replace(
                config,
                deploy_source_branch=str(data["capture_head_branch"]),
            )
            ci_provenance.validate_exact_sha_record(
                record,
                sidecar_config,
                requested_sha=str(data["capture_head_sha"]),
                config_path=config_path,
                expected_workflow_digest=ci_provenance.require_record_digest(
                    record, "workflow_digest"
                ),
            )
        except ci_provenance.ProvenanceError as exc:
            findings.append(f"{rel}: invalid capture artifact provenance: {exc}")
            continue
        run = {
            "id": record.get("run_id"),
            "path": record.get("workflow_path"),
            "event": record.get("event"),
            "head_branch": record.get("head_branch"),
            "head_sha": record.get("head_sha"),
            "status": "completed",
            "conclusion": "success",
        }
        if not ci_provenance.run_matches_exact_sha(
            run,
            dataclasses.replace(config, deploy_source_branch=str(data["capture_head_branch"])),
            str(data["capture_head_sha"]),
            current_run_id=None,
        ):
            findings.append(f"{rel}: capture run metadata does not match exact SHA")
        capture = record.get("capture")
        if not isinstance(capture, dict):
            findings.append(f"{rel}: capture record is missing capture object")
            continue
        if capture.get("fixture_sha256") != fixture_digest:
            findings.append(f"{rel}: capture artifact digest does not match fixture")
        if capture.get("frame_kind") != data["frame_kind"]:
            findings.append(f"{rel}: capture artifact frame_kind does not match sidecar")
        if capture.get("signature_verified") is not False:
            findings.append(f"{rel}: capture artifact must not claim signature verification")


def scan_static_wiring(root: Path, findings: list[str]) -> None:
    lane_config = tomllib.loads(read(root, "ci/rust-verification.toml"))
    labels = lane_config["local_lane_policy"]["cheap_lane_labels"]
    for label in (
        "test_verify_bolt_v3_boundary_evidence.py",
        "verify_bolt_v3_boundary_evidence.py",
    ):
        if label not in labels:
            findings.append(f"ci/rust-verification.toml: cheap_lane_labels missing {label}")

    workflow_path = ".github/workflows/ci.yml"
    workflow = read(root, workflow_path)
    if "schedule:" in workflow:
        findings.append(f"{workflow_path}: recurring schedule is out of scope")
    if '--check-suite-id "${{ github.run_id }}"' in workflow:
        findings.append(
            f"{workflow_path}: capture provenance must use workflow run check_suite_id, not github.run_id"
        )
    for needle in (
        "capture_reference_boundary_fixture",
        "credential_ssm_gate",
        "CREDENTIAL-SSM",
        "capture-gate",
        "ops capture-reference-boundary-fixture",
        "--root-config",
        "GH_TOKEN: ${{ github.token }}",
        'gh api "repos/${{ github.repository }}/actions/runs/${{ github.run_id }}" --jq \'.check_suite_id\'',
        'echo "check_suite_id=$check_suite_id"',
        '--check-suite-id "${{ steps.provenance.outputs.check_suite_id }}"',
        "GITHUB_TOKEN: ${{ github.token }}",
        "GITHUB_REPOSITORY: ${{ github.repository }}",
        "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
    ):
        if needle not in workflow:
            findings.append(f"{workflow_path}: missing {needle}")


DEPENDENCY_SCOPES = ("dependencies", "dev-dependencies", "build-dependencies")
TEST_VISIBLE_DEPENDENCY_SCOPE_KEYS = frozenset({"dependencies", "dev-dependencies"})
BINANCE_TIMESTAMP_CRATE_PACKAGE = "nautilus-binance"
BINANCE_TIMESTAMP_CRATE_EXTERN = "nautilus_binance"
NT_PACKAGE_IDENTITY_PATTERN = re.compile(
    r"(?<![A-Za-z0-9_-])nautilus-[A-Za-z0-9_-]+(?=$|[:@#/?])"
)


@dataclasses.dataclass(frozen=True)
class CargoDependencyEntry:
    location: str
    scope: tuple[str, ...]
    exposed_key: str
    extern_name: str
    package_name: str
    specification: object


def dependency_tables(
    manifest: object,
) -> list[tuple[tuple[str, ...], dict[str, object]]]:
    if not isinstance(manifest, dict):
        return []
    tables: list[tuple[tuple[str, ...], dict[str, object]]] = []
    for scope in DEPENDENCY_SCOPES:
        table = manifest.get(scope)
        if isinstance(table, dict):
            tables.append(((scope,), table))

    workspace = manifest.get("workspace")
    if isinstance(workspace, dict):
        table = workspace.get("dependencies")
        if isinstance(table, dict):
            tables.append((("workspace", "dependencies"), table))

    target = manifest.get("target")
    if isinstance(target, dict):
        for selector, target_configuration in target.items():
            if not isinstance(target_configuration, dict):
                continue
            for scope in DEPENDENCY_SCOPES:
                table = target_configuration.get(scope)
                if isinstance(table, dict):
                    tables.append((("target", str(selector), scope), table))
    return tables


def cargo_dependency_entries(manifest: object) -> list[CargoDependencyEntry]:
    entries: list[CargoDependencyEntry] = []
    for path, table in dependency_tables(manifest):
        for raw_name, specification in table.items():
            exposed_key = str(raw_name)
            package_name = exposed_key
            if isinstance(specification, dict) and isinstance(
                specification.get("package"), str
            ):
                package_name = specification["package"]
            entries.append(
                CargoDependencyEntry(
                    location=f"{'.'.join(path)}.{exposed_key}",
                    scope=path,
                    exposed_key=exposed_key,
                    extern_name=exposed_key.replace("-", "_"),
                    package_name=package_name,
                    specification=specification,
                )
            )
    return entries


def nt_manifest_dependencies(manifest: object) -> list[tuple[str, object]]:
    dependencies: list[tuple[str, object]] = []
    for entry in cargo_dependency_entries(manifest):
        git = (
            entry.specification.get("git")
            if isinstance(entry.specification, dict)
            else None
        )
        if (
            entry.package_name.startswith("nautilus-")
            or (isinstance(git, str) and "nautilus_trader.git" in git)
        ):
            dependencies.append((entry.location, entry.specification))
    return dependencies


def dependency_scope_is_visible_to_integration_tests(scope: tuple[str, ...]) -> bool:
    if scope in (("dependencies",), ("dev-dependencies",)):
        return True
    return (
        len(scope) == 3
        and scope[0] == "target"
        and scope[-1] in TEST_VISIBLE_DEPENDENCY_SCOPE_KEYS
    )


def binance_timestamp_dependency_identity_errors(manifest: object) -> list[str]:
    entries = [
        entry
        for entry in cargo_dependency_entries(manifest)
        if dependency_scope_is_visible_to_integration_tests(entry.scope)
    ]
    errors: list[str] = []
    canonical_entries = [
        entry
        for entry in entries
        if entry.extern_name == BINANCE_TIMESTAMP_CRATE_EXTERN
    ]
    if not canonical_entries:
        return [
            f"missing canonical {BINANCE_TIMESTAMP_CRATE_PACKAGE!r} dependency exposed as "
            f"{BINANCE_TIMESTAMP_CRATE_EXTERN!r}"
        ]

    for entry in canonical_entries:
        specification = entry.specification
        canonical_source = (
            isinstance(specification, dict)
            and specification.get("git") == EXPECTED_NT_GIT
            and isinstance(specification.get("rev"), str)
            and re.fullmatch(r"[0-9a-f]{40}", specification["rev"]) is not None
            and not any(
                key in specification
                for key in (
                    "branch",
                    "tag",
                    "path",
                    "version",
                    "registry",
                    "workspace",
                )
            )
        )
        if (
            entry.exposed_key != BINANCE_TIMESTAMP_CRATE_PACKAGE
            or entry.package_name != BINANCE_TIMESTAMP_CRATE_PACKAGE
            or not canonical_source
        ):
            errors.append(
                f"{entry.location} exposes {entry.extern_name!r} from package "
                f"{entry.package_name!r}; it must be the canonical "
                f"{BINANCE_TIMESTAMP_CRATE_PACKAGE!r} Git dependency"
            )

    for entry in entries:
        if (
            entry.package_name == BINANCE_TIMESTAMP_CRATE_PACKAGE
            and entry.exposed_key != BINANCE_TIMESTAMP_CRATE_PACKAGE
        ):
            errors.append(
                f"{entry.location} renames {BINANCE_TIMESTAMP_CRATE_PACKAGE!r}; the proof "
                "dependency must retain its canonical exposed key"
            )
    return errors


def cargo_identity_references_nt(value: object) -> bool:
    return isinstance(value, str) and (
        NT_PACKAGE_IDENTITY_PATTERN.search(value) is not None
        or "nautilus_trader.git" in value
    )


def cargo_specification_references_nt(specification: object) -> bool:
    if not isinstance(specification, dict):
        return False
    return any(
        cargo_identity_references_nt(specification.get(key))
        for key in ("package", "git")
    )


def nt_manifest_overrides(manifest: object) -> list[str]:
    if not isinstance(manifest, dict):
        return []
    overrides: list[str] = []

    patch = manifest.get("patch")
    if isinstance(patch, dict):
        for source, entries in patch.items():
            source_is_nt = cargo_identity_references_nt(source)
            if not isinstance(entries, dict):
                if source_is_nt:
                    overrides.append(f"patch.{source}")
                continue
            for package_id, specification in entries.items():
                if (
                    source_is_nt
                    or cargo_identity_references_nt(package_id)
                    or cargo_specification_references_nt(specification)
                ):
                    overrides.append(f"patch.{source}.{package_id}")

    replace = manifest.get("replace")
    if isinstance(replace, dict):
        for package_id, specification in replace.items():
            if cargo_identity_references_nt(
                package_id
            ) or cargo_specification_references_nt(specification):
                overrides.append(f"replace.{package_id}")

    return overrides


def cargo_surface_references_nt(surface: Path, text: str) -> bool:
    try:
        document = tomllib.loads(text)
    except tomllib.TOMLDecodeError:
        return "nautilus-" in text or "nautilus_trader.git" in text
    if surface.name == "Cargo.toml":
        return bool(nt_manifest_dependencies(document) or nt_manifest_overrides(document))
    packages = document.get("package")
    if not isinstance(packages, list):
        return False
    return any(
        isinstance(package, dict)
        and (
            (
                isinstance(package.get("name"), str)
                and package["name"].startswith("nautilus-")
            )
            or (
                isinstance(package.get("source"), str)
                and "nautilus_trader.git" in package["source"]
            )
        )
        for package in packages
    )


def tracked_cargo_surfaces(root: Path, findings: list[str]) -> set[Path]:
    result = subprocess.run(
        ["git", "-C", str(root), "ls-files", "--cached", "-z"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        findings.append(
            "NautilusTrader pin census could not discover tracked Cargo surfaces: "
            f"{detail or f'git ls-files exited {result.returncode}'}"
        )
        return set()

    surfaces: set[Path] = set()
    for raw_path in result.stdout.split(b"\0"):
        if not raw_path:
            continue
        decoded = os.fsdecode(raw_path)
        surface = Path(decoded)
        if (
            surface.is_absolute()
            or ".." in surface.parts
            or "target" in surface.parts
            or surface.name not in {"Cargo.toml", "Cargo.lock"}
        ):
            continue
        surfaces.add(surface)
    return surfaces


def scan_nt_manifest_pin(
    surface: Path,
    text: str,
    findings: list[str],
    expected_revision: str,
    *,
    allow_workspace_inheritance: bool = False,
) -> None:
    try:
        manifest = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        findings.append(f"{surface}: NautilusTrader pin census could not parse TOML: {error}")
        return

    dependencies = nt_manifest_dependencies(manifest)
    for location in nt_manifest_overrides(manifest):
        findings.append(
            f"{surface}: NautilusTrader pin census forbids NT-relevant Cargo override "
            f"{location}; dependencies must use the single canonical pinned Git path"
        )
    if not dependencies:
        findings.append(f"{surface}: NautilusTrader pin census found no nautilus-* dependencies")
        return

    for location, specification in dependencies:
        if not isinstance(specification, dict):
            findings.append(
                f"{surface}: NautilusTrader pin census {location} must use the canonical pinned Git table"
            )
            continue
        if specification.get("workspace") is True:
            if not allow_workspace_inheritance:
                findings.append(
                    f"{surface}: NautilusTrader pin census {location} must own the canonical "
                    "pinned Git table rather than inherit it"
                )
                continue
            source_selectors = sorted(
                key
                for key in ("git", "rev", "branch", "tag")
                if key in specification
            )
            if source_selectors:
                findings.append(
                    f"{surface}: NautilusTrader pin census {location} workspace inheritance "
                    f"must not also set source selector(s): {', '.join(source_selectors)}"
                )
            continue
        git = specification.get("git")
        rev = specification.get("rev")
        selectors = sorted(key for key in ("branch", "tag") if key in specification)
        if git != EXPECTED_NT_GIT or rev != expected_revision or selectors:
            findings.append(
                f"{surface}: NautilusTrader pin census {location} must use git={EXPECTED_NT_GIT!r} "
                f"and rev={expected_revision!r} with no branch/tag selector"
            )


def scan_nt_lock_pin(
    surface: Path,
    text: str,
    findings: list[str],
    expected_revision: str,
) -> None:
    try:
        lock = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        findings.append(f"{surface}: NautilusTrader pin census could not parse TOML: {error}")
        return

    packages = lock.get("package")
    if not isinstance(packages, list):
        findings.append(f"{surface}: NautilusTrader pin census found no package records")
        return
    nautilus_packages = [
        package
        for package in packages
        if isinstance(package, dict)
        and isinstance(package.get("name"), str)
        and package["name"].startswith("nautilus-")
    ]
    if not nautilus_packages:
        findings.append(f"{surface}: NautilusTrader pin census found no nautilus-* packages")
        return
    expected_source = (
        f"git+{EXPECTED_NT_GIT}?rev={expected_revision}#{expected_revision}"
    )
    for package in nautilus_packages:
        if package.get("source") != expected_source:
            findings.append(
                f"{surface}: NautilusTrader pin census package {package['name']} must use "
                f"source={expected_source!r}"
            )


def root_manifest_nt_revision(text: str, findings: list[str]) -> str | None:
    try:
        manifest = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        findings.append(
            "Cargo.toml: NautilusTrader pin census could not derive the canonical "
            f"revision because TOML parsing failed: {error}"
        )
        return None

    revisions = {
        specification.get("rev")
        for _, specification in nt_manifest_dependencies(manifest)
        if isinstance(specification, dict)
        and isinstance(specification.get("rev"), str)
        and re.fullmatch(r"[0-9a-f]{40}", specification["rev"]) is not None
    }
    if len(revisions) != 1:
        findings.append(
            "Cargo.toml: NautilusTrader pin census canonical root manifest must declare "
            "exactly one shared immutable 40-character Git revision"
        )
        return None
    return revisions.pop()


def markdown_section(text: str, heading: str) -> str | None:
    lines = text.splitlines()
    indexes = [index for index, line in enumerate(lines) if line == heading]
    if len(indexes) != 1:
        return None
    start = indexes[0]
    level = len(heading) - len(heading.lstrip("#"))
    end = len(lines)
    for index in range(start + 1, len(lines)):
        match = re.match(r"^(#+)\s", lines[index])
        if match is not None and len(match.group(1)) <= level:
            end = index
            break
    return "\n".join(lines[start + 1 : end])


def rust_open_delimiters_at(masked: str, end: int) -> tuple[str, ...] | None:
    openers = {"(", "[", "{"}
    closer_to_opener = {")": "(", "]": "[", "}": "{"}
    delimiter_stack: list[str] = []
    for char in masked[:end]:
        if char in openers:
            delimiter_stack.append(char)
        elif char in closer_to_opener:
            if (
                not delimiter_stack
                or delimiter_stack[-1] != closer_to_opener[char]
            ):
                return None
            delimiter_stack.pop()
    return tuple(delimiter_stack)


def rust_crate_inner_attributes(text: str) -> list[str]:
    masked = _mask_rust_non_code(text)
    attributes: list[str] = []
    for match in re.finditer(r"#\s*!\s*\[\s*([^\[\]]+?)\s*\]", masked):
        if rust_open_delimiters_at(masked, match.start()) == ():
            attributes.append(re.sub(r"\s+", "", match.group(1)))
    return attributes


def rust_ordinary_test_function_body(
    text: str, function_name: str
) -> tuple[str | None, str | None, bool]:
    masked = _mask_rust_non_code(text)
    function_header = re.compile(
        rf"\bfn\s+{re.escape(function_name)}\s*\([^)]*\)\s*\{{"
    )
    matches = list(function_header.finditer(masked))
    if len(matches) != 1:
        return None, None, False

    if rust_open_delimiters_at(masked, matches[0].start()) != ():
        return None, None, True

    attribute_cluster = re.search(
        r"(?P<attributes>(?:#\s*\[[^\[\]]+\]\s*)+)$",
        masked[: matches[0].start()],
    )
    if attribute_cluster is None:
        return None, None, True
    attributes = [
        re.sub(r"\s+", "", attribute)
        for attribute in re.findall(
            r"#\s*\[\s*([^\[\]]+?)\s*\]",
            attribute_cluster.group("attributes"),
        )
    ]
    if attributes != ["test"]:
        return None, None, True

    opening_brace = masked.find("{", matches[0].start(), matches[0].end())
    depth = 0
    for index in range(opening_brace, len(masked)):
        char = masked[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return (
                    masked[opening_brace + 1 : index],
                    text[opening_brace + 1 : index],
                    True,
                )
    return None, None, True


def rust_top_level_block_span(
    masked: str,
    header_pattern: re.Pattern[str],
) -> tuple[int, int] | None:
    matches = [
        match
        for match in header_pattern.finditer(masked)
        if rust_open_delimiters_at(masked, match.start()) == ()
    ]
    if len(matches) != 1:
        return None
    opening = masked.find("{", matches[0].start(), matches[0].end())
    if opening < 0:
        return None
    depth = 0
    for index in range(opening, len(masked)):
        char = masked[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return opening + 1, index
    return None


def binance_trade_loop_span(body: str) -> tuple[int, int] | None:
    return rust_top_level_block_span(
        body,
        re.compile(r"\bfor\s+data\s+in\s+trades\s*\{"),
    )


def binance_test_has_early_exit_bypass(body: str) -> bool:
    if re.search(r"#\s*!?\s*\[", body) is not None:
        return True
    tokenized = rust_tokens_and_delimiter_pairs(body)
    if tokenized is None:
        return True
    tokens, _ = tokenized
    for index, token in enumerate(tokens):
        identifier = token.value[2:] if token.value.startswith("r#") else token.value
        if identifier in {"return", "break", "continue"} or token.value == "?":
            return True
        if identifier == "exit" and index + 1 < len(tokens) and tokens[index + 1].value == "(":
            return True
    return False


def binance_parser_identity_is_shadowed(masked: str) -> bool:
    import_matches = [
        match
        for match in BINANCE_TIMESTAMP_PARSER_IMPORT_PATTERN.finditer(masked)
        if rust_open_delimiters_at(masked, match.start()) == ()
    ]
    if len(import_matches) != 1:
        return True

    allowed_alias_spans = {
        (import_matches[0].start("alias"), import_matches[0].end("alias"))
    }
    allowed_symbol_spans: set[tuple[int, int]] = set()
    qualified_parser = re.compile(
        rf"\b(?P<alias>{re.escape(BINANCE_TIMESTAMP_PARSER_ALIAS)})\s*::\s*"
        rf"(?P<symbol>{'|'.join(map(re.escape, BINANCE_TIMESTAMP_PARSER_SYMBOLS))})\b"
    )
    for match in qualified_parser.finditer(masked):
        allowed_alias_spans.add((match.start("alias"), match.end("alias")))
        allowed_symbol_spans.add((match.start("symbol"), match.end("symbol")))

    if any(
        (match.start(), match.end()) not in allowed_alias_spans
        for match in re.finditer(
            rf"\b{re.escape(BINANCE_TIMESTAMP_PARSER_ALIAS)}\b", masked
        )
    ):
        return True

    for symbol in BINANCE_TIMESTAMP_PARSER_SYMBOLS:
        if any(
            (match.start(), match.end()) not in allowed_symbol_spans
            for match in re.finditer(rf"\b{re.escape(symbol)}\b", masked)
        ):
            return True
    return False


@dataclasses.dataclass(frozen=True)
class RustToken:
    value: str
    start: int
    end: int


RUST_TOKEN_PATTERN = re.compile(
    r"""
    r\#[A-Za-z_][A-Za-z0-9_]*
    |[A-Za-z_][A-Za-z0-9_]*
    |::|=>|->|<<=|>>=|\.\.=|\.\.\.|==|!=|<=|>=|&&|\|\|
    |\+=|-=|\*=|/=|%=|&=|\|=|\^=|<<|>>|\.\.
    |\d(?:[A-Za-z0-9_]|\.(?!\.))*
    |\S
    """,
    re.VERBOSE,
)
RUST_OPEN_TO_CLOSE = {"(": ")", "[": "]", "{": "}"}
RUST_CLOSE_TO_OPEN = {value: key for key, value in RUST_OPEN_TO_CLOSE.items()}
GOVERNED_ASSERTION_MACROS = frozenset({"assert", "assert_eq", "assert_ne"})
INERT_BUILTIN_LINT_ATTRIBUTES = frozenset({"allow", "warn", "deny", "forbid"})


def rust_tokens_and_delimiter_pairs(
    masked: str,
) -> tuple[list[RustToken], dict[int, int]] | None:
    tokens = [
        RustToken(match.group(0), match.start(), match.end())
        for match in RUST_TOKEN_PATTERN.finditer(masked)
    ]
    stack: list[tuple[int, str]] = []
    pairs: dict[int, int] = {}
    for index, token in enumerate(tokens):
        if token.value in RUST_OPEN_TO_CLOSE:
            stack.append((index, token.value))
        elif token.value in RUST_CLOSE_TO_OPEN:
            if not stack or stack[-1][1] != RUST_CLOSE_TO_OPEN[token.value]:
                return None
            opening_index, _ = stack.pop()
            pairs[opening_index] = index
            pairs[index] = opening_index
    if stack:
        return None
    return tokens, pairs


def binance_crate_root_attribute_is_inert(
    tokens: list[RustToken],
    pairs: dict[int, int],
    opening: int,
    closing: int,
) -> bool:
    contents = [token.value for token in tokens[opening + 1 : closing]]
    if contents == ["test"]:
        return True
    attribute_name = tokens[opening + 1].value if opening + 1 < closing else None
    arguments = opening + 2
    return (
        attribute_name in INERT_BUILTIN_LINT_ATTRIBUTES
        and arguments < closing
        and tokens[arguments].value == "("
        and pairs.get(arguments) == closing - 1
        and arguments + 1 < closing - 1
    )


def binance_crate_root_identity_substitution_is_possible(masked: str) -> bool:
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        return True
    tokens, pairs = tokenized
    index = 0
    while index < len(tokens):
        value = tokens[index].value
        if value == "#":
            if index + 1 >= len(tokens) or tokens[index + 1].value != "[":
                return True
            closing = pairs.get(index + 1)
            if closing is None or not binance_crate_root_attribute_is_inert(
                tokens, pairs, index + 1, closing
            ):
                return True
            index = closing + 1
            continue
        if value in RUST_OPEN_TO_CLOSE:
            closing = pairs.get(index)
            if closing is None:
                return True
            index = closing + 1
            continue
        identifier = value[2:] if value.startswith("r#") else value
        if (
            identifier == "extern"
            and index + 1 < len(tokens)
            and tokens[index + 1].value == "crate"
        ):
            return True
        if value == "!" and index > 0:
            predecessor = tokens[index - 1].value
            if re.fullmatch(r"(?:r#)?[A-Za-z_][A-Za-z0-9_]*", predecessor):
                return True
        index += 1
    return False


def binance_assertion_contract_is_canonical(masked: str) -> bool:
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        return False
    tokens, _ = tokenized
    for index, token in enumerate(tokens):
        identifier = token.value[2:] if token.value.startswith("r#") else token.value
        if identifier not in GOVERNED_ASSERTION_MACROS:
            continue
        if index < 3 or index + 1 >= len(tokens):
            return False
        if [item.value for item in tokens[index - 3 : index + 2]] != [
            "::",
            "core",
            "::",
            identifier,
            "!",
        ]:
            return False
    return True


def rust_assertion_is_complete_expression_statement(
    masked: str, assertion_start: int
) -> bool:
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        return False
    tokens, pairs = tokenized
    assertion_index = next(
        (
            index
            for index, token in enumerate(tokens)
            if token.start == assertion_start
        ),
        None,
    )
    if assertion_index is None or assertion_index + 5 >= len(tokens):
        return False

    macro_name = tokens[assertion_index + 3].value
    if (
        macro_name not in GOVERNED_ASSERTION_MACROS
        or [token.value for token in tokens[assertion_index : assertion_index + 6]]
        != ["::", "core", "::", macro_name, "!", "("]
    ):
        return False

    closing_parenthesis = pairs.get(assertion_index + 5)
    if closing_parenthesis is None or closing_parenthesis + 1 >= len(tokens):
        return False
    if tokens[closing_parenthesis + 1].value != ";":
        return False

    predecessor = (
        tokens[assertion_index - 1].value if assertion_index > 0 else None
    )
    return predecessor in {None, ";", "{"}


def rust_find_top_level_token(
    tokens: list[RustToken],
    pairs: dict[int, int],
    start: int,
    end: int,
    values: set[str],
) -> int | None:
    index = start
    while index < end:
        value = tokens[index].value
        if value in values:
            return index
        if value in RUST_OPEN_TO_CLOSE:
            closing_index = pairs.get(index)
            if closing_index is None or closing_index >= end:
                return None
            index = closing_index + 1
            continue
        if value in RUST_CLOSE_TO_OPEN:
            return None
        index += 1
    return None


def rust_split_top_level_patterns(
    tokens: list[RustToken],
    pairs: dict[int, int],
    start: int,
    end: int,
) -> list[list[RustToken]] | None:
    patterns: list[list[RustToken]] = []
    segment_start = start
    index = start
    while index < end:
        value = tokens[index].value
        if value in RUST_OPEN_TO_CLOSE:
            closing_index = pairs.get(index)
            if closing_index is None or closing_index >= end:
                return None
            index = closing_index + 1
            continue
        if value in RUST_CLOSE_TO_OPEN:
            return None
        if value == ",":
            patterns.append(tokens[segment_start:index])
            segment_start = index + 1
        index += 1
    patterns.append(tokens[segment_start:end])
    return patterns


def rust_pattern_without_top_level_type(
    tokens: list[RustToken],
    pairs: dict[int, int],
    start: int,
    end: int,
) -> list[RustToken] | None:
    colon = rust_find_top_level_token(tokens, pairs, start, end, {":"})
    pattern_end = colon if colon is not None else end
    pattern = tokens[start:pattern_end]
    return pattern if pattern else None


def rust_binding_patterns(masked: str) -> tuple[list[list[RustToken]], bool]:
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        return [], False
    tokens, pairs = tokenized
    patterns: list[list[RustToken]] = []

    for index, token in enumerate(tokens):
        if token.value == "let":
            terminator = rust_find_top_level_token(
                tokens, pairs, index + 1, len(tokens), {"=", ";"}
            )
            if terminator is None:
                return [], False
            pattern = rust_pattern_without_top_level_type(
                tokens, pairs, index + 1, terminator
            )
            if pattern is None:
                return [], False
            patterns.append(pattern)
        elif token.value == "for":
            terminator = rust_find_top_level_token(
                tokens, pairs, index + 1, len(tokens), {"in"}
            )
            if terminator is None or terminator == index + 1:
                return [], False
            patterns.append(tokens[index + 1 : terminator])
        elif token.value == "fn":
            opening = rust_find_top_level_token(
                tokens, pairs, index + 1, len(tokens), {"(", "{", ";"}
            )
            if opening is None or tokens[opening].value != "(":
                return [], False
            closing = pairs.get(opening)
            if closing is None:
                return [], False
            parameters = rust_split_top_level_patterns(
                tokens, pairs, opening + 1, closing
            )
            if parameters is None:
                return [], False
            parameter_start = opening + 1
            for parameter in parameters:
                parameter_end = parameter_start + len(parameter)
                pattern = rust_pattern_without_top_level_type(
                    tokens, pairs, parameter_start, parameter_end
                )
                if pattern:
                    patterns.append(pattern)
                parameter_start = parameter_end + 1

    closure_closers: set[int] = set()
    closure_opening_predecessors = {
        "(",
        "[",
        "{",
        ",",
        "=",
        "=>",
        ";",
        ":",
        "move",
        "async",
        "return",
    }
    for index, token in enumerate(tokens):
        if token.value != "|" or index in closure_closers:
            continue
        predecessor = tokens[index - 1].value if index > 0 else None
        if predecessor not in closure_opening_predecessors:
            continue
        closing = rust_find_top_level_token(
            tokens, pairs, index + 1, len(tokens), {"|"}
        )
        if closing is None:
            return [], False
        closure_closers.add(closing)
        parameters = rust_split_top_level_patterns(
            tokens, pairs, index + 1, closing
        )
        if parameters is None:
            return [], False
        parameter_start = index + 1
        for parameter in parameters:
            parameter_end = parameter_start + len(parameter)
            pattern = rust_pattern_without_top_level_type(
                tokens, pairs, parameter_start, parameter_end
            )
            if pattern:
                patterns.append(pattern)
            parameter_start = parameter_end + 1

    for arrow, token in enumerate(tokens):
        if token.value != "=>":
            continue
        containing_openers = [
            opening
            for opening, closing in pairs.items()
            if tokens[opening].value in RUST_OPEN_TO_CLOSE
            and opening < arrow < closing
        ]
        if not containing_openers:
            return [], False
        arm_opening = max(containing_openers)
        if tokens[arm_opening].value != "{":
            return [], False
        pattern_start = arm_opening + 1
        scan = pattern_start
        while scan < arrow:
            value = tokens[scan].value
            if value in RUST_OPEN_TO_CLOSE:
                closing = pairs.get(scan)
                if closing is None or closing >= arrow:
                    return [], False
                scan = closing + 1
                continue
            if value in RUST_CLOSE_TO_OPEN:
                return [], False
            if value == ",":
                pattern_start = scan + 1
            scan += 1
        guard = rust_find_top_level_token(
            tokens, pairs, pattern_start, arrow, {"if"}
        )
        pattern_end = guard if guard is not None else arrow
        if pattern_start >= pattern_end:
            return [], False
        patterns.append(tokens[pattern_start:pattern_end])

    return patterns, True


@dataclasses.dataclass(frozen=True)
class RustTopLevelLet:
    pattern: tuple[RustToken, ...]
    rhs: tuple[RustToken, ...]
    start: int
    end: int


def rust_top_level_let_statements(
    masked: str,
) -> tuple[list[RustTopLevelLet], bool]:
    tokenized = rust_tokens_and_delimiter_pairs(masked)
    if tokenized is None:
        return [], False
    tokens, pairs = tokenized
    statements: list[RustTopLevelLet] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token.value in RUST_OPEN_TO_CLOSE:
            closing = pairs.get(index)
            if closing is None:
                return [], False
            index = closing + 1
            continue
        if token.value != "let":
            index += 1
            continue
        equals = rust_find_top_level_token(
            tokens, pairs, index + 1, len(tokens), {"=", ";"}
        )
        if equals is None or tokens[equals].value != "=":
            return [], False
        terminator = rust_find_top_level_token(
            tokens, pairs, equals + 1, len(tokens), {";"}
        )
        if terminator is None:
            return [], False
        pattern = rust_pattern_without_top_level_type(
            tokens, pairs, index + 1, equals
        )
        if pattern is None or equals + 1 == terminator:
            return [], False
        statements.append(
            RustTopLevelLet(
                pattern=tuple(pattern),
                rhs=tuple(tokens[equals + 1 : terminator]),
                start=token.start,
                end=tokens[terminator].end,
            )
        )
        index = terminator + 1
    return statements, True


def rust_pattern_binds_identifier(
    pattern: list[RustToken], identifier: str
) -> bool:
    for index, token in enumerate(pattern):
        token_identifier = (
            token.value[2:] if token.value.startswith("r#") else token.value
        )
        if token_identifier != identifier:
            continue
        predecessor = pattern[index - 1].value if index > 0 else None
        successor = pattern[index + 1].value if index + 1 < len(pattern) else None
        if predecessor == "::" or successor == "::":
            continue
        if successor == ":":
            continue
        return True
    return False


def rust_token_values(tokens: tuple[RustToken, ...] | list[RustToken]) -> list[str]:
    return [token.value for token in tokens]


def rust_event_rhs_uses_provider_scalar(
    rhs: tuple[RustToken, ...],
    event_type: str,
    scalar: str,
) -> bool:
    values = rust_token_values(rhs)
    if len(values) < 3 or values[0:2] != [event_type, "{"]:
        return False
    tokenized = rust_tokens_and_delimiter_pairs(" ".join(values))
    if tokenized is None:
        return False
    rebuilt, pairs = tokenized
    if len(rebuilt) != len(rhs) or pairs.get(1) != len(rebuilt) - 1:
        return False
    fields = rust_split_top_level_patterns(rebuilt, pairs, 2, len(rebuilt) - 1)
    if fields is None:
        return False
    return any(rust_token_values(field) == [scalar] for field in fields)


def has_governed_expected_event_contract(
    body: str,
    scalar: str,
    event_type: str,
    parser_symbol: str,
) -> bool:
    statements, statements_are_valid = rust_top_level_let_statements(body)
    binding_patterns, patterns_are_valid = rust_binding_patterns(body)
    if not statements_are_valid or not patterns_are_valid:
        return False

    governed_names = (scalar, "expected_ts_event", "event")
    governed_bindings = {
        name: [
            pattern
            for pattern in binding_patterns
            if rust_pattern_binds_identifier(pattern, name)
        ]
        for name in governed_names
    }
    if any(len(bindings) != 1 for bindings in governed_bindings.values()):
        return False

    top_level = {
        name: [
            statement
            for statement in statements
            if rust_token_values(statement.pattern) == [name]
        ]
        for name in governed_names
    }
    if any(len(items) != 1 for items in top_level.values()):
        return False

    scalar_statement = top_level[scalar][0]
    expected_statement = top_level["expected_ts_event"][0]
    event_statement = top_level["event"][0]
    scalar_rhs = rust_token_values(scalar_statement.rhs)
    expected_rhs = rust_token_values(expected_statement.rhs)
    if len(scalar_rhs) != 1 or re.fullmatch(r"[0-9][0-9_]*_i64", scalar_rhs[0]) is None:
        return False
    if expected_rhs != [
        "UnixNanos",
        "::",
        "from_micros",
        "(",
        scalar,
        "as",
        "u64",
        ")",
    ]:
        return False
    if not rust_event_rhs_uses_provider_scalar(
        event_statement.rhs,
        event_type,
        scalar,
    ):
        return False

    parser_call = re.compile(
        rf"\blet\s+[A-Za-z_][A-Za-z0-9_]*\s*=\s*"
        rf"{re.escape(BINANCE_TIMESTAMP_PARSER_ALIAS)}\s*::\s*"
        rf"{re.escape(parser_symbol)}\s*\(\s*&\s*event\s*,\s*&\s*instrument\s*,\s*"
        r"adapter_ts_init\s*\)"
    )
    parser_calls = [
        match
        for match in parser_call.finditer(body)
        if rust_open_delimiters_at(body, match.start()) == ()
    ]
    if len(parser_calls) != 1:
        return False
    parser_start = parser_calls[0].start()
    if not (
        scalar_statement.start
        < expected_statement.start
        < event_statement.start
        < parser_start
    ):
        return False

    for name in governed_names:
        assignments = re.findall(
            rf"(?<![A-Za-z0-9_])(?:r#)?{re.escape(name)}\b\s*"
            r"(?:<<=|>>=|\+=|-=|\*=|/=|%=|&=|\|=|\^=|=(?!=))",
            body,
        )
        if len(assignments) != 1:
            return False
    return True


def has_governed_binance_parser_result_contract(
    body: str,
    original_body: str,
    parser_symbol: str,
    result_variable: str,
) -> bool:
    statements, statements_are_valid = rust_top_level_let_statements(body)
    result_statements = [
        statement
        for statement in statements
        if rust_token_values(statement.pattern) == [result_variable]
    ]
    if not statements_are_valid or len(result_statements) != 1:
        return False

    result_statement = result_statements[0]
    if not result_statement.rhs:
        return False
    binding_tokens = rust_tokens_and_delimiter_pairs(
        body[result_statement.start : result_statement.rhs[0].start]
    )
    if binding_tokens is None or rust_token_values(binding_tokens[0]) != [
        "let",
        result_variable,
        "=",
    ]:
        return False

    expected_rhs = [
        BINANCE_TIMESTAMP_PARSER_ALIAS,
        "::",
        parser_symbol,
        "(",
        "&",
        "event",
        ",",
        "&",
        "instrument",
        ",",
        "adapter_ts_init",
        ")",
    ]
    expect_message = BINANCE_TIMESTAMP_DEPTH_EXPECT_MESSAGES.get(parser_symbol)
    if expect_message is not None:
        expected_rhs.extend([".", "expect", "(", ")"])
    if rust_token_values(result_statement.rhs) != expected_rhs:
        return False
    if expect_message is not None:
        expect_argument = original_body[
            result_statement.rhs[-2].end : result_statement.rhs[-1].start
        ].strip()
        if expect_argument != f'"{expect_message}"':
            return False

    binding_patterns, patterns_are_valid = rust_binding_patterns(body)
    result_bindings = [
        pattern
        for pattern in binding_patterns
        if rust_pattern_binds_identifier(pattern, result_variable)
    ]
    assignments = list(
        re.finditer(
            rf"\b{re.escape(result_variable)}\b\s*"
            r"(?:<<=|>>=|\+=|-=|\*=|/=|%=|&=|\|=|\^=|=(?!=))",
            body,
        )
    )
    return (
        patterns_are_valid
        and len(result_bindings) == 1
        and len(assignments) == 1
    )


def scan_binance_timestamp_behavioral_contract(root: Path, findings: list[str]) -> None:
    manifest_surface = Path("Cargo.toml")
    try:
        manifest = tomllib.loads(read(root, manifest_surface))
    except (OSError, tomllib.TOMLDecodeError) as error:
        findings.append(
            f"{manifest_surface}: could not verify required Binance SBE timestamp "
            f"behavioral test target: {error}"
        )
        return

    for error in binance_timestamp_dependency_identity_errors(manifest):
        findings.append(
            f"{manifest_surface}: required Binance SBE timestamp proof dependency identity: "
            f"{error}"
        )

    test_entries = manifest.get("test", [])
    if not isinstance(test_entries, list):
        findings.append(
            f"{manifest_surface}: required [[test]] target {BINANCE_TIMESTAMP_TEST_TARGET} "
            "cannot be verified because Cargo test entries are not an array"
        )
        test_entries = []
    conflicting_entries = [
        entry
        for entry in test_entries
        if isinstance(entry, dict)
        and (
            entry.get("name") == BINANCE_TIMESTAMP_TEST_TARGET
            or entry.get("path") == BINANCE_TIMESTAMP_TEST_PATH.as_posix()
        )
    ]
    exact_entries = [
        entry
        for entry in conflicting_entries
        if entry.get("name") == BINANCE_TIMESTAMP_TEST_TARGET
        and entry.get("path") == BINANCE_TIMESTAMP_TEST_PATH.as_posix()
    ]
    if len(exact_entries) != 1 or len(conflicting_entries) != 1:
        findings.append(
            f"{manifest_surface}: required [[test]] target {BINANCE_TIMESTAMP_TEST_TARGET} "
            f"must register exactly {BINANCE_TIMESTAMP_TEST_PATH}"
        )
    if len(exact_entries) == 1:
        exact_entry = exact_entries[0]
        unsafe_fields = sorted(
            set(exact_entry) - BINANCE_TIMESTAMP_TEST_SAFE_TARGET_FIELDS
        )
        if unsafe_fields:
            findings.append(
                f"{manifest_surface}: required [[test]] target "
                f"{BINANCE_TIMESTAMP_TEST_TARGET} has execution-unsafe field(s): "
                f"{', '.join(unsafe_fields)}"
            )
        for field in ("harness", "test"):
            if field in exact_entry and exact_entry[field] is not True:
                findings.append(
                    f"{manifest_surface}: required [[test]] target "
                    f"{BINANCE_TIMESTAMP_TEST_TARGET} {field} must be true when specified"
                )

    test_path = root / BINANCE_TIMESTAMP_TEST_PATH
    if not test_path.is_file():
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: required Binance SBE timestamp behavioral "
            "proof file is missing"
        )
        return

    try:
        test_text = test_path.read_text(encoding="utf-8")
    except OSError as error:
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: could not read required Binance SBE timestamp "
            f"behavioral proof: {error}"
        )
        return

    for attribute in rust_crate_inner_attributes(test_text):
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: crate-level inner attribute is forbidden: "
            f"{attribute}"
        )

    masked_test_text = _mask_rust_non_code(test_text)
    if binance_crate_root_identity_substitution_is_possible(masked_test_text):
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: crate-root identity substitution is forbidden; "
            "the proof harness must not contain extern crate items or item-producing macros"
        )
    if not binance_assertion_contract_is_canonical(masked_test_text):
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: governed assertions must use canonical "
            "::core paths without local shadowing"
        )
    import_matches = [
        match
        for match in BINANCE_TIMESTAMP_PARSER_IMPORT_PATTERN.finditer(masked_test_text)
        if rust_open_delimiters_at(masked_test_text, match.start()) == ()
    ]
    if len(import_matches) != 1:
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: required pinned NautilusTrader parser import is missing"
        )
    if binance_parser_identity_is_shadowed(masked_test_text):
        findings.append(
            f"{BINANCE_TIMESTAMP_TEST_PATH}: governed NautilusTrader parser identity must not be shadowed"
        )

    for function_name, requirements in BINANCE_TIMESTAMP_TEST_CASE_REQUIREMENTS.items():
        body, original_body, has_named_function = rust_ordinary_test_function_body(
            test_text, function_name
        )
        if body is None:
            if has_named_function:
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} must use exactly "
                    "one ordinary #[test] outer attribute and no other function attributes"
                )
            else:
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: missing required #[test] function "
                    f"{function_name}"
                )
            continue
        if original_body is None:
            findings.append(
                f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} original function body "
                "could not be verified"
            )
            continue
        if binance_test_has_early_exit_bypass(body):
            findings.append(
                f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} must not contain "
                "early-exit or conditional-compilation proof bypasses"
            )
        trade_loop_span = (
            binance_trade_loop_span(body)
            if function_name
            == "sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps"
            else None
        )
        for description, pattern in requirements:
            matches = list(re.finditer(pattern, body))
            if not matches:
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} missing {description}"
                )
                continue
            if not description.endswith("assertion"):
                continue
            if description in BINANCE_TIMESTAMP_TRADE_PER_ITEM_ASSERTIONS:
                assertion_is_canonical = (
                    len(matches) == 1
                    and trade_loop_span is not None
                    and trade_loop_span[0] <= matches[0].start() < trade_loop_span[1]
                    and rust_open_delimiters_at(body, matches[0].start()) == ("{",)
                )
                if not assertion_is_canonical:
                    findings.append(
                        f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} governed "
                        "assertion must remain inside the canonical per-item trade loop"
                    )
            elif len(matches) != 1 or rust_open_delimiters_at(
                body, matches[0].start()
            ) != ():
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} governed assertion "
                    "must remain at its canonical control-flow depth"
                )
            if len(matches) == 1 and not rust_assertion_is_complete_expression_statement(
                body, matches[0].start()
            ):
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} governed assertion "
                    "must be a complete expression statement in its canonical control-flow block"
                )
        if trade_loop_span is None and function_name.startswith("sbe_multi_trade_"):
            findings.append(
                f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} governed assertion "
                "must remain inside the canonical per-item trade loop"
            )
        elif trade_loop_span is not None:
            trade_extractions = list(re.finditer(r"\bData\s*::\s*Trade\b", body))
            if (
                len(trade_extractions) != 1
                or not (
                    trade_loop_span[0]
                    <= trade_extractions[0].start()
                    < trade_loop_span[1]
                )
                or rust_open_delimiters_at(body, trade_extractions[0].start())
                != ("{",)
            ):
                findings.append(
                    f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} must extract each "
                    "trade inside the canonical per-item trade loop"
                )
        (
            parser_symbol,
            result_variable,
        ) = BINANCE_TIMESTAMP_TEST_CASE_RESULT_CONTRACTS[function_name]
        scalar, event_type = BINANCE_TIMESTAMP_TEST_CASE_EVENT_CONTRACTS[
            function_name
        ]
        if not has_governed_expected_event_contract(
            body,
            scalar,
            event_type,
            parser_symbol,
        ):
            findings.append(
                f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} must derive "
                "expected_ts_event once from the event's canonical provider-time "
                "scalar before parsing"
            )
        if not has_governed_binance_parser_result_contract(
            body,
            original_body,
            parser_symbol,
            result_variable,
        ):
            findings.append(
                f"{BINANCE_TIMESTAMP_TEST_PATH}: {function_name} must bind "
                f"{result_variable} directly to pinned {parser_symbol} exactly once without "
                "rebinding or reassignment"
            )


def scan_runtime_contract_pin(
    surface: Path,
    text: str,
    findings: list[str],
    expected_revision: str,
) -> None:
    owner_sections: dict[str, str] = {}
    for heading, pattern in RUNTIME_CONTRACT_PIN_SECTIONS:
        section = markdown_section(text, heading)
        if section is None:
            findings.append(
                f"{surface}: NautilusTrader pin census required owner section {heading!r} "
                "must appear exactly once"
            )
            continue
        owner_sections[heading] = section
        revisions = [match.group(1) for match in pattern.finditer(section)]
        if revisions != [expected_revision]:
            findings.append(
                f"{surface}: NautilusTrader pin census {heading} must contain exactly one "
                f"governed pin {expected_revision}"
            )

    binance_owner = owner_sections.get(BINANCE_BOUNDARY_OWNER_HEADING)
    if binance_owner is None:
        return
    for symbol in (*BINANCE_SOURCE_SYMBOLS, *BINANCE_BOUNDARY_CONSUMERS):
        if f"`{symbol}`" not in binance_owner:
            findings.append(
                f"{surface}: NautilusTrader pin census {BINANCE_BOUNDARY_OWNER_HEADING} "
                f"missing {symbol}"
            )


def scan_nt_pin_census(root: Path, findings: list[str]) -> None:
    root_manifest = read_required_pin_surface(root, Path("Cargo.toml"), findings)
    if root_manifest is None:
        return
    expected_revision = root_manifest_nt_revision(root_manifest, findings)
    if expected_revision is None:
        return

    cargo_surfaces = set(CANONICAL_CARGO_PIN_SURFACES)
    for surface in tracked_cargo_surfaces(root, findings):
        if surface in CANONICAL_CARGO_PIN_SURFACES:
            continue
        try:
            text = read(root, surface)
        except OSError as error:
            findings.append(
                f"{surface}: NautilusTrader pin census could not read tracked Cargo surface: {error}"
            )
            continue
        if cargo_surface_references_nt(surface, text):
            cargo_surfaces.add(surface)

    for surface in sorted(cargo_surfaces):
        text = (
            root_manifest
            if surface == Path("Cargo.toml")
            else read_required_pin_surface(root, surface, findings)
        )
        if text is None:
            continue
        if surface.name == "Cargo.toml":
            scan_nt_manifest_pin(
                surface,
                text,
                findings,
                expected_revision,
                allow_workspace_inheritance=surface
                not in CANONICAL_CARGO_PIN_SURFACES,
            )
            continue
        scan_nt_lock_pin(surface, text, findings, expected_revision)

    for surface in NON_CARGO_PIN_SURFACES:
        text = read_required_pin_surface(root, surface, findings)
        if text is None:
            continue
        if surface == RUNTIME_CONTRACT_PIN_SURFACE:
            scan_runtime_contract_pin(surface, text, findings, expected_revision)
            continue
        pattern_revisions = [
            [match.group(1) for match in pattern.finditer(text)]
            for pattern in PIN_TEXT_PATTERNS[surface]
        ]
        invalid_pin_surface = any(
            revisions != [expected_revision] for revisions in pattern_revisions
        )
        if invalid_pin_surface:
            findings.append(
                f"{surface}: NautilusTrader pin census must contain exactly one governed "
                f"{expected_revision} value for each anchored pin claim"
            )

def scan_root(root: Path, *, today: dt.date | None = None) -> list[str]:
    if today is None:
        today = dt.date.today()
    findings: list[str] = []
    source_paths = boundary_source_paths(root)
    floor_findings: list[str] = []
    if not require_nonempty(source_paths, "Bolt-v3 boundary Rust source files", floor_findings):
        return floor_findings
    entries = scan_registry(root, findings)
    scan_exemptions(root, entries, findings, today=today)
    scan_exemption_issue_state(root, findings)
    scan_wire_boundary(root, findings, source_paths)
    scan_chainlink_tests(root, findings)
    scan_fixture_origin(root, findings)
    scan_static_wiring(root, findings)
    scan_binance_timestamp_behavioral_contract(root, findings)
    scan_nt_pin_census(root, findings)
    return findings


def main() -> int:
    findings = scan_root(REPO_ROOT)
    if findings:
        for finding in findings:
            print(f"FAIL: {finding}", file=sys.stderr)
        return 1
    print("OK: bolt-v3 boundary evidence audit passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
