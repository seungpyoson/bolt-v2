#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import pathlib
import tempfile
import unittest

import generate_polymarket_redemption_config as generator


RUNTIME_TOML = """\
schema_version = 1
production_activation_enabled = false

[wallet_authority]
root_client = "polymarket_main"

[redemption]
chain_id = 137
collateral_asset = "0x4444444444444444444444444444444444444444"
standard_adapter_target = "0x2222222222222222222222222222222222222222"
negative_risk_adapter_target = "0x3333333333333333333333333333333333333333"
parent_collection_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
dummy_index_sets = ["1", "2"]

[protocol_bounds]
maximum_safe_nonce_decimal_digits = 78
"""

ROOT_TOML = """\
[aws]
region = "us-east-1"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.execution]
signature_type = "poly_gnosis_safe"
funder = "0x1111111111111111111111111111111111111111"

[clients.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket/redemption/signer-private-key"
"""

STANDARD_SNAPSHOT = b"contract CtfCollateralAdapter {}\n"
NEGATIVE_RISK_SNAPSHOT = b"contract NegRiskCtfCollateralAdapter {}\n"
BUILDER_SNAPSHOT = b"export function buildSafeTransactionRequest() {}\n"
TYPES_SNAPSHOT = b'export enum TransactionType { SAFE = "SAFE" }\n'
SIGNATURE_PACK_SNAPSHOT = b"export function splitAndPackSig() {}\n"
DEPLOYMENT_SNAPSHOT = """\
# Contracts

All Polymarket contracts are deployed on **Polygon mainnet** (Chain ID: 137).

| Contract | Address |
| --- | --- |
| pUSD — CollateralToken (proxy) | [`0x4444444444444444444444444444444444444444`](https://example.invalid/collateral) |
| CtfCollateralAdapter | [`0x2222222222222222222222222222222222222222`](https://example.invalid/standard) |
| NegRiskCtfCollateralAdapter | [`0x3333333333333333333333333333333333333333`](https://example.invalid/negative-risk) |
""".encode("utf-8")

SNAPSHOTS = {
    "polymarket-redemption-sources/docs.polymarket.com/2026-07-17/resources/contracts.md.hex": DEPLOYMENT_SNAPSHOT,
    "polymarket-redemption-sources/github.com/Polymarket/ctf-exchange-v2/ccc0596074f4dfd62c944fbca4de252893b82b4b/src/adapters/CtfCollateralAdapter.sol.hex": STANDARD_SNAPSHOT,
    "polymarket-redemption-sources/github.com/Polymarket/ctf-exchange-v2/ccc0596074f4dfd62c944fbca4de252893b82b4b/src/adapters/NegRiskCtfCollateralAdapter.sol.hex": NEGATIVE_RISK_SNAPSHOT,
    "polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/builder/safe.ts.hex": BUILDER_SNAPSHOT,
    "polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/types.ts.hex": TYPES_SNAPSHOT,
    "polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/utils/index.ts.hex": SIGNATURE_PACK_SNAPSHOT,
}


def snapshot_sha256(path: str) -> str:
    return hashlib.sha256(SNAPSHOTS[f"{path}.hex"]).hexdigest()


EVIDENCE_TOML = f"""\
schema_version = 1

[adapter_abi]
repository = "https://github.com/Polymarket/ctf-exchange-v2"
revision = "ccc0596074f4dfd62c944fbca4de252893b82b4b"
deployment_source_url = "https://docs.polymarket.com/resources/contracts"
deployment_observed_date = "2026-07-17"
deployment_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/docs.polymarket.com/2026-07-17/resources/contracts.md")}"
standard_source_path = "src/adapters/CtfCollateralAdapter.sol"
standard_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/github.com/Polymarket/ctf-exchange-v2/ccc0596074f4dfd62c944fbca4de252893b82b4b/src/adapters/CtfCollateralAdapter.sol")}"
negative_risk_source_path = "src/adapters/NegRiskCtfCollateralAdapter.sol"
negative_risk_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/github.com/Polymarket/ctf-exchange-v2/ccc0596074f4dfd62c944fbca4de252893b82b4b/src/adapters/NegRiskCtfCollateralAdapter.sol")}"
function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"

[safe_request]
repository = "https://github.com/Polymarket/builder-relayer-client"
revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"
builder_source_path = "src/builder/safe.ts"
builder_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/builder/safe.ts")}"
types_source_path = "src/types.ts"
types_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/types.ts")}"
signature_pack_source_path = "src/utils/index.ts"
signature_pack_snapshot_sha256 = "{snapshot_sha256("polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/utils/index.ts")}"
operation = 0
value = "0"
safe_tx_gas = "0"
base_gas = "0"
gas_price = "0"
gas_token = "0x0000000000000000000000000000000000000000"
refund_receiver = "0x0000000000000000000000000000000000000000"
metadata = ""
"""


