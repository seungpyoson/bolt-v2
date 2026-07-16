#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_polymarket_redemption_preparation as verifier


OWNER = """\
use crate::{
    bolt_v3_providers::SsmSecretResolver,
    bolt_v3_risk_closure_workspace::RiskClosureWorkspaceLease,
    secrets::SsmResolverSession,
};

pub enum AttemptKind { Original, Fence }
pub struct PreparedRequest<'request> { bytes: &'request [u8] }
pub struct ResolvedRedemptionCredentials { signer_private_key: zeroize::Zeroizing<String> }
pub struct RedemptionPreparationConfig;
pub struct RedemptionRequestInput;

pub fn resolve_redemption_credentials(
    session: &SsmResolverSession,
    config: &RedemptionPreparationConfig,
) { let _ = (session, config); }

pub fn prepare_redemption_request(
    lease: &mut RiskClosureWorkspaceLease,
    config: &RedemptionPreparationConfig,
    credentials: &ResolvedRedemptionCredentials,
    input: RedemptionRequestInput,
    attempt: AttemptKind,
    use_prepared: impl for<'request> FnOnce(PreparedRequest<'request>),
) {
    let _ = (config, credentials, input, attempt);
    lease.with_workspace_mut(|workspace| {
        let prepared = PreparedRequest { bytes: workspace };
        use_prepared(prepared);
    });
}

#[cfg(test)]
mod tests {
    use crate::bolt_v3_risk_closure_workspace::RiskClosureWorkspaceAuthority;
}
"""

GENERATED = """\
pub(super) const POLYMARKET_REDEMPTION_PREPARATION_CONFIG: RedemptionPreparationConfig = RedemptionPreparationConfig;
pub(super) const POLYMARKET_REDEMPTION_PROTOCOL: RedemptionProtocolFacts = RedemptionProtocolFacts;
"""

RUNTIME = """\
schema_version = 1
production_activation_enabled = false
[redemption]
chain_id = 137
wallet_type = "SAFE"
safe_address = "0x1111111111111111111111111111111111111111"
collateral_asset = "0x2222222222222222222222222222222222222222"
output_asset = "0x3333333333333333333333333333333333333333"
standard_adapter_target = "0x4444444444444444444444444444444444444444"
negative_risk_adapter_target = "0x5555555555555555555555555555555555555555"
parent_collection_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
dummy_index_sets = ["1", "2"]
[protocol_bounds]
maximum_safe_nonce_decimal_digits = 78
[credential_set]
aws_region = "eu-west-2"
signer_private_key_ssm_path = "/signer"
builder_api_key_ssm_path = "/api-key"
builder_api_secret_ssm_path = "/api-secret"
builder_passphrase_ssm_path = "/passphrase"
"""

EVIDENCE = """\
schema_version = 1
[adapter_abi]
repository = "https://github.com/Polymarket/ctf-exchange-v2"
revision = "ccc0596074f4dfd62c944fbca4de252893b82b4b"
deployment_source_path = "README.md"
deployment_source_sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
standard_source_path = "src/adapters/CtfCollateralAdapter.sol"
standard_source_sha256 = "2222222222222222222222222222222222222222222222222222222222222222"
negative_risk_source_path = "src/adapters/NegRiskCtfCollateralAdapter.sol"
negative_risk_source_sha256 = "3333333333333333333333333333333333333333333333333333333333333333"
function_signature = "redeemPositions(address,bytes32,bytes32,uint256[])"
function_selector = "0x01b7037c"
[safe_request]
repository = "https://github.com/Polymarket/builder-relayer-client"
revision = "9122f6fb1856f1ecfe4406685bfa19a2c5a7b290"
builder_source_path = "src/builder/safe.ts"
builder_source_sha256 = "4444444444444444444444444444444444444444444444444444444444444444"
types_source_path = "src/types.ts"
types_source_sha256 = "5555555555555555555555555555555555555555555555555555555555555555"
signature_pack_source_path = "src/utils/index.ts"
signature_pack_source_sha256 = "6666666666666666666666666666666666666666666666666666666666666666"
operation = 0
value = "0"
safe_tx_gas = "0"
base_gas = "0"
gas_price = "0"
gas_token = "0x0000000000000000000000000000000000000000"
refund_receiver = "0x0000000000000000000000000000000000000000"
metadata = ""
"""


class PolymarketRedemptionPreparationVerifierTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], pathlib.Path]:
        temporary = tempfile.TemporaryDirectory()
        root = pathlib.Path(temporary.name)
        for directory in ("src", "src/bolt_v3_polymarket_redemption", "config", "tests"):
            (root / directory).mkdir()
        (root / "src/bolt_v3_polymarket_redemption.rs").write_text(OWNER, encoding="utf-8")
        (root / "src/bolt_v3_polymarket_redemption/generated.rs").write_text(
            GENERATED, encoding="utf-8"
        )
        (root / "src/lib.rs").write_text(
            "pub mod bolt_v3_polymarket_redemption;\n", encoding="utf-8"
        )
        (root / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
        (root / "config/polymarket-redemption.toml").write_text(RUNTIME, encoding="utf-8")
        (root / "config/polymarket-redemption-source-evidence.toml").write_text(
            EVIDENCE, encoding="utf-8"
        )
        (root / "tests/polymarket_redemption_preparation_compile_fail.rs").write_text(
            "RiskClosureWorkspaceReservation\nprepared_request_cannot_escape\nserde_json::to_string\n",
            encoding="utf-8",
        )
        (root / "Cargo.toml").write_text(
            'alloy-signer = "=2.1.0"\nalloy-signer-local = "=2.1.0"\n'
            '[[test]]\nname = "polymarket_redemption_preparation"\n'
            'path = "tests/polymarket_redemption_preparation.rs"\n',
            encoding="utf-8",
        )
        return temporary, root

    def test_closed_fixture_passes(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        self.assertEqual(verifier.boundary_errors(root), [])

    def test_new_risk_authority_in_production_owner_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "pub enum AttemptKind", "use crate::bolt_v3_risk_closure_workspace::RiskClosureWorkspaceReservation;\npub enum AttemptKind"
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("new-risk or authority surface" in error for error in verifier.boundary_errors(root)))

    def test_network_sink_in_owner_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "#[cfg(test)]", "fn sink(client: reqwest::Client) {}\n\n#[cfg(test)]", 1
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("network or durable sink" in error for error in verifier.boundary_errors(root)))

    def test_workspace_geometry_in_owner_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "#[cfg(test)]", "const SLOT_BYTES: usize = 4096;\n\n#[cfg(test)]", 1
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("workspace geometry" in error for error in verifier.boundary_errors(root)))

    def test_owned_prepared_bytes_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(owner.read_text(encoding="utf-8").replace("bytes: &'request [u8]", "bytes: Vec<u8>"), encoding="utf-8")
        self.assertTrue(any("borrowed slice" in error for error in verifier.boundary_errors(root)))

    def test_active_caller_outside_module_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "src/main.rs").write_text("fn main() { prepare_redemption_request(); }\n", encoding="utf-8")
        self.assertTrue(any("active production caller" in error for error in verifier.boundary_errors(root)))

    def test_second_session_or_secret_backend_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "#[cfg(test)]",
                'fn bad() { SsmResolverSession::new(); std::env::var("SECRET"); }\n\n#[cfg(test)]',
                1,
            ),
            encoding="utf-8",
        )
        errors = verifier.boundary_errors(root)
        self.assertTrue(any("second SSM session" in error for error in errors))
        self.assertTrue(any("alternate secret backend" in error for error in errors))


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
