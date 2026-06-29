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


def test_real_scan_covers_provider_neutral_source_files() -> None:
    verifier = load_verifier()
    core_files = set(verifier.discovered_core_files(REPO_ROOT))
    for rel in (
        "src/bin/stream_to_lake.rs",
        "src/lib.rs",
        "src/main.rs",
        "src/secrets.rs",
        "src/strategies/binary_oracle_edge_taker/config.rs",
        "src/strategies/binary_oracle_edge_taker/mod.rs",
        "src/strategies/binary_oracle_edge_taker/selection.rs",
    ):
        assert rel in core_files
    for rel in (
        "src/bolt_v3_providers/binance.rs",
        "src/bolt_v3_providers/polymarket.rs",
        "src/bolt_v3_providers/polymarket/fees.rs",
        "src/bolt_v3_market_families/updown.rs",
        "src/bolt_v3_outcome_group_polymarket.rs",
    ):
        assert rel not in core_files


def test_shared_market_data_provider_module_name_is_not_concrete_provider() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_providers/market_data.rs": (
                    'pub const BITMEX_KEY: &str = "BITMEX";\n'
                ),
                "src/bolt_v3_readiness.rs": """
                    pub enum DataClientReadinessProbeMarketDataKind {
                        Quote,
                        Book,
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "concrete provider type name in core production code" not in messages


def test_provider_key_constants_in_shared_market_data_module_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_providers/market_data.rs": (
                    'pub const BITMEX_KEY: &str = "BITMEX";\n'
                ),
                "src/bolt_v3_readiness.rs": """
                    pub struct BitmexAdapterLeak;

                    pub fn leaked(kind: &str) -> bool {
                        kind == "bitmex"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "concrete provider type name in core production code" in messages
        assert "provider-key string literal in core production code" in messages


def test_outcome_group_source_native_proof_labels_are_allowlisted() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_providers/hyperliquid.rs": 'pub const KEY: &str = "hyperliquid";\n',
                "src/bolt_v3_outcome_groups.rs": """
                    pub enum OutcomeGroupSourceKind {
                        Polymarket,
                        Hyperliquid,
                    }
                    pub enum GroupingProof {
                        PolymarketNegRisk {
                            discovery_scope: PolymarketDiscoveryScopeEvidence,
                        },
                        HyperliquidOutcome {
                            question: u32,
                        },
                    }
                    impl GroupingProof {
                        fn native_identity(&self) -> String {
                            match self {
                                Self::PolymarketNegRisk {
                                    ..
                                } => String::new(),
                                Self::HyperliquidOutcome { question, .. } => format!("hyperliquid:{question}"),
                            }
                        }
                    }
                    pub struct PolymarketDiscoveryScopeEvidence {
                        source_id: String,
                    }
                    fn labels(source_kind: OutcomeGroupSourceKind, proof: GroupingProof) -> &'static str {
                        match source_kind {
                            OutcomeGroupSourceKind::Polymarket => "polymarket",
                            OutcomeGroupSourceKind::Hyperliquid => "hyperliquid",
                        }
                    }
                    fn metadata(proof: GroupingProof) {
                        match proof {
                            GroupingProof::PolymarketNegRisk {
                                ..
                            } => {}
                            GroupingProof::HyperliquidOutcome {
                                ..
                            } => {}
                        }
                    }
                """,
            },
        )

        assert verifier.scan_root(root) == []


def test_outcome_group_allowance_does_not_hide_other_provider_type_leaks() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_outcome_groups.rs": """
                    pub struct PolymarketClientLeak;
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "concrete provider type name in core production code" in messages


def test_shared_secret_module_provider_import_is_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/secrets.rs": """
                    use nautilus_binance::common::credential::Ed25519Credential;
                    pub struct SsmResolverSession;
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert any(
            finding.path == "src/secrets.rs"
            and finding.message == "concrete NT provider crate in core production code"
            for finding in findings
        )


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


def test_concrete_market_family_paths_are_not_allowlisted() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::{
                        bolt_v3_market_families::updown::InstrumentFilterConfig,
                    };
                """,
                "src/bolt_v3_providers/mod.rs": """
                    use crate::{
                        bolt_v3_adapters::{BoltV3AdapterMappingError, BoltV3VenueAdapterConfig},
                        bolt_v3_market_families::updown::InstrumentFilterConfig,
                    };
                """,
                "src/bolt_v3_readiness.rs": """
                    use crate::bolt_v3_market_families::updown::InstrumentFilterConfig;
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

        for path in (
            "src/bolt_v3_adapters.rs",
            "src/bolt_v3_providers/mod.rs",
            "src/bolt_v3_readiness.rs",
        ):
            assert (
                path,
                "core accesses concrete market-family module path",
            ) in by_path_and_message
        assert (
            "src/bolt_v3_validate.rs",
            "market-family key string literal in core production code",
        ) in by_path_and_message


