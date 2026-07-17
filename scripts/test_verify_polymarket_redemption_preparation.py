#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_polymarket_redemption_preparation as verifier


OWNER = """\
use crate::{
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
) { session.resolve("region", "path"); let _ = config; }

fn resolve_redemption_credentials_from<E>(
    config: &RedemptionPreparationConfig,
    resolver: impl FnMut(&str, &str) -> Result<String, E>,
) { let _ = (config, resolver); }

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
[wallet_authority]
root_client = "polymarket_main"
[redemption]
chain_id = 137
collateral_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"
output_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"
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
deployment_fact_format_version = 2
deployment_fact_sha256 = "7844264e5c6c456224820af716c000438d72736a5f45315ae88f4f92dc068667"
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
        (root / "config/root.toml").write_text(ROOT_RUNTIME, encoding="utf-8")
        (root / "config/polymarket-redemption-source-evidence.toml").write_text(
            EVIDENCE, encoding="utf-8"
        )
        (root / "tests/polymarket_redemption_preparation_compile_fail.rs").write_text(
            "RiskClosureWorkspaceReservation\nprepared_request_cannot_escape\nserde_json::to_string\n",
            encoding="utf-8",
        )
        (root / "Cargo.toml").write_text(
            '[package]\nname = "redemption-fence-fixture"\nversion = "0.0.0"\n'
            'edition = "2024"\n[dependencies]\n'
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

    def test_deployment_fact_hash_binds_runtime_protocol_facts(self) -> None:
        mutations = (
            ("chain_id = 137", "chain_id = 138"),
            (
                'collateral_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"',
                'collateral_asset = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
            ),
            (
                'output_asset = "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"',
                'output_asset = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"',
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
                    runtime.read_text(encoding="utf-8").replace(expected, replacement),
                    encoding="utf-8",
                )
                self.assertTrue(
                    any(
                        "deployment fact hash must bind" in error
                        for error in verifier.boundary_errors(root)
                    )
                )

    def test_source_evidence_paths_and_hashes_are_pinned(self) -> None:
        mutations = (
            ("https://docs.polymarket.com/resources/contracts", "https://docs.polymarket.com/resources/other"),
            ("2026-07-16", "2026-07-15"),
            ("deployment_fact_format_version = 2", "deployment_fact_format_version = 1"),
            ("7844264e5c6c456224820af716c000438d72736a5f45315ae88f4f92dc068667", "0" * 64),
            ("src/adapters/CtfCollateralAdapter.sol", "src/adapters/Other.sol"),
            ("f9f85b1ac652030bf458be2130b5f977fa6670a04b2ad412241c9e9b0c444a90", "1" * 64),
            ("src/adapters/NegRiskCtfCollateralAdapter.sol", "src/adapters/OtherNeg.sol"),
            ("2461eb793fa5571a6902a52c5276f02a8621814fdc026cf3a7814879b1b3db76", "2" * 64),
            ("src/builder/safe.ts", "src/builder/other.ts"),
            ("1142cb7fe786128361586d6fc9313a3e120e1633bdfc064169bfa78951d66cc5", "3" * 64),
            ("src/types.ts", "src/other-types.ts"),
            ("059c02b19a23d57e7b354df8c01d706cf508c27460067c1d57dad96cf5455ad3", "4" * 64),
            ("src/utils/index.ts", "src/utils/other.ts"),
            ("0a1b6036fb7e3f7d1629002a491a448974a69c7556741f449c441cb3e3af2941", "5" * 64),
        )
        for expected, replacement in mutations:
            with self.subTest(expected=expected):
                temporary, root = self.fixture()
                self.addCleanup(temporary.cleanup)
                manifest = root / "config/polymarket-redemption-source-evidence.toml"
                manifest.write_text(
                    manifest.read_text(encoding="utf-8").replace(expected, replacement),
                    encoding="utf-8",
                )
                self.assertTrue(
                    any(
                        "source evidence must remain pinned" in error
                        for error in verifier.boundary_errors(root)
                    )
                )

    def test_stale_adapter_deployments_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        runtime = root / "config/polymarket-redemption.toml"
        runtime.write_text(
            runtime.read_text(encoding="utf-8").replace(
                "0xAdA100Db00Ca00073811820692005400218FcE1f",
                "0xADa100874d00e3331D00F2007a9c336a65009718",
            ),
            encoding="utf-8",
        )
        self.assertTrue(
            any(
                "deployment fact hash" in error
                for error in verifier.boundary_errors(root)
            )
        )

    def test_wallet_and_signer_runtime_duplicates_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        runtime = root / "config/polymarket-redemption.toml"
        runtime.write_text(
            runtime.read_text(encoding="utf-8")
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

    def test_unapproved_prepared_field_owners_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        for field_type in ("Vec<u8>", "Box<[u8]>", "std::sync::Arc<[u8]>", "std::borrow::Cow<'request, [u8]>"):
            with self.subTest(field_type=field_type):
                mutated = owner.read_text(encoding="utf-8").replace("bytes: &'request [u8]", f"bytes: {field_type}")
                owner.write_text(mutated, encoding="utf-8")
                self.assertTrue(any("borrowed slice" in error for error in verifier.boundary_errors(root)))
                owner.write_text(OWNER, encoding="utf-8")

    def test_active_caller_outside_module_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "src/main.rs").write_text("fn main() { prepare_redemption_request(); }\n", encoding="utf-8")
        self.assertTrue(any("active production caller" in error for error in verifier.boundary_errors(root)))

    def test_active_caller_in_lib_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "src/lib.rs").write_text(
            "fn active() { prepare_redemption_request(); }\n", encoding="utf-8"
        )
        self.assertTrue(any("active production caller" in error for error in verifier.boundary_errors(root)))

    def test_aliased_function_pointer_reference_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        (root / "src/lib.rs").write_text(
            "use crate::bolt_v3_polymarket_redemption::prepare_redemption_request as prepare;\n"
            "fn active() { let prepare_pointer = prepare; }\n",
            encoding="utf-8",
        )
        self.assertTrue(any("active production caller" in error for error in verifier.boundary_errors(root)))

    def test_active_caller_in_owner_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "#[cfg(test)]",
                "fn active() { let prepare = prepare_redemption_request; }\n\n#[cfg(test)]",
                1,
            ),
            encoding="utf-8",
        )
        self.assertTrue(
            any("active production caller" in error for error in verifier.boundary_errors(root))
        )

    def test_active_caller_in_nested_package_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        package = root / "crates/consumer"
        (package / "src").mkdir(parents=True)
        (package / "Cargo.toml").write_text(
            '[package]\nname = "consumer"\nversion = "0.0.0"\nedition = "2024"\n',
            encoding="utf-8",
        )
        (package / "src/lib.rs").write_text(
            "fn active() {\n"
            "    let prepare = bolt_v2::bolt_v3_polymarket_redemption::"
            "prepare_redemption_request;\n"
            "}\n",
            encoding="utf-8",
        )
        self.assertTrue(
            any("active production caller" in error for error in verifier.boundary_errors(root))
        )

    def test_direct_secret_output_macros_are_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        snippets = (
            "fn leak(credentials: &ResolvedRedemptionCredentials) { "
            "dbg!(&credentials.signer_private_key); }",
            "fn leak(credentials: &ResolvedRedemptionCredentials) { "
            'print!("{}", credentials.signer_private_key.as_str()); }',
            "fn leak(credentials: &ResolvedRedemptionCredentials) { "
            'eprint!("{}", credentials.signer_private_key.as_str()); }',
        )
        for snippet in snippets:
            with self.subTest(snippet=snippet):
                owner.write_text(
                    OWNER.replace("#[cfg(test)]", f"{snippet}\n\n#[cfg(test)]", 1),
                    encoding="utf-8",
                )
                self.assertTrue(
                    any(
                        "forbidden logging or observability sink" in error
                        for error in verifier.boundary_errors(root)
                    )
                )

    def test_nonzeroizing_signer_parser_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            OWNER.replace(
                "#[cfg(test)]",
                "fn parse(credentials: &ResolvedRedemptionCredentials) {\n"
                "    let _ = PrivateKeySigner::from_str("
                "credentials.signer_private_key.as_str());\n"
                "}\n\n#[cfg(test)]",
                1,
            ),
            encoding="utf-8",
        )
        self.assertTrue(
            any(
                "zeroizing signer-key decode" in error
                for error in verifier.boundary_errors(root)
            )
        )

    def test_public_injectable_secret_resolver_is_rejected(self) -> None:
        temporary, root = self.fixture()
        self.addCleanup(temporary.cleanup)
        owner = root / "src/bolt_v3_polymarket_redemption.rs"
        owner.write_text(
            owner.read_text(encoding="utf-8").replace(
                "fn resolve_redemption_credentials_from<E>",
                "pub fn resolve_redemption_credentials_from<E>",
                1,
            ),
            encoding="utf-8",
        )
        self.assertTrue(
            any("injectable secret resolver" in error for error in verifier.boundary_errors(root))
        )

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
