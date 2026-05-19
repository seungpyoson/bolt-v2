#!/usr/bin/env python3
"""Tests for verify_bolt_v3_legacy_default_fence.py."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import verify_bolt_v3_legacy_default_fence as fence
from verify_bolt_v3_pure_rust_runtime import production_text


class LegacyDefaultFenceTests(unittest.TestCase):
    def test_clean_source_produces_no_violations(self) -> None:
        self.assertEqual(
            fence.find_violations_in_text(
                "src/bolt_v3_live_node.rs",
                "use crate::bolt_v3_config::LoadedBoltV3Config;\n",
            ),
            [],
        )

    def test_detects_legacy_config_module_reference(self) -> None:
        violations = fence.find_violations_in_text(
            "src/bolt_v3_live_node.rs",
            "use crate::live_config::LiveLocalConfig;",
        )

        self.assertEqual(len(violations), 2)
        self.assertEqual(violations[0].label, "legacy live_config module")
        self.assertEqual(violations[1].label, "legacy live-local materialization path")

    def test_detects_legacy_provider_modules(self) -> None:
        source = "\n".join(
            [
                "use crate::clients::polymarket::PolymarketDataClient;",
                "use crate::clients::chainlink::ChainlinkReferenceClient;",
                "use crate::platform::polymarket_catalog::CatalogClient;",
            ]
        )

        labels = {
            violation.label
            for violation in fence.find_violations_in_text("src/bolt_v3_adapters.rs", source)
        }

        self.assertEqual(
            labels,
            {
                "legacy Polymarket client module",
                "legacy Chainlink client module",
                "legacy Polymarket catalog defaults",
            },
        )

    def test_detects_nested_legacy_provider_module_imports(self) -> None:
        source = "\n".join(
            [
                "use crate::{",
                "    platform::{",
                "        polymarket_catalog::polymarket_instrument_id,",
                "        ruleset::CandidateMarket,",
                "    },",
                "};",
            ]
        )

        labels = [
            violation.label
            for violation in fence.find_violations_in_text(
                "src/strategies/binary_oracle_edge_taker.rs",
                source,
            )
        ]

        self.assertEqual(labels, ["legacy Polymarket catalog defaults"])

    def test_strategy_does_not_reach_legacy_polymarket_catalog(self) -> None:
        strategy = Path("src/strategies/binary_oracle_edge_taker.rs").read_text(
            encoding="utf-8"
        )

        self.assertNotIn("polymarket_catalog", production_text_from_string(strategy))

    def test_detects_external_crate_legacy_provider_modules(self) -> None:
        source = "\n".join(
            [
                "use bolt_v2::clients::polymarket::PolymarketDataClient;",
                "use bolt_v2::clients::chainlink::ChainlinkReferenceClient;",
                "use bolt_v2::platform::polymarket_catalog::CatalogClient;",
            ]
        )

        labels = {
            violation.label
            for violation in fence.find_violations_in_text("src/bolt_v3_adapters.rs", source)
        }

        self.assertEqual(
            labels,
            {
                "legacy Polymarket client module",
                "legacy Chainlink client module",
                "legacy Polymarket catalog defaults",
            },
        )

    def test_detects_legacy_loader_paths(self) -> None:
        source = "\n".join(
            [
                "let config = Config::load(path)?;",
                "let runtime = RuntimeConfig::load(path)?;",
                "let live = materialize_live_config(input, output)?;",
            ]
        )

        labels = [
            violation.label
            for violation in fence.find_violations_in_text("src/main.rs", source)
        ]

        self.assertEqual(
            labels,
            [
                "legacy Config::load path",
                "legacy live-local materialization path",
                "legacy live-local materialization path",
            ],
        )

    def test_detects_production_default_residues(self) -> None:
        source = "\n".join(
            [
                "#[derive(Debug, Default)]",
                "struct RuntimeState { value: Option<String> }",
                "let state = RuntimeState::default();",
                "let cursor = usize::default();",
                "let value = maybe_value.unwrap_or_default();",
                "entry.or_default();",
            ]
        )

        labels = [
            violation.label
            for violation in fence.find_violations_in_text(
                "src/strategies/binary_oracle_edge_taker.rs",
                source,
            )
        ]

        self.assertEqual(
            labels,
            [
                "production derive Default",
                "production type default",
                "production type default",
                "production unwrap_or_default",
                "production or_default",
            ],
        )

    def test_detects_production_serde_and_enum_defaults(self) -> None:
        source = "\n".join(
            [
                "#[serde(default)]",
                "field: Option<String>,",
                "#[default]",
                "Idle,",
                "let raw = Default::default();",
            ]
        )

        labels = [
            violation.label
            for violation in fence.find_violations_in_text(
                "src/strategies/binary_oracle_edge_taker.rs",
                source,
            )
        ]

        self.assertEqual(
            labels,
            [
                "production serde default",
                "production enum default",
                "production Default::default",
            ],
        )

    def test_allows_nt_runtime_support_default_reference(self) -> None:
        self.assertEqual(
            fence.find_violations_in_text(
                "src/bolt_v3_validate.rs",
                "let nt_data_default = nautilus_live::config::LiveDataEngineConfig::default();",
            ),
            [],
        )

    def test_cfg_test_references_are_stripped_before_collection(self) -> None:
        source = (
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    use crate::live_config::LiveLocalConfig;\n"
            "}\n"
            "pub fn production() {}\n"
        )
        handle = tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            suffix=".rs",
            delete=False,
        )
        temp_path = Path(handle.name)
        try:
            with handle:
                handle.write(source)
            self.assertEqual(
                fence.find_violations_in_text("test.rs", production_text(temp_path)),
                [],
            )
        finally:
            temp_path.unlink(missing_ok=True)

    def test_runtime_source_paths_include_entrypoint_and_strategy(self) -> None:
        self.assertIn("src/main.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/bolt_v3_live_node.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/lake_batch.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/log_sweep.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/secrets.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/venue_contract.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn("src/strategies/registry.rs", fence.RUNTIME_SOURCE_PATHS)
        self.assertIn(
            "src/strategies/binary_oracle_edge_taker.rs",
            fence.RUNTIME_SOURCE_PATHS,
        )


def production_text_from_string(source: str) -> str:
    handle = tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".rs",
        delete=False,
    )
    temp_path = Path(handle.name)
    try:
        with handle:
            handle.write(source)
        return production_text(temp_path)
    finally:
        temp_path.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
