#!/usr/bin/env python3
"""Tests for verify_bolt_v3_legacy_default_fence.py."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import bolt_v3_source_roots as source_roots
import verify_bolt_v3_legacy_default_fence as fence
from bolt_v3_source_roots import STRATEGY_SOURCE_ROOT, module_text, source_files
from verify_bolt_v3_pure_rust_runtime import production_text

# A current strategy source file, resolved layout-independently (the strategy
# root is a directory after the A3 split); used as a representative path label in
# synthetic find_violations_in_text cases and as the runtime-path-membership anchor.
STRATEGY_SOURCE_FILE = source_files(STRATEGY_SOURCE_ROOT)[0].relative_to(
    fence.REPO_ROOT
).as_posix()


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
                STRATEGY_SOURCE_FILE,
                source,
            )
        ]

        self.assertEqual(labels, ["legacy Polymarket catalog defaults"])

    def test_strategy_does_not_reach_legacy_polymarket_catalog(self) -> None:
        # Resolve the strategy module layout-independently (a directory after the
        # A3 split) and scan every file's production text.
        strategy = production_text_from_string(module_text(STRATEGY_SOURCE_ROOT))

        self.assertNotIn("polymarket_catalog", strategy)

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

    def test_source_root_helper_rejects_symlink_root(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            target = root / "real.rs"
            target.write_text("fn real() {}\n", encoding="utf-8")
            link = root / "linked.rs"
            try:
                link.symlink_to(target)
            except OSError as error:
                self.skipTest(f"symlink creation unavailable: {error}")

            original_root = source_roots.REPO_ROOT
            source_roots.REPO_ROOT = root
            try:
                with self.assertRaisesRegex(ValueError, "source root is a symlink"):
                    source_roots.source_files("linked.rs")
            finally:
                source_roots.REPO_ROOT = original_root

    def test_source_root_helper_rejects_symlink_inside_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            module = root / "module"
            module.mkdir()
            target = module / "real.rs"
            target.write_text("fn real() {}\n", encoding="utf-8")
            link = module / "linked.rs"
            try:
                link.symlink_to(target)
            except OSError as error:
                self.skipTest(f"symlink creation unavailable: {error}")

            original_root = source_roots.REPO_ROOT
            source_roots.REPO_ROOT = root
            try:
                with self.assertRaisesRegex(ValueError, "source root contains a symlink"):
                    source_roots.source_files("module")
            finally:
                source_roots.REPO_ROOT = original_root

    def test_source_root_helper_rejects_symlink_directory_inside_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            module = root / "module"
            module.mkdir()
            target = root / "outside"
            target.mkdir()
            (target / "evil.rs").write_text("fn evil() {}\n", encoding="utf-8")
            link = module / "linked_dir"
            try:
                link.symlink_to(target, target_is_directory=True)
            except OSError as error:
                self.skipTest(f"symlink creation unavailable: {error}")

            original_root = source_roots.REPO_ROOT
            source_roots.REPO_ROOT = root
            try:
                with self.assertRaisesRegex(ValueError, "source root contains a symlink"):
                    source_roots.source_files("module")
            finally:
                source_roots.REPO_ROOT = original_root

    def test_python_source_roots_match_rust_registry_relative_roots(self) -> None:
        registry = (
            source_roots.REPO_ROOT / "src/source_canonicalization.rs"
        ).read_text(encoding="utf-8")
        rust_roots = {
            line.split('"')[1]
            for line in registry.splitlines()
            if line.strip().startswith("relative_root:")
        }

        self.assertEqual(
            rust_roots,
            {
                source_roots.STRATEGY_SOURCE_ROOT,
                source_roots.SUBMIT_ADMISSION_SOURCE_ROOT,
            },
        )

    def test_source_root_helper_rejects_oversized_module_text_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            module = root / "module"
            module.mkdir()
            (module / "big.rs").write_text(
                "x" * (source_roots.MAX_SOURCE_FILE_BYTES + 1),
                encoding="utf-8",
            )

            original_root = source_roots.REPO_ROOT
            source_roots.REPO_ROOT = root
            try:
                with self.assertRaisesRegex(ValueError, "source file exceeds 1 MiB limit"):
                    source_roots.module_text("module")
            finally:
                source_roots.REPO_ROOT = original_root

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
                STRATEGY_SOURCE_FILE,
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
                STRATEGY_SOURCE_FILE,
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
            STRATEGY_SOURCE_FILE,
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
