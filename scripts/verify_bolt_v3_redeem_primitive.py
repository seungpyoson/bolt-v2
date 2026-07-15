#!/usr/bin/env python3
"""Fail-closed static fence for the disabled AO-REDEEM primitive."""

from __future__ import annotations

import hashlib
import pathlib
import re
import sys
import tomllib


MANIFEST_PATH = pathlib.Path("ci/polymarket-redemption-provider-manifest.toml")
CONFIG_PATH = pathlib.Path("config/polymarket-redemption.toml")
MODULE_PATH = pathlib.Path("src/bolt_v3_providers/polymarket/redemption.rs")
REQUIRED_PATHS = (
    MANIFEST_PATH,
    CONFIG_PATH,
    MODULE_PATH,
    pathlib.Path("src/bolt_v3_providers/polymarket/redemption/config.rs"),
    pathlib.Path("src/bolt_v3_providers/polymarket/redemption/query.rs"),
    pathlib.Path("src/bolt_v3_providers/polymarket/redemption/request.rs"),
    pathlib.Path("src/bolt_v3_providers/polymarket/redemption/wire.rs"),
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
    ("adapter", "negative_risk_interface_blob"): "9e03079796543609cb8c25292bc42872f3029a3d",
    ("adapter", "deployment_table_blob"): "78aeefbb48439460ccb2c4e7de04d0aff619b0dd",
    ("relayer", "safe_builder_blob"): "3a05ac53d005d92822582a2a87d6bdbb13827187",
    ("relayer", "types_blob"): "daa09af14528ba5be49a0063b359fbc17dc5e505",
    ("relayer", "client_blob"): "2ca627dc7f853d03c9bd7dbbec505e86a944d101",
    ("relayer", "endpoints_blob"): "b33dfe0d1062e7db2615f1b155f11caf0c31a5d7",
    ("relayer", "safe_abi_blob"): "5bbd1622d5d561fbe7ec472532deb25ef4f21687",
    ("relayer", "config_blob"): "18ebf7fe942fddde28bbf717fa15669b631d5cfb",
    ("independent_fixture", "safe_builder_blob"): "c9e8cacbe2f0e55c6e8bba230a5e072628136b4f",
    ("independent_fixture", "client_blob"): "6fcf7915301ac0ac0542e9fb1de25e0fd8118af5",
}