def test_sibling_family_path_on_same_line_is_reported() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_adapters.rs": """
                    use crate::bolt_v3_market_families::updown::InstrumentFilterConfig; use crate::bolt_v3_market_families::updown::UpdownInstrumentFilterTarget;
                """,
            },
        )

        findings = verifier.scan_root(root)
        path_findings = [
            finding
            for finding in findings
            if finding.message == "core accesses concrete market-family module path"
        ]

        assert path_findings, "sibling family path must be reported"


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


def test_constructed_provider_key_literals_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {"src/bolt_v3_providers/hyperliquid.rs": 'pub const KEY: &str = "hyperliquid";\n'}
            | {
                "src/bolt_v3_readiness.rs": """
                    pub fn leaked() -> (String, &'static str) {
                        (
                            format!("{}{}", "poly", "market"),
                            concat!("hyper", "liquid"),
                        )
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        provider_key_findings = [
            finding
            for finding in findings
            if finding.message == "provider-key string literal in core production code"
        ]

        assert len(provider_key_findings) >= 2, findings


def test_venue_id_from_provider_key_literal_is_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    use nautilus_model::identifiers::VenueId;

                    pub fn leaked() -> VenueId {
                        VenueId::from("polymarket")
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_native_identity_literal_namespaces_are_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {"src/bolt_v3_providers/hyperliquid.rs": 'pub const KEY: &str = "hyperliquid";\n'}
            | {
                "src/bolt_v3_outcome_groups.rs": """
                    pub enum GroupingProof {
                        PolymarketNegRisk { neg_risk_market_id: String },
                        HyperliquidOutcome { question: u32 },
                        OperatorAttested { settlement_contract_id: String },
                    }

                    impl GroupingProof {
                        fn native_identity(&self) -> String {
                            match self {
                                Self::PolymarketNegRisk { neg_risk_market_id } => {
                                    format!("polymarket:{neg_risk_market_id}")
                                }
                                Self::HyperliquidOutcome { question } => {
                                    format!("hyperliquid:{question}")
                                }
                                Self::OperatorAttested { settlement_contract_id } => {
                                    format!("operator:{settlement_contract_id}")
                                }
                            }
                        }
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        provider_key_findings = [
            finding
            for finding in findings
            if finding.message == "provider-key string literal in core production code"
        ]
        native_identity_findings = [
            finding
            for finding in findings
            if finding.message == "native-identity namespace string literal in outcome-group code"
        ]

        assert len(provider_key_findings) >= 2, findings
        assert len(native_identity_findings) == 3, findings


def test_hyperliquid_outcome_group_literal_namespace_is_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_outcome_group_hyperliquid.rs": """
                    pub fn group_id(question: &str) -> String {
                        format!("hyperliquid:{question}")
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        native_identity_findings = [
            finding
            for finding in findings
            if finding.message == "native-identity namespace string literal in outcome-group code"
        ]

        assert len(native_identity_findings) == 1, findings


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


def test_cfg_any_test_feature_is_scanned_as_production() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(any(test, feature = "fixture-only"))]
                    pub fn leaked(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_cfg_all_test_feature_is_stripped_as_test_only() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(all(test, feature = "fixture-only"))]
                    pub fn fixture(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_cfg_not_not_test_is_stripped_as_test_only() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(not(not(test)))]
                    pub fn fixture(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_inner_cfg_test_attr_strips_file_contents() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #![cfg(test)]

                    pub fn fixture(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_multiline_cfg_test_attr_strips_test_item() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(
                        all(test, feature = "fixture-only")
                    )]
                    pub fn fixture(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_whitespace_cfg_test_attr_strips_test_item() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[ cfg ( test ) ]
                    pub fn fixture(kind: &str) -> bool {
                        kind == "polymarket"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_inline_cfg_test_item_does_not_hide_next_production_line() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(test)] fn fixture() { let _ = "polymarket"; }
                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_inline_cfg_test_semicolon_item_does_not_hide_next_production_line() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(test)] use crate::bolt_v3_providers::polymarket;
                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_cfg_test_comma_item_does_not_hide_next_production_line() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    pub enum Fixture {
                        #[cfg(test)]
                        Polymarket,
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
        assert "concrete provider type name in core production code" not in messages


def test_cfg_test_comma_less_item_does_not_hide_next_production_line() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    pub enum Fixture {
                        #[cfg(test)]
                        Polymarket
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
        assert "concrete provider type name in core production code" not in messages


def test_cfg_test_where_clause_comma_does_not_scan_test_body() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": """
                    #[cfg(test)]
                    fn fixture<T>() -> &'static str
                    where
                        T: Sized,
                    {
                        "polymarket"
                    }

                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                """,
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)
        excerpts = "\n".join(finding.excerpt for finding in findings)

        assert "provider-key string literal in core production code" in messages
        assert "polymarket" not in excerpts


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


def test_raw_string_cfg_text_does_not_hide_following_production() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": r'''
                    pub fn fixture_text() -> &'static str {
                        r#"
                    #[cfg(test)]
                    mod fake {
                        fn fixture() {
                        }
                    }
                    "#
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


def test_byte_and_multi_hash_raw_strings_do_not_create_fake_comments() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": r'''
                    pub fn leaked(kind: &str) -> bool {
                        let _fixture = br##"raw " quote // not a comment"##;
                        let _other = r##"raw /* not a block comment */ text"##;
                        kind == "binance"
                    }
                ''',
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages


def test_cfg_test_multiline_raw_string_item_does_not_hide_next_production() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            binding_files()
            | {
                "src/bolt_v3_readiness.rs": r'''
                    #[cfg(test)] static FIXTURE: &str = r#"
                        ;
                        polymarket
                    "#;

                    pub fn leaked(kind: &str) -> bool {
                        kind == "binance"
                    }
                ''',
            },
        )

        findings = verifier.scan_root(root)
        messages = "\n".join(finding.message for finding in findings)

        assert "provider-key string literal in core production code" in messages
        assert "polymarket" not in "\n".join(finding.excerpt for finding in findings)


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


def test_scan_root_cache_does_not_mask_changed_file_content() -> None:
    """scan_root memoizes stripped file text per path, but the cache is scoped
    to a single scan_root call. Hoisting it to module scope would let a file
    edited between two scans be served from stale cache, masking a
    newly-introduced leak (fail open). Scan with a clean readiness file, then
    overwrite the SAME path with a provider leak on the same module instance and
    rescan: the leak must surface."""
    verifier = load_verifier()
    base = binding_files() | {
        "src/bolt_v3_providers/market_data.rs": 'pub const BITMEX_KEY: &str = "BITMEX";\n',
    }
    readiness = "src/bolt_v3_readiness.rs"
    clean = "pub enum Mode {\n    Quote,\n    Book,\n}\n"
    leak = (
        "pub struct BitmexAdapterLeak;\n\n"
        "pub fn leaked(kind: &str) -> bool {\n"
        '    kind == "bitmex"\n'
        "}\n"
    )
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, base | {readiness: clean})
        assert verifier.scan_root(root) == [], "clean readiness fixture must have no findings"

        # Overwrite the SAME path with leaky content and rescan on the same module.
        write_fixture(root, {readiness: leak})
        messages = "\n".join(finding.message for finding in verifier.scan_root(root))
        assert "provider-key string literal in core production code" in messages, (
            "scan_root must re-read changed file content; a hoisted text cache "
            "masked a newly-introduced provider-key leak (fail open)"
        )


def main() -> int:
    tests = [
        test_clean_fixture_has_no_findings,
        test_real_scan_covers_provider_neutral_source_files,
        test_closed_provider_variants_and_factory_imports_are_findings,
        test_family_module_and_type_leaks_are_findings_for_new_families,
        test_concrete_market_family_paths_are_not_allowlisted,
        test_sibling_family_path_on_same_line_is_reported,
        test_new_core_file_is_auto_scanned,
        test_production_after_cfg_test_block_is_scanned,
        test_cfg_not_test_is_scanned_as_production,
        test_cfg_not_any_test_is_scanned_as_production,
        test_shared_market_data_provider_module_name_is_not_concrete_provider,
        test_provider_key_constants_in_shared_market_data_module_are_findings,
        test_constructed_provider_key_literals_are_findings,
        test_venue_id_from_provider_key_literal_is_finding,
        test_native_identity_literal_namespaces_are_findings,
        test_hyperliquid_outcome_group_literal_namespace_is_finding,
        test_cfg_any_test_feature_is_scanned_as_production,
        test_cfg_all_test_feature_is_stripped_as_test_only,
        test_cfg_not_not_test_is_stripped_as_test_only,
        test_inner_cfg_test_attr_strips_file_contents,
        test_multiline_cfg_test_attr_strips_test_item,
        test_whitespace_cfg_test_attr_strips_test_item,
        test_inline_cfg_test_item_does_not_hide_next_production_line,
        test_inline_cfg_test_semicolon_item_does_not_hide_next_production_line,
        test_cfg_test_comma_item_does_not_hide_next_production_line,
        test_cfg_test_comma_less_item_does_not_hide_next_production_line,
        test_cfg_test_where_clause_comma_does_not_scan_test_body,
        test_raw_strings_do_not_create_fake_comments,
        test_raw_string_cfg_text_does_not_hide_following_production,
        test_byte_and_multi_hash_raw_strings_do_not_create_fake_comments,
        test_cfg_test_multiline_raw_string_item_does_not_hide_next_production,
        test_multiline_raw_string_braces_do_not_keep_cfg_test_open,
        test_char_literal_parser_accepts_rust_escape_lengths,
        test_char_literal_braces_do_not_keep_cfg_test_open,
        test_strict_mode_fails_on_fixture_findings,
        test_scan_root_cache_does_not_mask_changed_file_content,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 provider-leak verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
