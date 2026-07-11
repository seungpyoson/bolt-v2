#!/usr/bin/env python3
"""Verify bolt-v3 provider/runtime boundary evidence wiring."""

from __future__ import annotations

import dataclasses
import datetime as dt
import hashlib
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
from verify_bolt_v3_provider_leaks import production_text
from verifier_io import require_nonempty


REPO_ROOT = Path(__file__).resolve().parent.parent
EXPECTED_NT_REV = "fa3391d90c1aace4733fc73dae082b4cfee6b8fa"
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
PIN_SURFACES = (
    Path("Cargo.toml"),
    Path("Cargo.lock"),
    Path("crates/backtesting-vertical-slice/Cargo.toml"),
    Path("crates/backtesting-vertical-slice/Cargo.lock"),
    Path("docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md"),
    Path("docs/bolt-v3/research/naming/nt-owned-name-audit.yaml"),
    Path("scripts/verify_bolt_v3_boundary_evidence.py"),
    Path("scripts/test_verify_bolt_v3_boundary_evidence.py"),
)
BINANCE_SOURCE_SYMBOLS = (
    "BinanceSpotDataClient::handle_ws_message",
    "decode_market_data",
    "parse_bbo_event",
)
PIN_TEXT_PATTERNS = {
    PIN_SURFACES[4]: (
        re.compile(r"current value: `([0-9a-f]{40})`"),
        re.compile(r"live Binance Spot SBE quote boundary is owned by NautilusTrader revision\s+`([0-9a-f]{40})`"),
        re.compile(r"Current status: this branch pins NautilusTrader to\s+`([0-9a-f]{40})`"),
    ),
    PIN_SURFACES[5]: (re.compile(r'nautilus_trader_revision:\s*"([0-9a-f]{40})"'),),
    PIN_SURFACES[6]: (re.compile(r'EXPECTED_NT_REV\s*=\s*"([0-9a-f]{40})"'),),
    PIN_SURFACES[7]: (re.compile(r'EXPECTED_NT_REV\s*=\s*"([0-9a-f]{40})"'),),
}
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


def reference_named_provider_binding_keys(keys: set[str]) -> set[str]:
    reference_keys = set()
    for key in keys:
        module = key.rsplit("::", 1)[0].rsplit("::", 1)[-1]
        if module == "reference" or module.endswith("_reference") or "_reference_" in module:
            reference_keys.add(key)
    return reference_keys


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
    if re.search(r"class:\s*\"", text):
        findings.append(f"{REGISTRY}: registry class tags must use enum variants, not strings")

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

    extra_http = sorted(
        entry
        for entry in entries
        if entry[1] == "HttpResponseBody" and entry not in REQUIRED_NON_WS_REGISTRY_ENTRIES
    )
    if extra_http:
        findings.append(f"{REGISTRY}: unexpected http_response_body registry entry {extra_http}")

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

    cargo_toml = read(root, "Cargo.toml")
    if f'rev = "{EXPECTED_NT_REV}"' not in cargo_toml:
        findings.append(f"Cargo.toml: nautilus_network rev must remain pinned to {EXPECTED_NT_REV}")

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
    if re.search(r"\.as_text\(", production):
        findings.append("src/bolt_v3_providers/chainlink_reference.rs: Chainlink handler must not use parser-only as_text")

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


def remote_fixture_context(root: Path, findings: list[str]) -> tuple[str, str, str | None] | None:
    if root.resolve() != REPO_ROOT.resolve() or os.environ.get("GITHUB_ACTIONS") != "true":
        return None

    token = os.environ.get("GITHUB_TOKEN")
    repo = os.environ.get("GITHUB_REPOSITORY")
    if not token or not repo:
        findings.append(
            f"{FIXTURE_DIR}: GitHub Actions fixture-origin check requires GITHUB_TOKEN and GITHUB_REPOSITORY"
        )
        return None
    return repo, token, os.environ.get("GITHUB_RUN_ID")