def _load_toml(path: pathlib.Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def verify(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    missing = [str(path) for path in REQUIRED_PATHS if not (root / path).is_file()]
    if missing:
        return [f"AO-REDEEM required path missing: {path}" for path in missing]

    try:
        manifest = _load_toml(root / MANIFEST_PATH)
        config = _load_toml(root / CONFIG_PATH)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return [f"AO-REDEEM TOML parse failure: {exc}"]

    for section, revision in EXPECTED_REVISIONS.items():
        if manifest.get(section, {}).get("reviewed_revision") != revision:
            errors.append(f"AO-REDEEM {section} reviewed revision drifted from {revision}")
    for (section, field), expected in EXPECTED_BLOBS.items():
        if manifest.get(section, {}).get(field) != expected:
            errors.append(f"AO-REDEEM reviewed blob drifted: {section}.{field}")

    adapter = manifest.get("adapter", {})
    if adapter.get("external_abi") != "redeemPositions(address,bytes32,bytes32,uint256[])":
        errors.append("AO-REDEEM exact external four-argument ABI is not source-fenced")
    if adapter.get("external_selector") != "0x01b7037c":
        errors.append("AO-REDEEM external ABI selector drifted")
    if adapter.get("ignored_argument_indices") != [0, 1, 3]:
        errors.append("AO-REDEEM ignored dummy arguments are not exactly source-fenced")

    safe = manifest.get("safe", {})
    if safe.get("nonce_abi") != "nonce()" or safe.get("nonce_selector") != "0xaffed0e0":
        errors.append("AO-REDEEM Safe nonce fence ABI drifted")
    if safe.get("operation") != "call" or safe.get("value") != "0":
        errors.append("AO-REDEEM fence is not a zero-value Safe call")

    relayer = manifest.get("relayer", {})
    if relayer.get("explicit_nonce") != "source-proven":
        errors.append("AO-REDEEM explicit Safe nonce support is not source-proven")
    if relayer.get("competing_same_nonce") != "unproven":
        errors.append("AO-REDEEM competing-same-nonce support cannot be claimed without conformance")

    activation = manifest.get("activation", {})
    if activation != {
        "primitive_enabled": False,
        "requires_competing_same_nonce_conformance": True,
        "has_active_caller": False,
        "has_durable_state": False,
    }:
        errors.append("AO-REDEEM manifest activation contract is not mechanically disabled and pure")
    if config.get("enabled") is not False:
        errors.append("AO-REDEEM grouped TOML must remain mechanically disabled")
    if config.get("provider_manifest_id") != manifest.get("manifest_id"):
        errors.append("AO-REDEEM grouped TOML is not bound to the provider manifest")
    configured_relayer = config.get("relayer", {})
    for field in ("submit_path", "transaction_path", "nonce_path"):
        if configured_relayer.get(field) != relayer.get(field):
            errors.append(f"AO-REDEEM relayer {field} is not manifest-bound")
    if configured_relayer.get("competing_same_nonce_conformance") is not False:
        errors.append("AO-REDEEM competing-same-nonce profile must fail closed")

    deployment = manifest.get("deployment", {})
    configured_wallet = config.get("wallet", {})
    configured_adapter = config.get("adapter", {})
    for config_section, manifest_section, fields in (
        (
            configured_wallet,
            deployment,
            ("chain_id", "wallet_type", "safe_address", "safe_factory"),
        ),
        (
            configured_adapter,
            deployment,
            ("standard_target", "negative_risk_target", "collateral", "output_asset"),
        ),
        (
            configured_adapter,
            manifest.get("adapter_arguments", {}),
            ("dummy_account", "dummy_parent_collection_id", "dummy_index_sets"),
        ),
        (
            configured_wallet,
            manifest.get("safe_boundary", {}),
            ("safe_implementation", "fallback_handler", "guard", "modules"),
        ),
    ):
        for field in fields:
            manifest_field = {
                "safe_implementation": "implementation",
            }.get(field, field)
            configured = config_section.get(field)
            fenced = manifest_section.get(manifest_field)
            if isinstance(configured, str) and isinstance(fenced, str):
                matches = configured.lower() == fenced.lower()
            else:
                matches = configured == fenced
            if not matches:
                errors.append(f"AO-REDEEM grouped TOML field {field} is not manifest-bound")
    if manifest.get("safe_boundary", {}).get("verification") != "exact-query-required":
        errors.append("AO-REDEEM Safe implementation boundary is not exact-query fenced")

    credentials = config.get("credentials", {})
    expected_credential_fields = {
        "signer_private_key_ssm_path",
        "builder_api_key_ssm_path",
        "builder_api_secret_ssm_path",
        "builder_passphrase_ssm_path",
        "max_value_bytes",
    }
    if set(credentials) != expected_credential_fields or any(
        not isinstance(credentials[field], str) or not credentials[field].startswith("/bolt/")
        for field in expected_credential_fields - {"max_value_bytes"}
    ):
        errors.append("AO-REDEEM credentials must use the exact grouped SSM-only schema")
    forbidden_credential_keys = re.compile(
        r"^(?:api_key|api_secret|passphrase|private_key|secret|credential)$", re.IGNORECASE
    )
    if any(forbidden_credential_keys.match(str(key)) for key in config):
        errors.append("AO-REDEEM grouped TOML contains a non-SSM credential field")

    source_fixtures = {
        "tests/fixtures/bolt_v3/redeem/source/ctf-collateral-adapter.txt": (
            "function redeemPositions(address, bytes32, bytes32 _conditionId, uint256[] calldata)",
            "CTFHelpers.partition()",
            "CollateralToken(COLLATERAL_TOKEN).wrap",
        ),
        "tests/fixtures/bolt_v3/redeem/source/negative-risk-collateral-adapter.txt": (
            "function _redeemPositions(bytes32 _conditionId, uint256[] memory)",
            "balanceOf(address(this)",
            "INegRiskAdapter(NEG_RISK_ADAPTER).redeemPositions(_conditionId, amounts)",
        ),
        "tests/fixtures/bolt_v3/redeem/source/relayer-safe-builder.txt": (
            "nonce: string",
            "nonce: args.nonce",
            'GET_TRANSACTION = "/transaction"',
            "competing_same_nonce_hosted_acceptance = unproven",
        ),
    }
    for relative, needles in source_fixtures.items():
        fixture_bytes = (root / relative).read_bytes()
        text = fixture_bytes.decode("utf-8")
        for needle in needles:
            if needle not in text:
                errors.append(f"AO-REDEEM source fixture {relative} lost assertion {needle!r}")
    snapshot_digests = manifest.get("source_snapshots", {})
    for relative, field in (
        ("tests/fixtures/bolt_v3/redeem/source/ctf-collateral-adapter.txt", "standard_sha256"),
        ("tests/fixtures/bolt_v3/redeem/source/negative-risk-collateral-adapter.txt", "negative_risk_sha256"),
        ("tests/fixtures/bolt_v3/redeem/source/relayer-safe-builder.txt", "relayer_sha256"),
    ):
        digest = hashlib.sha256((root / relative).read_bytes()).hexdigest()
        if snapshot_digests.get(field) != digest:
            errors.append(f"AO-REDEEM source snapshot digest drifted: {relative}")

    module_sources = "\n".join(
        (root / path).read_text(encoding="utf-8")
        for path in REQUIRED_PATHS
        if str(path).startswith("src/bolt_v3_providers/polymarket/redemption")
    )
    for pattern in (r"\basync\s+fn\s+submit", r"\bfn\s+send", r"reqwest", r"TcpStream"):
        if re.search(pattern, module_sources):
            errors.append("AO-REDEEM active caller or network transport is forbidden")
            break
    for pattern in (r"std::fs", r"tokio::fs", r"File::create", r"OpenOptions", r"rusqlite"):
        if re.search(pattern, module_sources):
            errors.append("AO-REDEEM durable state is forbidden")
            break
    for pattern in (r"log::", r"tracing::", r"println!", r"eprintln!"):
        if re.search(pattern, module_sources):
            errors.append("AO-REDEEM observability sink is forbidden")
            break

    allowed_references = {
        pathlib.Path("src/bolt_v3_providers/polymarket.rs"),
        pathlib.Path("src/bolt_v3_providers/boundary_registry.rs"),
    }
    for path in (root / "src").rglob("*.rs"):
        relative = path.relative_to(root)
        if str(relative).startswith("src/bolt_v3_providers/polymarket/redemption"):
            continue
        if relative in allowed_references:
            continue
        source = path.read_text(encoding="utf-8")
        if (
            "polymarket::redemption" in source
            or "redemption::build_request_pair" in source
            or "build_request_pair(" in source
            or "resolve_competing_nonce(" in source
        ):
            errors.append(f"AO-REDEEM disabled reachability violated by {relative}")

    provider_source = (root / "src/bolt_v3_providers/polymarket.rs").read_text(encoding="utf-8")
    if provider_source.count("pub mod redemption;") != 1:
        errors.append("AO-REDEEM pure module is not provider-owned exactly once")
    cargo = (root / "Cargo.toml").read_text(encoding="utf-8")
    if 'name = "bolt_v3_redeem_primitive"' not in cargo:
        errors.append("AO-REDEEM behavior harness is not registered")

    tests = (root / "tests/bolt_v3_redeem_primitive.rs").read_text(encoding="utf-8")
    required_tests = (
        "standard_and_negative_risk_fixtures",
        "original_and_fence_body_boundaries",
        "response_loss_requires_exact_queries",
        "original_wins_only_with_finalized_post_state",
        "fence_wins_only_with_unchanged_post_state",
        "unrelated_nonce_fails_closed",
        "retry_requires_exact_body_bytes",
        "sentinels_do_not_reach_redacted_diagnostics",
        "primitive_is_mechanically_disabled",
    )
    for name in required_tests:
        if f"fn {name}" not in tests:
            errors.append(f"AO-REDEEM required behavior test missing: {name}")

    registry = (root / "src/bolt_v3_providers/boundary_registry.rs").read_text(encoding="utf-8")
    for marker in ("POLYMARKET_RELAYER_ADAPTER_ID", "POLYGON_REDEMPTION_RPC_ADAPTER_ID"):
        if marker not in registry:
            errors.append(f"AO-REDEEM provider/runtime boundary is not registered: {marker}")
    return errors


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[1]
    errors = verify(root)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print("AO-REDEEM source fence passed")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
