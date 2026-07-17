#!/usr/bin/env python3
"""Fence the disabled, lease-bound Polymarket redemption preparation surface."""

from __future__ import annotations

import hashlib
import pathlib
import re
import subprocess
import sys
import tomllib


OWNER = pathlib.Path("src/bolt_v3_polymarket_redemption.rs")
GENERATED = pathlib.Path("src/bolt_v3_polymarket_redemption/generated.rs")
RUNTIME = pathlib.Path("config/polymarket-redemption.toml")
ROOT_RUNTIME = pathlib.Path("config/root.toml")
EVIDENCE = pathlib.Path("config/polymarket-redemption-source-evidence.toml")
COMPILE_FAIL = pathlib.Path("tests/polymarket_redemption_preparation_compile_fail.rs")
GENERATOR = pathlib.Path("scripts/generate_polymarket_redemption_config.py")
RUNTIME_AUTHORITY_KEYS = frozenset(
    {
        "standard_adapter_target",
        "negative_risk_adapter_target",
        "builder_api_key_ssm_path",
        "builder_api_secret_ssm_path",
        "builder_passphrase_ssm_path",
    }
)
EXPECTED_EVIDENCE = {
    "adapter_repository": "https://github.com/Polymarket/ctf-exchange-v2",
    "adapter_revision": "ccc0596074f4dfd62c944fbca4de252893b82b4b",
    "deployment_source_url": "https://docs.polymarket.com/resources/contracts",
    "deployment_observed_date": "2026-07-16",
    "deployment_fact_format_version": 2,
    "deployment_fact_sha256": "7844264e5c6c456224820af716c000438d72736a5f45315ae88f4f92dc068667",
    "standard_source_path": "src/adapters/CtfCollateralAdapter.sol",
    "standard_source_sha256": "f9f85b1ac652030bf458be2130b5f977fa6670a04b2ad412241c9e9b0c444a90",
    "negative_risk_source_path": "src/adapters/NegRiskCtfCollateralAdapter.sol",
    "negative_risk_source_sha256": "2461eb793fa5571a6902a52c5276f02a8621814fdc026cf3a7814879b1b3db76",
    "function_signature": "redeemPositions(address,bytes32,bytes32,uint256[])",
    "function_selector": "0x01b7037c",
    "request_repository": "https://github.com/Polymarket/builder-relayer-client",
    "request_revision": "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290",
    "builder_source_path": "src/builder/safe.ts",
    "builder_source_sha256": "1142cb7fe786128361586d6fc9313a3e120e1633bdfc064169bfa78951d66cc5",
    "types_source_path": "src/types.ts",
    "types_source_sha256": "059c02b19a23d57e7b354df8c01d706cf508c27460067c1d57dad96cf5455ad3",
    "signature_pack_source_path": "src/utils/index.ts",
    "signature_pack_source_sha256": "0a1b6036fb7e3f7d1629002a491a448974a69c7556741f449c441cb3e3af2941",
}


def _read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def _raw_string_end(text: str, start: int) -> int | None:
    cursor = start
    if text.startswith(("br", "cr"), cursor):
        cursor += 2
    elif text.startswith("r", cursor):
        cursor += 1
    else:
        return None
    hashes = 0
    while cursor < len(text) and text[cursor] == "#":
        hashes += 1
        cursor += 1
    if cursor >= len(text) or text[cursor] != '"':
        return None
    delimiter = '"' + "#" * hashes
    end = text.find(delimiter, cursor + 1)
    return None if end < 0 else end + len(delimiter)


def _quoted_end(text: str, start: int, quote: str) -> int | None:
    cursor = start + 1
    while cursor < len(text):
        if text[cursor] == "\\":
            cursor += 2
            continue
        if text[cursor] == quote:
            return cursor + 1
        cursor += 1
    return None


def _char_literal_end(text: str, start: int) -> int | None:
    if start + 1 >= len(text):
        return None
    if text[start + 1] == "\\":
        return _quoted_end(text, start, "'")
    if start + 2 < len(text) and text[start + 2] == "'":
        return start + 3
    return None


def _block_comment_end(text: str, start: int) -> int | None:
    depth = 1
    cursor = start + 2
    while cursor < len(text) and depth:
        if text.startswith("/*", cursor):
            depth += 1
            cursor += 2
        elif text.startswith("*/", cursor):
            depth -= 1
            cursor += 2
        else:
            cursor += 1
    return cursor if depth == 0 else None


