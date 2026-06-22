#!/usr/bin/env python3
"""Tests for verify_bolt_v3_legacy_default_fence.py."""

from __future__ import annotations

import re
import tempfile
import unittest
from pathlib import Path

import bolt_v3_source_roots as source_roots
import verify_bolt_v3_legacy_default_fence as fence
from bolt_v3_source_roots import STRATEGY_SOURCE_ROOT, STRATEGY_SOURCE_ROOTS, module_text, source_files
from verify_bolt_v3_pure_rust_runtime import production_text

# A current strategy source file, resolved layout-independently (the strategy
# root is a directory after the A3 split); used as a representative path label in
# synthetic find_violations_in_text cases and as the runtime-path-membership anchor.
STRATEGY_SOURCE_FILE = source_files(STRATEGY_SOURCE_ROOT)[0].relative_to(
    fence.REPO_ROOT
).as_posix()


def rust_text_accessor_max_bytes(source: str) -> int:
    match = re.search(r"\bconst\s+TEXT_ACCESSOR_MAX_BYTES:\s*u64\s*=\s*([^;]+);", source)
    if not match:
        raise AssertionError("TEXT_ACCESSOR_MAX_BYTES constant not found")
    product = 1
    for factor in match.group(1).split("*"):
        product *= int(factor.strip())
    return product


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
        strategy = production_text_from_string(module_text(STRATEGY_SOURCE_ROOTS))

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

    def test_source_root_helper_rejects_backslash_path_component(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            module = root / "module"
            module.mkdir()
            bad = module / "a\\b.rs"
            if bad.name != "a\\b.rs":
                self.skipTest("platform does not permit backslash in a file name")
            bad.write_text("fn bad() {}\n", encoding="utf-8")

            original_root = source_roots.REPO_ROOT
            source_roots.REPO_ROOT = root
            try:
                with self.assertRaisesRegex(ValueError, "contains a backslash"):
                    source_roots.source_files("module")
            finally:
                source_roots.REPO_ROOT = original_root

    def test_python_source_roots_match_manifest(self) -> None:
        # The gated root list lives in one place: gated_source_roots.manifest,
        # read by both build.rs (Rust) and bolt_v3_source_roots.py (Python). This
        # asserts the Python module's exposed roots equal an INDEPENDENT, PER-KEY
        # parse of that manifest — section membership and order preserved, not a
        # flat set — so a Python-side parser regression, including a root assigned
        # to the wrong section, fails loudly. The Rust side is pinned by the
        # registry-membership tests in bolt_v3_source_integrity.rs, which assert
        # the generated constant equals the same expected list.
        manifest = (
            source_roots.REPO_ROOT / "gated_source_roots.manifest"
        ).read_text(encoding="utf-8")
        sections: dict[str, list[str]] = {}
        current: str | None = None
        for raw_line in manifest.split("\n"):
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("[") and line.endswith("]"):
                current = line[1:-1].strip()
                sections[current] = []
                continue
            self.assertIsNotNone(current, f"root {line!r} precedes any [section]")
            assert current is not None  # narrow type for the checker
            sections[current].append(line)

        self.assertEqual(
            sections,
            {
                source_roots.STRATEGY_KEY: list(source_roots.STRATEGY_SOURCE_ROOTS),
                source_roots.SUBMIT_ADMISSION_KEY: list(
                    source_roots.SUBMIT_ADMISSION_SOURCE_ROOTS
                ),
                source_roots.OUTCOME_GROUP_KEY: list(
                    source_roots.OUTCOME_GROUP_SOURCE_ROOTS
                ),
                source_roots.MAKER_KEY: list(source_roots.MAKER_SOURCE_ROOTS),
            },
        )

    def test_manifest_parser_matches_rust_line_semantics(self) -> None:
        # build.rs parses the manifest with Rust ``str::lines()`` (splits on
        # ``\n`` and ``\r\n`` only). The Python parser must use the same line
        # semantics, else a manifest the Rust build rejects could parse cleanly
        # in Python and the two source-of-truth parsers would disagree. A
        # manifest whose lines are joined by a bare ``\r`` is ONE Rust line (a
        # malformed section header) and must be rejected on both sides; Python's
        # ``str.splitlines()`` would wrongly break it into valid lines and accept
        # it. This guards the parser against regressing back to ``splitlines()``.
        body = (
            "[strategy]\nsrc/a.rs\n"
            "[submit_admission]\nsrc/b.rs\n"
            "[outcome_group]\nsrc/c.rs\n"
            "[maker]\nsrc/d.rs\n"
        )
        self.assertEqual(
            source_roots._parse_manifest_text(body),
            {
                source_roots.STRATEGY_KEY: ("src/a.rs",),
                source_roots.SUBMIT_ADMISSION_KEY: ("src/b.rs",),
                source_roots.OUTCOME_GROUP_KEY: ("src/c.rs",),
                source_roots.MAKER_KEY: ("src/d.rs",),
            },
        )
        # Same bytes, bare-CR separators: Rust ``str::lines()`` sees one line, so
        # the Python parser must reject it too (matching the loud Rust build
        # failure) rather than silently accept the ``splitlines()`` split.
        with self.assertRaises(ValueError):
            source_roots._parse_manifest_text(body.replace("\n", "\r"))

    def test_manifest_parser_matches_rust_trim_whitespace(self) -> None:
        # build.rs trims each line/key with Rust ``str::trim()`` (the Unicode
        # ``White_Space`` set). Python's bare ``str.strip()`` strips a SUPERSET —
        # it also removes U+001C–U+001F (the information separators) — so the
        # parser strips the exact Rust set instead. A section header with a
        # trailing U+001C is a malformed header to Rust (the control char stays,
        # so it no longer ends with ``]``) and must be rejected on the Python
        # side too; bare ``str.strip()`` would silently accept it. This guards
        # against the parser regressing back to ``.strip()``.
        info_separator = "\x1c"
        body = (
            f"[strategy]{info_separator}\nsrc/a.rs\n"
            "[submit_admission]\nsrc/b.rs\n"
            "[outcome_group]\nsrc/c.rs\n"
            "[maker]\nsrc/d.rs\n"
        )
        with self.assertRaises(ValueError):
            source_roots._parse_manifest_text(body)
        # Ordinary trailing whitespace (space + tab) is still trimmed on both
        # sides, so the same manifest with real whitespace parses cleanly.
        spaced = (
            "[strategy] \t\nsrc/a.rs\n"
            "[submit_admission]\nsrc/b.rs\n"
            "[outcome_group]\nsrc/c.rs\n"
            "[maker]\nsrc/d.rs\n"
        )
        self.assertEqual(
            source_roots._parse_manifest_text(spaced),
            {
                source_roots.STRATEGY_KEY: ("src/a.rs",),
                source_roots.SUBMIT_ADMISSION_KEY: ("src/b.rs",),
                source_roots.OUTCOME_GROUP_KEY: ("src/c.rs",),
                source_roots.MAKER_KEY: ("src/d.rs",),
            },
        )

    def test_python_source_root_file_cap_matches_rust_text_accessor_cap(self) -> None:
        source = (
            source_roots.REPO_ROOT / "src/bolt_v3_source_integrity.rs"
        ).read_text(encoding="utf-8")

        self.assertEqual(
            source_roots.MAX_SOURCE_FILE_BYTES,
            rust_text_accessor_max_bytes(source),
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
                with self.assertRaisesRegex(ValueError, "source file exceeds 8 MiB limit"):
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
                # A hand-written `impl Default` bypasses the derive/`::default(`
                # patterns, so the fence matches the `impl Default for` form
                # directly (the StrategyRegistry/Imdsv2HostFactsSource class).
                "impl Default for RuntimeState {",
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
                "production impl Default",
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
    import lane_governor

    lane_governor.acquire()
    unittest.main()
