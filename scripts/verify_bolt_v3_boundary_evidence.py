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

from verify_bolt_v3_provider_leaks import production_text
from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
EXPECTED_NT_GIT = "https://github.com/nautechsystems/nautilus_trader.git"
FORBIDDEN_PERSONAL_NT_SOURCE = "github.com/seungpyoson/nautilus_trader"
REGISTRY = Path("src/bolt_v3_providers/boundary_registry.rs")
WIRE_BOUNDARY = Path("src/bolt_v3_wire_boundary.rs")
EXEMPTIONS = Path("ci/bolt-v3-boundary-exemptions.toml")

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
CURRENT_STATUS_MAP_SURFACE = Path(
    "docs/bolt-v3/2026-04-28-source-grounded-status-map.md"
)
CURRENT_STATUS_MAP_CAPABILITY_CLAIM = (
    "provides Binance Spot SBE schema 3:5 and adapter receive-clock ownership; "
    "those capabilities enable the composite new-risk quorum"
)
NT_SOURCE_CAPABILITIES_PIN_SURFACE = Path("ci/nautilus-source-capabilities.toml")
POLYMARKET_QUERY_FIXTURE_PIN_SURFACE = Path(
    "tests/fixtures/nt_polymarket_query_post_order_params_d81be0bc.txt"
)
NON_CARGO_PIN_SURFACES = (
    RUNTIME_CONTRACT_PIN_SURFACE,
    ROOT_SCHEMA_PIN_SURFACE,
    NT_BOUNDARY_DOCTRINE_PIN_SURFACE,
    NT_NAMING_LEDGER_PIN_SURFACE,
    POLYMARKET_QUERY_FIXTURE_PIN_SURFACE,
    NT_SOURCE_CAPABILITIES_PIN_SURFACE,
)
BINANCE_SOURCE_SYMBOLS = (
    "BinanceSpotDataClient::handle_ws_message",
    "NAUTILUS_SOURCE_CAPABILITIES",
    "ProviderCapabilityUnavailable",
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
    NT_SOURCE_CAPABILITIES_PIN_SURFACE: (
        re.compile(r'^revision = "([0-9a-f]{40})"$', re.MULTILINE),
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
            r"^Current status: this branch pins the official NautilusTrader repository at the\s+immutable upstream PR #4474 merge commit\s+`([0-9a-f]{40})`\.",
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


def scan_active_personal_nt_source(root: Path, findings: list[str]) -> None:
    surfaces = set(CANONICAL_CARGO_PIN_SURFACES)
    surfaces.update(tracked_cargo_surfaces(root, findings))
    surfaces.add(NT_SOURCE_CAPABILITIES_PIN_SURFACE)
    for surface in sorted(surfaces):
        try:
            text = read(root, surface)
        except OSError as error:
            findings.append(
                f"{surface}: active NautilusTrader source scan could not read surface: {error}"
            )
            continue
        if FORBIDDEN_PERSONAL_NT_SOURCE in text:
            findings.append(
                f"{surface}: active NautilusTrader source must contain zero personal-fork references"
            )


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
        if surface == NT_SOURCE_CAPABILITIES_PIN_SURFACE:
            try:
                capabilities = tomllib.loads(text)
            except tomllib.TOMLDecodeError as error:
                findings.append(f"{surface}: could not parse capability manifest: {error}")
                continue
            source = capabilities.get("source", {})
            binance = capabilities.get("binance_spot_sbe", {})
            if source.get("git") != EXPECTED_NT_GIT:
                findings.append(
                    f"{surface}: capability manifest must use official git={EXPECTED_NT_GIT!r}"
                )
            required_flags = (
                binance.get("schema_3_5"),
                binance.get("adapter_receive_clock"),
                binance.get("new_risk_quorum"),
            )
            if required_flags != (True, True, True):
                findings.append(
                    f"{surface}: official post-PR-4474 Binance SBE capabilities must remain "
                    "true/true/true"
                )

    status_map = read_required_pin_surface(root, CURRENT_STATUS_MAP_SURFACE, findings)
    if (
        status_map is not None
        and status_map.count(CURRENT_STATUS_MAP_CAPABILITY_CLAIM) != 2
    ):
        findings.append(
            f"{CURRENT_STATUS_MAP_SURFACE}: current status map must state available "
            "Binance Spot SBE capabilities in both the summary and readiness row"
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
    scan_static_wiring(root, findings)
    scan_active_personal_nt_source(root, findings)
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
