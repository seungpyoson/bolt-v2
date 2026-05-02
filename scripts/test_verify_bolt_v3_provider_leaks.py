#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 provider-leak verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_provider_leaks.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_provider_leaks", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, files: dict[str, str]) -> None:
    for rel, text in files.items():
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")


def binding_files() -> dict[str, str]:
    return {
        "src/bolt_v3_providers/polymarket.rs": "pub const KEY: &str = \"polymarket\";\n",
        "src/bolt_v3_providers/binance.rs": "pub const KEY: &str = \"binance\";\n",
        "src/bolt_v3_market_families/updown.rs": "pub const KEY: &str = \"updown\";\n",
    }


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_clean_fixture_has_no_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    /// Historical note: MarketSlugFilter used to live here.
                    /* Historical note:
                       "polymarket" and "updown" used to be mentioned here.
                    */
                    pub struct ProviderOwnedAdapterConfig;

                    #[cfg(test)]

                    // Test module comments may sit between cfg and item.
                    mod tests {
                        fn fixture() {
                            let _brace = "}";
                            let _ = "BoltV3VenueAdapterConfig::Polymarket";
                        }
                    }

                    #[cfg(test)]
                    fn multiline_fixture(
                        value: &str,
                    ) {
                        let _ = value;
                        let _ = "polymarket";
                    }

                    pub struct ProductionAfterTests;
                """,
                "src/bolt_v3_secrets.rs": "pub struct ResolvedProviderSecrets;\n",
                "src/bolt_v3_client_registration.rs": "pub fn register(binding: &dyn ProviderBinding) {}\n",
            },
        )

        assert verifier.scan_root(root) == []


def test_closed_provider_variants_and_factory_imports_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use nautilus_polymarket::filters::MarketSlugFilter;
                    pub enum BoltV3VenueAdapterConfig {
                        Polymarket(Box<PolymarketAdapters>),
                        Binance(BinanceAdapters),
                    }
                    pub fn map(kind: &str) {
                        match kind {
                            polymarket::KEY => {}
                            binance::KEY => {}
                            _ => {}
                        }
                    }
                """,
                "src/bolt_v3_secrets.rs": """
                    pub use crate::bolt_v3_providers::{
                        binance::ResolvedBoltV3BinanceSecrets,
                        polymarket::ResolvedBoltV3PolymarketSecrets,
                    };
                    pub enum ResolvedBoltV3VenueSecrets {
                        Polymarket(PolymarketSecrets),
                        Binance(BinanceSecrets),
                    }
                    pub fn resolve(kind: &str) {
                        match kind {
                            polymarket::KEY => {}
                            binance::KEY => {}
                            _ => {}
                        }
                    }
                """,
                "src/bolt_v3_client_registration.rs": """
                    use nautilus_polymarket::factories::PolymarketDataClientFactory;
                    use nautilus_binance::factories::BinanceDataClientFactory;
                    pub enum BoltV3RegisteredVenue {
                        Polymarket { data: bool },
                        Binance { data: bool },
                    }
                """,
                "src/bolt_v3_live_node.rs": """
                    use nautilus_polymarket::config::PolymarketDataClientConfig;
                    pub fn literal(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
                "src/bolt_v3_validate.rs": """
                    use crate::bolt_v3_providers;
                    pub fn literal(kind: &str, family: &str) -> bool {
                        kind == "binance"
                            || family == "updown"
                            || bolt_v3_providers::polymarket::KEY == kind
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "closed provider adapter config enum" in messages
        assert "adapter mapping dispatches on concrete provider key" in messages
        assert "provider-specific NT filter in adapter mapper" in messages
        assert "concrete NT provider crate in core production code" in messages
        assert "concrete provider type name in core production code" in messages
        assert "core imports or re-exports concrete provider module" in messages
        assert "core accesses concrete provider module path" in messages
        assert "provider-key string literal in core production code" in messages
        assert "market-family key string literal in core production code" in messages
        assert "closed resolved venue secret enum" in messages
        assert "secret resolution dispatches on concrete provider key" in messages
        assert "concrete NT provider factory import" in messages
        assert "closed registered venue summary enum" in messages


def test_family_module_and_type_leaks_are_findings_for_new_families() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_market_families/fixed_time.rs": """
                    pub const KEY: &str = "fixed_time";
                    pub struct FixedTimeTargetPlan;
                """,
                "src/bolt_v3_readiness.rs": """
                    use crate::bolt_v3_market_families::fixed_time::FixedTimeTargetPlan;
                    pub type BoltV3FixedTimeNowFn = fn() -> i64;
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "core accesses concrete market-family module path" in messages
        assert "concrete market-family type name in core production code" in messages


def test_finding_allowances_are_exact_and_path_scoped() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::{
                        bolt_v3_market_families::updown::MarketIdentityPlan,
                    };
                    pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
                """,
                "src/bolt_v3_providers/mod.rs": """
                    use crate::{
                        bolt_v3_adapters::{BoltV3AdapterMappingError, BoltV3UpdownNowFn, BoltV3VenueAdapterConfig},
                        bolt_v3_market_families::updown::MarketIdentityPlan,
                    };
                """,
                "src/bolt_v3_readiness.rs": """
                    use crate::bolt_v3_market_families::updown::MarketIdentityPlan;
                """,
                "src/bolt_v3_validate.rs": """
                    pub fn leaked_family_literal() -> &'static str {
                        "updown"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        by_path_and_message = {
            (finding.path, finding.message) for finding in findings
        }

        assert (
            "src/bolt_v3_adapters.rs",
            "core accesses concrete market-family module path",
        ) not in by_path_and_message
        assert (
            "src/bolt_v3_providers/mod.rs",
            "core accesses concrete market-family module path",
        ) not in by_path_and_message
        assert (
            "src/bolt_v3_readiness.rs",
            "core accesses concrete market-family module path",
        ) in by_path_and_message
        assert (
            "src/bolt_v3_validate.rs",
            "market-family key string literal in core production code",
        ) in by_path_and_message


def test_allowance_does_not_absorb_sibling_family_path_on_same_line() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::bolt_v3_market_families::updown::MarketIdentityPlan; use crate::bolt_v3_market_families::updown::UpdownTargetPlan;
                """,
            },
        )

        findings = verifier.scan_root(root)
        path_findings = [
            finding
            for finding in findings
            if finding.message == "core accesses concrete market-family module path"
        ]

        assert path_findings, "sibling family path must not be hidden by the allowance"


def test_new_core_file_is_auto_scanned() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_cost_facts.rs": """
                    pub fn leaked(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_production_after_cfg_test_block_is_scanned() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(test)]
                    mod tests {
                        fn fixture() {
                            let _ = "}";
                            let _ = "polymarket";
                        }
                    }

                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_cfg_not_test_is_scanned_as_production() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(not(test))]
                    pub fn leaked(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_cfg_not_any_test_is_scanned_as_production() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(not(any(test, feature = "fixture-only")))]
                    pub fn leaked(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_raw_strings_do_not_create_fake_comments() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": r'''
                    pub fn leaked(kind: &str) -> bool {
                        let _fixture = r#"raw " quote // not a comment"#;
                        kind == "binance"
                    }
                ''',
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_multiline_raw_string_braces_do_not_keep_cfg_test_open() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": r'''
                    #[cfg(test)]
                    mod tests {
                        fn fixture() {
                            let _fixture = r#"
                                {
                            "#;
                            let _ = "polymarket";
                        }
                    }

                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                ''',
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_char_literal_parser_accepts_rust_escape_lengths() -> None:
    verifier = load_verifier()

    assert verifier.char_literal_end_at(r"'\x7F'", 0) == len(r"'\x7F'")
    assert verifier.char_literal_end_at(r"'\u{1234}'", 0) == len(r"'\u{1234}'")


def test_char_literal_braces_do_not_keep_cfg_test_open() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(test)]
                    mod tests {
                        fn fixture() {
                            let _brace = '{';
                            let _ = "polymarket";
                        }
                    }

                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_strict_mode_fails_on_fixture_findings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_client_registration.rs": """
                    use nautilus_binance::factories::BinanceDataClientFactory;
                """,
            },
        )

        result = run_script("--root", str(root))

        assert result.returncode == 1
        assert "FAIL:" in result.stderr
        assert "concrete NT provider factory import" in result.stderr


def main() -> int:
    tests = [
        test_clean_fixture_has_no_findings,
        test_closed_provider_variants_and_factory_imports_are_findings,
        test_family_module_and_type_leaks_are_findings_for_new_families,
        test_finding_allowances_are_exact_and_path_scoped,
        test_allowance_does_not_absorb_sibling_family_path_on_same_line,
        test_new_core_file_is_auto_scanned,
        test_production_after_cfg_test_block_is_scanned,
        test_cfg_not_test_is_scanned_as_production,
        test_cfg_not_any_test_is_scanned_as_production,
        test_raw_strings_do_not_create_fake_comments,
        test_multiline_raw_string_braces_do_not_keep_cfg_test_open,
        test_char_literal_parser_accepts_rust_escape_lengths,
        test_char_literal_braces_do_not_keep_cfg_test_open,
        test_strict_mode_fails_on_fixture_findings,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 provider-leak verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