def capture_workflow_digest(root: Path, config: ci_provenance.ProvenanceConfig, record: dict[str, object]) -> str | None:
    tested_sha = record.get("tested_sha")
    if not isinstance(tested_sha, str) or not re.fullmatch(r"[0-9a-f]{40}", tested_sha):
        raise ci_provenance.ProvenanceError("record tested_sha must be a 40-character hex SHA")
    result = subprocess.run(
        ["git", "-C", str(root), "show", f"{tested_sha}:{config.workflow_path}"],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    if result.returncode != 0:
        return None
    return hashlib.sha256(result.stdout).hexdigest()


def scan_fixture_origin(root: Path, findings: list[str]) -> None:
    directory = root / FIXTURE_DIR
    sidecars = sorted(directory.glob("*.toml")) if directory.exists() else []
    if not sidecars:
        findings.append(f"{FIXTURE_DIR}: missing Chainlink fixture sidecar")
        return

    remote_context = remote_fixture_context(root, findings)
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
            workflow_digest = capture_workflow_digest(root, config, record)
            if workflow_digest is None:
                if remote_context is None:
                    findings.append(
                        f"{rel}: invalid capture artifact provenance: workflow {config.workflow_path} "
                        f"is not resolvable at tested_sha {record.get('tested_sha')}"
                    )
                    continue
            else:
                ci_provenance.validate_exact_sha_record(
                    record,
                    sidecar_config,
                    requested_sha=str(data["capture_head_sha"]),
                    config_path=config_path,
                    expected_workflow_digest=workflow_digest,
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
        if remote_context is not None:
            repo, token, current_run_id = remote_context
            try:
                resolved = ci_provenance.resolve_exact_sha_evidence(
                    repo=repo,
                    token=token,
                    requested_sha=str(data["capture_head_sha"]),
                    config=sidecar_config,
                    config_path=config_path,
                    current_run_id=current_run_id,
                    allow_incomplete_run_with_successful_jobs=True,
                )
            except ci_provenance.ProvenanceError as exc:
                findings.append(f"{rel}: GitHub capture provenance resolution failed: {exc}")
                continue
            if resolved.record != record:
                findings.append(f"{rel}: committed capture artifact record does not match GitHub artifact")
            remote_capture = resolved.record.get("capture")
            if not isinstance(remote_capture, dict):
                findings.append(f"{rel}: GitHub capture artifact is missing capture object")
                continue
            if remote_capture.get("fixture_sha256") != fixture_digest:
                findings.append(f"{rel}: GitHub capture artifact digest does not match fixture")
            if remote_capture.get("frame_kind") != data["frame_kind"]:
                findings.append(f"{rel}: GitHub capture artifact frame_kind does not match sidecar")


def scan_static_wiring(root: Path, findings: list[str]) -> None:
    justfile = read(root, "justfile")
    for command in (
        "python3 scripts/test_verify_bolt_v3_boundary_evidence.py",
        "python3 scripts/verify_bolt_v3_boundary_evidence.py",
    ):
        if command not in justfile:
            findings.append(f"justfile: source-fence-static-inner missing {command}")

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


def scan_nt_pin_census(root: Path, findings: list[str]) -> None:
    manifest_pattern = re.compile(
        r'git\s*=\s*"https://github\.com/seungpyoson/nautilus_trader\.git"[^\n]*?rev\s*=\s*"([0-9a-f]{40})"'
    )
    lock_pattern = re.compile(
        r'git\+https://github\.com/seungpyoson/nautilus_trader\.git\?rev=([0-9a-f]{40})#([0-9a-f]{40})'
    )
    for surface in PIN_SURFACES:
        text = read(root, surface)
        revisions: list[str]
        if surface.name == "Cargo.toml":
            revisions = manifest_pattern.findall(text)
        elif surface.name == "Cargo.lock":
            lock_revisions = lock_pattern.findall(text)
            revisions = [revision for pair in lock_revisions for revision in pair]
        else:
            revisions = [
                match.group(1)
                for pattern in PIN_TEXT_PATTERNS[surface]
                for match in pattern.finditer(text)
            ]
        if not revisions or any(revision != EXPECTED_NT_REV for revision in revisions):
            findings.append(
                f"{surface}: NautilusTrader pin census must contain only {EXPECTED_NT_REV}"
            )

    runtime_contract = read(root, PIN_SURFACES[4])
    for symbol in BINANCE_SOURCE_SYMBOLS:
        if f"`{symbol}`" not in runtime_contract:
            findings.append(
                f"{PIN_SURFACES[4]}: missing pinned Binance Spot SBE source symbol {symbol}"
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
