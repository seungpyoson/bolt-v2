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
arena_bytes = 160
slot_bytes = 16
""",
            encoding="utf-8",
        )
        (self.root / "src" / "bolt_v3_risk_closure_workspace_generated.rs").write_text(
            "// generated fixture\n",
            encoding="utf-8",
        )
        (self.root / "src" / "bolt_v3_risk_closure_workspace.rs").write_text(
            "struct RiskClosureWorkspaceConfig { slot_bytes: usize }\n",
            encoding="utf-8",
        )

    def test_accepts_one_toml_authority_and_derived_rust_field(self) -> None:
        self.assertEqual(verifier.authority_errors(self.root), [])

    def test_rejects_a_second_toml_slot_size_authority(self) -> None:
        (self.root / "config" / "duplicate.toml").write_text(
            "[risk_closure_workspaces]\nslot_bytes = 16\n",
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
