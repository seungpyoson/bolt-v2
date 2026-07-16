#!/usr/bin/env python3
"""Tests for the single-authority risk-closure workspace fence."""

from __future__ import annotations

import pathlib
import tempfile
import unittest

import generate_risk_closure_workspace_config as generator
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
        (self.root / "src" / "bolt_v3_risk_closure_workspace").mkdir()
        (self.root / "src" / "bolt_v3_risk_closure_workspace" / "generated.rs").write_text(
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
        (self.root / "src" / "bolt_v3_risk_closure_workspace" / "generated.rs").write_text(
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

    def test_rejects_a_second_toml_authority_outside_config(self) -> None:
        crate = self.root / "crates" / "consumer"
        crate.mkdir(parents=True)
        (crate / "runtime.toml").write_text(
            "[risk_closure_workspaces]\nslot_bytes = 16777216\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("exactly one TOML authority" in error for error in errors))

    def test_rejects_any_second_canonical_toml_table(self) -> None:
        for text in (
            "[risk_closure_workspaces]\ncapacity = 10\n",
            "[risk_closure_workspaces]\n",
            "risk_closure_workspaces = 10\n",
        ):
            with self.subTest(text=text):
                crate = self.root / "crates" / "consumer"
                crate.mkdir(parents=True, exist_ok=True)
                (crate / "runtime.toml").write_text(text, encoding="utf-8")

                errors = verifier.authority_errors(self.root)

                self.assertTrue(
                    any("exactly one TOML authority" in error for error in errors)
                )

    def test_rejects_nested_toml_authority_outside_config(self) -> None:
        crate = self.root / "crates" / "consumer"
        crate.mkdir(parents=True)
        (crate / "runtime.toml").write_text(
            "[probe.risk_closure_workspaces]\ncapacity = 10\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(
            any('["probe"]["risk_closure_workspaces"]' in error for error in errors)
        )

    def test_rejects_nested_authority_in_canonical_toml(self) -> None:
        source = self.root / "config" / "risk-closure-workspaces.toml"
        source.write_text(
            source.read_text(encoding="utf-8")
            + "\n[probe.risk_closure_workspaces]\ncapacity = 10\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(
            any('["probe"]["risk_closure_workspaces"]' in error for error in errors)
        )

    def test_rejects_authority_nested_in_array_table(self) -> None:
        crate = self.root / "crates" / "consumer"
        crate.mkdir(parents=True)
        (crate / "runtime.toml").write_text(
            "[[owners]]\n[owners.risk_closure_workspaces]\nslot_bytes = 16777216\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(
            any(
                '["owners"][0]["risk_closure_workspaces"]' in error
                for error in errors
            )
        )

    def test_toml_key_path_rendering_is_unambiguous(self) -> None:
        self.assertNotEqual(
            verifier._render_toml_key_path(("probe.a", "risk_closure_workspaces")),
            verifier._render_toml_key_path(("probe", "a", "risk_closure_workspaces")),
        )
        self.assertNotEqual(
            verifier._render_toml_key_path(("owners[0]", "risk_closure_workspaces")),
            verifier._render_toml_key_path(("owners", 0, "risk_closure_workspaces")),
        )

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

    def test_rejects_arena_size_literal(self) -> None:
        (self.root / "src" / "consumer.rs").write_text(
            "const PLANTED_ARENA_TOTAL: usize = 167_772_160;\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(any("runtime workspace-size literal" in error for error in errors))

    def test_does_not_predict_arithmetic_equivalence(self) -> None:
        for source in (
            "const HASH_CHUNK: usize = 160 * 1024 * 1024;\n",
            "const BUFFER_CHUNK: usize = 16 * 1024 * 1024;\n",
            "const SHIFT_MASK: usize = 1usize << 24;\n",
            "const NESTED_ARITHMETIC: usize = 1 << (12 + 12);\n",
            "const COMPLEMENT_MASK: u32 = (!0_u32 >> 8) + 1;\n",
        ):
            with self.subTest(source=source):
                (self.root / "src" / "consumer.rs").write_text(
                    source,
                    encoding="utf-8",
                )

                errors = verifier.authority_errors(self.root)

                self.assertFalse(
                    any("runtime workspace-size expression" in error for error in errors)
                )

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


class RiskClosureWorkspaceConfigGeneratorTests(unittest.TestCase):
    def write_source(self, text: str) -> pathlib.Path:
        temporary = tempfile.NamedTemporaryFile(mode="w", suffix=".toml", delete=False)
        self.addCleanup(pathlib.Path(temporary.name).unlink, missing_ok=True)
        with temporary:
            temporary.write(text)
        return pathlib.Path(temporary.name)

    def test_derives_capacity_without_a_duplicate_slot_count(self) -> None:
        source = self.write_source(
            """
schema_version = 1
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 160
slot_bytes = 16
"""
        )

        config = generator.load_config(source)

        self.assertEqual(config.capacity, 10)
        rendered = generator.render_rust(config, source.name)
        self.assertIn("capacity: 10", rendered)
        self.assertIn("const RISK_CLOSURE_WORKSPACE_CONFIG", rendered)
        self.assertNotIn("pub const RISK_CLOSURE_WORKSPACE_CONFIG", rendered)
        self.assertNotIn("owner_slots", rendered)

    def test_rejects_non_integral_slot_geometry(self) -> None:
        source = self.write_source(
            """
schema_version = 1
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 161
slot_bytes = 16
"""
        )

        with self.assertRaisesRegex(generator.ConfigError, "evenly divide"):
            generator.load_config(source)

    def test_rejects_enabled_production_activation(self) -> None:
        source = self.write_source(
            """
schema_version = 1
production_activation_enabled = true

[risk_closure_workspaces]
arena_bytes = 160
slot_bytes = 16
"""
        )

        with self.assertRaisesRegex(generator.ConfigError, "must remain false"):
            generator.load_config(source)

    def test_rejects_non_integer_or_unsupported_schema_versions(self) -> None:
        for schema_version in ("true", "1.0", '"1"', "2"):
            with self.subTest(schema_version=schema_version):
                source = self.write_source(
                    f"""
schema_version = {schema_version}
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 160
slot_bytes = 16
"""
                )

                with self.assertRaisesRegex(generator.ConfigError, "schema_version"):
                    generator.load_config(source)

    def test_rejects_missing_schema_version(self) -> None:
        source = self.write_source(
            """
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 160
slot_bytes = 16
"""
        )

        with self.assertRaisesRegex(generator.ConfigError, "missing field"):
            generator.load_config(source)

    def test_rejects_unknown_or_duplicate_capacity_authorities(self) -> None:
        for field in ("owner_slots = 10", "capacity = 10", "workspace_bytes = 16"):
            with self.subTest(field=field):
                source = self.write_source(
                    f"""
schema_version = 1
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 160
slot_bytes = 16
{field}
"""
                )

                with self.assertRaisesRegex(generator.ConfigError, "unknown field"):
                    generator.load_config(source)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
