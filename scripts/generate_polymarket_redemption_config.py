#!/usr/bin/env python3
"""Generate private Rust redemption request configuration from strict TOML."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import pathlib
import sys
import tomllib
import urllib.parse
from datetime import date

from ethereum_keccak import keccak_256


HEX_DIGITS = frozenset("0123456789abcdefABCDEF")
LOWER_HEX_DIGITS = frozenset("0123456789abcdef")
MAX_U256 = (1 << 256) - 1


class ConfigError(ValueError):
    """The TOML sources cannot produce a closed request-preparation config."""


@dataclasses.dataclass(frozen=True)
class RuntimeConfig:
    schema_version: int
    production_activation_enabled: bool
    chain_id: int
    wallet_type: str
    safe_address: str
    collateral_asset: str
    standard_adapter_target: str
    negative_risk_adapter_target: str
    parent_collection_id: str
    dummy_index_sets: tuple[int, int]
    maximum_safe_nonce_decimal_digits: int


@dataclasses.dataclass(frozen=True)
class ProtocolEvidence:
    function_selector: tuple[int, int, int, int]
    operation: int
    value: int
    safe_tx_gas: int
    base_gas: int
    gas_price: int
    gas_token: str
    refund_receiver: str
    metadata: str


@dataclasses.dataclass(frozen=True)
class ConfigProjection:
    runtime: RuntimeConfig
    evidence: ProtocolEvidence


def _exact_keys(table: dict[str, object], expected: set[str], context: str) -> None:
    unknown = sorted(set(table) - expected)
    missing = sorted(expected - set(table))
    if unknown:
        raise ConfigError(f"{context}: unknown field(s): {', '.join(unknown)}")
    if missing:
        raise ConfigError(f"{context}: missing field(s): {', '.join(missing)}")


def _table(value: object, field: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ConfigError(f"{field} must be a table")
    return value


def _string(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or value.strip() != value:
        raise ConfigError(f"{field} must be a non-empty trimmed string")
    return value


def _integer(value: object, field: str, *, minimum: int, maximum: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < minimum
        or value > maximum
    ):
        raise ConfigError(f"{field} must be an integer in [{minimum}, {maximum}]")
    return value


def _schema_version(document: dict[str, object], field: str) -> int:
    return _integer(document[field], field, minimum=1, maximum=1)


def _is_exact_hex(text: str, digits: int, alphabet: frozenset[str]) -> bool:
    return len(text) == digits and all(character in alphabet for character in text)


def _address(value: object, field: str, *, allow_zero: bool = False) -> str:
    text = _string(value, field)
    if not text.startswith("0x") or not _is_exact_hex(text[2:], 40, HEX_DIGITS):
        raise ConfigError(f"{field} must be a 20-byte 0x-prefixed EVM address")
    normalized = text.lower()
    if not allow_zero and normalized == "0x" + ("0" * 40):
        raise ConfigError(f"{field} must not be the zero address")
    return normalized


def _bytes32(value: object, field: str) -> str:
    text = _string(value, field)
    if not text.startswith("0x") or not _is_exact_hex(text[2:], 64, HEX_DIGITS):
        raise ConfigError(f"{field} must be a 32-byte 0x-prefixed value")
    return text.lower()


def _u256(value: object, field: str) -> int:
    text = _string(value, field)
    if not text.isascii() or not text.isdigit():
        raise ConfigError(f"{field} must be a canonical uint256 decimal string")
    parsed = int(text, 10)
    if parsed < 0 or parsed > MAX_U256:
        raise ConfigError(f"{field} must fit uint256")
    if text != str(parsed):
        raise ConfigError(f"{field} must be a canonical uint256 decimal string")
    return parsed


def _revision(value: object, field: str) -> str:
    text = _string(value, field)
    if not _is_exact_hex(text, 40, LOWER_HEX_DIGITS):
        raise ConfigError(f"{field} must be a 40 lowercase hexadecimal commit SHA")
    return text


def _sha256(value: object, field: str) -> str:
    text = _string(value, field)
    if not _is_exact_hex(text, 64, LOWER_HEX_DIGITS):
        raise ConfigError(f"{field} must be a 64 lowercase hexadecimal SHA-256")
    return text


def _source_path(value: object, field: str) -> str:
    text = _string(value, field)
    path = pathlib.PurePosixPath(text)
    if path.is_absolute() or ".." in path.parts or path.as_posix() != text:
        raise ConfigError(f"{field} must be a normalized repository-relative path")
    return text


def _verified_snapshot(
    evidence_path: pathlib.Path,
    relative_path: pathlib.PurePosixPath,
    expected_sha256: object,
    field: str,
) -> bytes:
    expected = _sha256(expected_sha256, f"{field}_sha256")
    evidence_root = evidence_path.parent.resolve()
    snapshot_path = (evidence_root / relative_path).resolve()
    if not snapshot_path.is_relative_to(evidence_root):
        raise ConfigError(f"derived {field} path must remain below the evidence directory")
    try:
        encoded = snapshot_path.read_text(encoding="ascii")
    except (OSError, UnicodeDecodeError) as error:
        raise ConfigError(f"cannot read derived {field} path {snapshot_path}: {error}") from error
    if (
        not encoded.endswith("\n")
        or not encoded[:-1]
        or len(encoded[:-1]) % 2 != 0
        or any(character not in "0123456789abcdef" for character in encoded[:-1])
    ):
        raise ConfigError(f"derived {field} capture must be lowercase hexadecimal plus LF")
    snapshot = bytes.fromhex(encoded[:-1])
    observed = hashlib.sha256(snapshot).hexdigest()
    if observed != expected:
        raise ConfigError(
            f"{field}_sha256 does not match captured bytes at {relative_path}"
        )
    return snapshot


def _https_snapshot_root(value: object, field: str) -> pathlib.PurePosixPath:
    source_url = _string(value, field)
    try:
        parsed = urllib.parse.urlsplit(source_url)
        port = parsed.port
    except ValueError as error:
        raise ConfigError(f"{field} must be a canonical HTTPS URL") from error
    if (
        parsed.scheme != "https"
        or not parsed.netloc
        or parsed.username is not None
        or parsed.password is not None
        or port is not None
        or parsed.query
        or parsed.fragment
    ):
        raise ConfigError(f"{field} must be a canonical HTTPS URL")
    source_path = pathlib.PurePosixPath(parsed.path.removeprefix("/"))
    if not source_path.parts:
        raise ConfigError(f"{field} must include a path")
    return pathlib.PurePosixPath(
        "polymarket-redemption-sources", parsed.netloc, *source_path.parts
    )


def _repository_snapshot_path(
    repository: object,
    revision: object,
    source_path: object,
    field: str,
) -> pathlib.PurePosixPath:
    root = _https_snapshot_root(repository, f"{field}.repository")
    pinned_revision = _revision(revision, f"{field}.revision")
    pinned_source_path = _source_path(source_path, f"{field}.source_path")
    return root / pinned_revision / f"{pinned_source_path}.hex"


def _deployment_snapshot_path(
    source_url: object, observed_date: object
) -> pathlib.PurePosixPath:
    root = _https_snapshot_root(source_url, "adapter_abi.deployment_source_url")
    observed = _string(observed_date, "adapter_abi.deployment_observed_date")
    try:
        date.fromisoformat(observed)
    except ValueError as error:
        raise ConfigError(
            "adapter_abi.deployment_observed_date must be an ISO calendar date"
        ) from error
    return pathlib.PurePosixPath(
        "polymarket-redemption-sources",
        root.parts[1],
        observed,
        *root.parts[2:-1],
        f"{root.name}.md.hex",
    )


def _markdown_contract_address(snapshot: str, contract: str) -> str:
    rows = [
        line.split("|")
        for line in snapshot.splitlines()
        if line.startswith("|")
    ]
    matching = [row for row in rows if len(row) >= 4 and row[1].strip() == contract]
    if len(matching) != 1:
        raise ConfigError(
            f"deployment snapshot must contain exactly one {contract} address row"
        )
    link = matching[0][2].strip()
    prefix, separator, remainder = link.partition("[`")
    address, closing, suffix = remainder.partition("`]")
    if prefix or separator != "[`" or closing != "`]" or not suffix.startswith("("):
        raise ConfigError(f"deployment snapshot has malformed {contract} address cell")
    return _address(address, f"deployment snapshot {contract}")


def _verify_deployment_snapshot(snapshot: bytes, runtime: RuntimeConfig) -> None:
    try:
        text = snapshot.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ConfigError("deployment snapshot must be UTF-8 Markdown") from error
    chain_marker = "(Chain ID: "
    if text.count(chain_marker) != 1:
        raise ConfigError("deployment snapshot must contain exactly one Polygon chain ID")
    chain_text = text.split(chain_marker, maxsplit=1)[1].split(")", maxsplit=1)[0]
    try:
        chain_id = int(chain_text, 10)
    except ValueError as error:
        raise ConfigError("deployment snapshot chain ID must be decimal") from error
    observed = {
        "chain_id": chain_id,
        "collateral_asset": _markdown_contract_address(
            text, "pUSD — CollateralToken (proxy)"
        ),
        "standard_adapter_target": _markdown_contract_address(
            text, "CtfCollateralAdapter"
        ),
        "negative_risk_adapter_target": _markdown_contract_address(
            text, "NegRiskCtfCollateralAdapter"
        ),
    }
    expected = {
        "chain_id": runtime.chain_id,
        "collateral_asset": runtime.collateral_asset,
        "standard_adapter_target": runtime.standard_adapter_target,
        "negative_risk_adapter_target": runtime.negative_risk_adapter_target,
    }
    if observed != expected:
        raise ConfigError(
            "deployment snapshot facts do not match normalized runtime protocol targets"
        )


def _read_toml(path: pathlib.Path) -> dict[str, object]:
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        raise ConfigError(f"cannot read {path}: {error}") from error
    if not isinstance(document, dict):
        raise ConfigError(f"{path} root must be a table")
    return document


def _load_runtime(path: pathlib.Path, root_path: pathlib.Path) -> RuntimeConfig:
    document = _read_toml(path)
    root_document = _read_toml(root_path)
    _exact_keys(
        document,
        {
            "schema_version",
            "production_activation_enabled",
            "wallet_authority",
            "redemption",
            "protocol_bounds",
        },
        "runtime root",
    )
    schema_version = _schema_version(document, "schema_version")
    activation = document["production_activation_enabled"]
    if activation is not False:
        raise ConfigError("production_activation_enabled must remain false")

    wallet_authority = _table(document["wallet_authority"], "wallet_authority")
    _exact_keys(wallet_authority, {"root_client"}, "wallet_authority")
    root_client = _string(wallet_authority["root_client"], "wallet_authority.root_client")

    clients = _table(root_document.get("clients"), "root.clients")
    client = _table(clients.get(root_client), f"root.clients.{root_client}")
    if _string(client.get("venue"), f"root.clients.{root_client}.venue") != "POLYMARKET":
        raise ConfigError("wallet_authority.root_client must select a POLYMARKET client")
    execution = _table(
        client.get("execution"), f"root.clients.{root_client}.execution"
    )
    signature_type = _string(
        execution.get("signature_type"),
        f"root.clients.{root_client}.execution.signature_type",
    )
    if signature_type != "poly_gnosis_safe":
        raise ConfigError("wallet_authority.root_client must select a poly_gnosis_safe client")
    wallet_type = signature_type.removeprefix("poly_gnosis_").upper()
    safe_address = _address(
        execution.get("funder"), f"root.clients.{root_client}.execution.funder"
    )
    redemption = _table(document["redemption"], "redemption")
    _exact_keys(
        redemption,
        {
            "chain_id",
            "collateral_asset",
            "standard_adapter_target",
            "negative_risk_adapter_target",
            "parent_collection_id",
            "dummy_index_sets",
        },
        "redemption",
    )
    chain_id = _integer(redemption["chain_id"], "redemption.chain_id", minimum=1, maximum=(1 << 64) - 1)
    dummy_values = redemption["dummy_index_sets"]
    if not isinstance(dummy_values, list) or len(dummy_values) != 2:
        raise ConfigError("redemption.dummy_index_sets must contain exactly two uint256 strings")
    dummy_index_sets = (
        _u256(dummy_values[0], "redemption.dummy_index_sets[0]"),
        _u256(dummy_values[1], "redemption.dummy_index_sets[1]"),
    )

    bounds = _table(document["protocol_bounds"], "protocol_bounds")
    _exact_keys(bounds, {"maximum_safe_nonce_decimal_digits"}, "protocol_bounds")
    maximum_digits = _integer(
        bounds["maximum_safe_nonce_decimal_digits"],
        "protocol_bounds.maximum_safe_nonce_decimal_digits",
        minimum=1,
        maximum=len(str(MAX_U256)),
    )

    return RuntimeConfig(
        schema_version=schema_version,
        production_activation_enabled=activation,
        chain_id=chain_id,
        wallet_type=wallet_type,
        safe_address=safe_address,
        collateral_asset=_address(redemption["collateral_asset"], "redemption.collateral_asset"),
        standard_adapter_target=_address(
            redemption["standard_adapter_target"], "redemption.standard_adapter_target"
        ),
        negative_risk_adapter_target=_address(
            redemption["negative_risk_adapter_target"],
            "redemption.negative_risk_adapter_target",
        ),
        parent_collection_id=_bytes32(
            redemption["parent_collection_id"], "redemption.parent_collection_id"
        ),
        dummy_index_sets=dummy_index_sets,
        maximum_safe_nonce_decimal_digits=maximum_digits,
    )


def _load_evidence(path: pathlib.Path, runtime: RuntimeConfig) -> ProtocolEvidence:
    document = _read_toml(path)
    _exact_keys(document, {"schema_version", "adapter_abi", "safe_request"}, "evidence root")
    _schema_version(document, "schema_version")

    adapter = _table(document["adapter_abi"], "adapter_abi")
    _exact_keys(
        adapter,
        {
            "repository",
            "revision",
            "deployment_source_url",
            "deployment_observed_date",
            "deployment_snapshot_sha256",
            "standard_source_path",
            "standard_snapshot_sha256",
            "negative_risk_source_path",
            "negative_risk_snapshot_sha256",
            "function_signature",
        },
        "adapter_abi",
    )
    adapter_repository = adapter["repository"]
    adapter_revision = adapter["revision"]
    deployment_snapshot_path = _deployment_snapshot_path(
        adapter["deployment_source_url"], adapter["deployment_observed_date"]
    )
    deployment_snapshot = _verified_snapshot(
        path,
        deployment_snapshot_path,
        adapter["deployment_snapshot_sha256"],
        "adapter_abi.deployment_snapshot",
    )
    _verify_deployment_snapshot(deployment_snapshot, runtime)
    _verified_snapshot(
        path,
        _repository_snapshot_path(
            adapter_repository,
            adapter_revision,
            adapter["standard_source_path"],
            "adapter_abi.standard_snapshot",
        ),
        adapter["standard_snapshot_sha256"],
        "adapter_abi.standard_snapshot",
    )
    _verified_snapshot(
        path,
        _repository_snapshot_path(
            adapter_repository,
            adapter_revision,
            adapter["negative_risk_source_path"],
            "adapter_abi.negative_risk_snapshot",
        ),
        adapter["negative_risk_snapshot_sha256"],
        "adapter_abi.negative_risk_snapshot",
    )
    function_signature = _string(
        adapter["function_signature"], "adapter_abi.function_signature"
    )
    try:
        signature_bytes = function_signature.encode("ascii")
    except UnicodeEncodeError as error:
        raise ConfigError("adapter_abi.function_signature must be ASCII") from error
    selector_bytes = keccak_256(signature_bytes)[:4]

    safe = _table(document["safe_request"], "safe_request")
    _exact_keys(
        safe,
        {
            "repository",
            "revision",
            "builder_source_path",
            "builder_snapshot_sha256",
            "types_source_path",
            "types_snapshot_sha256",
            "signature_pack_source_path",
            "signature_pack_snapshot_sha256",
            "operation",
            "value",
            "safe_tx_gas",
            "base_gas",
            "gas_price",
            "gas_token",
            "refund_receiver",
            "metadata",
        },
        "safe_request",
    )
    safe_repository = safe["repository"]
    safe_revision = safe["revision"]
    for source_name in ("builder", "types", "signature_pack"):
        _verified_snapshot(
            path,
            _repository_snapshot_path(
                safe_repository,
                safe_revision,
                safe[f"{source_name}_source_path"],
                f"safe_request.{source_name}_snapshot",
            ),
            safe[f"{source_name}_snapshot_sha256"],
            f"safe_request.{source_name}_snapshot",
        )
    operation = _integer(safe["operation"], "safe_request.operation", minimum=0, maximum=0)
    zero_fields: dict[str, int] = {}
    for key in ("value", "safe_tx_gas", "base_gas", "gas_price"):
        parsed = _u256(safe[key], f"safe_request.{key}")
        if parsed != 0:
            raise ConfigError(f"safe_request.{key} must remain zero")
        zero_fields[key] = parsed
    metadata = safe["metadata"]
    if metadata != "":
        raise ConfigError("safe_request.metadata must remain empty")
    zero_address = "0x" + ("0" * 40)
    gas_token = _address(safe["gas_token"], "safe_request.gas_token", allow_zero=True)
    refund_receiver = _address(
        safe["refund_receiver"], "safe_request.refund_receiver", allow_zero=True
    )
    if gas_token != zero_address:
        raise ConfigError("safe_request.gas_token must remain the zero address")
    if refund_receiver != zero_address:
        raise ConfigError("safe_request.refund_receiver must remain the zero address")

    return ProtocolEvidence(
        function_selector=tuple(selector_bytes),
        operation=operation,
        value=zero_fields["value"],
        safe_tx_gas=zero_fields["safe_tx_gas"],
        base_gas=zero_fields["base_gas"],
        gas_price=zero_fields["gas_price"],
        gas_token=gas_token,
        refund_receiver=refund_receiver,
        metadata=metadata,
    )


def load_config(
    runtime_path: pathlib.Path,
    evidence_path: pathlib.Path,
    root_path: pathlib.Path,
) -> ConfigProjection:
    runtime = _load_runtime(runtime_path, root_path)
    return ConfigProjection(
        runtime=runtime,
        evidence=_load_evidence(evidence_path, runtime),
    )


def _rust_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=True)


def _u256_limbs(value: int) -> str:
    limbs = [(value >> (64 * index)) & ((1 << 64) - 1) for index in range(4)]
    return f"U256::from_limbs([{', '.join(str(limb) for limb in limbs)}])"


def render_rust(
    config: ConfigProjection,
    runtime_name: str,
    evidence_name: str,
    root_name: str,
) -> str:
    runtime = config.runtime
    evidence = config.evidence
    selector = ", ".join(str(byte) for byte in evidence.function_selector)
    dummy = ",\n            ".join(_u256_limbs(value) for value in runtime.dummy_index_sets)
    return f"""// @generated by scripts/generate_polymarket_redemption_config.py from {runtime_name}, {evidence_name}, and {root_name}.
