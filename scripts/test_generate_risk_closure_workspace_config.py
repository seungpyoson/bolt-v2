#!/usr/bin/env python3
"""Focused tests for the risk-closure workspace config generator."""

from __future__ import annotations

import pathlib
import tempfile
import unittest

import generate_risk_closure_workspace_config as generator


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
