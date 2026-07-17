#!/usr/bin/env python3
"""Tests for the structured risk-closure workspace configuration checks."""

from __future__ import annotations

import contextlib
import io
import pathlib
import tempfile
import unittest

import generate_risk_closure_workspace_config as generator
import verify_risk_closure_workspace_authority as verifier


VALID_CONFIG = """\
schema_version = 1
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = 167772160
slot_bytes = 16777216
"""


class RiskClosureWorkspaceAuthorityVerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "config").mkdir()
        (self.root / "config" / "risk-closure-workspaces.toml").write_text(
            VALID_CONFIG,
            encoding="utf-8",
        )

    def test_accepts_one_toml_authority(self) -> None:
        self.assertEqual(verifier.authority_errors(self.root), [])

    def test_does_not_interpret_rust_source_shape(self) -> None:
        source = self.root / "src"
        source.mkdir()
        (source / "lib.rs").write_text(
            "this is intentionally not parsed as Rust\n",
            encoding="utf-8",
        )
        (source / "consumer.rs").write_text(
            "pub use aliases and factories are compiler concerns\n",
            encoding="utf-8",
        )

        self.assertEqual(verifier.authority_errors(self.root), [])

    def test_rejects_a_second_toml_authority(self) -> None:
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

    def test_rejects_nested_toml_authority(self) -> None:
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

    def test_rejects_missing_canonical_authority(self) -> None:
        source = self.root / "config" / "risk-closure-workspaces.toml"
        source.write_text(
            "schema_version = 1\nproduction_activation_enabled = false\n",
            encoding="utf-8",
        )

        errors = verifier.authority_errors(self.root)

        self.assertTrue(errors)

    def test_authority_census_defers_schema_validation_to_generator(self) -> None:
        source = self.root / "config" / "risk-closure-workspaces.toml"
        for text in (
            "risk_closure_workspaces = 10\n",
            "[risk_closure_workspaces]\narena_bytes = 0\nslot_bytes = 16\n",
            "[risk_closure_workspaces]\narena_bytes = 160\nslot_bytes = false\n",
        ):
            with self.subTest(text=text):
                source.write_text(text, encoding="utf-8")

                self.assertEqual(verifier.authority_errors(self.root), [])

    def test_toml_key_path_rendering_is_unambiguous(self) -> None:
        self.assertNotEqual(
            verifier._render_toml_key_path(("probe.a", "risk_closure_workspaces")),
            verifier._render_toml_key_path(("probe", "a", "risk_closure_workspaces")),
        )
        self.assertNotEqual(
            verifier._render_toml_key_path(("owners[0]", "risk_closure_workspaces")),
            verifier._render_toml_key_path(("owners", 0, "risk_closure_workspaces")),
        )

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
        self.assertNotIn("owner_slots", rendered)

    def test_check_requires_exact_generated_output(self) -> None:
        source = self.write_source(VALID_CONFIG)
        output = source.with_suffix(".rs")
        self.addCleanup(output.unlink, missing_ok=True)
        output.write_text("stale\n", encoding="utf-8")

        with contextlib.redirect_stderr(io.StringIO()):
            stale_result = generator.main(
                ["--source", str(source), "--output", str(output), "--check"]
            )
        write_result = generator.main(["--source", str(source), "--output", str(output)])
        current_result = generator.main(
            ["--source", str(source), "--output", str(output), "--check"]
        )

        self.assertEqual(stale_result, 1)
        self.assertEqual(write_result, 0)
        self.assertEqual(current_result, 0)

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

    def test_rejects_non_table_workspace_config(self) -> None:
        source = self.write_source(
            """
schema_version = 1
production_activation_enabled = false
risk_closure_workspaces = 10
"""
        )

        with self.assertRaisesRegex(generator.ConfigError, "must be a table"):
            generator.load_config(source)

    def test_rejects_non_positive_geometry_values(self) -> None:
        for arena_bytes, slot_bytes in (
            ("0", "16"),
            ("160", "0"),
            ("true", "16"),
        ):
            with self.subTest(
                arena_bytes=arena_bytes,
                slot_bytes=slot_bytes,
            ):
                source = self.write_source(
                    f"""
schema_version = 1
production_activation_enabled = false

[risk_closure_workspaces]
arena_bytes = {arena_bytes}
slot_bytes = {slot_bytes}
"""
                )

                with self.assertRaisesRegex(generator.ConfigError, "positive integer"):
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
