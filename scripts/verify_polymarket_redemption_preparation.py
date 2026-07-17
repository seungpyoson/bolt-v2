#!/usr/bin/env python3
"""Verify deterministic configuration evidence for redemption preparation."""

from __future__ import annotations

import hashlib
import pathlib
import subprocess
import sys
import tomllib
from collections.abc import Iterator, Mapping


OWNER = pathlib.Path("src/bolt_v3_polymarket_redemption.rs")
GENERATED = pathlib.Path("src/bolt_v3_polymarket_redemption/generated.rs")
RUNTIME = pathlib.Path("config/polymarket-redemption.toml")
ROOT_RUNTIME = pathlib.Path("config/root.toml")
EVIDENCE = pathlib.Path("config/polymarket-redemption-source-evidence.toml")
COMPILE_TEST = pathlib.Path("tests/polymarket_redemption_preparation.rs")
COMPILE_FAIL = pathlib.Path("tests/polymarket_redemption_preparation_compile_fail.rs")
GENERATOR = pathlib.Path("scripts/generate_polymarket_redemption_config.py")

EXPECTED_RUNTIME_AUTHORITY_PATHS = {
    "standard_adapter_target": ("redemption", "standard_adapter_target"),
    "negative_risk_adapter_target": ("redemption", "negative_risk_adapter_target"),
    "builder_api_key_ssm_path": ("credential_set", "builder_api_key_ssm_path"),
    "builder_api_secret_ssm_path": (
        "credential_set",
        "builder_api_secret_ssm_path",
    ),
    "builder_passphrase_ssm_path": (
        "credential_set",
        "builder_passphrase_ssm_path",
    ),
}
ROOT_OWNED_WALLET_FIELDS = frozenset(
    {"aws_region", "safe_address", "signer_private_key_ssm_path"}
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


def _toml(path: pathlib.Path) -> dict[str, object]:
    return tomllib.loads(_read(path))


def _repository_toml(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.toml")
        if not ignored.intersection(path.relative_to(root).parts)
    )


def _key_locations(
    value: object,
    prefix: tuple[str, ...] = (),
) -> Iterator[tuple[str, tuple[str, ...]]]:
    if isinstance(value, Mapping):
        for key, child in value.items():
            path = (*prefix, key)
            yield key, path
            yield from _key_locations(child, path)
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from _key_locations(child, (*prefix, f"[{index}]"))


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


def _evidence_projection(evidence: Mapping[str, object]) -> dict[str, object]:
    adapter = evidence.get("adapter_abi")
    safe_request = evidence.get("safe_request")
    if not isinstance(adapter, Mapping) or not isinstance(safe_request, Mapping):
        return {}
    return {
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
        "signature_pack_source_path": safe_request.get(
            "signature_pack_source_path"
        ),
        "signature_pack_source_sha256": safe_request.get(
            "signature_pack_source_sha256"
        ),
    }


def _manifest_errors(cargo: Mapping[str, object]) -> list[str]:
    errors: list[str] = []
    dependencies = cargo.get("dependencies")
    if not isinstance(dependencies, Mapping):
        return ["Cargo.toml dependencies must be a table"]
    for dependency in ("alloy-signer", "alloy-signer-local"):
        if dependencies.get(dependency) != "=2.1.0":
            errors.append(
                f"direct signer dependency must remain exact and locked: {dependency} = =2.1.0"
            )

    tests = cargo.get("test")
    expected_target = {
        "name": "polymarket_redemption_preparation",
        "path": str(COMPILE_TEST),
    }
    if not isinstance(tests, list) or expected_target not in tests:
        errors.append("compile-fail test target is not wired")
    return errors


def boundary_errors(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    required = [
        OWNER,
        GENERATED,
        RUNTIME,
        ROOT_RUNTIME,
        EVIDENCE,
        COMPILE_TEST,
        COMPILE_FAIL,
        pathlib.Path("Cargo.toml"),
    ]
    missing = [str(path) for path in required if not (root / path).is_file()]
    if missing:
        return [f"missing required redemption preparation artifact(s): {missing}"]

    try:
        runtime = _toml(root / RUNTIME)
        root_runtime = _toml(root / ROOT_RUNTIME)
        evidence = _toml(root / EVIDENCE)
        cargo = _toml(root / "Cargo.toml")
    except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
        return [f"cannot inspect redemption preparation artifacts: {error}"]

    authorities: dict[str, list[tuple[pathlib.Path, tuple[str, ...]]]] = {
        key: [] for key in EXPECTED_RUNTIME_AUTHORITY_PATHS
    }
    for path in _repository_toml(root):
        try:
            parsed = _toml(path)
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect TOML authority {path.relative_to(root)}: {error}")
            continue
        relative = path.relative_to(root)
        for key, key_path in _key_locations(parsed):
            if key in authorities:
                authorities[key].append((relative, key_path))

    for key, expected_path in EXPECTED_RUNTIME_AUTHORITY_PATHS.items():
        expected = [(RUNTIME, expected_path)]
        if authorities[key] != expected:
            errors.append(
                f"runtime field {key} must have one parsed TOML authority at "
                f"{RUNTIME}:{'.'.join(expected_path)}; found {authorities[key]}"
            )

    if runtime.get("production_activation_enabled") is not False:
        errors.append("production_activation_enabled must remain false")

    wallet_authority = runtime.get("wallet_authority")
    redemption = runtime.get("redemption")
    if not isinstance(wallet_authority, Mapping) or not isinstance(redemption, Mapping):
        errors.append("runtime config must contain wallet_authority and redemption tables")
    elif not isinstance(wallet_authority.get("root_client"), str):
        errors.append("wallet_authority.root_client must select a root config client")

    runtime_wallet_duplicates = sorted(
        key
        for key, _ in _key_locations(runtime)
        if key in ROOT_OWNED_WALLET_FIELDS
    )
    if runtime_wallet_duplicates:
        errors.append(
            "redemption wallet and signer fields must remain single-sourced from config/root.toml: "
            f"{runtime_wallet_duplicates}"
        )

    root_clients = root_runtime.get("clients")
    selected_client = (
        wallet_authority.get("root_client")
        if isinstance(wallet_authority, Mapping)
        else None
    )
    if (
        not isinstance(root_clients, Mapping)
        or not isinstance(selected_client, str)
        or selected_client not in root_clients
    ):
        errors.append("wallet_authority.root_client must exist in config/root.toml")

    observed_evidence = _evidence_projection(evidence)
    if observed_evidence != EXPECTED_EVIDENCE:
        errors.append(
            "source evidence must remain pinned to the reviewed adapter/request revisions and ABI: "
            f"{observed_evidence}"
        )

    adapter = evidence.get("adapter_abi")
    if isinstance(adapter, Mapping) and isinstance(redemption, Mapping):
        expected_hash = _deployment_fact_sha256(
            adapter.get("deployment_source_url"),
            adapter.get("deployment_observed_date"),
            redemption.get("chain_id"),
            redemption.get("collateral_asset"),
            redemption.get("output_asset"),
            redemption.get("standard_adapter_target"),
            redemption.get("negative_risk_adapter_target"),
            redemption.get("parent_collection_id"),
            redemption.get("dummy_index_sets"),
        )
        if adapter.get("deployment_fact_sha256") != expected_hash:
            errors.append(
                "deployment fact hash must bind the source observation to normalized runtime protocol facts"
            )

    errors.extend(_manifest_errors(cargo))
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
        detail = generation.stderr.strip() or generation.stdout.strip()
        errors.append(f"generated redemption projection is stale: {detail}")
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print("polymarket redemption preparation boundary: deterministic evidence verified")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
