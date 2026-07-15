#!/usr/bin/env python3
"""Fail-closed structural and source fence for disabled AO-REDEEM."""

from __future__ import annotations

import hashlib
import pathlib
import re
import sys
import tomllib


MANIFEST_PATH = pathlib.Path("ci/polymarket-redemption-provider-manifest.toml")
CONFIG_PATH = pathlib.Path("config/polymarket-redemption.toml")
MODULE_PATH = pathlib.Path("src/bolt_v3_providers/polymarket/redemption.rs")
REDEMPTION_ROOT = pathlib.Path("src/bolt_v3_providers/polymarket/redemption")
REQUIRED_PATHS = (
    MANIFEST_PATH,
    CONFIG_PATH,
    MODULE_PATH,
    *(REDEMPTION_ROOT / name for name in (
        "bounded.rs", "capability.rs", "config.rs", "nonce.rs", "query.rs",
        "request.rs", "tests.rs", "wire.rs",
    )),
    pathlib.Path("src/bolt_v3_providers/polymarket.rs"),
    pathlib.Path("src/bolt_v3_providers/boundary_registry.rs"),
    pathlib.Path("tests/bolt_v3_redeem_primitive.rs"),
    pathlib.Path("tests/fixtures/bolt_v3/redeem/standard.toml"),
    pathlib.Path("tests/fixtures/bolt_v3/redeem/negative-risk.toml"),
    pathlib.Path("tests/fixtures/bolt_v3/redeem/source/ctf-collateral-adapter.txt"),
    pathlib.Path("tests/fixtures/bolt_v3/redeem/source/negative-risk-collateral-adapter.txt"),
    pathlib.Path("tests/fixtures/bolt_v3/redeem/source/relayer-safe-builder.txt"),
    pathlib.Path("Cargo.toml"),
)

EXPECTED_REVISIONS = {
    "adapter": "ccc0596074f4dfd62c944fbca4de252893b82b4b",
    "relayer": "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290",
    "independent_fixture": "267a36d84d7839b6e4ac134297d9230fc224cf8f",
}
EXPECTED_BLOBS = {
    ("adapter", "standard_blob"): "4af744d991e0eb4fbf93e72aba051788edb81688",
    ("adapter", "negative_risk_blob"): "698691ab6134ab31fbebb0fa62ffdb7a0395bcb8",
    ("relayer", "safe_builder_blob"): "3a05ac53d005d92822582a2a87d6bdbb13827187",
    ("relayer", "types_blob"): "daa09af14528ba5be49a0063b359fbc17dc5e505",
    ("relayer", "client_blob"): "2ca627dc7f853d03c9bd7dbbec505e86a944d101",
    ("relayer", "response_blob"): "8be9b84d6b9e45c021744fac478087c536f6233a",
    ("wire", "receipt_schema_blob"): "ab67b3a1356ca693e89fa088be2179353654ca3d",
}

CAPABILITIES = (
    "ExactConditionSnapshotLease",
    "SafeNonceBodyCapacityPermit",
    "FreshPreSendValidation",
    "OriginalMayHaveStartedPermit",
    "FenceMayHaveStartedPermit",
)
OPERATIONAL_SYMBOLS = set(CAPABILITIES) | {
    "AuthorizedRequest", "FinalizedChainSourceResponse", "RelayerSourceResponse",
    "ExactQueryResponses", "ExactQuerySet",
    "FenceMayHaveStartedRequest", "OriginalMayHaveStartedRequest", "build_request_pair",
    "resolve_credentials",
}
FORBIDDEN_PROVIDER_AUTHORITY = {
    "ConditionRegistry", "ConditionLease", "TerminalLeaseCertificate", "write_request",
    "mark_dispatched", "complete_verified_resolution", "recover_verified_resolution",
    "RedemptionAuthority", "SsmSecretResolver", "write_query",
}