def _rust_brace_end(text: str, opening: int) -> int | None:
    if opening >= len(text) or text[opening] != "{":
        return None
    depth = 0
    cursor = opening
    while cursor < len(text):
        if text.startswith("//", cursor):
            newline = text.find("\n", cursor + 2)
            cursor = len(text) if newline < 0 else newline + 1
            continue
        if text.startswith("/*", cursor):
            comment_end = _block_comment_end(text, cursor)
            if comment_end is None:
                return None
            cursor = comment_end
            continue
        raw_end = _raw_string_end(text, cursor)
        if raw_end is not None:
            cursor = raw_end
            continue
        if text[cursor] == '"':
            quoted_end = _quoted_end(text, cursor, '"')
            if quoted_end is None:
                return None
            cursor = quoted_end
            continue
        if text[cursor] == "'":
            char_end = _char_literal_end(text, cursor)
            if char_end is not None:
                cursor = char_end
                continue
        if text[cursor] == "{":
            depth += 1
        elif text[cursor] == "}":
            depth -= 1
            if depth == 0:
                return cursor + 1
        cursor += 1
    return None


def _rust_code_projection(text: str) -> str:
    projected = list(text)

    def blank(start: int, end: int) -> None:
        projected[start:end] = " " * (end - start)

    cursor = 0
    while cursor < len(text):
        if text.startswith("//", cursor):
            newline = text.find("\n", cursor + 2)
            end = len(text) if newline < 0 else newline
            blank(cursor, end)
            cursor = end
            continue
        if text.startswith("/*", cursor):
            comment_end = _block_comment_end(text, cursor)
            if comment_end is None:
                raise ValueError("unterminated Rust block comment")
            blank(cursor, comment_end)
            cursor = comment_end
            continue
        raw = re.match(r'(?:br|cr|r)(?P<hashes>#+)?"', text[cursor:])
        if raw is not None:
            delimiter = '"' + (raw.group("hashes") or "")
            closing = text.find(delimiter, cursor + raw.end())
            if closing < 0:
                raise ValueError("unterminated Rust raw string")
            raw_end = closing + len(delimiter)
            blank(cursor, raw_end)
            cursor = raw_end
            continue
        if text[cursor] == '"':
            quoted_end = _quoted_end(text, cursor, '"')
            if quoted_end is None:
                raise ValueError("unterminated Rust string")
            blank(cursor, quoted_end)
            cursor = quoted_end
            continue
        if text[cursor] == "'":
            char_end = _char_literal_end(text, cursor)
            if char_end is not None:
                blank(cursor, char_end)
                cursor = char_end
                continue
        cursor += 1
    return "".join(projected)


def _rust_code_contains(text: str, needle: str) -> bool:
    return needle in _rust_code_projection(text)


def _production_owner(text: str) -> tuple[str, str | None]:
    marker = "#[cfg(test)]"
    try:
        code = _rust_code_projection(text)
    except ValueError as error:
        return text, f"cannot lex owner test boundary: {error}"
    if code.count(marker) != 1:
        return text, "owner must contain one final cfg(test) module for active production caller proof"
    start = code.index(marker)
    module = re.match(r"#\[cfg\(test\)\]\s*mod\s+tests\s*\{", code[start:])
    if module is None:
        return text, "owner cfg(test) item must be the final tests module for active production caller proof"
    opening = start + module.end() - 1
    end = _rust_brace_end(text, opening)
    if end is None or text[end:].strip():
        return text[:start], "owner tests module must be the final item for active production caller proof"
    return text[:start], None


def _toml(path: pathlib.Path) -> dict[str, object]:
    return tomllib.loads(_read(path))


def _repository_toml(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not ignored.intersection(path.relative_to(root).parts)
    )


def _repository_rust_sources(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.rs")
        if not ignored.intersection(path.relative_to(root).parts)
    )


def _prepare_signature(text: str) -> str:
    start = text.find("pub fn prepare_redemption_request(")
    if start < 0:
        return ""
    body = text.find("{", start)
    return text[start:body] if body >= 0 else text[start:]


def _prepare_function(text: str) -> str:
    start = text.find("pub fn prepare_redemption_request(")
    if start < 0:
        return ""
    opening = text.find("{", start)
    if opening < 0:
        return text[start:]
    end = _rust_brace_end(text, opening)
    return text[start:] if end is None else text[start:end]


