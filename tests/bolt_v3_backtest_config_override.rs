use std::path::Path;

use bolt_v2::bolt_v3_config::{
    BacktestConfigOverride, RealizedVolatilitySourceSelector, apply_backtest_config_override,
    load_bolt_v3_config,
};
use nautilus_model::identifiers::{ClientId, InstrumentId};

const OVERRIDE_LABEL: &str = "production config + documented OKX/Bybit override";

fn issue_789_branch_b_override() -> BacktestConfigOverride {
    BacktestConfigOverride {
        label: OVERRIDE_LABEL.to_string(),
        strategy_instance_id: "binary_oracle_btc".to_string(),
        signal_role: "primary".to_string(),
        signal_data_client_id: ClientId::from("okx_data"),
        signal_instrument_id: InstrumentId::from("BTC-USDT.OKX"),
        realized_volatility_surface_id: "btc_usdt_midpoint_rv".to_string(),
        keep_realized_volatility_sources: vec![
            RealizedVolatilitySourceSelector {
                data_client_id: ClientId::from("okx_data"),
                instrument_id: InstrumentId::from("BTC-USDT.OKX"),
            },
            RealizedVolatilitySourceSelector {
                data_client_id: ClientId::from("bybit_data"),
                instrument_id: InstrumentId::from("BTCUSDT-SPOT.BYBIT"),
            },
        ],
    }
}

#[test]
fn branch_b_override_keeps_production_toml_as_source_of_truth() {
    let loaded = load_bolt_v3_config(Path::new("config/root.toml"))
        .expect("production root config should load");
    let production_strategy_files = loaded.root.strategy_files.clone();
    let production_checksum = loaded.config_bundle_checksum.clone();
    let live_btc = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "binary_oracle_btc")
        .expect("production BTC strategy exists");
    let live_signal = live_btc
        .config
        .signal_data
        .get("primary")
        .expect("BTC primary signal exists");
    assert_eq!(
        live_signal.data_client_id,
        ClientId::from("binance_spot_data")
    );
    assert_eq!(
        live_signal.instrument_id,
        InstrumentId::from("BTCUSDT.BINANCE")
    );

    let (overridden, report) =
        apply_backtest_config_override(loaded, &issue_789_branch_b_override())
            .expect("Branch B override should apply to production config");

    assert_eq!(overridden.root.strategy_files, production_strategy_files);
    assert_eq!(report.label, OVERRIDE_LABEL);
    assert_eq!(
        report.production_config_bundle_checksum,
        production_checksum
    );
    assert_eq!(report.signal_before.data_client_id, "binance_spot_data");
    assert_eq!(report.signal_before.instrument_id, "BTCUSDT.BINANCE");
    assert_eq!(report.signal_after.data_client_id, "okx_data");
    assert_eq!(report.signal_after.instrument_id, "BTC-USDT.OKX");

    let effective_btc = overridden
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "binary_oracle_btc")
        .expect("effective BTC strategy exists");
    let effective_signal = effective_btc
        .config
        .signal_data
        .get("primary")
        .expect("effective BTC primary signal exists");
    assert_eq!(effective_signal.data_client_id, ClientId::from("okx_data"));
    assert_eq!(
        effective_signal.instrument_id,
        InstrumentId::from("BTC-USDT.OKX")
    );

    let surface = overridden
        .root
        .realized_volatility_surfaces
        .as_ref()
        .expect("RV surfaces exist")
        .get("btc_usdt_midpoint_rv")
        .expect("BTC RV surface exists");
    let rv_sources = surface
        .sources
        .iter()
        .map(|source| {
            (
                source.data_client_id.to_string(),
                source.instrument_id.to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rv_sources,
        vec![
            ("okx_data".to_string(), "BTC-USDT.OKX".to_string()),
            ("bybit_data".to_string(), "BTCUSDT-SPOT.BYBIT".to_string()),
        ]
    );
    assert_eq!(surface.policy.min_ready_sources, 1);

    let removed = report
        .realized_volatility_sources_removed
        .iter()
        .map(|source| {
            (
                source.data_client_id.as_str(),
                source.instrument_id.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(removed, vec![("binance_spot_data", "BTCUSDT.BINANCE")]);

    let reloaded = load_bolt_v3_config(Path::new("config/root.toml"))
        .expect("production root config should still load");
    let reloaded_btc = reloaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "binary_oracle_btc")
        .expect("reloaded BTC strategy exists");
    let reloaded_signal = reloaded_btc
        .config
        .signal_data
        .get("primary")
        .expect("reloaded BTC primary signal exists");
    assert_eq!(
        reloaded_signal.data_client_id,
        ClientId::from("binance_spot_data")
    );
    assert_eq!(
        reloaded_signal.instrument_id,
        InstrumentId::from("BTCUSDT.BINANCE")
    );
}

#[test]
fn branch_b_override_rejects_missing_kept_rv_source() {
    let loaded = load_bolt_v3_config(Path::new("config/root.toml"))
        .expect("production root config should load");
    let mut override_spec = issue_789_branch_b_override();
    override_spec.keep_realized_volatility_sources[0].instrument_id =
        InstrumentId::from("BTC-USDT.MISSING");

    let error = apply_backtest_config_override(loaded, &override_spec)
        .expect_err("missing RV source selector should fail closed");
    assert!(
        error
            .messages()
            .iter()
            .any(|message| message.contains("matched 0 sources")),
        "expected missing selector error, got {:?}",
        error.messages()
    );
}
