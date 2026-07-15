#!/usr/bin/env python3
"""Regression tests for the mechanically disabled AO-REDEEM source fence."""

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
            source = REPO_ROOT / relative
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
        return root

    def test_current_tree_satisfies_redeem_fence(self) -> None:
        self.assertEqual(verifier.verify(REPO_ROOT), [])

    def test_manifest_drift_fails_closed(self) -> None:
        root = self.fixture()
        path = root / verifier.MANIFEST_PATH
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "ccc0596074f4dfd62c944fbca4de252893b82b4b",
                "0000000000000000000000000000000000000000",
                1,
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("reviewed revision" in error for error in verifier.verify(root)))

    def test_source_snapshot_drift_fails_closed(self) -> None:
        root = self.fixture()
        path = root / "tests/fixtures/bolt_v3/redeem/source/ctf-collateral-adapter.txt"
        path.write_text(path.read_text(encoding="utf-8") + "drift\n", encoding="utf-8")
        self.assertTrue(any("source snapshot digest" in error for error in verifier.verify(root)))

    def test_competing_nonce_cannot_be_claimed_without_conformance(self) -> None:
        root = self.fixture()
        path = root / verifier.MANIFEST_PATH
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                'competing_same_nonce = "unproven"',
                'competing_same_nonce = "supported"',
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("competing-same-nonce" in error for error in verifier.verify(root)))

    def test_config_cannot_enable_or_add_non_ssm_credentials(self) -> None:
        root = self.fixture()
        path = root / verifier.CONFIG_PATH
        config = path.read_text(encoding="utf-8")
        config = config.replace("enabled = false", "enabled = true", 1)
        config += '\napi_key = "sentinel-secret"\n'
        path.write_text(config, encoding="utf-8")
        errors = verifier.verify(root)
        self.assertTrue(any("mechanically disabled" in error for error in errors))
        self.assertTrue(any("SSM-only" in error for error in errors))

    def test_config_deployment_drift_fails_closed(self) -> None:
        root = self.fixture()
        path = root / verifier.CONFIG_PATH
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "0xADa100874d00e3331D00F2007a9c336a65009718",
                "0x0000000000000000000000000000000000000001",
                1,
            ),
            encoding="utf-8",
        )
        self.assertTrue(any("not manifest-bound" in error for error in verifier.verify(root)))

    def test_active_caller_and_durable_state_are_rejected(self) -> None:
        root = self.fixture()
        path = root / verifier.MODULE_PATH
        source = path.read_text(encoding="utf-8")
        source += "\npub async fn submit_redemption() {}\n"
        source += "\nuse std::fs::File;\n"
        path.write_text(source, encoding="utf-8")
        errors = verifier.verify(root)
        self.assertTrue(any("active caller" in error for error in errors))
        self.assertTrue(any("durable state" in error for error in errors))

    def test_observability_and_strategy_reachability_are_rejected(self) -> None:
        root = self.fixture()
        module = root / verifier.MODULE_PATH
        module.write_text(
            module.read_text(encoding="utf-8") + '\nlog::info!("sentinel");\n',
            encoding="utf-8",
        )
        strategy = root / "src/strategies/redeem.rs"
        strategy.parent.mkdir(parents=True, exist_ok=True)
        strategy.write_text(
            "use crate::bolt_v3_providers::polymarket::redemption;\n",
            encoding="utf-8",
        )
        errors = verifier.verify(root)
        self.assertTrue(any("observability sink" in error for error in errors))
        self.assertTrue(any("disabled reachability" in error for error in errors))


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