class GeneratePolymarketRedemptionConfigTests(unittest.TestCase):
    def write(self, directory: pathlib.Path, name: str, text: str) -> pathlib.Path:
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def load(
        self,
        runtime_text: str = RUNTIME_TOML,
        evidence_text: str = EVIDENCE_TOML,
        root_text: str = ROOT_TOML,
        snapshot_overrides: dict[str, bytes] | None = None,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            snapshots = SNAPSHOTS | (snapshot_overrides or {})
            for relative_path, content in snapshots.items():
                snapshot_path = root / relative_path
                snapshot_path.parent.mkdir(parents=True, exist_ok=True)
                snapshot_path.write_text(content.hex() + "\n", encoding="ascii")
            runtime = self.write(root, "runtime.toml", runtime_text)
            evidence = self.write(root, "evidence.toml", evidence_text)
            root_runtime = self.write(root, "root.toml", root_text)
            return generator.load_config(runtime, evidence, root_runtime)

    def test_valid_sources_render_private_closed_rust_projection(self) -> None:
        config = self.load()
        rendered = generator.render_rust(
            config, "runtime.toml", "evidence.toml", "root.toml"
        )

        self.assertIn("production_activation_enabled: false", rendered)
        self.assertIn("chain_id: 137", rendered)
        self.assertIn('wallet_type: "SAFE"', rendered)
        self.assertIn(
            'safe_address: alloy_primitives::address!("0x1111111111111111111111111111111111111111")',
            rendered,
        )
        self.assertNotIn("signer_private_key_ssm_path", rendered)
        self.assertNotIn("builder_api_key_ssm_path", rendered)
        self.assertNotIn("output_asset", rendered)
        self.assertIn("function_selector: [1, 183, 3, 124]", rendered)
        self.assertIn("dummy_index_sets: [", rendered)
        self.assertIn("U256::from_limbs([1, 0, 0, 0])", rendered)
        self.assertIn("U256::from_limbs([2, 0, 0, 0])", rendered)
        self.assertNotIn("pub const", rendered)

    def test_activation_true_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "must remain false"):
            self.load(RUNTIME_TOML.replace("production_activation_enabled = false", "production_activation_enabled = true"))

    def test_unknown_runtime_field_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(RUNTIME_TOML.replace("chain_id = 137", "chain_id = 137\nsecond_chain_id = 137"))

    def test_output_asset_is_rejected_as_a_dead_runtime_authority(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(
                RUNTIME_TOML.replace(
                    'collateral_asset = "0x4444444444444444444444444444444444444444"',
                    'collateral_asset = "0x4444444444444444444444444444444444444444"\n'
                    'output_asset = "0x5555555555555555555555555555555555555555"',
                )
            )

    def test_dead_builder_credential_authority_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(
                runtime_text=RUNTIME_TOML
                + "\n[credential_set]\n"
                + 'builder_api_key_ssm_path = "/duplicate"\n'
            )

    def test_wallet_is_derived_from_root_client(self) -> None:
        changed = ROOT_TOML.replace(
            "0x1111111111111111111111111111111111111111",
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ).replace(
            "/bolt/polymarket/redemption/signer-private-key",
            "/bolt/polymarket/rotated-private-key",
        )
        config = self.load(root_text=changed)
        self.assertEqual(
            config.runtime.safe_address,
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        self.assertFalse(hasattr(config.runtime, "signer_private_key_ssm_path"))

    def test_wallet_authority_must_select_safe_polymarket_client(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "poly_gnosis_safe"):
            self.load(
                root_text=ROOT_TOML.replace("poly_gnosis_safe", "ed25519")
            )

    def test_unpinned_evidence_revision_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "40 lowercase hexadecimal"):
            self.load(evidence_text=EVIDENCE_TOML.replace("ccc0596074f4dfd62c944fbca4de252893b82b4b", "main"))

    def test_evidence_repository_must_be_canonical_https(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "canonical HTTPS URL"):
            self.load(
                evidence_text=EVIDENCE_TOML.replace(
                    "https://github.com/Polymarket/ctf-exchange-v2",
                    "https://github.com:invalid/Polymarket/ctf-exchange-v2",
                )
            )

    def test_repository_url_aliases_are_rejected(self) -> None:
        aliases = (
            "https://github.com/Polymarket/../Polymarket/ctf-exchange-v2",
            "https://github.com//Polymarket/ctf-exchange-v2",
            "https://github.com/Polymarket/%2e%2e/Polymarket/ctf-exchange-v2",
            "https://github.com/Polymarket/%2E%2E%2FPolymarket/ctf-exchange-v2",
            "https://github.com./Polymarket/ctf-exchange-v2",
            r"https://github.com/Polymarket\\ctf-exchange-v2",
            "http://github.com/Polymarket/ctf-exchange-v2",
            "https://reviewer@github.com/Polymarket/ctf-exchange-v2",
            "https://github.com:443/Polymarket/ctf-exchange-v2",
            "https://github.com/Polymarket/ctf-exchange-v2?ref=main",
            "https://github.com/Polymarket/ctf-exchange-v2#source",
        )
        for alias in aliases:
            with self.subTest(alias=alias):
                with self.assertRaisesRegex(
                    generator.ConfigError, "canonical HTTPS URL"
                ):
                    self.load(
                        evidence_text=EVIDENCE_TOML.replace(
                            "https://github.com/Polymarket/ctf-exchange-v2",
                            alias,
                        )
                    )

    def test_deployment_url_aliases_are_rejected(self) -> None:
        aliases = (
            "https://docs.polymarket.com/resources/../resources/contracts",
            "https://docs.polymarket.com//resources/contracts",
            "https://docs.polymarket.com/resources/%2e%2e/resources/contracts",
            "https://docs.polymarket.com./resources/contracts",
            r"https://docs.polymarket.com/resources\\contracts",
        )
        for alias in aliases:
            with self.subTest(alias=alias):
                with self.assertRaisesRegex(
                    generator.ConfigError, "canonical HTTPS URL"
                ):
                    self.load(
                        evidence_text=EVIDENCE_TOML.replace(
                            "https://docs.polymarket.com/resources/contracts",
                            alias,
                        )
                    )

    def test_deployment_snapshot_must_match_runtime_targets(self) -> None:
        mutations = (
            (
                "0x2222222222222222222222222222222222222222",
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            (
                "0x3333333333333333333333333333333333333333",
                "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        )
        for expected, replacement in mutations:
            with self.subTest(expected=expected):
                with self.assertRaisesRegex(
                    generator.ConfigError, "deployment snapshot facts"
                ):
                    self.load(
                        runtime_text=RUNTIME_TOML.replace(expected, replacement)
                    )

    def test_deployment_snapshot_must_match_runtime_chain(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "deployment snapshot facts"):
            self.load(runtime_text=RUNTIME_TOML.replace("chain_id = 137", "chain_id = 138"))

    def test_snapshot_bytes_must_match_registered_sha256(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "does not match captured bytes"):
            self.load(
                snapshot_overrides={
                    "polymarket-redemption-sources/github.com/Polymarket/builder-relayer-client/9122f6fb1856f1ecfe4406685bfa19a2c5a7b290/src/builder/safe.ts.hex": b"mutated\n"
                }
            )

    def test_snapshot_path_components_cannot_escape_evidence_directory(self) -> None:
        mutations = (
            (
                'builder_source_path = "src/builder/safe.ts"',
                'builder_source_path = "../safe.ts"',
                "repository-relative path",
            ),
            (
                'builder_source_path = "src/builder/safe.ts"',
                'builder_source_path = "/src/builder/safe.ts"',
                "repository-relative path",
            ),
            (
                'builder_source_path = "src/builder/safe.ts"',
                'builder_source_path = "src/./builder/safe.ts"',
                "repository-relative path",
            ),
            (
                'builder_source_path = "src/builder/safe.ts"',
                'builder_source_path = "src//builder/safe.ts"',
                "repository-relative path",
            ),
            (
                'revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"',
                'revision = "../9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"',
                "40 lowercase hexadecimal",
            ),
            (
                'revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"',
                'revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b29"',
                "40 lowercase hexadecimal",
            ),
            (
                'deployment_observed_date = "2026-07-17"',
                'deployment_observed_date = "../2026-07-17"',
                "ISO calendar date",
            ),
        )
        for expected, replacement, message in mutations:
            with self.subTest(replacement=replacement):
                with self.assertRaisesRegex(generator.ConfigError, message):
                    self.load(
                        evidence_text=EVIDENCE_TOML.replace(expected, replacement)
                    )

    def test_deployment_observation_date_has_one_spelling(self) -> None:
        for alias in ("20260717", "2026-W29-5"):
            with self.subTest(alias=alias):
                with self.assertRaisesRegex(
                    generator.ConfigError, "canonical ISO calendar date"
                ):
                    self.load(
                        evidence_text=EVIDENCE_TOML.replace("2026-07-17", alias)
                    )

    def test_missing_derived_snapshot_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "cannot read derived"):
            self.load(
                evidence_text=EVIDENCE_TOML.replace(
                    'builder_source_path = "src/builder/safe.ts"',
                    'builder_source_path = "src/builder/missing.ts"',
                )
            )

    def test_evidence_cannot_duplicate_runtime_values(self) -> None:
        duplicated = EVIDENCE_TOML + '\nruntime_safe_address = "0x1111111111111111111111111111111111111111"\n'
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(evidence_text=duplicated)

    def test_function_signature_is_the_only_selector_authority(self) -> None:
        config = self.load()
        self.assertEqual(config.evidence.function_selector, (1, 183, 3, 124))

    def test_function_signature_change_changes_derived_selector(self) -> None:
        changed = self.load(
            evidence_text=EVIDENCE_TOML.replace(
                "redeemPositions(address,bytes32,bytes32,uint256[])",
                "redeemPositions(address,bytes32,bytes32,uint256[2])",
            )
        )
        self.assertNotEqual(changed.evidence.function_selector, (1, 183, 3, 124))

    def test_function_signature_must_be_ascii(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "must be ASCII"):
            self.load(
                evidence_text=EVIDENCE_TOML.replace(
                    "redeemPositions(address,bytes32,bytes32,uint256[])",
                    "redeemPositions(address,bytes32,bytes32,uint256[])—",
                )
            )

    def test_handwritten_selector_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(
                evidence_text=EVIDENCE_TOML.replace(
                    'function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"',
                    'function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"\n'
                    'function_selector = "0x01b7037c"',
                )
            )

    def test_u256_dummy_argument_overflow_is_rejected(self) -> None:
        overflow = str(1 << 256)
        with self.assertRaisesRegex(generator.ConfigError, "uint256"):
            self.load(RUNTIME_TOML.replace('dummy_index_sets = ["1", "2"]', f'dummy_index_sets = ["{overflow}", "2"]'))

    def test_u256_values_have_one_decimal_toml_spelling(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "decimal string"):
            self.load(
                RUNTIME_TOML.replace(
                    'dummy_index_sets = ["1", "2"]',
                    'dummy_index_sets = ["0x1", "2"]',
                )
            )

    def test_safe_gas_addresses_must_remain_zero(self) -> None:
        for field in ("gas_token", "refund_receiver"):
            with self.subTest(field=field):
                with self.assertRaisesRegex(generator.ConfigError, f"safe_request.{field} must remain the zero address"):
                    self.load(
                        evidence_text=EVIDENCE_TOML.replace(
                            f'{field} = "0x0000000000000000000000000000000000000000"',
                            f'{field} = "0x0000000000000000000000000000000000000001"',
                        )
                    )


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
