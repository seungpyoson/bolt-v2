use std::{fs, path::Path};

const FORBIDDEN_STRATEGY_RV_TERMS: &[&str] = &[
    "CrossSourceDispersion",
    "min_ready_sources",
    "max_cross_source_dispersion",
    "upper_quantile",
    "coverage_ratio",
];

const RV_AGNOSTIC_ARTIFACTS: &[&str] = &[
    "specs/026-realized-volatility-surfaces/spec.md",
    "specs/026-realized-volatility-surfaces/plan.md",
    "specs/026-realized-volatility-surfaces/implementation-prompt.md",
    "src/bolt_v3_realized_volatility.rs",
    "tests/bolt_v3_realized_volatility.rs",
];

const FORBIDDEN_RV_CONCRETE_LITERALS: &[&str] = &[
    "BTC",
    "ETH",
    "USDT",
    "OKX",
    "BYBIT",
    "BINANCE",
    "POLYMARKET",
    "okx_data",
    "polymarket_main",
    "binance_reference",
];

#[test]
fn strategy_code_does_not_own_realized_volatility_engine_policy() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/strategies/binary_oracle_edge_taker/mod.rs");
    let source = fs::read_to_string(path).expect("strategy source should be readable");

    for forbidden in FORBIDDEN_STRATEGY_RV_TERMS {
        assert!(
            !source.contains(forbidden),
            "strategy code must not own realized-volatility policy term `{forbidden}`"
        );
    }
}

#[test]
fn realized_volatility_artifacts_stay_market_and_venue_agnostic() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));

    for relative_path in RV_AGNOSTIC_ARTIFACTS {
        let path = repo.join(relative_path);
        let source = fs::read_to_string(&path).expect("RV artifact should be readable");
        for forbidden in FORBIDDEN_RV_CONCRETE_LITERALS {
            assert!(
                !source.contains(forbidden),
                "RV artifact `{relative_path}` must use opaque placeholders, not concrete literal `{forbidden}`"
            );
        }
    }
}
