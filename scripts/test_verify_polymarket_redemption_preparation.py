#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_polymarket_redemption_preparation as verifier


RUNTIME = """\
schema_version = 1
production_activation_enabled = false
[wallet_authority]
root_client = "polymarket_main"
[redemption]
chain_id = 137
collateral_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"
standard_adapter_target = "0xAdA100Db00Ca00073811820692005400218FcE1f"
negative_risk_adapter_target = "0xadA2005600Dec949baf300f4C6120000bDB6eAab"
parent_collection_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
dummy_index_sets = ["1", "2"]
[protocol_bounds]
maximum_safe_nonce_decimal_digits = 78
[credential_set]
builder_api_key_ssm_path = "/api-key"
builder_api_secret_ssm_path = "/api-secret"
builder_passphrase_ssm_path = "/passphrase"
"""

ROOT_RUNTIME = """\
[aws]
region = "eu-west-2"
[clients.polymarket_main]
venue = "POLYMARKET"
[clients.polymarket_main.execution]
signature_type = "poly_gnosis_safe"
funder = "0x1111111111111111111111111111111111111111"
[clients.polymarket_main.secrets]
private_key_ssm_path = "/signer"
"""

EVIDENCE = """\
schema_version = 1
[adapter_abi]
repository = "https://github.com/Polymarket/ctf-exchange-v2"
revision = "ccc0596074f4dfd62c944fbca4de252893b82b4b"
deployment_source_url = "https://docs.polymarket.com/resources/contracts"
deployment_observed_date = "2026-07-16"
deployment_fact_format_version = 3
deployment_fact_sha256 = "223c425c49db5c1e3da22c5f9113e892d4f78e2aa1ab1d7ed23c39e04931e1c7"
standard_source_path = "src/adapters/CtfCollateralAdapter.sol"
standard_source_sha256 = "f9f85b1ac652030bf458be2130b5f977fa6670a04b2ad412241c9e9b0c444a90"
negative_risk_source_path = "src/adapters/NegRiskCtfCollateralAdapter.sol"
negative_risk_source_sha256 = "2461eb793fa5571a6902a52c5276f02a8621814fdc026cf3a7814879b1b3db76"
function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"
function_selector = "0x01b7037c"
[safe_request]
repository = "https://github.com/Polymarket/builder-relayer-client"
revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"
builder_source_path = "src/builder/safe.ts"
builder_source_sha256 = "1142cb7fe786128361586d6fc9313a3e120e1633bdfc064169bfa78951d66cc5"
types_source_path = "src/types.ts"
types_source_sha256 = "059c02b19a23d57e7b354df8c01d706cf508c27460067c1d57dad96cf5455ad3"
signature_pack_source_path = "src/utils/index.ts"
signature_pack_source_sha256 = "0a1b6036fb7e3f7d1629002a491a448974a69c7556741f449c441cb3e3af2941"
operation = 0
value = "0"
safe_tx_gas = "0"
base_gas = "0"
gas_price = "0"
gas_token = "0x0000000000000000000000000000000000000000"
refund_receiver = "0x0000000000000000000000000000000000000000"
metadata = ""
"""

CARGO = """\
[package]
name = "redemption-fence-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
alloy-signer = "=2.1.0"
alloy-signer-local = "=2.1.0"
[[test]]
name = "polymarket_redemption_preparation"
path = "tests/polymarket_redemption_preparation.rs"
"""


class PolymarketRedemptionPreparationVerifierTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], pathlib.Path]:
        temporary = tempfile.TemporaryDirectory()
        root = pathlib.Path(temporary.name)
        for directory in (
            "src/bolt_v3_polymarket_redemption",
            "config",
            "tests",
        ):
            (root / directory).mkdir(parents=True)
        (root / "src/bolt_v3_polymarket_redemption.rs").write_text(
            "// implementation is compiler-verified\n", encoding="utf-8"
        )
        (root / "src/bolt_v3_polymarket_redemption/generated.rs").write_text(
            "// generated projection\n", encoding="utf-8"
        )
        (root / "config/polymarket-redemption.toml").write_text(
            RUNTIME, encoding="utf-8"
        )
        (root / "config/root.toml").write_text(ROOT_RUNTIME, encoding="utf-8")
        (root / "config/polymarket-redemption-source-evidence.toml").write_text(
            EVIDENCE, encoding="utf-8"
        )
        (root / "tests/polymarket_redemption_preparation.rs").write_text(
            "mod polymarket_redemption_preparation_compile_fail;\n",
            encoding="utf-8",
        )
        (root / "tests/polymarket_redemption_preparation_compile_fail.rs").write_text(
            "// compiler evidence\n", encoding="utf-8"
        )
        (root / "Cargo.toml").write_text(CARGO, encoding="utf-8")
        return temporary, root

    def test_closed_fixture_passes(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        self.assertEqual(verifier.boundary_errors(root), [])

    def test_source_text_is_not_policy_input(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "src/main.rs").write_text(
            "fn main() { prepare_redemption_request(); }\n", encoding="utf-8"
        )
        self.assertEqual(verifier.boundary_errors(root), [])

    def test_missing_required_artifact_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "tests/polymarket_redemption_preparation_compile_fail.rs").unlink()
        self.assertTrue(
            any("missing required" in error for error in verifier.boundary_errors(root))
        )

    def test_activation_must_remain_false(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        runtime = root / "config/polymarket-redemption.toml"
        runtime.write_text(
            RUNTIME.replace("production_activation_enabled = false", "production_activation_enabled = true"),
            encoding="utf-8",
        )
        self.assertIn(
            "production_activation_enabled must remain false",
            verifier.boundary_errors(root),
        )

    def test_parsed_toml_authority_is_single(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "config/duplicate.toml").write_text(
            '[duplicate]\nstandard_adapter_target = "0x0"\n', encoding="utf-8"
        )
        self.assertTrue(
            any(
                "runtime field standard_adapter_target" in error
                for error in verifier.boundary_errors(root)
            )
        )

    def test_toml_comments_do_not_create_authority(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "config/comment.toml").write_text(
            "# standard_adapter_target = \"not-a-value\"\n", encoding="utf-8"
        )
        self.assertEqual(verifier.boundary_errors(root), [])

    def test_wallet_and_signer_runtime_duplicates_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        runtime = root / "config/polymarket-redemption.toml"
        runtime.write_text(
            RUNTIME
            + '\nsafe_address = "0x1111111111111111111111111111111111111111"\n'
            + 'signer_private_key_ssm_path = "/signer"\n',
            encoding="utf-8",
        )
        self.assertTrue(
            any(
                "single-sourced from config/root.toml" in error
                for error in verifier.boundary_errors(root)
            )
        )

    def test_root_client_must_exist(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        runtime = root / "config/polymarket-redemption.toml"
        runtime.write_text(
            RUNTIME.replace('root_client = "polymarket_main"', 'root_client = "missing"'),
            encoding="utf-8",
        )
        self.assertIn(
            "wallet_authority.root_client must exist in config/root.toml",
            verifier.boundary_errors(root),
        )

    def test_deployment_fact_hash_binds_runtime_protocol_facts(self) -> None:
        mutations = (
            ("chain_id = 137", "chain_id = 138"),
            (
                'collateral_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"',
                'collateral_asset = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
            ),
            (
                'standard_adapter_target = "0xAdA100Db00Ca00073811820692005400218FcE1f"',
                'standard_adapter_target = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
            ),
            (
                'negative_risk_adapter_target = "0xadA2005600Dec949baf300f4C6120000bDB6eAab"',
                'negative_risk_adapter_target = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"',
            ),
            (
                "0x0000000000000000000000000000000000000000000000000000000000000000",
                "0x0000000000000000000000000000000000000000000000000000000000000001",
            ),
            ('dummy_index_sets = ["1", "2"]', 'dummy_index_sets = ["1", "3"]'),
        )
        for expected, replacement in mutations:
            with self.subTest(expected=expected):
                temporary, root = self.fixture()
                self.addCleanup(temporary.cleanup)
                runtime = root / "config/polymarket-redemption.toml"
                runtime.write_text(
                    RUNTIME.replace(expected, replacement), encoding="utf-8"
                )
                self.assertTrue(
                    any(
                        "deployment fact hash must bind" in error
                        for error in verifier.boundary_errors(root)
                    )
                )

    def test_source_evidence_paths_and_hashes_are_pinned(self) -> None:
        mutations = (
            ("https://docs.polymarket.com/resources/contracts", "https://example.invalid"),
            ("2026-07-16", "2026-07-15"),
            ("deployment_fact_format_version = 3", "deployment_fact_format_version = 2"),
            (
                "f9f85b1ac652030bf458be2130b5f977fa6670a04b2ad412241c9e9b0c444a90",
                "1" * 64,
            ),
            ("src/builder/safe.ts", "src/builder/other.ts"),
            (
                "0a1b6036fb7e3f7d1629002a491a448974a69c7556741f449c441cb3e3af2941",
                "5" * 64,
            ),
        )
        for expected, replacement in mutations:
            with self.subTest(expected=expected):
                temporary, root = self.fixture()
                self.addCleanup(temporary.cleanup)
                evidence = root / "config/polymarket-redemption-source-evidence.toml"
                evidence.write_text(
                    EVIDENCE.replace(expected, replacement), encoding="utf-8"
                )
                self.assertTrue(
                    any(
                        "source evidence must remain pinned" in error
                        for error in verifier.boundary_errors(root)
                    )
                )

    def test_signer_dependencies_are_exact(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "Cargo.toml").write_text(
            CARGO.replace('alloy-signer = "=2.1.0"', 'alloy-signer = "2"'),
            encoding="utf-8",
        )
        self.assertTrue(
            any(
                "direct signer dependency must remain exact" in error
                for error in verifier.boundary_errors(root)
            )
        )

    def test_compile_fail_target_is_structurally_wired(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "Cargo.toml").write_text(
            CARGO.replace(
                'name = "polymarket_redemption_preparation"',
                'name = "different_test"',
            ),
            encoding="utf-8",
        )
        self.assertIn("compile-fail test target is not wired", verifier.boundary_errors(root))


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
