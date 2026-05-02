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
            {
                "src/bolt_v3_adapters.rs": """
                    /// Historical note: MarketSlugFilter used to live here.
                    pub struct ProviderOwnedAdapterConfig;

                    #[cfg(test)]
                    mod tests {
                        fn fixture() {
                            let _ = "BoltV3VenueAdapterConfig::Polymarket";
                        }
                    }
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
            {
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
        assert "provider-key string literal in core production code" in messages
        assert "closed resolved venue secret enum" in messages
        assert "secret resolution dispatches on concrete provider key" in messages
        assert "concrete NT provider factory import" in messages
        assert "closed registered venue summary enum" in messages


def test_strict_mode_fails_on_fixture_findings() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            {
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
        test_strict_mode_fails_on_fixture_findings,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 provider-leak verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
