use std::{fs, path::Path};

const FORBIDDEN_STRATEGY_RV_TERMS: &[&str] = &[
    "CrossSourceDispersion",
    "min_ready_sources",
    "max_cross_source_dispersion",
    "upper_quantile",
    "coverage_ratio",
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
