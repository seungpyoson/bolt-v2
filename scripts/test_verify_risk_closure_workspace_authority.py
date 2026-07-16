#!/usr/bin/env python3
"""Tests for the single-authority risk-closure workspace fence."""

from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_risk_closure_workspace_authority as verifier


class RiskClosureWorkspaceAuthorityVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "config").mkdir()
        (self.root / "src").mkdir()
        (self.root / "config" / "risk-closure-workspaces.toml").write_text(
            """
schema_version = 1
production_activation_enabled = false
[risk_closure_workspaces]
arena_bytes = 167772160
slot_bytes = 16777216
""",
            encoding="utf-8",
        )
        (self.root / "src" / "bolt_v3_risk_closure_workspace_generated.rs").write_text(
            "const RISK_CLOSURE_WORKSPACE_CONFIG: RiskClosureWorkspaceConfig = fixture();\n",
            encoding="utf-8",
        )
        (self.root / "src" / "bolt_v3_risk_closure_workspace.rs").write_text(
            "struct RiskClosureWorkspaceConfig { slot_bytes: usize }\n",
            encoding="utf-8",
        )

    def test_accepts_one_toml_authority_and_derived_rust_field(self) -> None:
        self.assertEqual(verifier.authority_errors(self.root), [])

    def test_rejects_public_workspace_configuration_type(self) -> None:
        (self.root / "src" / "bolt_v3_risk_closure_workspace.rs").write_text(
            "pub struct RiskClosureWorkspaceConfig { slot_bytes: usize }\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("configuration type must remain private" in error for error in errors))

    def test_rejects_public_generated_workspace_configuration(self) -> None:
        (self.root / "src" / "bolt_v3_risk_closure_workspace_generated.rs").write_text(
            "pub const RISK_CLOSURE_WORKSPACE_CONFIG: RiskClosureWorkspaceConfig = fixture();\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("generated workspace configuration must remain private" in error for error in errors))

    def test_rejects_workspace_configuration_reference_outside_owner(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "use crate::bolt_v3_risk_closure_workspace::RISK_CLOSURE_WORKSPACE_CONFIG;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("private workspace configuration referenced" in error for error in errors))

    def test_rejects_a_second_toml_slot_size_authority(self) -> None:
        (self.root / "config" / "duplicate.toml").write_text(
            "[risk_closure_workspaces]\nslot_bytes = 16777216\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("exactly one TOML authority" in error for error in errors))

    def test_rejects_a_runtime_workspace_size_literal(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "const RISK_CLOSURE_WORKSPACE_BYTES: usize = 16_777_216;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size literal" in error for error in errors))

    def test_rejects_hexadecimal_runtime_workspace_size_literal(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "let workspace = vec![0_u8; 0x0100_0000usize];\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size literal" in error for error in errors))

    def test_rejects_runtime_workspace_size_expression(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "let workspace = vec![0_u8; 16 * 1024 * 1024];\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size expression" in error for error in errors))

    def test_rejects_runtime_workspace_size_shift_expression(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "let workspace = vec![0_u8; 1usize << 24];\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size expression" in error for error in errors))

    def test_rejects_parenthesized_runtime_workspace_size_expression(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "const HIDDEN: usize = 16 * (1024 * 1024);\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size expression" in error for error in errors))

    def test_rejects_nested_shift_arithmetic_expression(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "const HIDDEN: usize = 1 << (12 + 12);\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size expression" in error for error in errors))

    def test_scans_root_build_script(self) -> None:
        (self.root / "build.rs").write_text(
            "const CLOSURE_SLOT_BYTES: usize = 0x0100_0000;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("build.rs" in error for error in errors))

    def test_scans_workspace_crate_production_sources(self) -> None:
        crate_source = self.root / "crates" / "consumer" / "src"
        crate_source.mkdir(parents=True)
        (crate_source / "lib.rs").write_text(
            "const CLOSURE_SLOT_BYTES: usize = 1 << 24;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("crates/consumer/src/lib.rs" in error for error in errors))

    def test_rejects_a_symbolic_runtime_workspace_size_authority(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "const RISK_CLOSURE_WORKSPACE_SLOT_BYTES: usize = usize::MAX;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("symbolic workspace-size authority" in error for error in errors))

    def test_malformed_toml_fails_closed_during_authority_census(self) -> None:
        (self.root / "config" / "malformed.toml").write_text(
            "[risk_closure_workspaces\nslot_bytes = 16\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("cannot inspect config/malformed.toml" in error for error in errors))


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