// Do not edit this file directly.

use alloy_primitives::U256;

use super::{{RedemptionPreparationConfig, RedemptionProtocolFacts}};

pub(super) const POLYMARKET_REDEMPTION_PREPARATION_CONFIG: RedemptionPreparationConfig =
    RedemptionPreparationConfig {{
        schema_version: {runtime.schema_version},
        production_activation_enabled: {str(runtime.production_activation_enabled).lower()},
        chain_id: {runtime.chain_id},
        wallet_type: {_rust_string(runtime.wallet_type)},
        safe_address: alloy_primitives::address!({_rust_string(runtime.safe_address)}),
        collateral_asset: alloy_primitives::address!({_rust_string(runtime.collateral_asset)}),
        standard_adapter_target: alloy_primitives::address!(
            {_rust_string(runtime.standard_adapter_target)}
        ),
        negative_risk_adapter_target: alloy_primitives::address!(
            {_rust_string(runtime.negative_risk_adapter_target)}
        ),
        parent_collection_id: alloy_primitives::b256!(
            {_rust_string(runtime.parent_collection_id)}
        ),
        dummy_index_sets: [
            {dummy},
        ],
        maximum_safe_nonce_decimal_digits: {runtime.maximum_safe_nonce_decimal_digits},
    }};

pub(super) const POLYMARKET_REDEMPTION_PROTOCOL: RedemptionProtocolFacts =
    RedemptionProtocolFacts {{
        function_selector: [{selector}],
        operation: {evidence.operation},
        value: {_u256_limbs(evidence.value)},
        safe_tx_gas: {_u256_limbs(evidence.safe_tx_gas)},
        base_gas: {_u256_limbs(evidence.base_gas)},
        gas_price: {_u256_limbs(evidence.gas_price)},
        gas_token: alloy_primitives::address!({_rust_string(evidence.gas_token)}),
        refund_receiver: alloy_primitives::address!({_rust_string(evidence.refund_receiver)}),
        metadata: {_rust_string(evidence.metadata)},
    }};
"""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime-source", type=pathlib.Path, required=True)
    parser.add_argument("--evidence-source", type=pathlib.Path, required=True)
    parser.add_argument("--root-source", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args(argv)
    try:
        rendered = render_rust(
            load_config(
                arguments.runtime_source,
                arguments.evidence_source,
                arguments.root_source,
            ),
            arguments.runtime_source.name,
            arguments.evidence_source.name,
            arguments.root_source.name,
        )
        if arguments.check:
            current = arguments.output.read_text(encoding="utf-8")
            if current != rendered:
                raise ConfigError(f"{arguments.output} is stale; regenerate it")
            return 0
        arguments.output.write_text(rendered, encoding="utf-8")
    except (ConfigError, OSError, UnicodeDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
