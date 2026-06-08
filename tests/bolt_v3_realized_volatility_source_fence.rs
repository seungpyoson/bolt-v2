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

const RV_CONSUMER_ARTIFACTS: &[&str] = &[
    "src/bolt_v3_taker_pricing.rs",
    "src/strategies/binary_oracle_edge_taker/mod.rs",
];

const PRODUCTION_LEGACY_RV_ARTIFACTS: &[&str] = &[
    "src/lib.rs",
    "src/bolt_v3_taker_pricing.rs",
    "src/strategies/binary_oracle_edge_taker/mod.rs",
    "src/strategies/binary_oracle_edge_taker/config.rs",
    "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs",
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

const FORBIDDEN_RAW_RV_CONSUMER_PATTERNS: &[&str] = &[
    "current_realized_vol_at(now_ms)\n            .filter(|value| is_non_negative_finite(*value))",
    "current_realized_vol_at(now_ms)\n            .filter(|value| is_positive_finite(*value))",
    "current_realized_vol_for_config_at(config, request.now_ms)\n            .filter(|value| is_non_negative_finite(*value))",
    "annualized_realized_vol_decimal\n                .is_some_and(is_non_negative_finite)",
];

const FORBIDDEN_LEGACY_RV_TERMS: &[&str] = &[
    "bolt_v3_volatility",
    "RealizedVolEstimator",
    "OptionalRealizedVolEstimator",
    "realized_vol_by_venue",
    "vol_window_secs",
    "vol_gap_reset_secs",
    "vol_min_observations",
    "vol_bridge_valid_secs",
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

#[test]
fn production_taker_path_has_no_legacy_internal_realized_volatility() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));

    for relative_path in PRODUCTION_LEGACY_RV_ARTIFACTS {
        let path = repo.join(relative_path);
        let source = fs::read_to_string(&path).expect("production artifact should be readable");
        for forbidden in FORBIDDEN_LEGACY_RV_TERMS {
            assert!(
                !source.contains(forbidden),
                "production taker path `{relative_path}` must not expose legacy internal RV term `{forbidden}`"
            );
        }
    }

    let strategy_config_dir = repo.join("config/strategies");
    for entry in fs::read_dir(strategy_config_dir).expect("strategy config directory should exist")
    {
        let entry = entry.expect("strategy config entry should be readable");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("toml") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("strategy config should be readable");
        for forbidden in FORBIDDEN_LEGACY_RV_TERMS {
            assert!(
                !source.contains(forbidden),
                "shipped strategy config `{}` must not expose legacy internal RV term `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn rv_consumers_do_not_revalidate_raw_snapshot_numbers() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));

    for relative_path in RV_CONSUMER_ARTIFACTS {
        let path = repo.join(relative_path);
        let source = fs::read_to_string(&path).expect("RV consumer source should be readable");
        for forbidden in FORBIDDEN_RAW_RV_CONSUMER_PATTERNS {
            assert!(
                !source.contains(forbidden),
                "RV consumer `{relative_path}` must use the ready snapshot accessor, not raw predicate `{forbidden}`"
            );
        }
    }
}
