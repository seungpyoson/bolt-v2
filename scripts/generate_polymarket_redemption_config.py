#!/usr/bin/env python3
"""Generate private Rust redemption request configuration from strict TOML."""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import json
import pathlib
import re
import sys
import tomllib


HEX_40 = re.compile(r"^0x[0-9a-fA-F]{40}$")
HEX_64 = re.compile(r"^0x[0-9a-fA-F]{64}$")
HEX_8 = re.compile(r"^0x[0-9a-fA-F]{8}$")
LOWER_HEX_40 = re.compile(r"^[0-9a-f]{40}$")
LOWER_HEX_64 = re.compile(r"^[0-9a-f]{64}$")
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
    output_asset: str
    standard_adapter_target: str
    negative_risk_adapter_target: str
    parent_collection_id: str
    dummy_index_sets: tuple[int, int]
    maximum_safe_nonce_decimal_digits: int
    aws_region: str
    signer_private_key_ssm_path: str
    builder_api_key_ssm_path: str
    builder_api_secret_ssm_path: str
    builder_passphrase_ssm_path: str


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


def _address(value: object, field: str, *, allow_zero: bool = False) -> str:
    text = _string(value, field)
    if HEX_40.fullmatch(text) is None:
        raise ConfigError(f"{field} must be a 20-byte 0x-prefixed EVM address")
    normalized = text.lower()
    if not allow_zero and normalized == "0x" + ("0" * 40):
        raise ConfigError(f"{field} must not be the zero address")
    return normalized


def _bytes32(value: object, field: str) -> str:
    text = _string(value, field)
    if HEX_64.fullmatch(text) is None:
        raise ConfigError(f"{field} must be a 32-byte 0x-prefixed value")
    return text.lower()


def _ssm_path(value: object, field: str) -> str:
    text = _string(value, field)
    if (
        not text.startswith("/")
        or text.endswith("/")
        or "//" in text
        or any(character.isspace() for character in text)
    ):
        raise ConfigError(f"{field} must be a valid absolute SSM path")
    return text


def _u256(value: object, field: str) -> int:
    text = _string(value, field)
    try:
        parsed = int(text, 16 if text.lower().startswith("0x") else 10)
    except ValueError as error:
        raise ConfigError(f"{field} must be a canonical uint256 string") from error
    if parsed < 0 or parsed > MAX_U256:
        raise ConfigError(f"{field} must fit uint256")
    canonical = hex(parsed).lower() if text.lower().startswith("0x") else str(parsed)
    if text.lower() != canonical:
        raise ConfigError(f"{field} must be a canonical uint256 string")
    return parsed


def _revision(value: object, field: str) -> str:
    text = _string(value, field)
    if LOWER_HEX_40.fullmatch(text) is None:
        raise ConfigError(f"{field} must be a 40 lowercase hexadecimal commit SHA")
    return text


def _sha256(value: object, field: str) -> str:
    text = _string(value, field)
    if LOWER_HEX_64.fullmatch(text) is None:
        raise ConfigError(f"{field} must be a 64 lowercase hexadecimal SHA-256")
    return text


