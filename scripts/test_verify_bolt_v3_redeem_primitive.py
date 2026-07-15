#!/usr/bin/env python3
"""Negative regression tests for the AO-REDEEM structural fence."""

from __future__ import annotations

import pathlib
import shutil
import tempfile
import unittest

import verify_bolt_v3_redeem_primitive as verifier


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


class RedeemPrimitiveFenceTests(unittest.TestCase):
    def fixture(self) -> pathlib.Path:
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        for relative in verifier.REQUIRED_PATHS:
            source, destination = REPO_ROOT / relative, root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        return root

    def mutate(self, root: pathlib.Path, relative: str, old: str, new: str) -> None:
        path = root / relative
        source = path.read_text(encoding="utf-8")
        self.assertIn(old, source)
        path.write_text(source.replace(old, new, 1), encoding="utf-8")

    def assert_rejected(self, root: pathlib.Path, needle: str) -> None:
        errors = verifier.verify(root)
        self.assertTrue(any(needle in error for error in errors), errors)

    def test_current_tree_satisfies_redeem_fence(self) -> None:
        self.assertEqual(verifier.verify(REPO_ROOT), [])

    def test_manifest_and_source_snapshot_drift_fail_closed(self) -> None:
        root = self.fixture()
        self.mutate(root, str(verifier.MANIFEST_PATH), verifier.EXPECTED_REVISIONS["adapter"], "0" * 40)
        self.assert_rejected(root, "reviewed revision")
        root = self.fixture()
        path = root / "tests/fixtures/bolt_v3/redeem/source/relayer-safe-builder.txt"
        path.write_text(path.read_text(encoding="utf-8") + "drift\n", encoding="utf-8")
        self.assert_rejected(root, "source snapshot digest")

    def test_enable_and_alternate_secret_fail_closed(self) -> None:
        root = self.fixture()
        self.mutate(root, str(verifier.CONFIG_PATH), "enabled = false", "enabled = true")
        self.assert_rejected(root, "disabled")
        root = self.fixture()
        path = root / verifier.CONFIG_PATH
        path.write_text(path.read_text(encoding="utf-8") + '\napi_key = "sentinel"\n', encoding="utf-8")
        self.assert_rejected(root, "SSM-only")

    def test_wrapper_alias_reexport_renamed_and_exempt_reachability_fail(self) -> None:
        cases = {
            "src/wrapper.rs": "fn wrapper(x: ExactConditionSnapshotLease) { drop(x); }",
            "src/alias.rs": "use crate::bolt_v3_providers::polymarket::redemption as hidden;",
            "src/reexport.rs": "pub use crate::bolt_v3_providers::polymarket::redemption::*;",
            "src/renamed.rs": "use crate::bolt_v3_providers::polymarket::redemption::build_request_pair as prepare;",
        }
        for relative, source in cases.items():
            with self.subTest(relative=relative):
                root = self.fixture()
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source, encoding="utf-8")
                self.assert_rejected(root, "structural disabled reachability")
        for relative, addition, expected in (
            ("src/bolt_v3_providers/polymarket.rs", "\npub use redemption::*;\n", "provider parent"),
            ("src/bolt_v3_providers/boundary_registry.rs", "\nuse super::polymarket::redemption::build_request_pair;\n", "boundary registry"),
        ):
            root = self.fixture()
            path = root / relative
            path.write_text(path.read_text(encoding="utf-8") + addition, encoding="utf-8")
            self.assert_rejected(root, expected)

    def test_build_generated_and_direct_construction_fail(self) -> None:
        root = self.fixture()
        (root / "build.rs").write_text("fn main() { let _: Option<OriginalMayHaveStartedPermit> = None; }", encoding="utf-8")
        self.assert_rejected(root, "build/generated")
        root = self.fixture()
        path = root / "src/direct.rs"
        path.write_text("fn forge() { let _ = ExactConditionSnapshotLease {}; }", encoding="utf-8")
        self.assert_rejected(root, "structural disabled reachability")

    def test_production_capability_issuer_clone_and_formatting_fail(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "capability.rs"
        source = path.read_text(encoding="utf-8")
        insertion = "pub fn forge() -> ExactConditionSnapshotLease { panic!() }\n"
        path.write_text(insertion + source, encoding="utf-8")
        self.assert_rejected(root, "production can mint")
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "capability.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(source.replace("pub struct FreshPreSendValidation", "#[derive(Debug, Clone)]\npub struct FreshPreSendValidation", 1), encoding="utf-8")
        self.assert_rejected(root, "formatting/serialization")

    def test_send_without_durable_permit_and_fence_first_fail(self) -> None:
        root = self.fixture()
        path = root / "src/bolt_v3_providers/polymarket/redemption/request.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(source.replace("durable: OriginalMayHaveStartedPermit,", "durable: (),"), encoding="utf-8")
        self.assert_rejected(root, "capability binding incomplete: authorize_original")
        root = self.fixture()
        path = root / "src/bolt_v3_providers/polymarket/redemption/request.rs"
        source = path.read_text(encoding="utf-8")
        source = source.replace("impl OriginalMayHaveStartedRequest {", "impl PreparedRequestPair {", 1)
        path.write_text(source, encoding="utf-8")
        self.assert_rejected(root, "fence-first")

    def test_provider_registry_terminal_release_and_effect_sink_fail(self) -> None:
        for symbol in ("ConditionRegistry", "TerminalLeaseCertificate", "write_request"):
            root = self.fixture()
            path = root / verifier.REDEMPTION_ROOT / "request.rs"
            path.write_text(path.read_text(encoding="utf-8") + f"\nstruct {symbol};\n", encoding="utf-8")
            self.assert_rejected(root, "provider-owned authority")
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "request.rs"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nfn emit<W: std::io::Write>(_sink: &mut W) {}\n",
            encoding="utf-8",
        )
        self.assert_rejected(root, "arbitrary effect sink")

    def test_arbitrary_string_credentials_and_aggregate_chain_truth_fail(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "config.rs"
        path.write_text(path.read_text(encoding="utf-8") + "\ntype BadSecret = Zeroizing<String>;\n", encoding="utf-8")
        self.assert_rejected(root, "credential acquisition")
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "wire.rs"
        path.write_text(path.read_text(encoding="utf-8") + "\nstruct ChainWire { winner: bool }\n", encoding="utf-8")
        self.assert_rejected(root, "caller-classified")

    def test_forged_terminal_outcome_provenance_fails(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "request.rs"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nfn forge() { let _ = SourceBoundVerifiedOutcome::from_raw_verifier(RedemptionResolution::RedemptionFinalized); }\n",
            encoding="utf-8",
        )
        self.assert_rejected(root, "outcome provenance")

    def test_raw_schema_and_finality_removal_fail(self) -> None:
        root = self.fixture()
        self.mutate(root, "src/bolt_v3_providers/polymarket/redemption/wire.rs", "#[serde(deny_unknown_fields)]\nstruct NonceCallWire", "struct NonceCallWire")
        self.assert_rejected(root, "raw source-bound schema")
        root = self.fixture()
        path = root / "src/bolt_v3_providers/polymarket/redemption/wire.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(source.replace("required_confirmations", "ignored_depth"), encoding="utf-8")
        self.assert_rejected(root, "configured finality")

        root = self.fixture()
        self.mutate(
            root,
            "src/bolt_v3_providers/polymarket/redemption/wire.rs",
            "    finalized_head: FinalizedChainSourceResponse,\n",
            "",
        )
        self.assert_rejected(root, "response set is partial")

    def test_raw_getter_and_public_payload_field_fail(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "request.rs"
        path.write_text(path.read_text(encoding="utf-8") + "\nimpl PreparedRequestPair { pub fn body_bytes(&self) -> &[u8] { &[] } }\n", encoding="utf-8")
        self.assert_rejected(root, "raw/effect surface")
        root = self.fixture()
        self.mutate(root, "src/bolt_v3_providers/polymarket/redemption/request.rs", "pub struct PreparedRequestPair {\n    original:", "pub struct PreparedRequestPair {\n    pub original:")
        self.assert_rejected(root, "public field")

    def test_arbitrary_reader_cannot_mint_source_proof(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "wire.rs"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nimpl RelayerSourceResponse { pub fn fabricate(_reader: impl std::io::Read) -> Self { panic!() } }\n",
            encoding="utf-8",
        )
        self.assert_rejected(root, "arbitrary reader can mint source proof")

    def test_source_capabilities_and_outcome_binding_cannot_be_weakened(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "wire.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(
            source.replace(
                "pub struct FinalizedChainSourceResponse {",
                "pub struct FinalizedChainSourceResponse { pub bytes: Vec<u8>,",
                1,
            ),
            encoding="utf-8",
        )
        self.assert_rejected(root, "public field")

        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "wire.rs"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nimpl RelayerSourceResponse { pub fn forge() -> Self { panic!() } }\n",
            encoding="utf-8",
        )
        self.assert_rejected(root, "production can mint source response")

        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "query.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(
            source.replace("    action_digest: [u8; WORD_BYTES],\n", "", 1),
            encoding="utf-8",
        )
        self.assert_rejected(root, "verified outcome binding is incomplete")

    def test_terminal_resolution_requires_exact_binding_consumption(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "query.rs"
        source = path.read_text(encoding="utf-8")
        path.write_text(
            source.replace(
                "pub fn consume_after_original(",
                "pub fn resolution_unchecked(",
                1,
            ),
            encoding="utf-8",
        )
        self.assert_rejected(root, "exact terminal consumption")

    def test_two_claim_only_terminal_state_is_rejected(self) -> None:
        root = self.fixture()
        self.mutate(
            root,
            "src/bolt_v3_providers/polymarket/redemption/wire.rs",
            "    collateral_balance: &'a str,\n",
            "",
        )
        self.assert_rejected(root, "post-state balance contract is incomplete")

    def test_runtime_dummy_index_sets_cannot_be_reconstructed(self) -> None:
        root = self.fixture()
        path = root / verifier.REDEMPTION_ROOT / "config.rs"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\nstruct BadRuntimeIndexes { dummy_index_sets: [1, 2] }\n",
            encoding="utf-8",
        )
        self.assert_rejected(root, "runtime dummy index sets are reconstructed")

    def test_prepared_action_must_own_exact_context_identity(self) -> None:
        root = self.fixture()
        self.mutate(
            root,
            "src/bolt_v3_providers/polymarket/redemption/request.rs",
            "    profile_digest: [u8; WORD_BYTES],\n",
            "",
        )
        self.assert_rejected(root, "prepared action context binding is incomplete")

    def test_source_response_must_own_exact_query_binding(self) -> None:
        root = self.fixture()
        self.mutate(
            root,
            "src/bolt_v3_providers/polymarket/redemption/wire.rs",
            "    request_binding: ExactQueryBinding,\n",
            "",
        )
        self.assert_rejected(root, "source response query binding is incomplete")


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