def _load_toml(path: pathlib.Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def rust_tokens(source: str) -> list[str]:
    """Tokenize Rust while discarding comments and literal contents."""
    tokens: list[str] = []
    index = 0
    while index < len(source):
        if source[index].isspace():
            index += 1
            continue
        if source.startswith("//", index):
            end = source.find("\n", index + 2)
            index = len(source) if end < 0 else end + 1
            continue
        if source.startswith("/*", index):
            depth, index = 1, index + 2
            while index < len(source) and depth:
                if source.startswith("/*", index):
                    depth, index = depth + 1, index + 2
                elif source.startswith("*/", index):
                    depth, index = depth - 1, index + 2
                else:
                    index += 1
            continue
        raw = re.match(r'(?:b|c)?r(#+)?"', source[index:])
        if raw:
            hashes = raw.group(1) or ""
            index += raw.end()
            end = source.find('"' + hashes, index)
            index = len(source) if end < 0 else end + 1 + len(hashes)
            tokens.append("<literal>")
            continue
        if source[index] == '"' or (source[index] in "bc" and source[index + 1:index + 2] == '"'):
            if source[index] != '"':
                index += 1
            index += 1
            while index < len(source):
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            tokens.append("<literal>")
            continue
        if source[index].isalpha() or source[index] == "_":
            end = index + 1
            while end < len(source) and (source[end].isalnum() or source[end] == "_"):
                end += 1
            tokens.append(source[index:end])
            index = end
            continue
        matched = False
        for punctuation in ("::", "->", "=>", "..=", "..", "==", "!=", "&&", "||"):
            if source.startswith(punctuation, index):
                tokens.append(punctuation)
                index += len(punctuation)
                matched = True
                break
        if not matched:
            tokens.append(source[index])
            index += 1
    return tokens


def _contains_sequence(tokens: list[str], sequence: tuple[str, ...]) -> bool:
    return any(tokens[index:index + len(sequence)] == list(sequence) for index in range(len(tokens) - len(sequence) + 1))


def _function_signatures(tokens: list[str]) -> dict[str, list[list[str]]]:
    signatures: dict[str, list[list[str]]] = {}
    for index, token in enumerate(tokens[:-2]):
        if token != "fn":
            continue
        name, cursor, parens, angles = tokens[index + 1], index + 2, 0, 0
        while cursor < len(tokens):
            current = tokens[cursor]
            angles += current == "<"
            angles -= current == ">" and angles > 0
            parens += current == "("
            parens -= current == ")" and parens > 0
            if current in {"{", ";"} and not parens and not angles:
                break
            cursor += 1
        signatures.setdefault(name, []).append(tokens[index:cursor])
    return signatures


def _struct_fields(source: str, name: str) -> set[str]:
    match = re.search(
        rf"(?:pub(?:\([^)]*\))?\s+)?struct\s+{name}(?:<'[^>]+>)?\s*{{([^}}]*)}}",
        source,
        re.S,
    )
    return set(re.findall(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:", match.group(1), re.M)) if match else set()


def verify_structural_reachability(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    provider = pathlib.Path("src/bolt_v3_providers/polymarket.rs")
    registry = pathlib.Path("src/bolt_v3_providers/boundary_registry.rs")
    for path in (root / "src").rglob("*.rs"):
        relative = path.relative_to(root)
        if relative == MODULE_PATH or str(relative).startswith(str(REDEMPTION_ROOT)):
            continue
        tokens = rust_tokens(path.read_text(encoding="utf-8"))
        if relative == provider:
            if tokens.count("redemption") != 1 or not _contains_sequence(tokens, ("pub", "mod", "redemption", ";")):
                errors.append("AO-REDEEM provider parent exceeds the exact module declaration")
            if OPERATIONAL_SYMBOLS.intersection(tokens):
                errors.append("AO-REDEEM provider parent references operational authority")
            continue
        if relative == registry:
            allowed = {"POLYMARKET_RELAYER_ADAPTER_ID", "POLYGON_REDEMPTION_RPC_ADAPTER_ID"}
            if tokens.count("redemption") != 1 or OPERATIONAL_SYMBOLS.intersection(tokens) - allowed:
                errors.append("AO-REDEEM boundary registry exceeds registered identifiers")
            continue
        if "redemption" in tokens or OPERATIONAL_SYMBOLS.intersection(tokens):
            errors.append(f"AO-REDEEM structural disabled reachability violated by {relative}")

    for relative in (pathlib.Path("build.rs"), pathlib.Path("src/generated.rs")):
        path = root / relative
        if path.exists() and OPERATIONAL_SYMBOLS.intersection(rust_tokens(path.read_text(encoding="utf-8"))):
            errors.append(f"AO-REDEEM build/generated reachability violated by {relative}")
    return errors


def verify_capabilities(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    capability = (root / REDEMPTION_ROOT / "capability.rs").read_text(encoding="utf-8")
    production, marker, hermetic = capability.partition("#[cfg(test)]")
    if not marker:
        errors.append("AO-REDEEM capability issuers are not isolated behind cfg(test)")
    production_tokens = rust_tokens(production)
    for name in CAPABILITIES:
        if not _contains_sequence(production_tokens, ("pub", "struct", name, "{")):
            errors.append(f"AO-REDEEM linear capability missing: {name}")
        if _contains_sequence(production_tokens, ("impl", "Clone", "for", name)) or _contains_sequence(production_tokens, ("impl", "Copy", "for", name)):
            errors.append(f"AO-REDEEM capability is duplicable: {name}")
        if re.search(rf"derive\([^)]*(?:Debug|Clone|Copy|Serialize)[^)]*\)\s*pub struct {name}\b", production):
            errors.append(f"AO-REDEEM capability exposes formatting/copy surface: {name}")
        if re.search(rf"pub\s+(?:const\s+)?fn\s+\w+[^{{;]*->\s*{name}\b", production):
            errors.append(f"AO-REDEEM production can mint capability: {name}")
        if name not in hermetic:
            errors.append(f"AO-REDEEM hermetic issuer missing for {name}")

    request_tokens = rust_tokens((root / REDEMPTION_ROOT / "request.rs").read_text(encoding="utf-8"))
    signatures = _function_signatures(request_tokens)
    required_bindings = {
        "build_request_pair": {"ExactConditionSnapshotLease", "SafeNonceBodyCapacityPermit"},
        "authorize_original": {"FreshPreSendValidation", "OriginalMayHaveStartedPermit"},
        "authorize_fence": {"FreshPreSendValidation", "FenceMayHaveStartedPermit"},
    }
    for name, required in required_bindings.items():
        if name not in signatures or not any(required.issubset(signature) for signature in map(set, signatures[name])):
            errors.append(f"AO-REDEEM capability binding incomplete: {name}")
    if _contains_sequence(request_tokens, ("impl", "PreparedRequestPair")) and "authorize_fence" in signatures:
        source = (root / REDEMPTION_ROOT / "request.rs").read_text(encoding="utf-8")
        if "impl OriginalMayHaveStartedRequest" not in source or source.index("fn authorize_fence") < source.index("impl OriginalMayHaveStartedRequest"):
            errors.append("AO-REDEEM fence-first remains structurally representable")

    all_source = "\n".join((root / path).read_text(encoding="utf-8") for path in REQUIRED_PATHS if str(path).startswith(str(REDEMPTION_ROOT)) or path == MODULE_PATH)
    all_tokens = rust_tokens(all_source)
    for forbidden in FORBIDDEN_PROVIDER_AUTHORITY:
        if forbidden in all_tokens:
            errors.append(f"AO-REDEEM provider-owned authority is forbidden: {forbidden}")
    if "Write" in all_tokens:
        errors.append("AO-REDEEM arbitrary effect sink is forbidden")
    for pattern in (r"\basync\s+fn\b", r"\bfn\s+send\b", r"reqwest", r"TcpStream", r"std::fs", r"tokio::fs", r"rusqlite", r"log::", r"tracing::", r"println!"):
        if re.search(pattern, all_source):
            errors.append("AO-REDEEM active caller, durable state, or observability sink is forbidden")
            break
    outcome_issuer_uses = {
        path.name
        for path in (root / REDEMPTION_ROOT).glob("*.rs")
        if "from_raw_verifier" in path.read_text(encoding="utf-8")
    }
    if outcome_issuer_uses != {"query.rs", "wire.rs"}:
        errors.append("AO-REDEEM verified outcome provenance escapes the raw wire verifier")
    wire_source = (root / REDEMPTION_ROOT / "wire.rs").read_text(encoding="utf-8")
    if re.search(r"pub\s+(?:fn|struct)[^{;]*(?:impl\s+)?(?:std::io::)?Read\b", wire_source):
        errors.append("AO-REDEEM arbitrary reader can mint source proof")
    if re.search(r"pub\s+struct\s+BoundedWireResponse\b", wire_source):
        errors.append("AO-REDEEM arbitrary reader can mint source proof")
    for capability in ("RelayerSourceResponse", "FinalizedChainSourceResponse"):
        if not re.search(rf"pub struct {capability}\s*{{", wire_source):
            errors.append(f"AO-REDEEM source-specific response capability missing: {capability}")
        if re.search(
            rf"pub\s+fn\s+\w+[^{{;]*->\s*(?:Self|{capability})\b",
            wire_source,
        ):
            errors.append(f"AO-REDEEM production can mint source response: {capability}")
    wire_signatures = _function_signatures(
        rust_tokens((root / REDEMPTION_ROOT / "wire.rs").read_text(encoding="utf-8"))
    )
    if any("RedemptionResolution" in signature for name in ("verify", "verify_after_original", "verify_after_fence") for signature in wire_signatures.get(name, [])):
        errors.append("AO-REDEEM terminality accepts a caller-classified resolution")
    return errors


def verify_opaque_surfaces(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    source = "\n".join((root / REDEMPTION_ROOT / name).read_text(encoding="utf-8") for name in ("request.rs", "query.rs", "wire.rs", "config.rs", "capability.rs"))
    payloads = CAPABILITIES + (
        "PreparedRequestPair", "AuthorizedRequest", "OriginalMayHaveStartedRequest",
        "FenceMayHaveStartedRequest", "SourceBoundVerifiedOutcome",
        "RelayerSourceResponse", "FinalizedChainSourceResponse",
        "ExactQueryResponses", "RelayerObservation", "ResolvedRedemptionCredentials",
    )
    for name in payloads:
        match = re.search(rf"(?:pub\s+)?struct\s+{name}(?:<'[^>]+>)?\s*{{([^}}]*)}}", source, re.S)
        if not match:
            errors.append(f"AO-REDEEM opaque type missing: {name}")
            continue
        if re.search(r"\bpub\s+\w+\s*:", match.group(1)):
            errors.append(f"AO-REDEEM opaque payload has public field: {name}")
        prefix = source[max(0, match.start() - 100):match.start()]
        if re.search(r"derive\([^)]*(?:Debug|Serialize)[^)]*\)", prefix):
            errors.append(f"AO-REDEEM opaque payload derives formatting/serialization: {name}")
    for forbidden in ("redaction_values", "raw_bytes", "body_bytes", "write_request", "write_query"):
        if re.search(rf"pub(?:\([^)]*\))?\s+fn\s+{forbidden}\b", source):
            errors.append(f"AO-REDEEM raw/effect surface exposed: {forbidden}")
    query_source = (root / REDEMPTION_ROOT / "query.rs").read_text(encoding="utf-8")
    outcome = re.search(r"pub struct SourceBoundVerifiedOutcome\s*{([^}]*)}", query_source, re.S)
    required_binding = {
        "resolution", "profile_digest", "config_digest", "key_version", "chain_id",
        "relayer_source_identity", "chain_source_identity", "action_digest", "condition_id",
        "pre_claim_balances", "pre_collateral_balance", "expected_redeemed_collateral_balance",
        "safe_nonce", "original_body_hash", "fence_body_hash", "finalized_block_number",
        "finalized_block_hash", "fence_authorized",
    }
    actual_binding = set(re.findall(r"^\s*(\w+)\s*:", outcome.group(1), re.M)) if outcome else set()
    if actual_binding != required_binding:
        errors.append("AO-REDEEM verified outcome binding is incomplete")
    for method in ("consume_after_original", "consume_after_fence"):
        if not re.search(rf"pub fn {method}\s*\(", query_source):
            errors.append("AO-REDEEM exact terminal consumption is missing")
        signatures = _function_signatures(rust_tokens(query_source)).get(method, [])
        if not any("ExactQueryResponses" in signature for signature in signatures):
            errors.append("AO-REDEEM exact terminal consumption lacks source capability binding")
    if re.search(r"pub fn resolution\s*\(", query_source):
        errors.append("AO-REDEEM terminal resolution bypasses exact terminal consumption")
    request_source = (root / REDEMPTION_ROOT / "request.rs").read_text(encoding="utf-8")
    capability_source = (root / REDEMPTION_ROOT / "capability.rs").read_text(encoding="utf-8")
    required_state = {
        "pre_claim_balances", "pre_collateral_balance",
        "expected_redeemed_collateral_balance",
    }
    if not required_state.issubset(_struct_fields(request_source, "PreparedRequestPair")) or not required_state.issubset(
        _struct_fields(capability_source, "ExactConditionSnapshotLease")
    ):
        errors.append("AO-REDEEM post-state balance contract is incomplete")
    required_context = {
        "profile_digest", "config_digest", "relayer_source_identity", "chain_source_identity",
        "credential_key_version",
    }
    request_signatures = _function_signatures(rust_tokens(request_source))
    context_signatures = request_signatures.get("matches_context", [])
    if not required_context.issubset(_struct_fields(request_source, "PreparedRequestPair")) or not any(
        {"ValidatedRedemptionProfile", "ResolvedRedemptionCredentials", "bool"}.issubset(signature)
        for signature in map(set, context_signatures)
    ):
        errors.append("AO-REDEEM prepared action context binding is incomplete")
    config_source = (root / REDEMPTION_ROOT / "config.rs").read_text(encoding="utf-8")
    if re.search(r"dummy_index_sets\s*:\s*\[\s*1\s*,\s*2\s*\]", config_source):
        errors.append("AO-REDEEM runtime dummy index sets are reconstructed")
    wire_source = (root / REDEMPTION_ROOT / "wire.rs").read_text(encoding="utf-8")
    query_source = (root / REDEMPTION_ROOT / "query.rs").read_text(encoding="utf-8")
    exact_query_fields = {
        "kind", "request_digest", "path_digest", "calldata_digest", "response_class",
    }
    response_binding_fields = {
        name: _struct_fields(wire_source, name)
        for name in ("RelayerSourceResponse", "FinalizedChainSourceResponse")
    }
    query_signatures = _function_signatures(rust_tokens(wire_source)).get("matches_query", [])
    if _struct_fields(query_source, "ExactQueryBinding") != exact_query_fields or any(
        "request_binding" not in fields for fields in response_binding_fields.values()
    ) or not any(
        {"ExactQuerySet", "QueryKind", "bool"}.issubset(signature)
        for signature in map(set, query_signatures)
    ):
        errors.append("AO-REDEEM source response query binding is incomplete")
    return errors


def verify(root: pathlib.Path) -> list[str]:
    missing = [str(path) for path in REQUIRED_PATHS if not (root / path).is_file()]
    if missing:
        return [f"AO-REDEEM required path missing: {path}" for path in missing]
    errors: list[str] = []
    try:
        manifest, config = _load_toml(root / MANIFEST_PATH), _load_toml(root / CONFIG_PATH)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return [f"AO-REDEEM TOML parse failure: {exc}"]

    for section, revision in EXPECTED_REVISIONS.items():
        if manifest.get(section, {}).get("reviewed_revision") != revision:
            errors.append(f"AO-REDEEM {section} reviewed revision drifted from {revision}")
    for (section, field), expected in EXPECTED_BLOBS.items():
        if manifest.get(section, {}).get(field) != expected:
            errors.append(f"AO-REDEEM reviewed blob drifted: {section}.{field}")
    adapter, safe, relayer = manifest.get("adapter", {}), manifest.get("safe", {}), manifest.get("relayer", {})
    if adapter.get("external_abi") != "redeemPositions(address,bytes32,bytes32,uint256[])" or adapter.get("ignored_argument_indices") != [0, 1, 3]:
        errors.append("AO-REDEEM adapter ABI/dummy arguments are not exactly source-fenced")
    if manifest.get("adapter_arguments", {}).get("dummy_index_sets") != [1, 2]:
        errors.append("AO-REDEEM adapter dummy index structural contract drifted")
    if safe.get("nonce_abi") != "nonce()" or safe.get("nonce_selector") != "0xaffed0e0" or safe.get("operation") != "call" or safe.get("value") != "0":
        errors.append("AO-REDEEM same-nonce fence ABI drifted")
    if relayer.get("explicit_nonce") != "source-proven" or relayer.get("competing_same_nonce") != "unproven":
        errors.append("AO-REDEEM relayer nonce capability is overstated")
    rpc, wire = config.get("rpc", {}), manifest.get("wire", {})
    if (
        relayer.get("origin") != config.get("relayer", {}).get("origin")
        or wire.get("chain_origin") != rpc.get("origin")
        or not str(rpc.get("origin", "")).startswith("https://")
        or set(rpc)
        != {
            "origin", "path", "max_origin_bytes", "max_path_bytes", "max_response_bytes",
            "overflow_probe_bytes", "max_receipt_logs", "finality_confirmations",
        }
    ):
        errors.append("AO-REDEEM configured source identities are not exact and source-fenced")
    if manifest.get("activation") != {"primitive_enabled": False, "requires_competing_same_nonce_conformance": True, "has_active_caller": False, "has_durable_state": False}:
        errors.append("AO-REDEEM manifest is not mechanically disabled and pure")
    if config.get("enabled") is not False or config.get("provider_manifest_id") != manifest.get("manifest_id"):
        errors.append("AO-REDEEM grouped TOML is not disabled/manifest-bound")
    if "registry" in config or "max_condition_slots" in manifest.get("wire", {}):
        errors.append("AO-REDEEM provider must not own condition registry capacity")

    credentials = config.get("credentials", {})
    expected_credentials = {
        "signer_private_key_ssm_path", "builder_api_key_ssm_path", "builder_api_secret_ssm_path",
        "builder_passphrase_ssm_path", "redaction_hmac_key_ssm_path", "max_value_bytes",
        "max_acquisition_bytes", "max_path_bytes", "key_version",
    }
    if set(credentials) != expected_credentials or any(not credentials[field].startswith("/bolt/") for field in expected_credentials if field.endswith("_ssm_path")):
        errors.append("AO-REDEEM credentials must use exact grouped SSM-only capacity schema")
    boundary = manifest.get("credential_boundary", {})
    if boundary.get("source") != "aws-ssm-capped-sink" or not isinstance(boundary.get("max_acquisition_bytes"), int) or credentials.get("max_acquisition_bytes", 0) > boundary.get("max_acquisition_bytes", 0):
        errors.append("AO-REDEEM credential acquisition capacity is not source-owned and manifest-bounded")
    config_source = (root / REDEMPTION_ROOT / "config.rs").read_text(encoding="utf-8")
    if "CappedSsmCredentialSource" not in config_source or "CredentialSink" not in config_source or "SsmSecretResolver" in config_source or "Zeroizing<String>" in config_source:
        errors.append("AO-REDEEM credential acquisition is not sealed, source-owned, capped, and zeroizing")

    fixtures = {
        "ctf-collateral-adapter.txt": ("function redeemPositions(address, bytes32, bytes32 _conditionId, uint256[] calldata)", "CTFHelpers.partition()"),
        "negative-risk-collateral-adapter.txt": ("function _redeemPositions(bytes32 _conditionId, uint256[] memory)", "INegRiskAdapter(NEG_RISK_ADAPTER).redeemPositions(_conditionId, amounts)"),
        "relayer-safe-builder.txt": ("nonce: string", "nonce: args.nonce", "STATE_CONFIRMED", "transactionID", "proxyAddress"),
    }
    digest_fields = {"ctf-collateral-adapter.txt": "standard_sha256", "negative-risk-collateral-adapter.txt": "negative_risk_sha256", "relayer-safe-builder.txt": "relayer_sha256"}
    for name, needles in fixtures.items():
        path = root / "tests/fixtures/bolt_v3/redeem/source" / name
        text = path.read_text(encoding="utf-8")
        for needle in needles:
            if needle not in text:
                errors.append(f"AO-REDEEM source fixture {name} lost {needle!r}")
        if manifest.get("source_snapshots", {}).get(digest_fields[name]) != hashlib.sha256(path.read_bytes()).hexdigest():
            errors.append(f"AO-REDEEM source snapshot digest drifted: {name}")

    wire_source = (root / REDEMPTION_ROOT / "wire.rs").read_text(encoding="utf-8")
    if "RelayerSourceResponse" not in wire_source or "FinalizedChainSourceResponse" not in wire_source:
        errors.append("AO-REDEEM source-specific response capabilities are missing")
    for raw_type in ("NonceCallWire", "ExecutionQueryWire", "CanonicalBlockWire", "ReceiptWire", "ExecutionLogWire", "PostStateWire", "SafeBoundaryWire", "FinalizedHeadWire"):
        if not re.search(rf"#\[serde\(deny_unknown_fields\)\]\s*(?:pub\(super\)\s+)?struct {raw_type}\b", wire_source):
            errors.append(f"AO-REDEEM raw source-bound schema is not closed: {raw_type}")
    if "struct ChainWire" in wire_source or re.search(r"\b(winner|conflicting_coordinates|safe_execution_succeeded)\s*:", wire_source):
        errors.append("AO-REDEEM accepts caller-classified chain truth")
    response_set = re.search(r"pub struct ExactQueryResponses\s*{([^}]*)}", wire_source, re.S)
    expected_response_fields = {
        "nonce", "original_execution", "fence_execution", "post_state", "safe_boundary",
        "finalized_head",
    }
    actual_response_fields = set(re.findall(r"^\s*(\w+)\s*:", response_set.group(1), re.M)) if response_set else set()
    if actual_response_fields != expected_response_fields:
        errors.append("AO-REDEEM exact raw response set is partial or open")
    if "required_confirmations" not in wire_source or "confirmed_at" not in wire_source:
        errors.append("AO-REDEEM terminal outcomes omit configured finality")
    post_state = re.search(r"struct PostStateWire<'a>\s*{([^}]*)}", wire_source, re.S)
    required_post_fields = {
        "query_id", "target", "condition_id", "collateral", "output_asset", "account",
        "block_number", "block_hash", "claim_results", "collateral_balance",
    }
    actual_post_fields = set(re.findall(r"^\s*(\w+)\s*:", post_state.group(1), re.M)) if post_state else set()
    if actual_post_fields != required_post_fields:
        errors.append("AO-REDEEM post-state balance contract is incomplete")

    errors.extend(verify_structural_reachability(root))
    errors.extend(verify_capabilities(root))
    errors.extend(verify_opaque_surfaces(root))
    tests = "\n".join((root / path).read_text(encoding="utf-8") for path in (REDEMPTION_ROOT / "tests.rs", pathlib.Path("tests/bolt_v3_redeem_primitive.rs")))
    required_tests = (
        "standard_and_negative_risk_fixtures", "original_and_fence_body_boundaries",
        "response_loss_requires_exact_queries", "original_wins_only_with_finalized_post_state",
        "exact_relayer_record_binds_every_source_field",
        "fence_wins_only_with_unchanged_post_state", "unrelated_nonce_fails_closed",
        "relayer_states_never_prove_terminal_effect", "retry_requires_exact_body_bytes",
        "stale_pre_send_token_is_rejected", "fence_first_is_unrepresentable_and_mismatched_fence_is_rejected",
        "pre_send_balance_and_lease_revalidation_fails_closed",
        "concurrent_conditions_cannot_share_one_nonce_permit", "full_width_nonce_domain_and_maximum_are_deterministic",
        "capped_reader_honors_limit_minus_one_limit_and_limit_plus_one",
        "oversized_credential_acquisition_is_rejected_before_append",
        "raw_queries_reject_duplicate_missing_conflicting_and_fabricated_fields",
        "sentinel_values_never_appear_in_redacted_projections", "primitive_is_mechanically_disabled",
        "sentinels_do_not_reach_redacted_diagnostics",
        "fabricated_reader_has_no_production_proof_path",
        "cross_action_outcome_reuse_is_rejected",
        "profile_key_source_and_finalized_bindings_fail_closed",
        "standard_negative_risk_original_and_fence_post_state_fixtures",
        "zero_and_dust_collateral_balances_are_exact",
        "wrong_output_and_post_state_drift_fail_closed",
        "swapped_or_replayed_post_state_source_fails_closed",
        "consistent_dummy_index_set_mutation_is_not_replaced",
        "old_prepared_new_profile_key_and_source_fail_closed",
        "swapped_query_capabilities_are_rejected_before_parsing",
    )
    for name in required_tests:
        if f"fn {name}" not in tests:
            errors.append(f"AO-REDEEM required behavior test missing: {name}")
    if 'name = "bolt_v3_redeem_primitive"' not in (root / "Cargo.toml").read_text(encoding="utf-8"):
        errors.append("AO-REDEEM behavior harness is not registered")
    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = verify(root)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("AO-REDEEM source fence passed")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