def _deployment_fact_sha256(
    source_url: object,
    observed_date: object,
    chain_id: object,
    collateral_asset: object,
    output_asset: object,
    standard_target: object,
    negative_risk_target: object,
    parent_collection_id: object,
    dummy_index_sets: object,
) -> str:
    normalized_dummy_index_sets = (
        ",".join(str(value) for value in dummy_index_sets)
        if isinstance(dummy_index_sets, list)
        else str(dummy_index_sets)
    )
    payload = (
        f"source_url={source_url}\n"
        f"observed_date={observed_date}\n"
        f"chain_id={chain_id}\n"
        f"collateral_asset={str(collateral_asset).lower()}\n"
        f"output_asset={str(output_asset).lower()}\n"
        f"CtfCollateralAdapter={str(standard_target).lower()}\n"
        f"NegRiskCtfCollateralAdapter={str(negative_risk_target).lower()}\n"
        f"parent_collection_id={str(parent_collection_id).lower()}\n"
        f"dummy_index_sets={normalized_dummy_index_sets}\n"
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def boundary_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    required = [
        OWNER,
        GENERATED,
        RUNTIME,
        ROOT_RUNTIME,
        EVIDENCE,
        COMPILE_FAIL,
        pathlib.Path("Cargo.toml"),
    ]
    missing = [str(path) for path in required if not (root / path).is_file()]
    if missing:
        return [f"missing required redemption preparation artifact(s): {missing}"]

    try:
        owner_text = _read(root / OWNER)
        generated_text = _read(root / GENERATED)
        runtime_text = _read(root / RUNTIME)
        runtime = _toml(root / RUNTIME)
        evidence = _toml(root / EVIDENCE)
        compile_fail_text = _read(root / COMPILE_FAIL)
        cargo_text = _read(root / "Cargo.toml")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        return [f"cannot inspect redemption preparation artifacts: {error}"]

    production, owner_structure_error = _production_owner(owner_text)
    if owner_structure_error is not None:
        errors.append(owner_structure_error)
    signature = _prepare_signature(production)
    prepare_function = _prepare_function(production)
    try:
        production_code = _rust_code_projection(production)
        prepare_code = _rust_code_projection(prepare_function)
    except ValueError as error:
        errors.append(f"cannot lex production owner: {error}")
        production_code = ""
        prepare_code = ""

    authorities: dict[str, list[pathlib.Path]] = {key: [] for key in RUNTIME_AUTHORITY_KEYS}
    for path in _repository_toml(root):
        try:
            text = _read(path)
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect TOML authority {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key in RUNTIME_AUTHORITY_KEYS:
            if re.search(rf"(?m)^\s*{re.escape(key)}\s*=", text):
                authorities[key].append(relative)
    for key, paths in authorities.items():
        if paths != [RUNTIME]:
            errors.append(
                f"runtime field {key} must have one TOML authority at {RUNTIME}; found {paths}"
            )

    if runtime.get("production_activation_enabled") is not False:
        errors.append("production_activation_enabled must remain false")
    wallet_authority = runtime.get("wallet_authority")
    redemption = runtime.get("redemption")
    if not isinstance(wallet_authority, dict) or not isinstance(redemption, dict):
        errors.append("runtime config must contain wallet_authority and redemption tables")
    elif not isinstance(wallet_authority.get("root_client"), str):
        errors.append("wallet_authority.root_client must select a root config client")
    duplicated_wallet_fields = sorted(
        field
        for field in ("aws_region", "safe_address", "signer_private_key_ssm_path")
        if re.search(rf"(?m)^\s*{re.escape(field)}\s*=", runtime_text)
    )
    if duplicated_wallet_fields:
        errors.append(
            "redemption wallet and signer fields must remain single-sourced from config/root.toml: "
            f"{duplicated_wallet_fields}"
        )
    evidence_text = _read(root / EVIDENCE)
    duplicated_keys = sorted(
        key
        for key in RUNTIME_AUTHORITY_KEYS
        if re.search(rf"(?m)^\s*{re.escape(key)}\s*=", evidence_text)
    )
    if duplicated_keys:
        errors.append(f"source evidence duplicates runtime TOML keys: {duplicated_keys}")

    adapter = evidence.get("adapter_abi")
    safe_request = evidence.get("safe_request")
    if not isinstance(adapter, dict) or not isinstance(safe_request, dict):
        errors.append("source evidence must contain adapter_abi and safe_request tables")
    else:
        observed = {
            "adapter_repository": adapter.get("repository"),
            "adapter_revision": adapter.get("revision"),
            "deployment_source_url": adapter.get("deployment_source_url"),
            "deployment_observed_date": adapter.get("deployment_observed_date"),
            "deployment_fact_format_version": adapter.get(
                "deployment_fact_format_version"
            ),
            "deployment_fact_sha256": adapter.get("deployment_fact_sha256"),
            "standard_source_path": adapter.get("standard_source_path"),
            "standard_source_sha256": adapter.get("standard_source_sha256"),
            "negative_risk_source_path": adapter.get("negative_risk_source_path"),
            "negative_risk_source_sha256": adapter.get("negative_risk_source_sha256"),
            "function_signature": adapter.get("function_signature"),
            "function_selector": adapter.get("function_selector"),
            "request_repository": safe_request.get("repository"),
            "request_revision": safe_request.get("revision"),
            "builder_source_path": safe_request.get("builder_source_path"),
            "builder_source_sha256": safe_request.get("builder_source_sha256"),
            "types_source_path": safe_request.get("types_source_path"),
            "types_source_sha256": safe_request.get("types_source_sha256"),
            "signature_pack_source_path": safe_request.get("signature_pack_source_path"),
            "signature_pack_source_sha256": safe_request.get("signature_pack_source_sha256"),
        }
        if observed != EXPECTED_EVIDENCE:
            errors.append(
                "source evidence must remain pinned to the reviewed adapter/request revisions and ABI: "
                f"{observed}"
            )
        if isinstance(redemption, dict) and adapter.get(
            "deployment_fact_sha256"
        ) != _deployment_fact_sha256(
            adapter.get("deployment_source_url"),
            adapter.get("deployment_observed_date"),
            redemption.get("chain_id"),
            redemption.get("collateral_asset"),
            redemption.get("output_asset"),
            redemption.get("standard_adapter_target"),
            redemption.get("negative_risk_adapter_target"),
            redemption.get("parent_collection_id"),
            redemption.get("dummy_index_sets"),
        ):
            errors.append(
                "deployment fact hash must bind the source observation to normalized runtime protocol facts"
            )

    if "pub enum AttemptKind" not in production or not all(
        variant in production for variant in ("Original", "Fence")
    ):
        errors.append("AttemptKind must expose Original and Fence")
    if "lease: &mut RiskClosureWorkspaceLease" not in signature:
        errors.append("request preparation must accept &mut RiskClosureWorkspaceLease")
    if "impl for<'request> FnOnce(PreparedRequest<'request>)" not in signature:
        errors.append("request bytes must be exposed through a callback-scoped PreparedRequest")
    if "&mut [u8]" in signature:
        errors.append("public request preparation must not accept a caller-provided raw buffer")
    if re.search(r"lease\s*\.\s*with_workspace_mut", production) is None:
        errors.append("request encoding must occur inside lease.with_workspace_mut")

    prepared_match = re.search(
        r"pub\s+struct\s+PreparedRequest<'request>\s*\{(?P<body>.*?)\}",
        production,
        flags=re.DOTALL,
    )
    if prepared_match is None:
        errors.append("PreparedRequest<'request> must be declared")
    else:
        body = prepared_match.group("body")
        fields = re.findall(
            r"\b[A-Za-z_][A-Za-z0-9_]*\s*:\s*([^,\n}]+)(?:,|$)",
            body,
            re.MULTILINE,
        )
        field_types = {field_type.strip() for field_type in fields}
        allowed_field_types = {
            "&'request [u8]",
            "PreparedRequestIdentity",
            "U256",
            "Address",
        }
        if (
            not fields
            or "&'request [u8]" not in field_types
            or not field_types.issubset(allowed_field_types)
        ):
            errors.append("PreparedRequest bytes must remain a borrowed slice without owned storage")

    forbidden_authority = (
        "RiskClosureWorkspaceAuthority",
        "RiskClosureWorkspaceReservation",
        "checkout_new_risk",
        "TerminalReleasePermit",
        "release_terminal",
    )
    if any(token in production for token in forbidden_authority):
        errors.append("production owner contains a forbidden new-risk or authority surface")

    forbidden_geometry = (
        "arena_bytes",
        "slot_bytes",
        "workspace_len",
        "SLOT_BYTES",
        "ARENA_BYTES",
        "WORKSPACE_CAPACITY",
    )
    if any(token in production for token in forbidden_geometry):
        errors.append("production owner contains copied or independent workspace geometry")
    if any(token in generated_text for token in ("arena_bytes", "slot_bytes", "capacity: 10")):
        errors.append("generated redemption config must not copy #1430 workspace geometry")

    forbidden_sinks = (
        "reqwest",
        "HttpClient",
        "TcpStream",
        "UdpSocket",
        "SocketAddr",
        "std::fs",
        "OpenOptions",
        "tokio::",
        "async fn",
    )
    if any(token in production for token in forbidden_sinks):
        errors.append("production owner contains a network or durable sink")
    if "SsmResolverSession::new" in production:
        errors.append("production owner constructs a forbidden second SSM session")
    forbidden_secret_backends = (
        "std::env",
        "Command::new",
        "aws ssm",
        "op item",
        "1password",
    )
    if any(token.lower() in production.lower() for token in forbidden_secret_backends):
        errors.append("production owner contains an alternate secret backend")
    if "SsmResolverSession" not in production or re.search(
        r"session\s*\.\s*resolve\s*\(", production
    ) is None:
        errors.append("production owner must resolve credentials through the supplied SsmResolverSession")
    if re.search(
        r"pub(?:\([^)]*\))?\s+fn\s+resolve_redemption_credentials_(?:from|with)\b",
        production,
    ):
        errors.append("production owner must not expose an injectable secret resolver")
    forbidden_signer_parsers = (
        "PrivateKeySigner::from_str",
        ".parse::<PrivateKeySigner",
        "FromStr>::from_str",
        "FromStr::from_str",
    )
    if any(token in production_code for token in forbidden_signer_parsers):
        errors.append("production owner must use a zeroizing signer-key decode buffer")
    signer_decode_markers = (
        "let mut signer_private_key = Zeroizing::new(B256::ZERO.into_array());",
        "hex::decode_to_slice(",
        "credentials.signer_private_key.as_bytes(),",
        "signer_private_key.as_mut(),",
        "PrivateKeySigner::from_slice(signer_private_key.as_ref())",
    )
    signer_decode_positions = [prepare_code.find(marker) for marker in signer_decode_markers]
    if (
        any(position < 0 for position in signer_decode_positions)
        or signer_decode_positions != sorted(signer_decode_positions)
        or any(prepare_code.count(marker) != 1 for marker in signer_decode_markers)
    ):
        errors.append("production owner must preserve the zeroizing signer-key decode path")

    forbidden_observability = (
        "dbg!",
        "print!",
        "eprint!",
        "println!",
        "eprintln!",
        "log::",
        "tracing::",
        "info!",
        "debug!",
        "warn!",
        "error!",
    )
    if any(token in production for token in forbidden_observability):
        errors.append("production owner contains a forbidden logging or observability sink")
    if re.search(r"derive\([^)]*Serialize", production):
        errors.append("prepared requests and resolved credentials must not derive serialization")

    if re.search(r"pub\(super\)\s+const\s+POLYMARKET_REDEMPTION", generated_text) is None:
        errors.append("generated runtime and protocol projections must use module-private visibility")
    if "pub const POLYMARKET_REDEMPTION" in generated_text:
        errors.append("generated runtime and protocol projections must remain private")
    if "POLYMARKET_REDEMPTION_PREPARATION_CONFIG" not in generated_text:
        errors.append("generated runtime projection is missing")
    if "POLYMARKET_REDEMPTION_PROTOCOL" not in generated_text:
        errors.append("generated protocol projection is missing")

    for dependency in ('alloy-signer = "=2.1.0"', 'alloy-signer-local = "=2.1.0"'):
        if dependency not in cargo_text:
            errors.append(f"direct signer dependency must remain exact and locked: {dependency}")
    if (
        'name = "polymarket_redemption_preparation"' not in cargo_text
        or 'path = "tests/polymarket_redemption_preparation.rs"' not in cargo_text
    ):
        errors.append("compile-fail test target is not wired")
    for marker in (
        "RiskClosureWorkspaceReservation",
        "prepared_request_cannot_escape",
        "serde_json::to_string",
    ):
        if marker not in compile_fail_text:
            errors.append(f"compile-fail proof is missing marker {marker}")

    if production_code.count("prepare_redemption_request") > 1:
        errors.append("active production caller found inside disabled module")

    for path in _repository_rust_sources(root):
        relative = path.relative_to(root)
        if relative == OWNER:
            continue
        try:
            text = _read(path)
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"cannot inspect production caller surface {relative}: {error}")
            continue
        if "prepare_redemption_request" not in text:
            continue
        try:
            has_caller = _rust_code_contains(text, "prepare_redemption_request")
        except ValueError as error:
            errors.append(f"cannot lex production caller surface {relative}: {error}")
            continue
        if has_caller:
            errors.append(f"active production caller found outside disabled module: {relative}")

    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = boundary_errors(root)
    generation = subprocess.run(
        [
            sys.executable,
            str(root / GENERATOR),
            "--runtime-source",
            str(root / RUNTIME),
            "--evidence-source",
            str(root / EVIDENCE),
            "--root-source",
            str(root / ROOT_RUNTIME),
            "--output",
            str(root / GENERATED),
            "--check",
        ],
        cwd=root,
        text=True,
        capture_output=True,
        check=False,
    )
    if generation.returncode != 0:
        errors.append(generation.stderr.strip() or "generated redemption config is stale")
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: Polymarket redemption preparation remains disabled and lease-bound.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