def _source_path(value: object, field: str) -> str:
    text = _string(value, field)
    path = pathlib.PurePosixPath(text)
    if path.is_absolute() or ".." in path.parts or path.as_posix() != text:
        raise ConfigError(f"{field} must be a normalized repository-relative path")
    return text


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
            "credential_set",
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

    aws = _table(root_document.get("aws"), "root.aws")
    aws_region = _string(aws.get("region"), "root.aws.region")
    if any(character.isspace() for character in aws_region):
        raise ConfigError("root.aws.region must not contain whitespace")
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
    secrets = _table(client.get("secrets"), f"root.clients.{root_client}.secrets")
    signer_private_key_ssm_path = _ssm_path(
        secrets.get("private_key_ssm_path"),
        f"root.clients.{root_client}.secrets.private_key_ssm_path",
    )

    redemption = _table(document["redemption"], "redemption")
    _exact_keys(
        redemption,
        {
            "chain_id",
            "collateral_asset",
            "output_asset",
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

    credentials = _table(document["credential_set"], "credential_set")
    _exact_keys(
        credentials,
        {
            "builder_api_key_ssm_path",
            "builder_api_secret_ssm_path",
            "builder_passphrase_ssm_path",
        },
        "credential_set",
    )
    return RuntimeConfig(
        schema_version=schema_version,
        production_activation_enabled=activation,
        chain_id=chain_id,
        wallet_type=wallet_type,
        safe_address=safe_address,
        collateral_asset=_address(redemption["collateral_asset"], "redemption.collateral_asset"),
        output_asset=_address(redemption["output_asset"], "redemption.output_asset"),
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
        aws_region=aws_region,
        signer_private_key_ssm_path=signer_private_key_ssm_path,
        builder_api_key_ssm_path=_ssm_path(
            credentials["builder_api_key_ssm_path"],
            "credential_set.builder_api_key_ssm_path",
        ),
        builder_api_secret_ssm_path=_ssm_path(
            credentials["builder_api_secret_ssm_path"],
            "credential_set.builder_api_secret_ssm_path",
        ),
        builder_passphrase_ssm_path=_ssm_path(
            credentials["builder_passphrase_ssm_path"],
            "credential_set.builder_passphrase_ssm_path",
        ),
    )


def _deployment_fact_payload(
    runtime: RuntimeConfig,
    source_url: str,
    observed_date: str,
) -> bytes:
    return (
        f"source_url={source_url}\n"
        f"observed_date={observed_date}\n"
        f"CtfCollateralAdapter={runtime.standard_adapter_target}\n"
        f"NegRiskCtfCollateralAdapter={runtime.negative_risk_adapter_target}\n"
    ).encode("utf-8")


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
            "deployment_fact_format_version",
            "deployment_fact_sha256",
            "standard_source_path",
            "standard_source_sha256",
            "negative_risk_source_path",
            "negative_risk_source_sha256",
            "function_signature",
            "function_selector",
        },
        "adapter_abi",
    )
    _string(adapter["repository"], "adapter_abi.repository")
    _revision(adapter["revision"], "adapter_abi.revision")
    source_url = _string(
        adapter["deployment_source_url"], "adapter_abi.deployment_source_url"
    )
    observed_date = _string(
        adapter["deployment_observed_date"], "adapter_abi.deployment_observed_date"
    )
    _integer(
        adapter["deployment_fact_format_version"],
        "adapter_abi.deployment_fact_format_version",
        minimum=1,
        maximum=1,
    )
    observed_fact_sha256 = _sha256(
        adapter["deployment_fact_sha256"], "adapter_abi.deployment_fact_sha256"
    )
    expected_fact_sha256 = hashlib.sha256(
        _deployment_fact_payload(runtime, source_url, observed_date)
    ).hexdigest()
    if observed_fact_sha256 != expected_fact_sha256:
        raise ConfigError(
            "adapter_abi.deployment_fact_sha256 must hash the v1 canonical source URL, "
            "observed date, and normalized runtime adapter targets"
        )
    _source_path(adapter["standard_source_path"], "adapter_abi.standard_source_path")
    _sha256(adapter["standard_source_sha256"], "adapter_abi.standard_source_sha256")
    _source_path(
        adapter["negative_risk_source_path"], "adapter_abi.negative_risk_source_path"
    )
    _sha256(
        adapter["negative_risk_source_sha256"],
        "adapter_abi.negative_risk_source_sha256",
    )
    _string(adapter["function_signature"], "adapter_abi.function_signature")
    selector_text = _string(adapter["function_selector"], "adapter_abi.function_selector")
    if HEX_8.fullmatch(selector_text) is None:
        raise ConfigError("adapter_abi.function_selector must be a four-byte 0x-prefixed hex value")
    selector_bytes = bytes.fromhex(selector_text[2:])

    safe = _table(document["safe_request"], "safe_request")
    _exact_keys(
        safe,
        {
            "repository",
            "revision",
            "builder_source_path",
            "builder_source_sha256",
            "types_source_path",
            "types_source_sha256",
            "signature_pack_source_path",
            "signature_pack_source_sha256",
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
    _string(safe["repository"], "safe_request.repository")
    _revision(safe["revision"], "safe_request.revision")
    for key in ("builder_source_path", "types_source_path", "signature_pack_source_path"):
        _source_path(safe[key], f"safe_request.{key}")
    for key in (
        "builder_source_sha256",
        "types_source_sha256",
        "signature_pack_source_sha256",
    ):
        _sha256(safe[key], f"safe_request.{key}")
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
        output_asset: alloy_primitives::address!({_rust_string(runtime.output_asset)}),
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
        aws_region: {_rust_string(runtime.aws_region)},
        signer_private_key_ssm_path: {_rust_string(runtime.signer_private_key_ssm_path)},
        builder_api_key_ssm_path: {_rust_string(runtime.builder_api_key_ssm_path)},
        builder_api_secret_ssm_path: {_rust_string(runtime.builder_api_secret_ssm_path)},
        builder_passphrase_ssm_path: {_rust_string(runtime.builder_passphrase_ssm_path)},
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
        else:
            arguments.output.write_text(rendered, encoding="utf-8")
    except (ConfigError, OSError, UnicodeDecodeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
