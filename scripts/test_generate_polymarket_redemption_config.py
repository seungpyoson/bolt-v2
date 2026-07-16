#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import generate_polymarket_redemption_config as generator


RUNTIME_TOML = """\
schema_version = 1
production_activation_enabled = false

[redemption]
chain_id = 137
wallet_type = "SAFE"
safe_address = "0x1111111111111111111111111111111111111111"
collateral_asset = "0x4444444444444444444444444444444444444444"
output_asset = "0x5555555555555555555555555555555555555555"
standard_adapter_target = "0x2222222222222222222222222222222222222222"
negative_risk_adapter_target = "0x3333333333333333333333333333333333333333"
parent_collection_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
dummy_index_sets = ["1", "2"]

[protocol_bounds]
maximum_safe_nonce_decimal_digits = 78

[credential_set]
aws_region = "us-east-1"
signer_private_key_ssm_path = "/bolt/polymarket/redemption/signer-private-key"
builder_api_key_ssm_path = "/bolt/polymarket/redemption/builder-api-key"
builder_api_secret_ssm_path = "/bolt/polymarket/redemption/builder-api-secret"
builder_passphrase_ssm_path = "/bolt/polymarket/redemption/builder-passphrase"
"""

EVIDENCE_TOML = """\
schema_version = 1

[adapter_abi]
repository = "https://github.com/Polymarket/ctf-exchange-v2"
revision = "ccc0596074f4dfd62c944fbca4de252893b82b4b"
deployment_source_path = "README.md"
deployment_source_sha256 = "6666666666666666666666666666666666666666666666666666666666666666"
standard_source_path = "src/adapters/CtfCollateralAdapter.sol"
standard_source_sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
negative_risk_source_path = "src/adapters/NegRiskCtfCollateralAdapter.sol"
negative_risk_source_sha256 = "2222222222222222222222222222222222222222222222222222222222222222"
function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"
function_selector = "0x01b7037c"

[safe_request]
repository = "https://github.com/Polymarket/builder-relayer-client"
revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"
builder_source_path = "src/builder/safe.ts"
builder_source_sha256 = "3333333333333333333333333333333333333333333333333333333333333333"
types_source_path = "src/types.ts"
types_source_sha256 = "4444444444444444444444444444444444444444444444444444444444444444"
signature_pack_source_path = "src/utils/index.ts"
signature_pack_source_sha256 = "5555555555555555555555555555555555555555555555555555555555555555"
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
        path.write_text(text, encoding="utf-8")
        return path

    def load(self, runtime_text: str = RUNTIME_TOML, evidence_text: str = EVIDENCE_TOML):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            runtime = self.write(root, "runtime.toml", runtime_text)
            evidence = self.write(root, "evidence.toml", evidence_text)
            return generator.load_config(runtime, evidence)

    def test_valid_sources_render_private_closed_rust_projection(self) -> None:
        config = self.load()
        rendered = generator.render_rust(config, "runtime.toml", "evidence.toml")

        self.assertIn("production_activation_enabled: false", rendered)
        self.assertIn("chain_id: 137", rendered)
        self.assertIn('wallet_type: "SAFE"', rendered)
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

    def test_non_ssm_credential_reference_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "valid absolute SSM path"):
            self.load(RUNTIME_TOML.replace("/bolt/polymarket/redemption/builder-api-key", "builder-api-key"))

    def test_unpinned_evidence_revision_is_rejected(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "40 lowercase hexadecimal"):
            self.load(evidence_text=EVIDENCE_TOML.replace("ccc0596074f4dfd62c944fbca4de252893b82b4b", "main"))

    def test_evidence_cannot_duplicate_runtime_values(self) -> None:
        duplicated = EVIDENCE_TOML + '\nruntime_safe_address = "0x1111111111111111111111111111111111111111"\n'
        with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
            self.load(evidence_text=duplicated)

    def test_selector_requires_exact_four_byte_hex(self) -> None:
        with self.assertRaisesRegex(generator.ConfigError, "four-byte"):
            self.load(evidence_text=EVIDENCE_TOML.replace("0x01b7037c", "0x01b703"))

    def test_u256_dummy_argument_overflow_is_rejected(self) -> None:
        overflow = str(1 << 256)
        with self.assertRaisesRegex(generator.ConfigError, "uint256"):
            self.load(RUNTIME_TOML.replace('dummy_index_sets = ["1", "2"]', f'dummy_index_sets = ["{overflow}", "2"]'))

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
